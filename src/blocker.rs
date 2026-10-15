//! Ads and trackers die at the door.
//! Lists sit on disk, the matcher is built off the UI thread, and every page
//! gets a hook that says no. The page never sees the bytes and never spends
//! the CPU.

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc::{Receiver, Sender, channel},
    thread,
    time::{Duration, SystemTime},
};

use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler, ExecuteScriptCompletedHandler, NavigationStartingEventHandler, Microsoft::Web::WebView2::Win32::*, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler,
};
use windows::{
    Win32::System::Com::CoTaskMemFree,
    core::{HSTRING, Interface, PWSTR},
};
use wry::{WebView, WebViewExtWindows};

/// Cache file name in `filters/`, and where it comes from.
const LISTS: &[(&str, &str)] = &[
    ("easylist.txt", "https://easylist.to/easylist/easylist.txt"),
    ("easyprivacy.txt", "https://easylist.to/easylist/easyprivacy.txt"),
    ("pgl.txt", "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext"),
    ("ublock.txt", "https://ublockorigin.github.io/uAssets/filters/filters.txt"),
    // Cookie banners: EasyList Cookie and uBlock's own. Mostly hiding rules.
    ("cookies-easylist.txt", "https://secure.fanboy.co.nz/fanboy-cookiemonster.txt"),
    ("cookies-ublock.txt", "https://ublockorigin.github.io/uAssets/filters/annoyances-cookies.txt"),
    // Tracking parameters ($removeparam), the ClearURLs job done by a list.
    ("privacy-ublock.txt", "https://ublockorigin.github.io/uAssets/filters/privacy.txt"),
    // uBlock's fast lane: YouTube and friends change weekly, fixes land here first.
    ("quick-fixes.txt", "https://ublockorigin.github.io/uAssets/filters/quick-fixes.txt"),
    ("unbreak.txt", "https://ublockorigin.github.io/uAssets/filters/unbreak.txt"),
];

/// uBlock's own lists may use uBlock's trusted scriptlets (`trusted-*`), as they do in uBlock.
const TRUSTED: &[&str] = &["ublock.txt", "cookies-ublock.txt", "privacy-ublock.txt", "quick-fixes.txt", "unbreak.txt"];
/// The trust bit the scriptlets file marks `requiresTrust` scriptlets with.
const TRUST: u8 = 1;

/// uBlock Origin 1.75.0's scriptlets (GPL-3.0), as adblock-rust resources. Made by
/// assets/ublock/export-scriptlets.mjs from the release source
/// (tarball sha256 a518c7d6e6b3f81d1a738befeb617abba59f93bd6b1e626da079a1003e9db52b).
const SCRIPTLETS: &str = include_str!("../assets/ublock/scriptlets.json");

/// In every top-level page. Asks Lumen for the page's hiding rules at once, then sends the
/// class names and ids it sees (new ones only, batched) so generic rules can match them.
/// Lumen answers by calling `__lumenCos(css)`.
pub const COSMETIC_JS: &str = r#"(()=>{if(window!==window.top)return;const W=window.chrome&&window.chrome.webview;if(!W)return;
let st=null;const put=t=>{try{if(!st)st=document.createElement('style');if(!st.isConnected)(document.head||document.documentElement).appendChild(st);st.textContent+=t}catch(e){}};
try{Object.defineProperty(window,'__lumenCos',{value:put})}catch(e){return}
const sc=new Set(),si=new Set();let qc=[],qi=[],tm=0;
const add=el=>{if(el.nodeType!==1)return;const id=el.id;if(id&&typeof id==='string'&&!si.has(id)){si.add(id);qi.push(id)}const cl=el.classList;if(cl)for(const c of cl)if(!sc.has(c)){sc.add(c);qc.push(c)}};
const flush=()=>{tm=0;if(!qc.length&&!qi.length)return;W.postMessage({lumen:'cos',c:qc.splice(0,3000),i:qi.splice(0,3000)});if(qc.length||qi.length)tm=setTimeout(flush,50)};
W.postMessage({lumen:'cos',first:1,c:[],i:[]});
const scan=()=>{for(const el of document.querySelectorAll('[id],[class]'))add(el);flush();
new MutationObserver(ms=>{for(const m of ms){if(m.type==='attributes'){add(m.target);continue}for(const n of m.addedNodes){if(n.nodeType!==1)continue;add(n);for(const e of n.querySelectorAll('[id],[class]'))add(e)}}if(!tm)tm=setTimeout(flush,300)}).observe(document.documentElement,{childList:true,subtree:true,attributes:true,attributeFilter:['class','id']})};
if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',scan,{once:true});else scan()})();"#;

