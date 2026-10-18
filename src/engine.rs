//! Engine layer. Everything that talks to WebView2 lives here.
//! main.rs only sees WebView handles and `Ev` messages.
//! To swap engines later (Servo), replace this file.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use tao::{event_loop::EventLoopProxy, window::Window};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, AddScriptToExecuteOnDocumentCreatedCompletedHandler, BytesReceivedChangedEventHandler,
    ClearBrowsingDataCompletedHandler, ContainsFullScreenElementChangedEventHandler, DownloadStartingEventHandler,
    FocusChangedEventHandler, NavigationCompletedEventHandler, NavigationStartingEventHandler, PermissionRequestedEventHandler, ProcessFailedEventHandler,
    StateChangedEventHandler, ZoomFactorChangedEventHandler, CallDevToolsProtocolMethodCompletedHandler,
    FaviconChangedEventHandler, GetCookiesCompletedHandler, GetFaviconCompletedHandler, HistoryChangedEventHandler, IsDefaultDownloadDialogOpenChangedEventHandler,
    IsDocumentPlayingAudioChangedEventHandler, LaunchingExternalUriSchemeEventHandler, Microsoft::Web::WebView2::Win32::*,
    NewWindowRequestedEventHandler, SourceChangedEventHandler, TrySuspendCompletedHandler, WebMessageReceivedEventHandler,
    WindowCloseRequestedEventHandler,
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
            WindowsAndMessaging::{GetClientRect, HWND_TOP, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE},
        },
    },
    core::{Interface, HSTRING, PCWSTR, PWSTR},
};
use wry::{
    NewWindowResponse, PageLoadEvent, Rect, Theme, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

/// What aria2 said after an add, and how the add was started.
#[derive(Debug)]
pub struct Added {
    pub gid: String,
    pub url: String,
    pub headers: Vec<String>,
    pub name: Option<String>,
    pub private: bool,
    /// The person started it (address bar, Add link, Open torrent…, Retry):
    /// it runs at once. A page started it: it waits for Resume.
    pub person: bool,
    /// Why a page-started torrent is waiting; the paused row shows it.
    pub hold: Option<String>,
}

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
    /// A top-level navigation started: (tab, URL). The journal times it.
    NavStart(u64, String),
    /// It ended: (tab, ok, WebErrorStatus code, HTTP status). One per top-level page.
    NavDone(u64, bool, i32, i32),
    /// A server redirect began: the page is not the one that was asked for.
    NavRedirected(u64),
    Audio(u64, bool),
    FullScreen(u64, bool),
    CloseMe(u64),
    /// New popup tab, and the tab that opened it.
    Popup(u64, u64),
    Download(Dl),
    /// A site asks for a permission. Answer with `answer_permission(pid, ..)`.
    Permission { pid: u64, tab: u64, origin: String, kind: &'static str },
    /// A page tried to open an app (mailto:, spotify:, ...): the tab, the page's origin, the
    /// address, and whether a click or key started it. WebView2's own box was already cancelled.
    ExternalUri { tab: u64, origin: String, uri: String, user: bool },
    /// The person answered the question about opening an app.
    ExternalAnswer { origin: String, uri: String, answer: crate::extlink::Answer },
    /// The zoom of a page changed (a key, the wheel, or Lumen itself): (tab, factor).
    Zoom(u64, f64),
    /// A plain http page was stopped to try https: (tab, the https address, the http one).
    Upgrade(u64, String, String),
    /// Whether the site answers on the https port: (tab, https address, http address, answer).
    Probed(u64, String, String, crate::sites::Probe),
    /// A click on the "this site does not use https" page: (tab, what was chosen).
    HttpChoice(u64, crate::sites::HttpChoice),
    /// A top-level navigation hit a known malware or phishing host: (tab, the url).
    Danger(u64, String),
    /// A click on the danger page: (tab, what was chosen).
    DangerChoice(u64, crate::sites::DangerChoice),
    /// How many cookies the page's site has, for the lock's popover: (tab, count).
    SiteCookies(u64, usize),
    /// A site's stored data was cleared: (origin, it worked).
    SiteCleared(String, bool),
    /// A navigation was stopped to drop tracking parameters; load this instead.
    Redirect(u64, String),
    /// A magnet link was cancelled out of a page; hand it to aria2.
    Magnet(u64, String),
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
    /// SponsorBlock init script for this tab: (tab id, script).
    SponsorBlock(u64, String),
    /// Read aloud: the article's sentences came back: (tab, JSON [[a,b,"text"],..]).
    AloudPrep(u64, String),
    /// Read aloud: (tab, sentence index; usize::MAX means "spoken through").
    AloudLine(u64, usize),
    /// The KeePass dialogs came back: (path, master password). Quiet: a
    /// password must never reach the debug log.
    VaultOpen(String, String),
    /// The KeePass file was parsed: the vault, or why it failed. Quiet too.
    VaultReady(Result<Box<crate::kdbx::Vault>, String>),
    /// A download WebView2 gave up so aria2 can take it: (tab, url, page url, file name).
    Handoff(u64, String, String, String),
    /// aria2 took a download; `Added` says how it was started.
    AriaAdded(Box<Added>),
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
    /// Shift+click on an anchor: open it as a glance (tab id, URL).
    Glance(u64, String),
    /// The focused glance card got a key: "esc" closes it, "enter" promotes it.
    GlanceKey(&'static str),
    /// A page took keyboard focus: a click landed in its half of the stage.
    PaneFocus(u64),
    /// The split divider is being dragged: pointer x, in window coordinates.
    SplitDrag(f64),
    /// The divider was let go of.
    SplitDrop,
    /// The divider was double-clicked: back to the middle.
    SplitReset,
    /// A page answered with its language and its text: (tab, target language, JSON).
    TrPage(u64, String, String),
    /// The model for a language pair is on disk (or could not be fetched).
    TrModel(u64, bool, String),
    /// A message from the translator page: JSON, tagged with "lumen":"tr".
    TrMsg(String),
    /// Servo has work: spin its event loop. Also its page's reports to Lumen.
    #[cfg(feature = "servo")]
    ServoWake,
    #[cfg(feature = "servo")]
    ServoUrl(u64, String),
    #[cfg(feature = "servo")]
    ServoTitle(u64, String),
    #[cfg(feature = "servo")]
    ServoLoad(u64, bool),
    /// A Servo page died, or its engine gave up on it.
    #[cfg(feature = "servo")]
    ServoGone(u64),
}

/// A download's state, copied out of WebView2 for the main loop.
#[derive(Debug, Clone)]
pub struct Dl {
    pub id: u64,
    pub url: String,
    pub path: String,
    pub received: i64,
    pub total: i64,
    /// "progress" | "done" | "failed" | "paused" | "seeding"
    pub state: &'static str,
    /// Bytes per second, when known (aria2).
    pub speed: u64,
    /// A short extra, e.g. "16 connections".
    pub note: String,
    /// Started by a private tab: works, but leaves no row in downloads.json.
    pub private: bool,
    /// Display name: torrent name when known, else the file name.
    pub name: String,
    /// "file" | "torrent" | "video"
    pub kind: &'static str,
    /// Upload speed, bytes per second (aria2 torrents).
    pub up: u64,
    /// Connected peers (aria2 torrents) or connections (plain files).
    pub peers: u64,
    /// Seeder count (aria2 torrents).
    pub seeds: u64,
    /// Uploaded / downloaded. 0 until both sides have bytes.
    pub ratio: f64,
    /// Number of files (a multi-file torrent opens its folder).
    pub files: usize,
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
    /// Ctrl+Shift+R: what Lumen stopped, by day and by site.
    Report,
    /// Ctrl+Shift+D: everything at a glance.
    Dashboard,
    DevTools,
    Reader,
    Listen,
    DarkSite,
    /// Ctrl+Alt+1..4: switch to workspace N.
    Workspace(usize),
    /// Ctrl+\: two tabs side by side, or back to one.
    Split,
    /// F8: the page's text in the target language, or the original text back.
    TranslatePage,
    /// Ctrl+Shift+A: find an open tab, of any workspace.
    TabSearch,
    /// Ctrl+K: the command palette over the sidebar.
    Palette,
}

pub struct Ctx {
    pub window: Rc<Window>,
    pub proxy: EventLoopProxy<Ev>,
    pub env: RefCell<Option<ICoreWebView2Environment>>,
    pub next_id: Cell<u64>,
    /// Webviews born from window.open, waiting for the main loop to adopt them.
    pub popups: RefCell<Vec<(u64, WebView)>>,
    pub dark: Cell<bool>,
    /// Runs on every page right after it is built. Popups included. The `Who`
    /// is the tab asking: what happens here is counted against it.
    pub on_built: RefCell<Option<Box<dyn Fn(&WebView, crate::privacy_stats::Who)>>>,
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
    pub rewrite: RefCell<Option<Rc<dyn Fn(&str, crate::privacy_stats::Who) -> Option<String>>>>,
    /// Per tab: the id WebView2 gave the injected Dark Reader script.
    pub dark_ids: Rc<RefCell<HashMap<u64, String>>>,
    /// Strict isolation: one environment per agent profile, made on first use.
    pub own_envs: RefCell<HashMap<String, ICoreWebView2Environment>>,
    /// The ones being made right now. WebView2 answers on the UI thread's next turn,
    /// so nobody waits: the tick picks the answer up and builds the waiting tabs.
    pub pending_envs: RefCell<Vec<PendingEnv>>,
    /// Profiles the machine would not give a browser of their own. Asking again would
    /// fail the same way; those agents fall back to a named profile of the shared one.
    pub own_env_failed: RefCell<std::collections::HashSet<String>>,
    /// The WebView2 profile of each agent (by profile key). Kept past its tabs, because
    /// a profile outlives them: its cookies are still there when the session ends and
    /// must be wiped then.
    pub profiles: RefCell<HashMap<String, ICoreWebView2Profile>>,
    /// Plain http pages: whether to try https first, and the hosts left alone.
    pub https: Rc<RefCell<crate::sites::HttpsPolicy>>,
    /// Send the Global Privacy Control signal to pages (the header lives in the blocker).
    pub gpc: Cell<bool>,
    /// Popup requests seen since the last one was handled: (was it a click, the page's site).
    /// A sensor COM handler fills this; wry's own handler reads the front and decides.
    pub popup_info: Rc<RefCell<VecDeque<(bool, String)>>>,
}

/// A strict-isolation environment on its way. See `start_own_env` / `poll_own_envs`.
pub struct PendingEnv {
    pub key: String,
    rx: std::sync::mpsc::Receiver<Result<ICoreWebView2Environment, String>>,
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

/// The WebView2 runtime's version, for the problem report. Empty when none has started.
pub fn browser_version(env: &ICoreWebView2Environment) -> String {
    let mut p = PWSTR::null();
    if unsafe { env.BrowserVersionString(&mut p) }.is_err() {
        return String::new();
    }
    take(p)
}

pub(crate) fn take(p: PWSTR) -> String {
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
    // WebRTC must not leak the local address: only the public interface is offered.
    a.push_str(" --force-webrtc-ip-handling-policy=default_public_interface_only");
    if let Ok(extra) = std::env::var("LUMEN_ARGS") {
        a.push(' ');
        a.push_str(&extra);
    }
    a
}

/// Which WebView2 world a page lives in. Every agent session gets its own: its own cookies,
/// storage, cache and permissions, whatever the others are doing.
#[derive(Clone, Debug, PartialEq)]
pub enum Profile {
    /// The person's own pages: the unnamed profile of the shared environment.
    Main,
    /// An agent in the shared environment, in a profile of its own (`agent-<key>`).
    /// One browser process for the whole app, one cookie jar per agent.
    Shared(String),
    /// An agent with an environment of its own: its own user-data folder, so its own
    /// browser process. The hardest wall; costs about one browser process per agent.
    Own(String),
}

impl Profile {
    /// The profile key, or "" for the person's own pages.
    #[allow(dead_code)]
    pub fn key(&self) -> &str {
        match self {
            Profile::Main => "",
            Profile::Shared(k) | Profile::Own(k) => k,
        }
    }

    /// An agent page, in whichever mode.
    pub fn agent(&self) -> bool {
        !matches!(self, Profile::Main)
    }
}

/// One page. Person's pages share the UI's environment, so one browser process,
/// one GPU process, one network process for the whole app. An agent's pages live in
/// their own profile (own cookies and storage) and, with strict isolation, in their
/// own environment and user-data folder.
/// `private`: WebView2 InPrivate. No cookies outlive the last private page, and
/// nothing the page does reaches this profile's history.
pub fn build_tab(ctx: &Rc<Ctx>, id: u64, url: Option<&str>, bounds: Rect, profile: Profile, private: bool) -> wry::Result<WebView> {
    let env = ctx.env.borrow().clone();
    build_tab_in(ctx, id, url, bounds, env, &profile, private)
}

fn build_tab_in(
    ctx: &Rc<Ctx>,
    id: u64,
    url: Option<&str>,
    bounds: Rect,
    opener_env: Option<ICoreWebView2Environment>,
    profile: &Profile,
    private: bool,
) -> wry::Result<WebView> {
    let dark = ctx.dark.get();
    // Who this page is: whose tab, private or not, person's or agent's. The
    // blocker counts everything that happens here against this.
    let who = crate::privacy_stats::Who::new(id, private, profile.agent());
    let p_title = ctx.proxy.clone();
    let p_load = ctx.proxy.clone();
    let popup_ctx = ctx.clone();
    let popup_profile = profile.clone();

    // A popup lives in its opener's environment, named profile and all: the cookie jar
    // of an OAuth popup must be the one its opener is logged in with. An agent with an
    // environment of its own never takes the shared one, whoever is calling.
    let env = match profile {
        Profile::Own(key) => own_env(ctx, key),
        _ => opener_env.clone(),
    };
    // An Own-environment page with no environment ready yet: the caller was too early
    // (the door builds the page when the environment lands). Never a fallback into the
    // shared environment: that would put the agent in the person's own cookie jar.
    if matches!(profile, Profile::Own(_)) && env.is_none() {
        return Err(wry::Error::InitScriptError);
    }
    // No environment yet: this page is the first, and it makes one for everyone.
    let first = env.is_none();
    let mut wc = first.then(|| ctx.web_context.borrow_mut());
    let base = match (env, wc.as_mut()) {
        (Some(env), _) => WebViewBuilder::new().with_environment(env),
        (None, Some(wc)) => WebViewBuilder::new_with_web_context(wc).with_additional_browser_args(ctx.args.clone()),
        (None, None) => unreachable!(),
    };
    // A named profile is what keeps two agents' cookies apart inside one environment.
    // In an Own environment the whole folder is the agent's, so the default profile is fine.
    let base = match profile {
        Profile::Shared(key) => base.with_profile_name(format!("agent-{key}")),
        _ => base,
    };
    let mut b = base
        .with_bounds(bounds)
        .with_visible(false)
        .with_devtools(true)
        .with_hotkeys_zoom(true)
        .with_initialization_script(crate::blocker::COSMETIC_JS)
        .with_initialization_script(crate::glance::GLANCE_JS)
        // Pages can ask whether the person said no to being sold: navigator.globalPrivacyControl.
        // Decided when the page is built; the header follows the setting at once.
        .with_initialization_script(if ctx.gpc.get() { GPC_JS } else { "" })
        .with_initialization_script(WEBRTC_JS)
        .with_theme(theme(dark))
        .with_background_color(page_bg(dark))
        .with_incognito(private)
        .with_document_title_changed_handler(title_pacer(id, p_title))
        .with_on_page_load_handler(move |e, _| {
            let _ = p_load.send_event(Ev::Loading(id, matches!(e, PageLoadEvent::Started)));
        })
        .with_new_window_req_handler(move |uri, features| {
            // window.open / target=_blank. The sensor above already saw whether a
            // click or key asked for this. No gesture, or a popup-host match, is a
            // popup: block it and count it. A missing queue entry also blocks.
            let ctx = &popup_ctx;
            let (gesture, site) = ctx.popup_info.borrow_mut().pop_front().unwrap_or((false, String::new()));
            if !gesture || crate::blocker::popup_rule(&uri) {
                if !site.is_empty() {
                    crate::privacy_stats::popup(&site, who);
                }
                return NewWindowResponse::Deny;
            }
            let new_id = ctx.alloc_id();
            let bounds = content_bounds_now(ctx);
            match build_tab_in(ctx, new_id, None, bounds, Some(features.opener.environment.clone()), &popup_profile, private) {
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
    let agent_page = profile.agent();
    let dark_first = !agent_page && ctx.dark_js.borrow().is_some();
    if let Some(u) = url
        && !dark_first
    {
        b = b.with_url(u);
    }
    let wv = b.build_as_child(&*ctx.window)?;
    drop(wc);
    if first {
        ctx.env.replace(Some(wv.environment()));
    }
    // Tracking prevention is a setting of the profile, and InPrivate pages and the agent's
    // pages each have a profile of their own. Every page says it again; it costs nothing.
    set_tracking(&wv, &ctx.tracking.borrow());
    hook_page(&wv, id, ctx.proxy.clone(), ctx.rewrite.borrow().clone(), ctx.https.clone(), who);
    hook_zoom(&wv, id, ctx.proxy.clone());
    crate::glance::hook(&wv, id, ctx.proxy.clone());
    hook_downloads(ctx, &wv, id, private);
    hook_crash(&wv, id, ctx.proxy.clone());
    hook_permissions(ctx, &wv, id, agent_page);
    hook_external(&wv, id, ctx.proxy.clone(), agent_page);
    hook_popup_sensor(&wv, ctx.popup_info.clone());
    hook_rtc(&wv, who);
    // Password saving is per profile. An agent's profile never saves passwords,
    // and InPrivate keeps nothing anyway, so private pages get it off too.
    set_passwords(&wv, ctx.passwords.get() && !agent_page && !private);
    hook_keys(&wv, ctx.proxy.clone());
    hook_focus(&wv, id, ctx.proxy.clone());
    if crate::headless() {
        set_muted(&wv, true);
    }
    // A new view has no scripts yet; forget any id an older view of this tab had.
    ctx.dark_ids.borrow_mut().remove(&id);
    if !agent_page {
        set_dark(ctx, &wv, id, false, url.filter(|_| dark_first).map(str::to_string));
    }
    if let Some(f) = ctx.on_built.borrow().as_ref() {
        f(&wv, who);
    }
    Ok(wv)
}

// ------------------------------------------------------------ agent profiles

/// A profile key as a folder name. Keys are hex from Lumen or a client name already
/// cleaned once; this is the last line of defence before a path.
fn safe_key(key: &str) -> String {
    let k: String = key.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).take(48).collect();
    if k.is_empty() || k == "." || k == ".." { "agent".into() } else { k }
}

/// The user-data folder of one strictly isolated agent. A folder of its own means a
/// browser process of its own: its cookies, cache and everything else are its own.
pub fn own_env_dir(key: &str) -> PathBuf {
    crate::store::data_dir().join("agent_envs").join(safe_key(key))
}

/// The environment of one strictly isolated agent, if it is ready.
pub fn own_env(ctx: &Rc<Ctx>, key: &str) -> Option<ICoreWebView2Environment> {
    ctx.own_envs.borrow().get(&safe_key(key)).cloned()
}

/// Whether this machine said no to a browser of its own for this agent. Asked once,
/// not per tab: the answer will not change.
pub fn own_env_failed(ctx: &Rc<Ctx>, key: &str) -> bool {
    ctx.own_env_failed.borrow().contains(&safe_key(key))
}

/// Ask for one agent's environment. False when it is already made, already on its way,
/// already refused, or the folder cannot be made. Nothing is waited on here: WebView2
/// answers through the thread's message turn and `poll_own_envs` picks it up.
pub fn start_own_env(ctx: &Rc<Ctx>, key: &str) -> bool {
    let key = safe_key(key);
    if ctx.own_envs.borrow().contains_key(&key) || ctx.own_env_failed.borrow().contains(&key) || ctx.pending_envs.borrow().iter().any(|p| p.key == key) {
        return false;
    }
    let dir = own_env_dir(&key);
    if std::fs::create_dir_all(&dir).is_err() {
        eprintln!("lumen: strict isolation: no folder for agent {key}");
        ctx.own_env_failed.borrow_mut().insert(key);
        return false;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let args = ctx.args.clone();
    let made = unsafe {
        let options = webview2_com::CoreWebView2EnvironmentOptions::default();
        if !args.is_empty() {
            options.set_additional_browser_arguments(args);
        }
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            &HSTRING::from(dir.as_os_str()),
            &ICoreWebView2EnvironmentOptions::from(options),
            &webview2_com::CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |hr, env| {
                let out = match (hr, env) {
                    (Ok(()), Some(env)) => Ok(env),
                    (Err(e), _) => Err(e.to_string()),
                    (_, None) => Err("WebView2 gave no environment".to_string()),
                };
                let _ = tx.send(out);
                Ok(())
            })),
        )
    };
    if let Err(e) = made {
        eprintln!("lumen: strict isolation: could not ask for agent {key}'s browser: {e}");
        ctx.own_env_failed.borrow_mut().insert(key);
        return false;
    }
    ctx.pending_envs.borrow_mut().push(PendingEnv { key, rx });
    true
}

/// Whatever came back from the strict-isolation environments asked for so far. The keys
/// returned are decided: the environment is in `own_envs`, or it failed and the agent
/// falls back to a named profile of the shared environment.
pub fn poll_own_envs(ctx: &Rc<Ctx>) -> Vec<String> {
    let mut ready = Vec::new();
    let mut keep = Vec::new();
    for p in ctx.pending_envs.borrow_mut().drain(..) {
        match p.rx.try_recv() {
            Ok(Ok(env)) => {
                ctx.own_envs.borrow_mut().insert(p.key.clone(), env);
                ready.push(p.key);
            }
            Ok(Err(e)) => {
                eprintln!("lumen: strict isolation: no browser of its own for agent {}: {e}", p.key);
                ctx.own_env_failed.borrow_mut().insert(p.key.clone());
                ready.push(p.key);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => keep.push(p),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                ctx.own_env_failed.borrow_mut().insert(p.key.clone());
                ready.push(p.key);
            }
        }
    }
    *ctx.pending_envs.borrow_mut() = keep;
    ready
}

/// The WebView2 profile a page lives in. Own cookies, storage, permissions.
pub fn webview_profile(wv: &WebView) -> Option<ICoreWebView2Profile> {
    unsafe { wv.webview().cast::<ICoreWebView2_13>().ok()?.Profile().ok() }
}

/// A profile's WebView2 name ("", the unnamed one, for the person's own pages).
pub fn profile_name(profile: &ICoreWebView2Profile) -> String {
    let mut p = PWSTR::null();
    unsafe { profile.ProfileName(&mut p).ok() };
    take(p)
}

/// A profile's folder on disk. None when WebView2 will not say.
pub fn profile_path(profile: &ICoreWebView2Profile) -> Option<PathBuf> {
    let mut p = PWSTR::null();
    unsafe { profile.ProfilePath(&mut p).ok()? };
    let s = take(p);
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// Point a profile's own downloads (the ones WebView2 handles itself) at one folder.
pub fn set_profile_download_dir(wv: &WebView, dir: &std::path::Path) {
    if let Some(profile) = webview_profile(wv) {
        unsafe {
            let _ = profile.SetDefaultDownloadFolderPath(&HSTRING::from(dir.as_os_str()));
        }
    }
}

/// Wipe a profile's cookies, storage, cache and permissions. An ephemeral session is
/// over: its profile data goes with it. Asynchronous; WebView2 does it when it can.
pub fn clear_profile(profile: &ICoreWebView2Profile) {
    let Ok(p2) = profile.cast::<ICoreWebView2Profile2>() else { return };
    let done = webview2_com::ClearBrowsingDataCompletedHandler::create(Box::new(|_| Ok(())));
    unsafe {
        let _ = p2.ClearBrowsingData(COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_PROFILE, &done);
    }
}

/// Remember an agent page's profile, so its cookies and storage can be wiped when the
/// session ends. The profile outlives its tabs; the handle must too.
pub fn remember_profile(ctx: &Rc<Ctx>, key: &str, wv: &WebView) {
    if key.is_empty() {
        return;
    }
    if let Some(p) = webview_profile(wv) {
        ctx.profiles.borrow_mut().entry(safe_key(key)).or_insert(p);
    }
}

/// The end of one agent session: its profile's cookies, storage, cache and permissions
/// are gone at once, and its folder is deleted. WebView2 keeps a live profile's files
/// open for the browser's whole life, so a folder of the shared environment goes at the
/// next start, before anything has touched it (see `purge_at_start`). A session with an
/// environment of its own loses its whole user-data folder right now.
/// Ephemeral sessions are ephemeral: nothing is kept for the next run.
pub fn purge_profile(ctx: &Rc<Ctx>, key: &str, strict: bool) {
    if key.is_empty() {
        return;
    }
    let key = safe_key(key);
    if let Some(p) = ctx.profiles.borrow_mut().remove(&key) {
        clear_profile(&p);
    }
    if strict {
        // Its own environment: let go of it first. That ends the agent's browser
        // process, and with it the last holds on the folder.
        ctx.own_envs.borrow_mut().remove(&key);
        own_env_dir(&key).is_dir().then(|| delete_profile_files(own_env_dir(&key)));
    }
    queue_purge_in(&crate::store::data_dir(), &key);
}

/// The profiles whose folders are waiting for the browser that holds them to go.
fn purge_queue_path(dir: &std::path::Path) -> PathBuf {
    dir.join("agent_purge.json")
}

/// Remember one profile's folder for the next start.
fn queue_purge_in(dir: &std::path::Path, key: &str) {
    let path = purge_queue_path(dir);
    let mut list: Vec<String> = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let key = safe_key(key);
    if !list.contains(&key) {
        list.push(key);
    }
    list.truncate(200);
    let tmp = path.with_extension("tmp");
    if let Ok(json) = serde_json::to_string(&list)
        && std::fs::write(&tmp, json).is_ok()
    {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Delete the folders of sessions that ended while Lumen was running. Called once at
/// startup, before any page is built, so nothing holds those profiles yet.
pub fn purge_at_start() {
    purge_at_start_in(&crate::store::data_dir(), &crate::store::data_dir().join("WebView2").join("EBWebView"));
}

fn purge_at_start_in(dir: &std::path::Path, wv_root: &std::path::Path) {
    let path = purge_queue_path(dir);
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let _ = std::fs::remove_file(&path);
    let list: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
    for key in list {
        let key = safe_key(&key);
        for p in profile_dirs(wv_root, &key) {
            let _ = std::fs::remove_dir_all(p);
        }
        let env = dir.join("agent_envs").join(&key);
        if env.is_dir() {
            let _ = std::fs::remove_dir_all(env);
        }
    }
}

/// Every folder that looks like one profile's: WebView2's own naming, and anything in
/// the user-data folder whose name ends with this profile's name.
fn profile_dirs(wv_root: &std::path::Path, key: &str) -> Vec<PathBuf> {
    let needle = format!("agent-{}", safe_key(key));
    let mut out = vec![wv_root.join(format!("WV2Profile_{needle}"))];
    for e in std::fs::read_dir(wv_root).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(&needle) && e.path().is_dir() {
            out.push(e.path());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Delete a profile's folder, over a few tries: the browser lets go of the last files a
/// moment after its last page of that profile closes. On its own thread, so the door
/// answering in between.
pub fn delete_profile_files(dir: PathBuf) {
    let _ = std::thread::Builder::new().name("lumen-agent-purge".into()).spawn(move || {
        for _ in 0..12 {
            if !dir.exists() {
                return;
            }
            if std::fs::remove_dir_all(&dir).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(350));
        }
    });
}

/// A page of Lumen's own, off screen: for work that needs a web engine but not the
/// person's attention (offline translation). Not a tab: no blocker, no shortcuts, no
/// dark reader, and nothing shows it. It starts blank; the caller maps a host name
/// and navigates. None when no page has made an environment yet.
pub fn build_hidden(ctx: &Rc<Ctx>, bounds: Rect) -> Option<WebView> {
    let env = ctx.env.borrow().clone()?;
    let dark = ctx.dark.get();
    WebViewBuilder::new()
        .with_environment(env)
        .with_bounds(bounds)
        .with_visible(false)
        .with_theme(theme(dark))
        .with_background_color(page_bg(dark))
        .build_as_child(&*ctx.window)
        .ok()
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

/// A page may change its title a hundred times a second (a chat that streams,
/// a ticker). Every change would wake the main loop and redraw the sidebar.
/// This lets one through per `TITLE_GAP`, and a thread sends the newest one when
/// the gap is over, so the tab always ends on the last title the page set.
fn title_pacer(id: u64, proxy: EventLoopProxy<Ev>) -> impl Fn(String) + 'static {
    use crate::pacing::{TITLE_GAP, Throttle};
    use std::sync::{Arc, Mutex};
    struct State {
        gate: Throttle<String>,
        /// What the main loop was last told.
        sent: String,
        /// A thread is waiting to send the newest title.
        waiting: bool,
    }
    let st = Arc::new(Mutex::new(State { gate: Throttle::default(), sent: String::new(), waiting: false }));
    move |t: String| {
        let Ok(mut s) = st.lock() else { return };
        let now = Instant::now();
        match s.gate.offer(now, TITLE_GAP, t) {
            Some(t) => {
                if t != s.sent {
                    s.sent = t.clone();
                    let _ = proxy.send_event(Ev::Title(id, t));
                }
            }
            None if !s.waiting => {
                s.waiting = true;
                let (st, proxy) = (st.clone(), proxy.clone());
                let wait = s.gate.due(TITLE_GAP).map_or(TITLE_GAP, |d| d.saturating_duration_since(now));
                let _ = std::thread::Builder::new().name("lumen-title".into()).spawn(move || {
                    std::thread::sleep(wait + Duration::from_millis(2));
                    let Ok(mut s) = st.lock() else { return };
                    s.waiting = false;
                    if let Some(t) = s.gate.release(Instant::now(), TITLE_GAP)
                        && t != s.sent
                    {
                        s.sent = t.clone();
                        let _ = proxy.send_event(Ev::Title(id, t));
                    }
                });
            }
            None => {}
        }
    }
}

fn hook_page(
    wv: &WebView,
    id: u64,
    proxy: EventLoopProxy<Ev>,
    rewrite: Option<Rc<dyn Fn(&str, crate::privacy_stats::Who) -> Option<String>>>,
    https: Rc<RefCell<crate::sites::HttpsPolicy>>,
    who: crate::privacy_stats::Who,
) {
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
                    let _ = p.send_event(Ev::NavRedirected(id));
                    return Ok(());
                }
                let mut u = PWSTR::null();
                args.Uri(&mut u)?;
                let u = take(u);
                // A magnet link is not a page: cancel and hand it to aria2.
                if u.starts_with("magnet:") {
                    args.SetCancel(true)?;
                    let _ = p.send_event(Ev::Magnet(id, u));
                    return Ok(());
                }
                // A click on the warning page for a site without https. Not a navigation.
                if u.starts_with("lumen-http:") {
                    args.SetCancel(true)?;
                    if let Some(choice) = crate::sites::parse_http_choice(&u) {
                        let _ = p.send_event(Ev::HttpChoice(id, choice));
                    }
                    return Ok(());
                }
                // A click on the page that blocks a dangerous site. Not a navigation.
                if u.starts_with("lumen-danger:") {
                    args.SetCancel(true)?;
                    if let Some(choice) = crate::sites::parse_danger_choice(&u) {
                        let _ = p.send_event(Ev::DangerChoice(id, choice));
                    }
                    return Ok(());
                }
                // A site on the malware or phishing lists: stop and show the warning.
                // Before the redirect check, so a listed link is never followed.
                if crate::blocker::danger_host(&u) {
                    args.SetCancel(true)?;
                    let _ = p.send_event(Ev::Danger(id, u));
                    return Ok(());
                }
                // Plain http: stop, and see whether the site speaks https. The main loop goes on.
                if let Some(secure) = crate::sites::https_upgrade(&u, &https.borrow()) {
                    args.SetCancel(true)?;
                    crate::privacy_stats::https_upgrade(&crate::privacy_stats::host(&u), who);
                    let _ = p.send_event(Ev::Upgrade(id, secure, u));
                    return Ok(());
                }
                // The journal times the page from here to NavigationCompleted.
                let _ = p.send_event(Ev::NavStart(id, u.clone()));
                let clean = clean_url(&u);
                let listed = rewrite.as_ref().and_then(|f| f(clean.as_deref().unwrap_or(&u), who));
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

        // The other end of the journal's timing: whether the page came, and with
        // what. The second interface carries the HTTP status when there was one.
        let p = proxy.clone();
        let _ = core.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut ok = Default::default();
                let _ = args.IsSuccess(&mut ok);
                let mut err = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                let _ = args.WebErrorStatus(&mut err);
                let mut http = 0i32;
                if let Ok(second) = args.cast::<ICoreWebView2NavigationCompletedEventArgs2>() {
                    let _ = second.HttpStatusCode(&mut http);
                }
                let _ = p.send_event(Ev::NavDone(id, ok.as_bool(), err.0, http));
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

/// The page took keyboard focus. In a split, that is a click landing in one half
/// of the stage: the main loop moves the pane the person is working in over there.
fn hook_focus(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>) {
    let mut token = 0i64;
    unsafe {
        let _ = wv.controller().add_GotFocus(
            &FocusChangedEventHandler::create(Box::new(move |_, _| {
                let _ = proxy.send_event(Ev::PaneFocus(id));
                Ok(())
            })),
            &mut token,
        );
    }
}

// ---------------------------------------------------------------------- zoom

/// WebView2 zooms by itself (Ctrl+plus, Ctrl+minus, Ctrl+0, Ctrl+wheel, pinch). This only
/// listens: the main loop remembers the factor for the site and shows it in the address bar.
fn hook_zoom(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>) {
    let mut token = 0i64;
    unsafe {
        let _ = wv.controller().add_ZoomFactorChanged(
            &ZoomFactorChangedEventHandler::create(Box::new(move |c, _| {
                if let Some(c) = c {
                    let mut f = 1.0f64;
                    if c.ZoomFactor(&mut f).is_ok() {
                        let _ = proxy.send_event(Ev::Zoom(id, f));
                    }
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

/// The page's zoom factor now (1.0 is 100%).
pub fn zoom_factor(wv: &WebView) -> Option<f64> {
    let mut f = 1.0f64;
    unsafe { wv.controller().ZoomFactor(&mut f).ok().map(|_| f) }
}

/// Zoom the page. True when WebView2 took it.
pub fn set_zoom_factor(wv: &WebView, f: f64) -> bool {
    unsafe { wv.controller().SetZoomFactor(f).is_ok() }
}

// ------------------------------------------------------------ site data

/// How many cookies this profile would send to `url`. Never blocks; 0 when it cannot be asked.
pub fn cookie_count(wv: &WebView, url: &str, done: impl FnOnce(usize) + 'static) {
    cookie_list(wv, url, move |list| done(list.len()));
}

/// The cookies for a URL, as WebView2's objects.
fn cookie_list(wv: &WebView, url: &str, done: impl FnOnce(Vec<ICoreWebView2Cookie>) + 'static) {
    let done = Rc::new(RefCell::new(Some(done)));
    let fire = {
        let done = done.clone();
        move |v: Vec<ICoreWebView2Cookie>| {
            if let Some(f) = done.borrow_mut().take() {
                f(v);
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
                                out.push(c);
                            }
                        }
                    }
                    fire2(out);
                    Ok(())
                })),
            )
        })
    };
    if asked.is_err() {
        fire(Vec::new());
    }
}

/// Forget what one site stored here: its cookies, and its other storage by origin (local
/// storage, IndexedDB, caches, service workers) through the DevTools Protocol. `wv` must be a
/// page of the profile that holds the data and never an InPrivate page: that store goes away
/// with its last page anyway. `done` runs when the storage call has answered.
pub fn clear_site(wv: &WebView, origin: &str, done: impl FnOnce(bool) + 'static) {
    let url = format!("{}/", origin.trim_end_matches('/'));
    let core = wv.webview();
    let origin = origin.to_string();
    cookie_list(wv, &url, move |list| unsafe {
        if let Ok(m) = core.cast::<ICoreWebView2_2>().and_then(|c| c.CookieManager()) {
            for c in &list {
                let _ = m.DeleteCookie(c);
            }
        }
        let params = serde_json::json!({"origin": origin, "storageTypes": "all"}).to_string();
        let _ = core.CallDevToolsProtocolMethod(
            &HSTRING::from("Storage.clearDataForOrigin"),
            &HSTRING::from(params),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr, _| {
                done(hr.is_ok());
                Ok(())
            })),
        );
    });
}

/// In every page when the signal is on: the property sites read the Global Privacy Control
/// answer from, and the older Do Not Track one.
const GPC_JS: &str = "try{Object.defineProperty(Navigator.prototype,'globalPrivacyControl',{get:()=>true,configurable:true});Object.defineProperty(Navigator.prototype,'doNotTrack',{get:()=>'1',configurable:true})}catch(e){}";

/// In every page: watch WebRTC so a peer connection that would leak the local
/// address is counted. The Chromium switch keeps the address off the wire; this
/// script only reports that a page tried.
const WEBRTC_JS: &str = "try{const O=window.RTCPeerConnection;if(O){window.RTCPeerConnection=function(c,i){const pc=new O(c,i);let n=false;pc.addEventListener('icecandidate',e=>{if(!n&&e.candidate){n=true;try{window.chrome.webview.postMessage({lumen:'rtc'})}catch(_){}}});return pc};window.RTCPeerConnection.prototype=O.prototype}}catch(e){}";

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
    const F7: u32 = 0x76;
    const F8: u32 = 0x77;
    const F9: u32 = 0x78;
    const F11: u32 = 0x7A;
    const F12: u32 = 0x7B;
    const ESC: u32 = 0x1B;
    const COMMA: u32 = 0xBC;
    // The \ key. A layout may move it, but Windows keeps the code.
    const BACKSLASH: u32 = 0xDC;
    let k = |c: char| c as u32;
    Some(match (ctrl, shift, alt, vk) {
        (true, false, false, v) if v == k('T') => NewTab,
        (true, true, false, v) if v == k('N') => NewPrivate,
        (true, true, false, v) if v == k('T') => Reopen,
        (true, false, false, v) if v == k('W') => CloseTab,
        (true, false, false, v) if v == k('L') => Omnibox,
        // Ctrl+K used to open the address bar with Ctrl+L; the palette has it now.
        (true, false, false, v) if v == k('K') => Palette,
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
        (false, false, false, F7) => Listen,
        (false, false, false, F8) => TranslatePage,
        (false, true, false, F9) => DarkSite,
        (false, true, false, ESC) => Control,
        (true, false, false, BACKSLASH) => Split,
        (true, false, false, v) if v == k('J') => Downloads,
        (true, false, false, v) if v == k('D') => Bookmark,
        (true, false, false, v) if v == k('H') => History,
        (true, true, false, v) if v == k('O') => Bookmarks,
        (true, true, false, v) if v == k('R') => Report,
        (true, true, false, v) if v == k('D') => Dashboard,
        // Not the page's own Ctrl+A: that is a text selection.
        (true, true, false, v) if v == k('A') => TabSearch,
        (true, true, false, v) if v == k('I') => DevTools,
        (true, false, true, v) if (k('1')..=k('4')).contains(&v) => Workspace((v - k('1')) as usize),
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
        Dl {
            id,
            url: take(uri),
            path: take(path),
            received: got,
            total,
            state,
            speed: 0,
            note: String::new(),
            private,
            name: String::new(),
            kind: "file",
            up: 0,
            peers: 0,
            seeds: 0,
            ratio: 0.0,
            files: 1,
        }
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

/// A page tries to open an app (mailto:, zoommtg:, spotify:...). WebView2 would show a box of its
/// own; Lumen cancels it and sends the request to the main loop, which asks the person (or
/// remembers they said always). An agent's profile never launches anything.
fn hook_external(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>, isolated: bool) {
    let Ok(c18) = wv.webview().cast::<ICoreWebView2_18>() else { return };
    let mut token = 0i64;
    unsafe {
        let _ = c18.add_LaunchingExternalUriScheme(
            &LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                args.SetCancel(true)?;
                if isolated {
                    return Ok(());
                }
                let (mut u, mut o) = (PWSTR::null(), PWSTR::null());
                args.Uri(&mut u)?;
                let _ = args.InitiatingOrigin(&mut o);
                let mut user = windows::core::BOOL::default();
                let _ = args.IsUserInitiated(&mut user);
                let _ = proxy.send_event(Ev::ExternalUri { tab: id, origin: origin_of(&take(o)), uri: take(u), user: user.as_bool() });
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

// ------------------------------------------------------------- popup sensor

/// The COM NewWindowRequested handler sees whether a click or key asked for the
/// new window; wry's own handler does not. This runs first (registered after
/// build, and wry's callback is dispatched later on the message loop), so the
/// flag is in the queue when the wry handler asks for it.
fn hook_popup_sensor(wv: &WebView, queue: Rc<RefCell<VecDeque<(bool, String)>>>) {
    let core = wv.webview();
    let mut token = 0i64;
    unsafe {
        let _ = core.add_NewWindowRequested(
            &NewWindowRequestedEventHandler::create(Box::new(move |sender, args| {
                let Some(args) = args else { return Ok(()) };
                let mut user = windows::core::BOOL::default();
                let _ = args.IsUserInitiated(&mut user);
                let site = sender
                    .and_then(|s| {
                        let mut p = PWSTR::null();
                        s.Source(&mut p).ok()?;
                        Some(crate::privacy_stats::host(&take(p)))
                    })
                    .unwrap_or_default();
                let mut q = queue.borrow_mut();
                // Bound: a page that spams window.open must not grow this forever.
                if q.len() < 32 {
                    q.push_back((user.as_bool(), site));
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

// ------------------------------------------------------------------- webrtc

/// The page's WebRTC script reports a peer connection here. The Chromium switch
/// keeps the local address off the wire; this only counts the attempt.
fn hook_rtc(wv: &WebView, who: crate::privacy_stats::Who) {
    let core = wv.webview();
    let mut token = 0i64;
    unsafe {
        let _ = core.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut j = PWSTR::null();
                if args.WebMessageAsJson(&mut j).is_err() {
                    return Ok(());
                }
                let Ok(msg) = serde_json::from_str::<serde_json::Value>(&take(j)) else { return Ok(()) };
                if msg["lumen"] == "rtc" {
                    let mut src = PWSTR::null();
                    let site = args.Source(&mut src).map(|_| crate::privacy_stats::host(&take(src))).unwrap_or_default();
                    crate::privacy_stats::rtc(&site, who);
                }
                Ok(())
            })),
            &mut token,
        );
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
    use super::{Action, clean_url, map_key, origin_of, purge_at_start_in, purge_queue_path, queue_purge_in, safe_key};

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
    fn ctrl_k_is_the_command_palette_and_ctrl_l_the_address_bar() {
        let k = 'K' as u32;
        let l = 'L' as u32;
        assert!(matches!(map_key(k, true, false, false), Some(Action::Palette)));
        assert!(matches!(map_key(l, true, false, false), Some(Action::Omnibox)));
        // F6 still opens the address bar on its own.
        assert!(matches!(map_key(0x75, false, false, false), Some(Action::Omnibox)));
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

    /// A session that ended queues its profile: the next start deletes the folder, both
    /// WebView2's own naming and any other that ends with the profile's name.
    #[test]
    fn finished_profiles_are_deleted_at_the_next_start() {
        let dir = std::env::temp_dir().join(format!("lumen-purge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("WebView2").join("EBWebView");
        std::fs::create_dir_all(&root).unwrap();
        let key = "a1b2c3d4e5f6";
        let mine = root.join(format!("WV2Profile_agent-{key}"));
        let its = root.join("WV2Profile_agent-other");
        let unrelated = root.join("Default");
        for d in [&mine, &its, &unrelated] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("Cookies"), "x").unwrap();
        }
        queue_purge_in(&dir, key);
        // Twice: one profile is one entry.
        queue_purge_in(&dir, key);
        assert_eq!(std::fs::read_to_string(purge_queue_path(&dir)).unwrap(), format!("[\"{key}\"]"));
        purge_at_start_in(&dir, &root);
        assert!(!mine.exists(), "the ended session's folder is gone");
        assert!(its.exists() && unrelated.exists(), "no other profile was touched");
        assert!(!purge_queue_path(&dir).exists(), "the queue is emptied");
        // A queue file that is junk does nothing at all.
        std::fs::write(purge_queue_path(&dir), "{not json").unwrap();
        purge_at_start_in(&dir, &root);
        assert!(its.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A profile key can never name a folder somewhere else.
    #[test]
    fn a_profile_key_names_one_folder() {
        assert_eq!(safe_key("a1b2"), "a1b2");
        // Separators never survive, and ".." is a name like any other: one folder, here.
        assert_eq!(safe_key("../../windows"), "....windows");
        for bad in ["..", "../..", "a\\b", "C:/x", "", "  "] {
            let k = safe_key(bad);
            assert!(!k.is_empty() && !k.contains(['/', '\\']), "{bad:?} -> {k:?}");
        }
        assert_eq!(safe_key(".."), "agent");
        assert_eq!(safe_key("a\\b/c:d"), "abcd");
        let long = safe_key(&"z".repeat(100));
        assert!(!long.is_empty() && long.len() <= 48);
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

/// Thaw a page `try_suspend` froze.
pub fn resume(wv: &WebView) {
    if let Ok(c3) = wv.webview().cast::<ICoreWebView2_3>() {
        unsafe {
            let _ = c3.Resume();
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

/// Bring a page's window above its siblings. The glance card sits over the
/// chrome's dim overlay, which is itself raised over the page.
pub fn raise(wv: &WebView) {
    unsafe {
        let mut hwnd = HWND::default();
        if wv.controller().ParentWindow(&mut hwnd).is_err() || hwnd.is_invalid() {
            return;
        }
        let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Private working set of this app plus every WebView2 process it owns.
/// Same number Task Manager shows as "Memory".
pub fn memory(env: &ICoreWebView2Environment) -> (u64, u32) {
    let (_, total, count, _) = memory_with_cpu(Some(env), false);
    (total, count)
}

/// One walk of the process tree, for everyone who needs a number from it:
/// (this process alone, the whole tree, process count, the tree's CPU time in
/// 100 ns units). `cpu` costs a `GetProcessTimes` per process, so the callers
/// that do not need it do not ask: there is no second walk for the journal.
pub fn memory_with_cpu(env: Option<&ICoreWebView2Environment>, cpu: bool) -> (u64, u64, u32, u64) {
    use crate::monitor::{cpu_time, private_ws};
    let me = unsafe { GetCurrentProcess() };
    let own = private_ws(me);
    let mut total = own;
    let mut ticks = if cpu { cpu_time(me) } else { 0 };
    let mut count = 1u32;
    // No page has been made yet (the start page is Lumen's own drawing): no WebView2
    // processes exist, and Lumen alone is the whole figure.
    let Some(env) = env else { return (own, total, count, ticks) };
    unsafe {
        let Ok(list) = env.cast::<ICoreWebView2Environment8>().and_then(|e| e.GetProcessInfos()) else {
            return (own, total, count, ticks);
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
                if cpu {
                    ticks += cpu_time(h);
                }
                count += 1;
                let _ = CloseHandle(h);
            }
        }
    }
    (own, total, count, ticks)
}
