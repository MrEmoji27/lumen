//! Ads and trackers die at the door.
//! Lists sit on disk, the matcher is built off the UI thread, and every page
//! gets a hook that says no. The page never sees the bytes and never spends
//! the CPU.

use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc::{Receiver, Sender, channel},
    thread,
    time::{Duration, SystemTime},
};

use webview2_com::{
    Microsoft::Web::WebView2::Win32::*,
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
];
const ENGINE_FILE: &str = "engine.dat";
/// Four days. Older than that and the list is a rumour.
const MAX_AGE: Duration = Duration::from_secs(4 * 24 * 60 * 60);
const NET_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Blocker {
    dir: PathBuf,
    on: Cell<bool>,
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
            blocked: Cell::new(0),
            engine: RefCell::new(None),
            rx: RefCell::new(rx),
        })
    }

    /// Turn blocking on/off at runtime.
    pub fn set_enabled(&self, on: bool) {
        self.on.set(on);
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
            let page = core.clone();
            let mut token = 0i64;
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
fn serialize_engine(lists: &[String]) -> Vec<u8> {
    let mut set = adblock::FilterSet::new(false);
    for text in lists {
        set.add_filter_list(text.clone(), adblock::lists::ParseOptions::default());
    }
    adblock::Engine::new_with_filter_set(set).serialize()
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
    let lists: Vec<String> = LISTS
        .iter()
        .filter_map(|(name, _)| fs::read_to_string(dir.join(name)).ok())
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
    fn engine_survives_a_round_trip() {
        let bytes = serialize_engine(&["||ads.example.com^".into()]);
        let mut e = adblock::Engine::new_with_filter_set(adblock::FilterSet::new(false));
        e.deserialize(&bytes).expect("serialized matcher loads");
        assert!(blocks(&e, "https://ads.example.com/x.js", "https://site.com", "script"));
        assert!(!blocks(&e, "https://site.com/app.js", "https://site.com", "script"));
    }
}