/// What one page was told on its first ask: its exceptions and whether generic rules apply.
#[derive(Default)]
struct PageCos {
    exceptions: HashSet<String>,
    generichide: bool,
}
/// Bumped when what goes into the matcher changes (2: list trust for scriptlets).
const ENGINE_FILE: &str = "engine2.dat";
/// Four days. Older than that and the list is a rumour.
const MAX_AGE: Duration = Duration::from_secs(4 * 24 * 60 * 60);
const NET_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Blocker {
    dir: PathBuf,
    on: Cell<bool>,
    /// Hide what slipped through, and cookie banners (cosmetic rules).
    cosmetic: Cell<bool>,
    blocked: Cell<u64>,
    /// `adblock::Engine` refuses to cross threads, so it is born here, on the
    /// UI thread, from bytes baked by the background thread.
    engine: RefCell<Option<adblock::Engine>>,
    rx: RefCell<Receiver<Vec<u8>>>,
}

#[allow(dead_code)]
impl Blocker {
    /// Returns at once. Loads filter lists and builds the matcher on a
    /// background thread. Until ready, nothing is blocked.
    pub fn start(data_dir: PathBuf) -> Rc<Blocker> {
        let rx = Self::spawn_build(&data_dir);
        Rc::new(Blocker {
            dir: data_dir,
            on: Cell::new(true),
            cosmetic: Cell::new(true),
            blocked: Cell::new(0),
            engine: RefCell::new(None),
            rx: RefCell::new(rx),
        })
    }

    /// Turn blocking on/off at runtime.
    pub fn set_enabled(&self, on: bool) {
        self.on.set(on);
    }

    pub fn set_cosmetic(&self, on: bool) {
        self.cosmetic.set(on);
    }

    /// The URL with tracking parameters removed by `$removeparam` rules, if any were.
    pub fn rewrite(&self, url: &str) -> Option<String> {
        if !self.on.get() || !url.starts_with("http") || !url.contains('?') {
            return None;
        }
        self.pump();
        let slot = self.engine.borrow();
        let engine = slot.as_ref()?;
        let req = adblock::request::Request::new(url, url, "document", "GET").ok()?;
        engine.check_network_request(&req).rewritten_url.filter(|u| u != url)
    }

    /// The scriptlets this page gets (`+js(...)` rules), ready to run, or empty.
    pub fn scriptlets(&self, url: &str) -> String {
        if !self.on.get() || !url.starts_with("http") {
            return String::new();
        }
        self.pump();
        let slot = self.engine.borrow();
        let Some(engine) = slot.as_ref() else { return String::new() };
        engine.url_cosmetic_resources(url).injected_script
    }

    /// CSS for one page. First call: the site's own rules. Every call: generic rules that
    /// match the class names and ids the page reported. One rule per selector, so a selector
    /// the browser cannot parse only loses itself.
    fn cosmetic_css(&self, url: &str, st: &mut PageCos, first: bool, classes: &[String], ids: &[String]) -> String {
        if !self.on.get() || !self.cosmetic.get() || !url.starts_with("http") {
            return String::new();
        }
        self.pump();
        let slot = self.engine.borrow();
        let Some(engine) = slot.as_ref() else { return String::new() };
        let mut css = String::new();
        let hide = |sel: &str, css: &mut String| {
            css.push_str(sel);
            css.push_str("{display:none!important}\n");
        };
        if first {
            let res = engine.url_cosmetic_resources(url);
            for sel in &res.hide_selectors {
                hide(sel, &mut css);
            }
            for action in &res.procedural_actions {
                if let Ok(f) = serde_json::from_str::<adblock::cosmetic_filter_cache::ProceduralOrActionFilter>(action)
                    && let Some((sel, style)) = f.as_css()
                {
                    css.push_str(&format!("{sel}{{{style}}}\n"));
                }
            }
            *st = PageCos { exceptions: res.exceptions, generichide: res.generichide };
        }
        if !st.generichide && (!classes.is_empty() || !ids.is_empty()) {
            for sel in engine.hidden_class_id_selectors(classes, ids, &st.exceptions) {
                hide(&sel, &mut css);
            }
        }
        css
    }

