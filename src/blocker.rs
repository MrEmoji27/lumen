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
    time::{Duration, Instant, SystemTime},
};

use crate::privacy_stats::{self, Cat, Who};
use webview2_com::{
    AddScriptToExecuteOnDocumentCreatedCompletedHandler, ExecuteScriptCompletedHandler, NavigationCompletedEventHandler, NavigationStartingEventHandler, Microsoft::Web::WebView2::Win32::*, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler, WebResourceResponseReceivedEventHandler,
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
/// Bumped when what goes into the matcher changes (4: no debug info; the
/// report's ad-or-tracker answer moved to TRACKERS_FILE).
const ENGINE_FILE: &str = "engine4.dat";
/// The tracker lists' host rules (`||host^`), as sorted FNV-1a hashes, eight
/// bytes each. How the report tells a tracker from an ad. About 380 KB, where
/// keeping every rule's text in the matcher for the same answer cost 6 MB.
const TRACKERS_FILE: &str = "trackers4.bin";
/// Matchers and sidecars of earlier builds: deleted on sight.
const OLD_FILES: &[&str] = &["engine.dat", "engine2.dat", "engine3.dat", "engine3.sources"];
/// The cookie lists' hiding rules, one per line, for counting banner hides
/// apart from ad hides. The engine knows the rules; this file is only a tally.
const COOKIE_FILE: &str = "cookie_selectors.txt";
/// Four days. Older than that and the list is a rumour.
const MAX_AGE: Duration = Duration::from_secs(4 * 24 * 60 * 60);
const NET_TIMEOUT: Duration = Duration::from_secs(30);

/// A matcher, and the tracker hosts that say what a block it makes was.
struct Built {
    bytes: Vec<u8>,
    trackers: Vec<u64>,
}

pub struct Blocker {
    dir: PathBuf,
    on: Cell<bool>,
    /// Hide what slipped through, and cookie banners (cosmetic rules).
    cosmetic: Cell<bool>,
    /// Add the Global Privacy Control and Do Not Track headers to every request.
    gpc: Cell<bool>,
    blocked: Cell<u64>,
    /// `adblock::Engine` refuses to cross threads, so it is born here, on the
    /// UI thread, from bytes baked by the background thread.
    engine: RefCell<Option<adblock::Engine>>,
    rx: RefCell<Receiver<Built>>,
    /// The tracker table: sorted host hashes (see TRACKERS_FILE).
    trackers: RefCell<Vec<u64>>,
    /// Cookie banners' selectors, loaded when first needed and dropped when
    /// the matcher is replaced.
    cookies: RefCell<Option<Rc<HashSet<String>>>>,
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
            gpc: Cell::new(false),
            blocked: Cell::new(0),
            engine: RefCell::new(None),
            rx: RefCell::new(rx),
            trackers: RefCell::new(Vec::new()),
            cookies: RefCell::new(None),
        })
    }

    /// Turn blocking on/off at runtime.
    pub fn set_enabled(&self, on: bool) {
        self.on.set(on);
    }

    pub fn set_cosmetic(&self, on: bool) {
        self.cosmetic.set(on);
    }

    /// Send "Sec-GPC: 1" and "DNT: 1" with every request. The handler already runs for
    /// each one, so this costs two header sets and no extra hook.
    pub fn set_gpc(&self, on: bool) {
        self.gpc.set(on);
    }

    /// The URL with tracking parameters removed by `$removeparam` rules, if any were.
    pub fn rewrite(&self, url: &str, who: Who) -> Option<String> {
        if !self.on.get() || !url.starts_with("http") || !url.contains('?') {
            return None;
        }
        self.pump();
        let slot = self.engine.borrow();
        let engine = slot.as_ref()?;
        let req = adblock::request::Request::new(url, url, "document", "GET").ok()?;
        let clean = engine.check_network_request(&req).rewritten_url.filter(|u| u != url)?;
        // The page counted as itself: the link was on it, not on an ad.
        privacy_stats::removeparam(&privacy_stats::host(url), who);
        Some(clean)
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

    /// CSS for one page, and how many elements were hidden, and how many of
    /// those were cookie banners. First call: the site's own rules. Every call:
    /// generic rules that match the class names and ids the page reported. One
    /// rule per selector, so a selector the browser cannot parse only loses itself.
    fn cosmetic_css(&self, url: &str, st: &mut PageCos, first: bool, classes: &[String], ids: &[String]) -> (String, u64, u64) {
        if !self.on.get() || !self.cosmetic.get() || !url.starts_with("http") {
            return (String::new(), 0, 0);
        }
        self.pump();
        let slot = self.engine.borrow();
        let Some(engine) = slot.as_ref() else { return (String::new(), 0, 0) };
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
        // Every hidden element was one rule firing. Lines that style rather
        // than hide are not counted.
        let cookies = self.cookie_set();
        let (mut hides, mut cookie_n) = (0u64, 0u64);
        for line in css.lines() {
            if !line.ends_with("{display:none!important}") {
                continue;
            }
            hides += 1;
            if cookies.contains(line.split('{').next().unwrap_or("")) {
                cookie_n += 1;
            }
        }
        (css, hides, cookie_n)
    }

    /// The cookie lists' selectors, read once and kept until the matcher is
    /// replaced. A missing file is an empty set, remembered so the read is
    /// not tried again on every page.
    fn cookie_set(&self) -> Rc<HashSet<String>> {
        if let Some(set) = self.cookies.borrow().clone() {
            return set;
        }
        let set: HashSet<String> = fs::read_to_string(self.dir.join("filters").join(COOKIE_FILE))
            .map(|t| t.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
            .unwrap_or_default();
        let set = Rc::new(set);
        *self.cookies.borrow_mut() = Some(set.clone());
        set
    }

    /// Total requests blocked since start.
    pub fn blocked(&self) -> u64 {
        self.blocked.get()
    }

    /// Hook one page. Call once per WebView after it is built. `who` is the tab
    /// asking: everything counted here is counted against it.
    pub fn attach(self: &Rc<Self>, wv: &WebView, who: Who) {
        let core = wv.webview();
        let environment = wv.environment();
        let weak = Rc::downgrade(self);
        // Bytes this page moved since its window began: a load's size, taken
        // as a speed sample when the load finishes or the next one starts.
        let nav_bytes = Rc::new(Cell::new(0u64));
        let nav_at = Rc::new(Cell::new(None::<Instant>));
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
            let (nav_load, nav_time) = (nav_bytes.clone(), nav_at.clone());
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
                    // A new page in this tab: the sidebar's chip starts over,
                    // and the page before it is now one speed sample.
                    privacy_stats::nav_start(who.tab);
                    if let Some(t0) = nav_time.get() {
                        privacy_stats::nav_sample(nav_load.get(), t0.elapsed().as_secs_f64());
                    }
                    nav_time.set(Some(Instant::now()));
                    nav_load.set(0);
                    let js = b.scriptlets(&url);
                    if js.is_empty() {
                        return Ok(());
                    }
                    // One injection, one count: the script comes whole.
                    privacy_stats::scriptlet(&privacy_stats::host(&url), who);
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
            // A finished page is a speed sample too: its own bytes over its own
            // seconds. Without this, one page visit never teaches the estimate
            // because the next navigation may never come. The window restarts
            // here, so late, lazy bytes form their own sample later.
            let (c_bytes, c_time) = (nav_bytes.clone(), nav_at.clone());
            let _ = core.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |_, _| {
                    if let Some(t0) = c_time.get() {
                        privacy_stats::nav_sample(c_bytes.get(), t0.elapsed().as_secs_f64());
                    }
                    c_time.set(Some(Instant::now()));
                    c_bytes.set(0);
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
                    let src = take(src);
                    let list = |k: &str| -> Vec<String> {
                        msg[k].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).take(3000).collect()).unwrap_or_default()
                    };
                    let (css, hides, cookie_n) = b.cosmetic_css(&src, &mut state.borrow_mut(), msg["first"] == 1, &list("c"), &list("i"));
                    if hides > 0 {
                        let site = privacy_stats::host(&src);
                        privacy_stats::hides(&site, hides, who);
                        privacy_stats::cookie_hides(&site, cookie_n, who);
                    }
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
                    on_request(&b, &environment, &page, &args, who)
                })),
                &mut token,
            );
            // What a response carried: sizes measure this machine's real speed
            // and teach the averages what a script or an image weighs. The
            // blocker's own 403 has no Content-Length, so a blocked request
            // never measures itself.
            let carried = nav_bytes.clone();
            if let Ok(c2) = core.cast::<ICoreWebView2_2>() {
                let _ = c2.add_WebResourceResponseReceived(
                    &WebResourceResponseReceivedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else { return Ok(()) };
                        let Ok(resp) = args.Response() else { return Ok(()) };
                        let Ok(headers) = resp.Headers() else { return Ok(()) };
                        let mut p = PWSTR::null();
                        if headers.GetHeader(&HSTRING::from("Content-Length"), &mut p).is_err() {
                            return Ok(());
                        }
                        let len = take(p).trim().parse::<u64>().unwrap_or(0);
                        if len == 0 {
                            return Ok(());
                        }
                        carried.set(carried.get().saturating_add(len));
                        // The "other" bucket keeps its seed: whole documents
                        // and stylesheets would drown the small requests
                        // that land there.
                        let kind = kind_of_content_type(&response_header(&headers, "Content-Type"));
                        if kind != "other" {
                            privacy_stats::note_response(kind, len);
                        }
                        Ok(())
                    })),
                    &mut token,
                );
            }
        }
    }

    /// Background thread in, serialized matcher out. A thread that will not
    /// spawn leaves the receiver empty, which means nothing blocks.
    fn spawn_build(data_dir: &Path) -> Receiver<Built> {
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
        while let Ok(built) = self.rx.borrow().try_recv() {
            latest = Some(built);
        }
        let Some(built) = latest else { return };
        let mut engine = adblock::Engine::new_with_filter_set(adblock::FilterSet::new(false));
        if engine.deserialize(&built.bytes).is_ok() {
            engine.use_resources(scriptlet_resources());
            *self.engine.borrow_mut() = Some(engine);
            *self.trackers.borrow_mut() = built.trackers;
            // The cookie tallies belong to the lists this matcher was built
            // from; drop them so the next page reads the new ones.
            *self.cookies.borrow_mut() = None;
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

    /// The whole hot path. Runs for every request of every page. The door's
    /// answer: the list that stopped it, or None to let it through.
    fn check(&self, url: &str, source: &str, kind: &str) -> Option<Cat> {
        if !self.on.get() || url.is_empty() || source.is_empty() {
            return None;
        }
        self.pump();
        let slot = self.engine.borrow();
        let engine = slot.as_ref()?;
        // A URL the matcher cannot read is a URL we leave alone.
        let res = match adblock::request::Request::new(url, source, kind, "GET") {
            Ok(req) => engine.check_network_request(&req),
            Err(_) => return None,
        };
        if !res.should_block() {
            return None;
        }
        Some(classify(url, &self.trackers.borrow()))
    }
}

/// One intercepted request. Answer 403 or get out of the way.
fn on_request(
    b: &Blocker,
    environment: &ICoreWebView2Environment,
    page: &ICoreWebView2,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
    who: Who,
) -> windows::core::Result<()> {
    unsafe {
        let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
        args.ResourceContext(&mut ctx)?;
        let request = args.Request()?;
        if b.gpc.get()
            && let Ok(h) = request.Headers()
        {
            let _ = h.SetHeader(&HSTRING::from("Sec-GPC"), &HSTRING::from("1"));
            let _ = h.SetHeader(&HSTRING::from("DNT"), &HSTRING::from("1"));
        }
        // The document is the page itself. Never block the page.
        let Some(kind) = map_context(ctx) else {
            return Ok(());
        };
        let mut uri = PWSTR::null();
        request.Uri(&mut uri)?;
        let url = take(uri);
        let mut top = PWSTR::null();
        if page.Source(&mut top).is_err() {
            return Ok(());
        }
        let source = take(top);
        let Some(cat) = b.check(&url, &source, kind) else {
            return Ok(());
        };
        let reason = HSTRING::from("Blocked");
        let headers = HSTRING::new();
        let Ok(reply) = environment.CreateWebResourceResponse(None, 403, &reason, &headers) else {
            return Ok(());
        };
        let _ = args.SetResponse(&reply);
        b.blocked.set(b.blocked.get() + 1);
        // The page it happened on, and the name behind the request that died.
        let site = privacy_stats::host(&source);
        let domain = privacy_stats::registrable(&privacy_stats::host(&url));
        privacy_stats::blocked(&site, &domain, cat, kind, who);
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

/// One response header, or nothing. Only used off the request path.
fn response_header(headers: &ICoreWebView2HttpResponseHeaders, name: &str) -> String {
    let mut p = PWSTR::null();
    if unsafe { headers.GetHeader(&HSTRING::from(name), &mut p) }.is_err() {
        return String::new();
    }
    take(p)
}

/// What a response says it is, in terms an average can sit in. Anything the
/// content type does not name is "other".
fn kind_of_content_type(content_type: &str) -> &'static str {
    let ct = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if ct.contains("javascript") || ct.contains("ecmascript") {
        "script"
    } else if ct.starts_with("image/") {
        "image"
    } else if ct.starts_with("font/") || ct.contains("font") {
        "font"
    } else if ct.starts_with("video/") || ct.starts_with("audio/") {
        "media"
    } else if ct == "application/json" || ct.ends_with("+json") {
        "xmlhttprequest"
    } else {
        "other"
    }
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
///
/// Built without debug info: the matcher stays as small as the rules allow.
/// The engine keeps these bytes in memory for as long as Lumen runs.
fn serialize_engine(lists: &[(String, bool)]) -> Vec<u8> {
    let mut set = adblock::FilterSet::new(false);
    for (text, trusted) in lists {
        let permissions = adblock::resources::PermissionMask::from_bits(if *trusted { TRUST } else { 0 });
        set.add_filter_list(text.clone(), adblock::lists::ParseOptions { permissions, ..Default::default() });
    }
    adblock::Engine::new_with_filter_set(set).serialize()
}

/// Which list a file is: the two boxes the report counts for.
fn list_cat(name: &str) -> Cat {
    match name {
        "easylist.txt" | "pgl.txt" | "ublock.txt" | "quick-fixes.txt" => Cat::Ads,
        "easyprivacy.txt" | "privacy-ublock.txt" | "cookies-easylist.txt" | "cookies-ublock.txt" => Cat::Trackers,
        _ => Cat::Other,
    }
}

/// FNV-1a of a host name: what TRACKERS_FILE holds, one per host.
fn host_hash(host: &str) -> u64 {
    host.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// The hosts of the tracker lists' plain host rules (`||host^`, options
/// allowed), hashed, sorted, unique. Path rules and exceptions are left out:
/// a block they make is counted as an ad, which is the safer mistake.
fn tracker_hosts(lists: &[(&str, &str)]) -> Vec<u64> {
    let mut out = Vec::new();
    for (name, text) in lists {
        if list_cat(name) != Cat::Trackers {
            continue;
        }
        for line in text.lines() {
            let Some(rest) = line.trim().strip_prefix("||") else { continue };
            let Some(end) = rest.find('^') else { continue };
            let host = &rest[..end];
            if !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
                out.push(host_hash(&host.to_ascii_lowercase()));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Ad or tracker, for a request the matcher blocked: a tracker when its host,
/// or a domain above it, is a tracker list's host. No tracker table at all
/// (the file is missing) means no way to tell, and that is "other".
fn classify(url: &str, trackers: &[u64]) -> Cat {
    if trackers.is_empty() {
        return Cat::Other;
    }
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    let mut h = host.as_str();
    loop {
        if trackers.binary_search(&host_hash(h)).is_ok() {
            return Cat::Trackers;
        }
        match h.split_once('.') {
            Some((_, up)) if up.contains('.') => h = up,
            _ => return Cat::Ads,
        }
    }
}

/// The tracker table off disk. A missing or torn file is an empty table.
fn read_trackers(path: &Path) -> Vec<u64> {
    let Ok(bytes) = fs::read(path) else { return Vec::new() };
    bytes.chunks_exact(8).map(|c| u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])).collect()
}

/// The tracker table to disk, beside the matcher (tmp + rename).
fn write_trackers(path: &Path, hashes: &[u64]) {
    let bytes: Vec<u8> = hashes.iter().flat_map(|h| h.to_le_bytes()).collect();
    let tmp = path.with_extension("part");
    if fs::write(&tmp, bytes).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// The cookie lists' hiding rules, one selector per line. Exceptions (`#@#`)
/// and procedural operators (`#?#`) have no plain `##`, so they never match
/// and never land here; the engine decides whether a rule fires anyway.
/// `###id` does land here, as `#id`, which is what the page will be asked to
/// hide too.
fn cookie_selectors(dir: &Path) -> String {
    let mut out = String::new();
    for (name, _) in LISTS {
        if !name.starts_with("cookies-") {
            continue;
        }
        let Ok(text) = fs::read_to_string(dir.join(name)) else { continue };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('!') {
                continue;
            }
            let Some(pos) = line.find("##") else { continue };
            let sel = line[pos + 2..].trim();
            // `####id` removes an element instead of hiding it: not a hide.
            // Exceptions (`#@#`) and procedural rules (`#?#`) have no plain
            // `##` anywhere, so they never get this far.
            if sel.is_empty() || sel.starts_with("##") {
                continue;
            }
            out.push_str(sel);
            out.push('\n');
        }
    }
    out
}

/// The tally file beside the matcher, written whenever the lists were read.
/// Cheap: two files, one pass each.
fn ensure_cookie_file(dir: &Path) {
    if dir.join(COOKIE_FILE).exists() {
        return;
    }
    let _ = fs::write(dir.join(COOKIE_FILE), cookie_selectors(dir));
}

fn scriptlet_resources() -> Vec<adblock::resources::Resource> {
    serde_json::from_str(SCRIPTLETS).unwrap_or_default()
}

fn build_engine(data_dir: &Path, tx: Sender<Built>, force: bool) {
    let dir = data_dir.join("filters");
    if fs::create_dir_all(&dir).is_err() {
        eprintln!("lumen: blocker: no filter dir at {}", dir.display());
    }
    ensure_cookie_file(&dir);
    for old in OLD_FILES {
        let _ = fs::remove_file(dir.join(old));
    }
    let fresh = !force && is_fresh(&dir);
    // Fast start: young lists and a matcher already built. Milliseconds.
    if let Ok(bytes) = fs::read(dir.join(ENGINE_FILE)) && !bytes.is_empty() && !force {
        // Whoever built it wrote the tracker table beside it; without it the
        // matcher still blocks, the report just cannot say ad or tracker.
        let trackers = read_trackers(&dir.join(TRACKERS_FILE));
        let _ = tx.send(Built { bytes, trackers });
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
    let mut names: Vec<&str> = Vec::new();
    let mut lists: Vec<(String, bool)> = Vec::new();
    for (name, _) in LISTS {
        let Ok(text) = fs::read_to_string(dir.join(name)) else { continue };
        lists.push((text, TRUSTED.contains(name)));
        names.push(name);
    }
    if lists.is_empty() {
        eprintln!("lumen: blocker: no lists at all, nothing gets blocked");
    }
    let bytes = serialize_engine(&lists);
    let named: Vec<(&str, &str)> = names.iter().copied().zip(lists.iter().map(|(t, _)| t.as_str())).collect();
    let trackers = tracker_hosts(&named);
    let _ = fs::write(dir.join(ENGINE_FILE), &bytes);
    write_trackers(&dir.join(TRACKERS_FILE), &trackers);
    // The lists just changed, so the tallies change with them: overwrite.
    let _ = fs::write(dir.join(COOKIE_FILE), cookie_selectors(&dir));
    let _ = tx.send(Built { bytes, trackers });
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

    /// A blocked request says ad or tracker from the tracker table, through the
    /// serialize -> deserialize round trip the real matcher takes.
    #[test]
    fn a_blocked_request_says_ad_or_tracker() {
        let bytes = serialize_engine(&[("||ads.example.com^".into(), false), ("||track.example.org^".into(), false)]);
        let mut e = adblock::Engine::new_with_filter_set(adblock::FilterSet::new(false));
        e.deserialize(&bytes).expect("matcher loads");
        let trackers = tracker_hosts(&[("easylist.txt", "||ads.example.com^"), ("easyprivacy.txt", "||track.example.org^$third-party\n/pixel.gif?\n@@||ok.example.org^")]);
        assert_eq!(trackers.len(), 1, "only the tracker list's host rule goes in");
        let (tx, rx) = channel();
        let b = Blocker {
            dir: PathBuf::from("."),
            on: Cell::new(true),
            cosmetic: Cell::new(true),
            blocked: Cell::new(0),
            engine: RefCell::new(Some(e)),
            rx: RefCell::new(rx),
            trackers: RefCell::new(trackers),
            cookies: RefCell::new(None),
            gpc: Cell::new(false),
        };
        assert_eq!(b.check("https://ads.example.com/x.js", "https://site.com", "script"), Some(Cat::Ads));
        assert_eq!(b.check("https://track.example.org/p", "https://site.com", "image"), Some(Cat::Trackers));
        assert_eq!(b.check("https://site.com/app.js", "https://site.com", "script"), None, "the page is let through");
        drop(tx);
        // No table at all is not a crash: the count just says "other".
        *b.trackers.borrow_mut() = Vec::new();
        assert_eq!(b.check("https://track.example.org/p", "https://site.com", "image"), Some(Cat::Other));
    }

    #[test]
    fn a_subdomain_of_a_tracker_is_a_tracker() {
        let t = tracker_hosts(&[("easyprivacy.txt", "||doubleclick.net^")]);
        assert_eq!(classify("https://stats.g.doubleclick.net:443/x?y", &t), Cat::Trackers);
        assert_eq!(classify("https://doubleclick.net/", &t), Cat::Trackers);
        assert_eq!(classify("https://notdoubleclick.net/", &t), Cat::Ads);
        assert_eq!(classify("https://example.com/", &t), Cat::Ads);
    }

    #[test]
    fn the_tracker_table_survives_disk() {
        let dir = std::env::temp_dir().join(format!("lumen-trackers-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(TRACKERS_FILE);
        let t = tracker_hosts(&[("easyprivacy.txt", "||a.example^\n||b.example^")]);
        write_trackers(&path, &t);
        assert_eq!(read_trackers(&path), t);
        assert!(read_trackers(&dir.join("missing.bin")).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lists_split_into_ads_and_trackers() {
        assert_eq!(list_cat("easylist.txt"), Cat::Ads);
        assert_eq!(list_cat("ublock.txt"), Cat::Ads);
        assert_eq!(list_cat("quick-fixes.txt"), Cat::Ads);
        assert_eq!(list_cat("easyprivacy.txt"), Cat::Trackers);
        assert_eq!(list_cat("privacy-ublock.txt"), Cat::Trackers);
        assert_eq!(list_cat("cookies-ublock.txt"), Cat::Trackers);
        assert_eq!(list_cat("unbreak.txt"), Cat::Other);
        assert_eq!(list_cat("something-new.txt"), Cat::Other);
    }

    #[test]
    fn a_response_says_what_it_is() {
        assert_eq!(kind_of_content_type("application/javascript; charset=utf-8"), "script");
        assert_eq!(kind_of_content_type("text/javascript"), "script");
        assert_eq!(kind_of_content_type("image/webp"), "image");
        assert_eq!(kind_of_content_type("font/woff2"), "font");
        assert_eq!(kind_of_content_type("application/font-woff"), "font");
        assert_eq!(kind_of_content_type("video/mp4"), "media");
        assert_eq!(kind_of_content_type("audio/mpeg"), "media");
        assert_eq!(kind_of_content_type("application/json"), "xmlhttprequest");
        assert_eq!(kind_of_content_type("application/vnd.api+json"), "xmlhttprequest");
        assert_eq!(kind_of_content_type("text/html"), "other");
        assert_eq!(kind_of_content_type("text/css"), "other");
        assert_eq!(kind_of_content_type(""), "other");
    }

    /// Cookie banners are counted apart from ads, by name, and the exceptions
    /// and procedural operators never land in the tally.
    #[test]
    fn cookie_lists_leave_a_tally_of_their_selectors() {
        let dir = std::env::temp_dir().join(format!("lumen-cookies-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("cookies-ublock.txt"), "! comment\nnews.com##.cookie-wall\nnews.com###onetrust-banner\n##.gdpr-modal\nnews.com#@#.cookie-wall\nnews.com#?#.banner:has(> .x)\nnews.com####ad-id\n").unwrap();
        fs::write(dir.join("cookies-easylist.txt"), "shop.com##.consent-bar\n").unwrap();
        let out = cookie_selectors(&dir);
        let set: HashSet<&str> = out.lines().collect();
        assert!(set.contains(".cookie-wall"));
        assert!(set.contains("#onetrust-banner"));
        assert!(set.contains(".gdpr-modal"));
        assert!(set.contains(".consent-bar"));
        assert_eq!(set.len(), 4, "no comments, exceptions, procedural rules or removes: {set:?}");
        // Written once, kept, refreshed only by a rebuild.
        ensure_cookie_file(&dir);
        assert!(dir.join(COOKIE_FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

