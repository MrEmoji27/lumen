//! Engine layer. Everything that talks to WebView2 lives here.
//! main.rs only sees WebView handles and `Ev` messages.
//! To swap engines later (Servo), replace this file.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use tao::{event_loop::EventLoopProxy, window::Window};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, AddScriptToExecuteOnDocumentCreatedCompletedHandler, BytesReceivedChangedEventHandler,
    ClearBrowsingDataCompletedHandler, ContainsFullScreenElementChangedEventHandler, DownloadStartingEventHandler,
    NavigationStartingEventHandler, PermissionRequestedEventHandler, ProcessFailedEventHandler, StateChangedEventHandler,
    FaviconChangedEventHandler, GetCookiesCompletedHandler, GetFaviconCompletedHandler, HistoryChangedEventHandler, IsDefaultDownloadDialogOpenChangedEventHandler,
    IsDocumentPlayingAudioChangedEventHandler, Microsoft::Web::WebView2::Win32::*,
    SourceChangedEventHandler, TrySuspendCompletedHandler, WindowCloseRequestedEventHandler,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, RECT},
        Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn},
        System::{
            Com::CoTaskMemFree,
            Threading::{GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT},
            WindowsAndMessaging::GetClientRect,
        },
    },
    core::{Interface, HSTRING, PWSTR},
};
use wry::{
    NewWindowResponse, PageLoadEvent, Rect, Theme, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

/// Messages from engine callbacks to the main loop.
/// Callbacks never touch app state. They post here. No re-entrancy bugs.
#[derive(Debug)]
pub enum Ev {
    Ui(String),
    Title(u64, String),
    Url(u64, String),
    Loading(u64, bool),
    Favicon(u64, String),
    Nav(u64, bool, bool),
    Audio(u64, bool),
    FullScreen(u64, bool),
    CloseMe(u64),
    /// New popup tab, and the tab that opened it.
    Popup(u64, u64),
    Download(Dl),
    /// A site asks for a permission. Answer with `answer_permission(pid, ..)`.
    Permission { pid: u64, tab: u64, origin: String, kind: &'static str },
    /// A navigation was stopped to drop tracking parameters; load this instead.
    Redirect(u64, String),
    /// Time to read this tab's text for history search: (tab, url it was loading).
    Capture(u64, String),
    /// The text came back: (tab, url, JSON or "null").
    PageText(u64, String, String),
    /// Text search answers for the address bar: (query, hits).
    TextHits(String, Vec<crate::recall::Hit>),
    /// Another `lumen.exe` was started: come to the front (and stop being headless).
    ShowMe,
    /// This tab only existed to start a download, now handed off.
    Orphan(u64),
    /// A video grab moved: (download id, what happened).
    Video(u64, crate::video::Up),
    /// Reader mode extract came back: (tab, page url, JSON or "null").
    Reader(u64, String, String),
    /// A download WebView2 gave up so aria2 can take it: (tab, url, page url, file name).
    Handoff(u64, String, String, String),
    /// aria2 took a download: (gid, url, headers sent, file name asked for, private).
    AriaAdded(String, String, Vec<String>, Option<String>, bool),
    AriaFailed(u64, String),
    AriaJobs(Vec<crate::aria::Job>),
    /// A request from the agent thread, answered through its channel.
    Agent(crate::agent::Call),
    Suspended(u64, bool),
    Key(Action),
    Usage(Vec<crate::monitor::Proc>),
    /// A page died: (tab, the whole browser process went with it).
    /// The browser going takes every page with it; one page going takes only itself.
    Crashed(u64, bool),
}

/// A download's state, copied out of WebView2 for the main loop.
#[derive(Debug, Clone)]
pub struct Dl {
    pub id: u64,
    pub url: String,
    pub path: String,
    pub received: i64,
    pub total: i64,
    /// "progress" | "done" | "failed" | "paused"
    pub state: &'static str,
    /// Bytes per second, when known (aria2).
    pub speed: u64,
    /// A short extra, e.g. "16 connections".
    pub note: String,
    /// Started by a private tab: works, but leaves no row in downloads.json.
    pub private: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Action {
    NewTab,
    NewPrivate,
    CloseTab,
    Reopen,
    NextTab,
    PrevTab,
    Nth(usize),
    LastTab,
    Omnibox,
    Reload,
    Back,
    Forward,
    Sidebar,
    Fullscreen,
    Settings,
    Control,
    Downloads,
    Bookmark,
    History,
    Bookmarks,
    DevTools,
    Reader,
    DarkSite,
}

pub struct Ctx {
    pub window: Rc<Window>,
    pub proxy: EventLoopProxy<Ev>,
    pub env: RefCell<Option<ICoreWebView2Environment>>,
    pub next_id: Cell<u64>,
    /// Webviews born from window.open, waiting for the main loop to adopt them.
    pub popups: RefCell<Vec<(u64, WebView)>>,
    pub dark: Cell<bool>,
    /// Runs on every page right after it is built. Popups included.
    pub on_built: RefCell<Option<Box<dyn Fn(&WebView)>>>,
    /// The first page creates the WebView2 environment with these.
    pub web_context: RefCell<wry::WebContext>,
    pub args: String,
    pub tracking: RefCell<String>,
    /// Live download operations, by Lumen's download id.
    pub downloads: RefCell<HashMap<u64, ICoreWebView2DownloadOperation>>,
    pub passwords: Cell<bool>,
    /// Permission requests waiting for the user.
    pub perms: RefCell<HashMap<u64, (ICoreWebView2PermissionRequestedEventArgs, ICoreWebView2Deferral)>>,
    /// aria2 is up and the setting is on: http(s) downloads go to it.
    pub aria_on: Cell<bool>,
    /// URLs aria2 refused; WebView2 downloads these itself.
    pub bypass: RefCell<std::collections::HashSet<String>>,
    /// Dark Reader script for new documents, when dark pages are on.
    pub dark_js: RefCell<Option<Rc<str>>>,
    /// Tracking parameters a filter list removes (set by the blocker).
    pub rewrite: RefCell<Option<Rc<dyn Fn(&str) -> Option<String>>>>,
    /// Per tab: the id WebView2 gave the injected Dark Reader script.
    pub dark_ids: Rc<RefCell<HashMap<u64, String>>>,
}

impl Ctx {
    pub fn alloc_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }
}

/// Drain a COM stream. Favicons are small; cap at 256 KB to be safe.
fn read_stream(stream: &windows::Win32::System::Com::IStream) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let mut n = 0u32;
        let hr = unsafe { stream.Read(buf.as_mut_ptr() as _, buf.len() as u32, Some(&mut n)) };
        if hr.is_err() || n == 0 || out.len() > 256 * 1024 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    out
}

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

pub fn theme(dark: bool) -> Theme {
    if dark { Theme::Dark } else { Theme::Light }
}

/// Page background shown before first paint. Kills the white flash.
fn page_bg(dark: bool) -> (u8, u8, u8, u8) {
    if dark { (22, 20, 19, 255) } else { (247, 244, 240, 255) }
}

// ---------------------------------------------------------------- building

/// Chromium work Lumen has no use for. One comma list, because Chromium keeps
/// only the last `--disable-features=` switch it is given.
const UNNEEDED: &[&str] = &[
    // A spare renderer is kept warm for the next navigation. Lumen has few tabs,
    // and a warm process is 20 MB that sits there doing nothing.
    "SpareRendererForSitePerProcess",
    // Hidden tabs are already frozen by Lumen's sleep policy; a second cache of
    // the same pages for back and forward is memory nobody asked for.
    "BackForwardCache",
    // No Cast dialog, no translation prompt, no new tab feed in a browser shell.
    "Translate",
    "MediaRouter",
    "InterestFeedContentSuggestions",
    // Autofill and page hints fetched from a server. Lumen fills nothing in.
    "AutofillServerCommunication",
    "OptimizationHints",
    // Edge chrome that never shows here.
    "msWebView2EnableShoppingFeatures",
    "msEdgeCollections",
    "msEdgeRewards",
    "msEdgeWallet",
];

/// Chromium switches for the whole browser. Set once, when the environment is born.
/// LUMEN_ARGS appends more, for measuring without a rebuild.
pub fn browser_args(gpu_in_process: bool, share_processes: bool) -> String {
    // wry's own defaults. Passing args replaces them, so keep them.
    let mut a = String::from("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    // One --disable-features only: Chromium keeps the last one it sees, so every
    // feature Lumen does not need goes into this list.
    for f in UNNEEDED {
        a.push(',');
        a.push_str(f);
    }
    if crate::headless() {
        // Off-screen is "occluded" to Chromium, which would then stop painting and slow timers.
        // One --disable-features only: Chromium keeps the last one it sees.
        // Sound is muted per page instead (see build_tab_in), so it can come back if the
        // headless Lumen is turned into a normal one.
        a.push_str(",CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows");
    }
    if gpu_in_process {
        // Measured: about 100 MB less commit, 55 MB less working set, one process fewer.
        // Cost: a GPU driver crash takes the browser down with it.
        a.push_str(" --in-process-gpu --enable-features=NetworkServiceInProcess2");
    }
    if share_processes {
        // Measured on 40 tabs: 54 -> 23 renderers, 310 MB less.
        // Cost: one heavy tab can slow other tabs of the same site.
        a.push_str(" --process-per-site");
    }
    if let Ok(extra) = std::env::var("LUMEN_ARGS") {
        a.push(' ');
        a.push_str(&extra);
    }
    a
}

/// One page. All pages share the UI's environment, so one browser process,
/// one GPU process, one network process for the whole app.
/// `isolated`: the page lives in the separate "agent" profile (own cookies and storage).
/// `private`: WebView2 InPrivate. No cookies outlive the last private page, and
/// nothing the page does reaches this profile's history.
pub fn build_tab(ctx: &Rc<Ctx>, id: u64, url: Option<&str>, bounds: Rect, isolated: bool, private: bool) -> wry::Result<WebView> {
    let env = ctx.env.borrow().clone();
    build_tab_in(ctx, id, url, bounds, env, isolated, private)
}

fn build_tab_in(
    ctx: &Rc<Ctx>,
    id: u64,
    url: Option<&str>,
    bounds: Rect,
    env: Option<ICoreWebView2Environment>,
    isolated: bool,
    private: bool,
) -> wry::Result<WebView> {
    let dark = ctx.dark.get();
    let p_title = ctx.proxy.clone();
    let p_load = ctx.proxy.clone();
    let popup_ctx = ctx.clone();

    // No environment yet: this page is the first, and it makes one for everyone.
    let first = env.is_none();
    let mut wc = first.then(|| ctx.web_context.borrow_mut());
    let base = match (env, wc.as_mut()) {
        (Some(env), _) => WebViewBuilder::new().with_environment(env),
        (None, Some(wc)) => WebViewBuilder::new_with_web_context(wc).with_additional_browser_args(ctx.args.clone()),
        (None, None) => unreachable!(),
    };
    let base = if isolated { base.with_profile_name("agent") } else { base };
    let mut b = base
        .with_bounds(bounds)
        .with_visible(false)
        .with_devtools(true)
        .with_hotkeys_zoom(true)
        .with_initialization_script(crate::blocker::COSMETIC_JS)
        .with_theme(theme(dark))
        .with_background_color(page_bg(dark))
        .with_incognito(private)
        .with_document_title_changed_handler(move |t| {
            let _ = p_title.send_event(Ev::Title(id, t));
        })
        .with_on_page_load_handler(move |e, _| {
            let _ = p_load.send_event(Ev::Loading(id, matches!(e, PageLoadEvent::Started)));
        })
        .with_new_window_req_handler(move |_uri, features| {
            // window.open / target=_blank. Make a real tab and hand it to WebView2,
            // so window.opener works (OAuth popups need it). A popup of a private
            // tab is private too: same InPrivate cookie jar, same silence.
            let ctx = &popup_ctx;
            let new_id = ctx.alloc_id();
            let bounds = content_bounds_now(ctx);
            match build_tab_in(ctx, new_id, None, bounds, Some(features.opener.environment.clone()), isolated, private) {
                Ok(wv) => {
                    let core = wv.webview();
                    ctx.popups.borrow_mut().push((new_id, wv));
                    let _ = ctx.proxy.send_event(Ev::Popup(new_id, id));
                    NewWindowResponse::Create { webview: core }
                }
                Err(_) => NewWindowResponse::Deny,
            }
        });
    // Dark pages: the script must be in place before the first document, so that
    // navigation waits for WebView2 to confirm it (set_dark starts it).
    let dark_first = !isolated && ctx.dark_js.borrow().is_some();
    if let Some(u) = url
        && !dark_first
    {
        b = b.with_url(u);
    }
    let wv = b.build_as_child(&*ctx.window)?;
    drop(wc);
    if first {
        ctx.env.replace(Some(wv.environment()));
        set_tracking(&wv, &ctx.tracking.borrow());
    }
    hook_page(&wv, id, ctx.proxy.clone(), ctx.rewrite.borrow().clone());
    hook_downloads(ctx, &wv, id, private);
    hook_crash(&wv, id, ctx.proxy.clone());
    hook_permissions(ctx, &wv, id, isolated);
    // Password saving is per profile. The agent's profile never saves passwords,
    // and InPrivate keeps nothing anyway, so private pages get it off too.
    set_passwords(&wv, ctx.passwords.get() && !isolated && !private);
    hook_keys(&wv, ctx.proxy.clone());
    if crate::headless() {
        set_muted(&wv, true);
    }
    // A new view has no scripts yet; forget any id an older view of this tab had.
    ctx.dark_ids.borrow_mut().remove(&id);
    if !isolated {
        set_dark(ctx, &wv, id, false, url.filter(|_| dark_first).map(str::to_string));
    }
    if let Some(f) = ctx.on_built.borrow().as_ref() {
        f(&wv);
    }
    Ok(wv)
}

/// Popups are born inside a callback with no access to app state.
/// They start at the bounds of whatever page is showing; main loop fixes them on adopt.
fn content_bounds_now(ctx: &Ctx) -> Rect {
    let size = ctx.window.inner_size();
    Rect {
        position: wry::dpi::PhysicalPosition::new(0, 0).into(),
        size: wry::dpi::PhysicalSize::new(size.width, size.height).into(),
    }
}

// ------------------------------------------------------------------- hooks

fn hook_page(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>, rewrite: Option<Rc<dyn Fn(&str) -> Option<String>>>) {
    let core = wv.webview();
    let mut token = 0i64;
    unsafe {
        // Links carry tracking tags (utm_*, fbclid, gclid...). Stop, strip, go.
        // Server redirects are left alone, or a site that adds them back would loop.
        let p = proxy.clone();
        let _ = core.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut redirected = Default::default();
                let _ = args.IsRedirected(&mut redirected);
                if redirected.as_bool() {
                    return Ok(());
                }
                let mut u = PWSTR::null();
                args.Uri(&mut u)?;
                let u = take(u);
                let clean = clean_url(&u);
                let listed = rewrite.as_ref().and_then(|f| f(clean.as_deref().unwrap_or(&u)));
                if let Some(clean) = listed.or(clean) {
                    args.SetCancel(true)?;
                    let _ = p.send_event(Ev::Redirect(id, clean));
                }
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let _ = core.add_SourceChanged(
            &SourceChangedEventHandler::create(Box::new(move |w, _| {
                if let Some(w) = w {
                    let mut s = PWSTR::null();
                    w.Source(&mut s)?;
                    let _ = p.send_event(Ev::Url(id, take(s)));
                }
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let _ = core.add_HistoryChanged(
            &HistoryChangedEventHandler::create(Box::new(move |w, _| {
                if let Some(w) = w {
                    let (mut b, mut f) = (Default::default(), Default::default());
                    w.CanGoBack(&mut b)?;
                    w.CanGoForward(&mut f)?;
                    let _ = p.send_event(Ev::Nav(id, b.as_bool(), f.as_bool()));
                }
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let _ = core.add_ContainsFullScreenElementChanged(
            &ContainsFullScreenElementChangedEventHandler::create(Box::new(move |w, _| {
                if let Some(w) = w {
                    let mut fs = Default::default();
                    w.ContainsFullScreenElement(&mut fs)?;
                    let _ = p.send_event(Ev::FullScreen(id, fs.as_bool()));
                }
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let _ = core.add_WindowCloseRequested(
            &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                let _ = p.send_event(Ev::CloseMe(id));
                Ok(())
            })),
            &mut token,
        );

        if let Ok(c15) = core.cast::<ICoreWebView2_15>() {
            // Ask WebView2 for the icon it already downloaded, as PNG bytes.
            // No second fetch, works offline, survives session restore.
            let p = proxy.clone();
            let _ = c15.add_FaviconChanged(
                &FaviconChangedEventHandler::create(Box::new(move |w, _| {
                    let Some(w) = w.and_then(|w| w.cast::<ICoreWebView2_15>().ok()) else { return Ok(()) };
                    let p = p.clone();
                    w.GetFavicon(
                        COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
                        &GetFaviconCompletedHandler::create(Box::new(move |hr, stream| {
                            if hr.is_ok()
                                && let Some(stream) = stream
                            {
                                let png = read_stream(&stream);
                                if !png.is_empty() {
                                    use base64::Engine as _;
                                    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
                                    let _ = p.send_event(Ev::Favicon(id, format!("data:image/png;base64,{b64}")));
                                }
                            }
                            Ok(())
                        })),
                    )?;
                    Ok(())
                })),
                &mut token,
            );
        }

        if let Ok(c8) = core.cast::<ICoreWebView2_8>() {
            let p = proxy.clone();
            let _ = c8.add_IsDocumentPlayingAudioChanged(
                &IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |w, _| {
                    if let Some(w) = w.and_then(|w| w.cast::<ICoreWebView2_8>().ok()) {
                        let mut on = Default::default();
                        w.IsDocumentPlayingAudio(&mut on)?;
                        let _ = p.send_event(Ev::Audio(id, on.as_bool()));
                    }
                    Ok(())
                })),
                &mut token,
            );
        }
    }
}

/// Shortcuts must work while a page has focus. Pages swallow keys,
/// so catch them at the controller, before the page sees them.
fn hook_keys(wv: &WebView, proxy: EventLoopProxy<Ev>) {
    let controller = wv.controller();
    let mut token = 0i64;
    unsafe {
        let _ = controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                let mut vk = 0u32;
                args.VirtualKey(&mut vk)?;
                // A lone Alt or F10 drops a Win32 window into menu mode, which eats the
                // next keys. Lumen has no menu bar. Swallow them.
                const ALT: u32 = 0x12;
                const F10: u32 = 0x79;
                if vk == ALT || vk == F10 {
                    args.SetHandled(true)?;
                    return Ok(());
                }
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }
                let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
                args.PhysicalKeyStatus(&mut status)?;
                // Async state: the key went to the WebView2 process, so this thread never saw it.
                let down = |k: u16| GetAsyncKeyState(k as i32) < 0;
                let (ctrl, shift, alt) = (down(VK_CONTROL.0), down(VK_SHIFT.0), down(VK_MENU.0));
                if let Some(a) = map_key(vk, ctrl, shift, alt) {
                    args.SetHandled(true)?;
                    // Holding a key repeats. Only tab cycling should repeat.
                    let repeat = status.WasKeyDown.as_bool();
                    if !repeat || matches!(a, Action::NextTab | Action::PrevTab) {
                        let _ = proxy.send_event(Ev::Key(a));
                    }
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

// ------------------------------------------------------------- crashed pages

/// A page that dies leaves a white rectangle that looks like a slow page.
/// WebView2 says so here; the main loop turns it into "crashed" and reloads on
/// the next click. Only the page's own renderer counts: a GPU or utility
/// process coming and going, or one dead iframe, does not take the page with it.
fn hook_crash(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>) {
    let mut token = 0i64;
    unsafe {
        let _ = wv.webview().add_ProcessFailed(
            &ProcessFailedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                let _ = args.ProcessFailedKind(&mut kind);
                let dead_page = kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED;
                let whole_browser = kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED;
                if dead_page || whole_browser {
                    let _ = proxy.send_event(Ev::Crashed(id, whole_browser));
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

pub fn map_key(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<Action> {
    use Action::*;
    const TAB: u32 = 0x09;
    const LEFT: u32 = 0x25;
    const RIGHT: u32 = 0x27;
    const F5: u32 = 0x74;
    const F6: u32 = 0x75;
    const F9: u32 = 0x78;
    const F11: u32 = 0x7A;
    const F12: u32 = 0x7B;
    const ESC: u32 = 0x1B;
    const COMMA: u32 = 0xBC;
    let k = |c: char| c as u32;
    Some(match (ctrl, shift, alt, vk) {
        (true, false, false, v) if v == k('T') => NewTab,
        (true, true, false, v) if v == k('N') => NewPrivate,
        (true, true, false, v) if v == k('T') => Reopen,
        (true, false, false, v) if v == k('W') => CloseTab,
        (true, false, false, v) if v == k('L') || v == k('K') => Omnibox,
        (false, false, false, F6) => Omnibox,
        (true, false, false, TAB) => NextTab,
        (true, true, false, TAB) => PrevTab,
        (true, false, false, v) if (k('1')..=k('8')).contains(&v) => Nth((v - k('1')) as usize),
        (true, false, false, v) if v == k('9') => LastTab,
        (true, false, false, v) if v == k('R') => Reload,
        (false, false, false, F5) => Reload,
        (false, false, true, LEFT) => Back,
        (false, false, true, RIGHT) => Forward,
        (true, false, false, v) if v == k('B') => Sidebar,
        (false, false, false, F11) => Fullscreen,
        (true, false, false, COMMA) => Settings,
        (false, false, false, F12) => DevTools,
        (false, false, false, F9) => Reader,
        (false, true, false, F9) => DarkSite,
        (false, true, false, ESC) => Control,
        (true, false, false, v) if v == k('J') => Downloads,
        (true, false, false, v) if v == k('D') => Bookmark,
        (true, false, false, v) if v == k('H') => History,
        (true, true, false, v) if v == k('O') => Bookmarks,
        (true, true, false, v) if v == k('I') => DevTools,
        _ => return None,
    })
}

// --------------------------------------------------------------- downloads

static NEXT_DL: AtomicU64 = AtomicU64::new(1);

fn dl_state(op: &ICoreWebView2DownloadOperation, id: u64, private: bool) -> Dl {
    unsafe {
        let (mut uri, mut path) = (PWSTR::null(), PWSTR::null());
        let _ = op.Uri(&mut uri);
        let _ = op.ResultFilePath(&mut path);
        let (mut got, mut total) = (0i64, 0i64);
        let _ = op.BytesReceived(&mut got);
        let _ = op.TotalBytesToReceive(&mut total);
        let mut st = COREWEBVIEW2_DOWNLOAD_STATE::default();
        let _ = op.State(&mut st);
        let mut resumable = Default::default();
        let _ = op.CanResume(&mut resumable);
        let state = match st {
            COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED => "done",
            COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED if resumable.as_bool() => "paused",
            COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED => "failed",
            _ => "progress",
        };
        Dl { id, url: take(uri), path: take(path), received: got, total, state, speed: 0, note: String::new(), private }
    }
}

/// Downloads come to Lumen. WebView2's own flyout stays hidden; the Downloads page shows them.
fn hook_downloads(ctx: &Rc<Ctx>, wv: &WebView, tab: u64, private: bool) {
    // Lumen has its own downloads page. WebView2's flyout never stays open.
    if let Ok(c9) = wv.webview().cast::<ICoreWebView2_9>() {
        let mut token = 0i64;
        unsafe {
            let _ = c9.add_IsDefaultDownloadDialogOpenChanged(
                &IsDefaultDownloadDialogOpenChangedEventHandler::create(Box::new(|w, _| {
                    if let Some(w) = w.and_then(|w| w.cast::<ICoreWebView2_9>().ok()) {
                        let mut open = Default::default();
                        if w.IsDefaultDownloadDialogOpen(&mut open).is_ok() && open.as_bool() {
                            let _ = w.CloseDefaultDownloadDialog();
                        }
                    }
                    Ok(())
                })),
                &mut token,
            );
        }
    }
    let Ok(c4) = wv.webview().cast::<ICoreWebView2_4>() else { return };
    let ctx = ctx.clone();
    let mut token = 0i64;
    unsafe {
        let _ = c4.add_DownloadStarting(
            &DownloadStartingEventHandler::create(Box::new(move |sender, args| {
                let Some(args) = args else { return Ok(()) };
                args.SetHandled(true)?;
                let op = args.DownloadOperation()?;
                // aria2 takes plain http(s) downloads: more connections, resume, queue.
                let mut u = PWSTR::null();
                let _ = op.Uri(&mut u);
                let url = take(u);
                if ctx.aria_on.get() && url.starts_with("http") && !ctx.bypass.borrow_mut().remove(&url) {
                    let mut p = PWSTR::null();
                    let _ = args.ResultFilePath(&mut p);
                    let path = take(p);
                    let name = path.rsplit(['\\', '/']).next().unwrap_or("").to_string();
                    let page = sender
                        .as_ref()
                        .map(|w| {
                            let mut s = PWSTR::null();
                            let _ = w.Source(&mut s);
                            take(s)
                        })
                        .unwrap_or_default();
                    // The page never showed anything: the link was the download. Nothing to keep.
                    let mut back = Default::default();
                    let _ = sender.as_ref().map(|w| w.CanGoBack(&mut back));
                    let orphan = !back.as_bool() && (page.is_empty() || page == "about:blank" || page == url);
                    args.SetCancel(true)?;
                    let _ = ctx.proxy.send_event(Ev::Handoff(tab, url, page, name));
                    if orphan {
                        let _ = ctx.proxy.send_event(Ev::Orphan(tab));
                    }
                    return Ok(());
                }
                let id = NEXT_DL.fetch_add(1, Ordering::Relaxed);
                let mut t = 0i64;
                let p = ctx.proxy.clone();
                let _ = op.add_BytesReceivedChanged(
                    &BytesReceivedChangedEventHandler::create(Box::new(move |op, _| {
                        if let Some(op) = op {
                            let _ = p.send_event(Ev::Download(dl_state(&op, id, private)));
                        }
                        Ok(())
                    })),
                    &mut t,
                );
                let p = ctx.proxy.clone();
                let _ = op.add_StateChanged(
                    &StateChangedEventHandler::create(Box::new(move |op, _| {
                        if let Some(op) = op {
                            let _ = p.send_event(Ev::Download(dl_state(&op, id, private)));
                        }
                        Ok(())
                    })),
                    &mut t,
                );
                let _ = ctx.proxy.send_event(Ev::Download(dl_state(&op, id, private)));
                ctx.downloads.borrow_mut().insert(id, op);
                Ok(())
            })),
            &mut token,
        );
    }
}

/// "pause" | "resume" | "cancel" on a running download.
pub fn dl_control(ctx: &Ctx, id: u64, action: &str) {
    let Some(op) = ctx.downloads.borrow().get(&id).cloned() else { return };
    unsafe {
        let _ = match action {
            "pause" => op.Pause(),
            "resume" => op.Resume(),
            "cancel" => op.Cancel(),
            _ => Ok(()),
        };
    }
}

// ------------------------------------------------------------- permissions

static NEXT_PERM: AtomicU64 = AtomicU64::new(1);

fn perm_name(k: COREWEBVIEW2_PERMISSION_KIND) -> &'static str {
    match k {
        COREWEBVIEW2_PERMISSION_KIND_MICROPHONE => "microphone",
        COREWEBVIEW2_PERMISSION_KIND_CAMERA => "camera",
        COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION => "location",
        COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS => "notifications",
        COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS => "motion sensors",
        COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ => "clipboard",
        COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS => "multiple downloads",
        COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE => "files",
        COREWEBVIEW2_PERMISSION_KIND_AUTOPLAY => "autoplay",
        COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS => "fonts",
        COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES => "MIDI devices",
        COREWEBVIEW2_PERMISSION_KIND_WINDOW_MANAGEMENT => "window placement",
        _ => "something",
    }
}

/// scheme://host[:port] of a URL.
pub fn origin_of(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}", rest.split(['/', '?', '#']).next().unwrap_or("")),
        None => url.to_string(),
    }
}

/// Sites ask; Lumen decides. The request waits (deferral) until the main loop answers.
/// Agent tabs in their own profile get no permissions at all.
fn hook_permissions(ctx: &Rc<Ctx>, wv: &WebView, id: u64, isolated: bool) {
    let ctx = ctx.clone();
    let mut token = 0i64;
    unsafe {
        let _ = wv.webview().add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                if isolated {
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                    return Ok(());
                }
                let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                args.PermissionKind(&mut kind)?;
                let mut u = PWSTR::null();
                args.Uri(&mut u)?;
                let deferral = args.GetDeferral()?;
                let pid = NEXT_PERM.fetch_add(1, Ordering::Relaxed);
                ctx.perms.borrow_mut().insert(pid, (args.clone(), deferral));
                let _ = ctx.proxy.send_event(Ev::Permission { pid, tab: id, origin: origin_of(&take(u)), kind: perm_name(kind) });
                Ok(())
            })),
            &mut token,
        );
    }
}

/// Some(true) allow, Some(false) block, None let WebView2 decide (it blocks by default).
pub fn answer_permission(ctx: &Ctx, pid: u64, allow: Option<bool>) {
    let Some((args, deferral)) = ctx.perms.borrow_mut().remove(&pid) else { return };
    unsafe {
        let _ = args.SetState(match allow {
            Some(true) => COREWEBVIEW2_PERMISSION_STATE_ALLOW,
            Some(false) => COREWEBVIEW2_PERMISSION_STATE_DENY,
            None => COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
        });
        let _ = deferral.Complete();
    }
}

// ---------------------------------------------------------- tracking params

/// Query keys that only exist to follow you around.
const TRACKERS: &[&str] = &[
    "fbclid", "gclid", "gclsrc", "dclid", "gbraid", "wbraid", "msclkid", "mc_cid", "mc_eid", "igshid", "yclid",
    "twclid", "ttclid", "li_fat_id", "_hsenc", "_hsmi", "mkt_tok", "oly_anon_id", "oly_enc_id", "vero_id", "wickedid",
];

/// The URL without tracking parameters, or None if it had none.
pub fn clean_url(url: &str) -> Option<String> {
    if !url.starts_with("http") {
        return None;
    }
    let (head, frag) = match url.split_once('#') {
        Some((h, f)) => (h, Some(f)),
        None => (url, None),
    };
    let (base, query) = head.split_once('?')?;
    let keep: Vec<&str> = query
        .split('&')
        .filter(|kv| {
            let k = kv.split('=').next().unwrap_or("").to_ascii_lowercase();
            !(k.starts_with("utm_") || TRACKERS.contains(&k.as_str()))
        })
        .collect();
    if keep.len() == query.split('&').count() {
        return None;
    }
    let mut out = base.to_string();
    if !keep.is_empty() {
        out.push('?');
        out.push_str(&keep.join("&"));
    }
    if let Some(f) = frag {
        out.push('#');
        out.push_str(f);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{Action, clean_url, map_key, origin_of};

    #[test]
    fn ctrl_shift_n_is_a_new_private_tab() {
        let n = 'N' as u32;
        assert!(matches!(map_key(n, true, true, false), Some(Action::NewPrivate)));
        // Ctrl+Shift+N must not be mistaken for reopen (Ctrl+Shift+T).
        let t = 'T' as u32;
        assert!(matches!(map_key(t, true, true, false), Some(Action::Reopen)));
        assert!(map_key(n, true, false, false).is_none());
        assert!(map_key(n, false, true, false).is_none());
    }

    #[test]
    fn strips_trackers_only() {
        assert_eq!(clean_url("https://a.com/x?utm_source=t&id=5&fbclid=z#top").as_deref(), Some("https://a.com/x?id=5#top"));
        assert_eq!(clean_url("https://a.com/x?gclid=1").as_deref(), Some("https://a.com/x"));
        assert_eq!(clean_url("https://a.com/x?id=5"), None);
        assert_eq!(clean_url("https://a.com/x"), None);
        assert_eq!(clean_url("data:text/html,?utm_source=x"), None);
    }

    #[test]
    fn origins() {
        assert_eq!(origin_of("https://meet.example.com:8443/room?x=1"), "https://meet.example.com:8443");
    }
}

/// The Cookie header this profile would send to `url`. Never blocks: `done` runs when
/// WebView2 answers (or at once, empty, if it cannot be asked).
pub fn cookie_header(wv: &WebView, url: &str, done: impl FnOnce(String) + 'static) {
    let done = Rc::new(RefCell::new(Some(done)));
    let fire = {
        let done = done.clone();
        move |s: String| {
            if let Some(f) = done.borrow_mut().take() {
                f(s);
            }
        }
    };
    let fire2 = fire.clone();
    let asked = unsafe {
        wv.webview().cast::<ICoreWebView2_2>().and_then(|c| c.CookieManager()).and_then(|m| {
            m.GetCookies(
                &HSTRING::from(url),
                &GetCookiesCompletedHandler::create(Box::new(move |hr, list| {
                    let mut out = Vec::new();
                    if hr.is_ok()
                        && let Some(list) = list
                    {
                        let mut n = 0u32;
                        let _ = list.Count(&mut n);
                        for i in 0..n {
                            if let Ok(c) = list.GetValueAtIndex(i) {
                                let (mut a, mut b) = (PWSTR::null(), PWSTR::null());
                                let _ = c.Name(&mut a);
                                let _ = c.Value(&mut b);
                                out.push(format!("{}={}", take(a), take(b)));
                            }
                        }
                    }
                    fire2(out.join("; "));
                    Ok(())
                })),
            )
        })
    };
    if asked.is_err() {
        fire(String::new());
    }
}

/// The page's User-Agent string, so aria2 looks like the browser to the server.
pub fn user_agent(wv: &WebView) -> String {
    unsafe {
        let Ok(s2) = wv.webview().Settings().and_then(|s| s.cast::<ICoreWebView2Settings2>()) else { return String::new() };
        let mut p = PWSTR::null();
        if s2.UserAgent(&mut p).is_ok() { take(p) } else { String::new() }
    }
}

// --------------------------------------------------------------- dark pages

/// Put this page's Dark Reader in line with `ctx.dark_js`: take the old script off,
/// add the current one for every future document. `now` also redoes the page showing.
/// `then`: a URL to open once the script is in (a page built for it, not yet navigated).
pub fn set_dark(ctx: &Ctx, wv: &WebView, tab: u64, now: bool, then: Option<String>) {
    let core = wv.webview();
    let old = ctx.dark_ids.borrow_mut().remove(&tab);
    unsafe {
        if let Some(old) = old.filter(|s| !s.is_empty()) {
            let _ = core.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(old));
        }
        let js = ctx.dark_js.borrow().clone();
        match js {
            Some(js) => {
                // Mark pending; the id arrives later.
                ctx.dark_ids.borrow_mut().insert(tab, String::new());
                let ids = ctx.dark_ids.clone();
                let c = core.clone();
                let _ = core.AddScriptToExecuteOnDocumentCreated(
                    &HSTRING::from(&*js),
                    &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |hr, sid| {
                        if let Some(u) = &then {
                            let _ = c.Navigate(&HSTRING::from(u.as_str()));
                        }
                        if hr.is_ok() {
                            let mut m = ids.borrow_mut();
                            match m.get_mut(&tab) {
                                Some(slot) if slot.is_empty() => *slot = sid,
                                // Switched off (or replaced) before the id came back.
                                _ => {
                                    let _ = c.RemoveScriptToExecuteOnDocumentCreated(&HSTRING::from(sid));
                                }
                            }
                        }
                        Ok(())
                    })),
                );
                if now {
                    let _ = wv.evaluate_script(&js);
                }
            }
            None => {
                if now {
                    let _ = wv.evaluate_script(crate::pagekit::DARK_OFF);
                }
                if let Some(u) = then {
                    let _ = wv.load_url(&u);
                }
            }
        }
    }
}

// --------------------------------------------------------------- passwords

/// Offer to save passwords, and fill them in. Per profile; Windows encrypts the store.
pub fn set_passwords(wv: &WebView, on: bool) {
    unsafe {
        if let Ok(p6) = wv
            .webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|c| c.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile6>())
        {
            let _ = p6.SetIsPasswordAutosaveEnabled(on);
            let _ = p6.SetIsGeneralAutofillEnabled(on);
        }
    }
}

/// Forget every saved password in this page's profile.
pub fn clear_passwords(wv: &WebView) {
    unsafe {
        if let Ok(p2) = wv
            .webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|c| c.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile2>())
        {
            let _ = p2.ClearBrowsingData(
                COREWEBVIEW2_BROWSING_DATA_KINDS_PASSWORD_AUTOSAVE,
                &ClearBrowsingDataCompletedHandler::create(Box::new(|_| Ok(()))),
            );
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Wipe WebView2's own records for one time range. `since` in unix seconds,
/// None for everything there is. `more` also takes cookies, cache and site data;
/// passwords and autofill are never touched. True when the call was made: the
/// work itself is WebView2's, and it may outlive this function.
pub fn clear_data(wv: &WebView, since: Option<u64>, more: bool) -> bool {
    let mut kinds = COREWEBVIEW2_BROWSING_DATA_KINDS_BROWSING_HISTORY | COREWEBVIEW2_BROWSING_DATA_KINDS_DOWNLOAD_HISTORY;
    if more {
        kinds |= COREWEBVIEW2_BROWSING_DATA_KINDS_COOKIES
            | COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE
            | COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_SITE
            | COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_DOM_STORAGE
            | COREWEBVIEW2_BROWSING_DATA_KINDS_CACHE_STORAGE;
    }
    let start = since.unwrap_or(0);
    unsafe {
        let Ok(p2) = wv
            .webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|c| c.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile2>())
        else {
            return false;
        };
        p2.ClearBrowsingDataInTimeRange(
            kinds,
            start as f64,
            unix_now() as f64,
            &ClearBrowsingDataCompletedHandler::create(Box::new(|_| Ok(()))),
        )
        .is_ok()
    }
}

// ------------------------------------------------------------------ capture

// ------------------------------------------------------------ memory knobs

/// Freeze a hidden page. Scripts stop, memory is trimmed hard.
/// WebView2 resumes it by itself when shown again.
pub fn try_suspend(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>) {
    if let Ok(c3) = wv.webview().cast::<ICoreWebView2_3>() {
        unsafe {
            let _ = c3.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(move |hr, ok| {
                let _ = proxy.send_event(Ev::Suspended(id, hr.is_ok() && ok));
                Ok(())
            })));
        }
    }
}

pub fn set_muted(wv: &WebView, muted: bool) {
    if let Ok(c8) = wv.webview().cast::<ICoreWebView2_8>() {
        unsafe {
            let _ = c8.SetIsMuted(muted);
        }
    }
}

pub fn set_tracking(wv: &WebView, level: &str) {
    let lvl = match level {
        "strict" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_STRICT,
        "basic" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BASIC,
        "off" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_NONE,
        _ => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BALANCED,
    };
    unsafe {
        if let Ok(p3) = wv
            .webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|c| c.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile3>())
        {
            let _ = p3.SetPreferredTrackingPreventionLevel(lvl);
        }
    }
}

/// Glass on: DWM backdrop through black client pixels, frame stretched over the client.
/// Glass off: a plain opaque window. Cheapest thing Windows can draw.
pub fn set_glass(hwnd: isize, on: bool) {
    use windows::Win32::{
        Graphics::{
            Dwm::DwmExtendFrameIntoClientArea,
            Gdi::{RDW_ERASE, RDW_ERASENOW, RDW_INVALIDATE, RedrawWindow},
        },
        UI::Controls::MARGINS,
    };
    let hwnd = HWND(hwnd as _);
    let m = if on { -1 } else { 0 };
    let margins = MARGINS { cxLeftWidth: m, cxRightWidth: m, cyTopHeight: m, cyBottomHeight: m };
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        // Erase now: tao repaints the background color only on erase, never on its own.
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ERASENOW);
    }
}

/// Clip the page to rounded corners. `r` in physical pixels.
pub fn round_corners(wv: &WebView, r: i32) {
    unsafe {
        let mut hwnd = HWND::default();
        if wv.controller().ParentWindow(&mut hwnd).is_err() || hwnd.is_invalid() {
            return;
        }
        if r <= 0 {
            SetWindowRgn(hwnd, None, true);
            return;
        }
        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_err() {
            return;
        }
        let rgn = CreateRoundRectRgn(0, 0, rc.right + 1, rc.bottom + 1, r * 2, r * 2);
        // The window owns the region after this call. Do not free it.
        SetWindowRgn(hwnd, Some(rgn), true);
    }
}

/// Private working set of this app plus every WebView2 process it owns.
/// Same number Task Manager shows as "Memory".
pub fn memory(env: &ICoreWebView2Environment) -> (u64, u32) {
    use crate::monitor::private_ws;
    let mut total = private_ws(unsafe { GetCurrentProcess() });
    let mut count = 1u32;
    unsafe {
        let Ok(list) = env.cast::<ICoreWebView2Environment8>().and_then(|e| e.GetProcessInfos()) else {
            return (total, count);
        };
        let mut n = 0u32;
        let _ = list.Count(&mut n);
        for i in 0..n {
            let Ok(info) = list.GetValueAtIndex(i) else { continue };
            let mut pid = 0i32;
            if info.ProcessId(&mut pid).is_err() {
                continue;
            }
            if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32) {
                total += private_ws(h);
                count += 1;
                let _ = CloseHandle(h);
            }
        }
    }
    (total, count)
}