    /// Total requests blocked since start.
    pub fn blocked(&self) -> u64 {
        self.blocked.get()
    }

    /// Hook one page. Call once per WebView after it is built.
    pub fn attach(self: &Rc<Self>, wv: &WebView) {
        let core = wv.webview();
        let environment = wv.environment();
        let weak = Rc::downgrade(self);
        unsafe {
            let star = HSTRING::from("*");
            // ICoreWebView2_22 also sees service workers. Older builds only
            // see the document's own requests, which is still most of the war.
            let hooked = match core.cast::<ICoreWebView2_22>() {
                Ok(c22) => c22
                    .AddWebResourceRequestedFilterWithRequestSourceKinds(
                        &star,
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                    )
                    .is_ok(),
                Err(_) => core
                    .AddWebResourceRequestedFilter(&star, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)
                    .is_ok(),
            };
            if !hooked {
                return;
            }
            // Scriptlets must run before the page's own scripts. At navigation start the URL is
            // known and no document exists yet: register this page's scriptlets for the document
            // about to be made, and take the last page's off. A generation number sorts out
            // registrations that answer after the next navigation already began.
            let w3 = Rc::downgrade(self);
            let current: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
            let generation = Rc::new(Cell::new(0u64));
            let mut token = 0i64;
            let _ = core.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |sender, args| {
                    let (Some(b), Some(args), Some(core)) = (w3.upgrade(), args, sender) else { return Ok(()) };
                    let my = generation.get() + 1;
                    generation.set(my);
                    if let Some(old) = current.borrow_mut().take() {
                        let _ = core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(old));
                    }
                    let mut u = PWSTR::null();
                    args.Uri(&mut u)?;
                    let url = take(u);
                    let js = b.scriptlets(&url);
                    if js.is_empty() {
                        return Ok(());
                    }
                    let host = url.split("://").nth(1).and_then(|r| r.split(['/', '?', '#', ':']).next()).unwrap_or("");
                    let wrapped = format!(
                        "(()=>{{if(window!==window.top||location.hostname!=={})return;\nconst scriptletGlobals={{}};\n{js}\n}})();",
                        serde_json::to_string(host).unwrap_or_default()
                    );
                    let (gen2, cur2, c2) = (generation.clone(), current.clone(), core.clone());
                    let _ = core.AddScriptToExecuteOnDocumentCreated(
                        &HSTRING::from(wrapped),
                        &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |hr, id| {
                            if hr.is_ok() {
                                if gen2.get() == my {
                                    *cur2.borrow_mut() = Some(id);
                                } else {
                                    let _ = c2.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(id));
                                }
                            }
                            Ok(())
                        })),
                    );
                    Ok(())
                })),
                &mut token,
            );
            // Cosmetic rules: the page script asks, this answers with CSS.
            let w2 = Rc::downgrade(self);
            let state = Rc::new(RefCell::new(PageCos::default()));
            let _ = core.add_WebMessageReceived(
                &WebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
                    let (Some(b), Some(args), Some(sender)) = (w2.upgrade(), args, sender) else { return Ok(()) };
                    let mut p = PWSTR::null();
                    if args.WebMessageAsJson(&mut p).is_err() {
                        return Ok(());
                    }
                    let Ok(msg) = serde_json::from_str::<serde_json::Value>(&take(p)) else { return Ok(()) };
                    if msg["lumen"] != "cos" {
                        return Ok(());
                    }
                    let mut src = PWSTR::null();
                    args.Source(&mut src)?;
                    let list = |k: &str| -> Vec<String> {
                        msg[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).take(3000).collect()).unwrap_or_default()
                    };
                    let css = b.cosmetic_css(&take(src), &mut state.borrow_mut(), msg["first"] == 1, &list("c"), &list("i"));
                    if !css.is_empty() {
                        let js = format!("window.__lumenCos&&window.__lumenCos({})", serde_json::to_string(&css).unwrap_or_default());
                        let _ = sender.ExecuteScript(&HSTRING::from(js), &ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(()))));
                    }
                    Ok(())
                })),
                &mut token,
            );
            let page = core.clone();
            let _ = core.add_WebResourceRequested(
                &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(b) = weak.upgrade() else { return Ok(()) };
                    let Some(args) = args else { return Ok(()) };
                    on_request(&b, &environment, &page, &args)
                })),
                &mut token,
            );
        }
    }

    /// Background thread in, serialized matcher out. A thread that will not
    /// spawn leaves the receiver empty, which means nothing blocks.
    fn spawn_build(data_dir: &Path) -> Receiver<Vec<u8>> {
        let (tx, rx) = channel();
        let dir = data_dir.to_path_buf();
        let _ = thread::Builder::new()
            .name("lumen-blocker".into())
            .spawn(move || build_engine(&dir, tx, false));
        rx
    }

    /// Take the newest built matcher, if one arrived. A stale matcher comes
    /// first, a fresh one may follow later; the last one wins. One atomic check
    /// when nothing is waiting.
    fn pump(&self) {
        let mut latest = None;
        while let Ok(bytes) = self.rx.borrow().try_recv() {
            latest = Some(bytes);
        }
        let Some(bytes) = latest else { return };
        let mut engine = adblock::Engine::new_with_filter_set(adblock::FilterSet::new(false));
        if engine.deserialize(&bytes).is_ok() {
            engine.use_resources(scriptlet_resources());
            *self.engine.borrow_mut() = Some(engine);
            return;
        }
        if self.engine.borrow().is_some() {
            return;
        }
        // The cached matcher is junk. Throw it out and build from text again.
        eprintln!("lumen: blocker: cached matcher is junk, rebuilding from text");
        let _ = fs::remove_file(self.dir.join("filters").join(ENGINE_FILE));
        let (tx, rx) = channel();
        let _ = thread::Builder::new()
            .name("lumen-blocker".into())
            .spawn({
                let dir = self.dir.clone();
                move || build_engine(&dir, tx, true)
            });
        *self.rx.borrow_mut() = rx;
    }

    /// The whole hot path. Runs for every request of every page.
    fn should_block(&self, url: &str, source: &str, kind: &str) -> bool {
        if !self.on.get() || url.is_empty() || source.is_empty() {
            return false;
        }
        self.pump();
        let mut slot = self.engine.borrow_mut();
        let Some(engine) = slot.as_mut() else {
            return false;
        };
        // A URL the matcher cannot read is a URL we leave alone.
        match adblock::request::Request::new(url, source, kind, "GET") {
            Ok(req) => engine.check_network_request(&req).should_block(),
            Err(_) => false,
        }
    }
}

/// One intercepted request. Answer 403 or get out of the way.
fn on_request(
    b: &Blocker,
    environment: &ICoreWebView2Environment,
    page: &ICoreWebView2,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
) -> windows::core::Result<()> {
    unsafe {
        let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
        args.ResourceContext(&mut ctx)?;
        // The document is the page itself. Never block the page.
        let Some(kind) = map_context(ctx) else {
            return Ok(());
        };
        let request = args.Request()?;
        let mut uri = PWSTR::null();
        request.Uri(&mut uri)?;
        let url = take(uri);
        let mut top = PWSTR::null();
        if page.Source(&mut top).is_err() {
            return Ok(());
        }
        let source = take(top);
        if !b.should_block(&url, &source, kind) {
            return Ok(());
        }
        let reason = HSTRING::from("Blocked");
        let headers = HSTRING::new();
        let Ok(reply) = environment.CreateWebResourceResponse(None, 403, &reason, &headers) else {
            return Ok(());
        };
        let _ = args.SetResponse(&reply);
        b.blocked.set(b.blocked.get() + 1);
        Ok(())
    }
}

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

/// WebView2's resource context to what a filter list calls a request.
fn map_context(ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> Option<&'static str> {
    if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT {
        return None;
    }
    Some(if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT {
        "script"
    } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE {
        "image"
    } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET {
        "stylesheet"
    } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST
        || ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH
    {
        "xmlhttprequest"
    } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT {
        "font"
    } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA {
        "media"
    } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET {
        "websocket"
    } else if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING {
        "ping"
    } else {
        // Event sources, manifests, text tracks, signed exchanges, the rest.
        "other"
    })
}

// ------------------------------------------------------------------- building

/// Rules in, serialized matcher out. This is the slow part, so it happens on
/// a background thread or never at all.
/// (list text, trusted). Trusted lists may use scriptlets that need trust.
fn serialize_engine(lists: &[(String, bool)]) -> Vec<u8> {
    let mut set = adblock::FilterSet::new(false);
    for (text, trusted) in lists {
        let permissions = adblock::resources::PermissionMask::from_bits(if *trusted { TRUST } else { 0 });
        set.add_filter_list(text.clone(), adblock::lists::ParseOptions { permissions, ..Default::default() });
    }
    adblock::Engine::new_with_filter_set(set).serialize()
}

fn scriptlet_resources() -> Vec<adblock::resources::Resource> {
    serde_json::from_str(SCRIPTLETS).unwrap_or_default()
}

fn build_engine(data_dir: &Path, tx: Sender<Vec<u8>>, force: bool) {
    let dir = data_dir.join("filters");
    if fs::create_dir_all(&dir).is_err() {
        eprintln!("lumen: blocker: no filter dir at {}", dir.display());
    }
    let fresh = !force && is_fresh(&dir);
    // Fast start: young lists and a matcher already built. Milliseconds.
    if let Ok(bytes) = fs::read(dir.join(ENGINE_FILE)) && !bytes.is_empty() && !force {
        let _ = tx.send(bytes);
        if fresh {
            return;
        }
        // Stale: block with the old matcher now, refresh below, swap in later.
    }
    for (name, url) in LISTS {
        let path = dir.join(name);
        let cached = fs::read_to_string(&path).unwrap_or_default();
        if fresh && !cached.is_empty() {
            continue;
        }
        if !fetch_to(url, &path) {
            // Carry on with whatever is already on disk.
            eprintln!("lumen: blocker: {name} did not download");
        }
    }
    let lists: Vec<(String, bool)> = LISTS
        .iter()
        .filter_map(|(name, _)| fs::read_to_string(dir.join(name)).ok().map(|t| (t, TRUSTED.contains(name))))
        .collect();
    if lists.is_empty() {
        eprintln!("lumen: blocker: no lists at all, nothing gets blocked");
    }
    let bytes = serialize_engine(&lists);
    let _ = fs::write(dir.join(ENGINE_FILE), &bytes);
    let _ = tx.send(bytes);
}

fn fetch_to(url: &str, path: &Path) -> bool {
    let config = ureq::config::Config::builder()
        .timeout_global(Some(NET_TIMEOUT))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let Ok(mut reply) = agent.get(url).call() else {
        return false;
    };
    let body = match reply.body_mut().read_to_string() {
        Ok(body) if !body.is_empty() => body,
        _ => return false,
    };
    // Write beside the target and move it, so a half list never survives.
    let tmp = path.with_extension("part");
    if fs::write(&tmp, body.as_bytes()).is_err() {
        return false;
    }
    fs::rename(&tmp, path).is_ok()
}

/// Every list file on disk was touched inside `max_age`.
fn cache_is_fresh(stamps: &[Option<SystemTime>], now: SystemTime, max_age: Duration) -> bool {
    if stamps.is_empty() {
        return false;
    }
    stamps.iter().all(|s| {
        s.as_ref()
            .and_then(|t| now.duration_since(*t).ok())
            .is_some_and(|age| age <= max_age)
    })
}

fn is_fresh(dir: &Path) -> bool {
    let stamps: Vec<Option<SystemTime>> = LISTS
        .iter()
        .map(|(name, _)| fs::metadata(dir.join(name)).and_then(|m| m.modified()).ok())
        .collect();
    cache_is_fresh(&stamps, SystemTime::now(), MAX_AGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(rules: &[&str]) -> adblock::Engine {
        let mut set = adblock::FilterSet::new(false);
        for rule in rules {
            set.add_filter_list((*rule).to_string(), adblock::lists::ParseOptions::default());
        }
        adblock::Engine::new_with_filter_set(set)
    }

    fn blocks(e: &adblock::Engine, url: &str, source: &str, kind: &str) -> bool {
        let Ok(req) = adblock::request::Request::new(url, source, kind, "GET") else {
            return false;
        };
        e.check_network_request(&req).should_block()
    }

    #[test]
    fn three_rules_block_ads_and_not_the_page() {
        let e = engine(&[
            "||ads.example.com^",
            "@@||ads.example.com^$domain=example.org",
            "@@||site.com^",
        ]);
        assert!(blocks(&e, "https://ads.example.com/x.js", "https://site.com", "script"));
        assert!(!blocks(&e, "https://site.com/app.js", "https://site.com", "script"));
        // The exception is scoped to another site. It must not open the door.
        assert!(!blocks(&e, "https://ads.example.com/x.js", "https://example.org", "script"));
    }

    #[test]
    fn contexts_map_to_request_types() {
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT), Some("script"));
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE), Some("image"));
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET), Some("stylesheet"));
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST), Some("xmlhttprequest"));
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH), Some("xmlhttprequest"));
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT), Some("font"));
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA), Some("media"));
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET), Some("websocket"));
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING), Some("ping"));
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_OTHER), Some("other"));
        // The document is the page. Never that.
        assert_eq!(map_context(COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT), None);
    }

    #[test]
    fn stale_lists_say_so() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let stamps = [Some(now)];
        assert!(cache_is_fresh(&stamps, now, MAX_AGE));
        assert!(cache_is_fresh(&stamps, now + MAX_AGE, MAX_AGE));
        assert!(!cache_is_fresh(&stamps, now + MAX_AGE + Duration::from_secs(1), MAX_AGE));
        // A file that ran away is the same as no file at all.
        assert!(!cache_is_fresh(&stamps, now - Duration::from_secs(1), MAX_AGE));
        assert!(!cache_is_fresh(&[None], now, MAX_AGE));
        assert!(!cache_is_fresh(&[Some(now), None], now, MAX_AGE));
        assert!(!cache_is_fresh(&[], now, MAX_AGE));
    }

    #[test]
    fn cosmetic_rules_reach_the_page() {
        let e = engine(&["example.com##.cookie-wall", "##.ad-banner", "example.com#@#.ad-banner", "||x.com^$removeparam=ref"]);
        let res = e.url_cosmetic_resources("https://example.com/a");
        assert!(res.hide_selectors.contains(".cookie-wall"));
        let generic = e.hidden_class_id_selectors(["ad-banner"], [] as [&str; 0], &HashSet::new());
        assert_eq!(generic, vec![".ad-banner".to_string()]);
        // The site said: not here.
        assert!(e.hidden_class_id_selectors(["ad-banner"], [] as [&str; 0], &res.exceptions).is_empty());
        let req = adblock::request::Request::new("https://x.com/p?ref=tw&id=1", "https://x.com/p?ref=tw&id=1", "document", "GET").unwrap();
        assert_eq!(e.check_network_request(&req).rewritten_url.as_deref(), Some("https://x.com/p?id=1"));
    }

    #[test]
    fn scriptlets_need_trust_and_resolve() {
        assert!(scriptlet_resources().len() > 100, "the vendored scriptlets parse");
        let rule = "www.youtube.com##+js(trusted-rpfr, '\"adPlacements\"', '\"no_ads\"', player?)\nexample.com##+js(set-constant, foo, true)";
        for (trusted, want) in [(true, true), (false, false)] {
            let mut e = adblock::Engine::new_with_filter_set(adblock::FilterSet::new(false));
            e.deserialize(&serialize_engine(&[(rule.into(), trusted)])).unwrap();
            e.use_resources(scriptlet_resources());
            let yt = e.url_cosmetic_resources("https://www.youtube.com/watch?v=x").injected_script;
            assert_eq!(yt.contains("trustedReplaceFetchResponse("), want, "trusted={trusted}");
            // Plain scriptlets need no trust.
            assert!(e.url_cosmetic_resources("https://example.com/").injected_script.contains("setConstant("));
        }
    }

    #[test]
    fn engine_survives_a_round_trip() {
        let bytes = serialize_engine(&[("||ads.example.com^".into(), false)]);
        let mut e = adblock::Engine::new_with_filter_set(adblock::FilterSet::new(false));
        e.deserialize(&bytes).expect("serialized matcher loads");
        assert!(blocks(&e, "https://ads.example.com/x.js", "https://site.com", "script"));
        assert!(!blocks(&e, "https://site.com/app.js", "https://site.com", "script"));
    }
}

