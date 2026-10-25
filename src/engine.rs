//! Engine layer.

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
    WindowCloseRequestedEventHandler, BasicAuthenticationRequestedEventHandler, ContextMenuRequestedEventHandler,
    ScriptDialogOpeningEventHandler, StatusBarTextChangedEventHandler,
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

#[derive(Debug)]
pub struct Added {
    pub gid: String,
    pub url: String,
    pub headers: Vec<String>,
    pub name: Option<String>,
    pub private: bool,
    pub person: bool,
    pub hold: Option<String>,
}

#[derive(Debug)]
pub enum Ev {
    Ui(String),
    Title(u64, String),
    Url(u64, String),
    Loading(u64, bool),
    Favicon(u64, String),
    Nav(u64, bool, bool),
    NavStart(u64, String),
    NavDone(u64, bool, i32, i32),
    NavRedirected(u64),
    Audio(u64, bool),
    Call(u64, bool),
    PageAsk { aid: u64, tab: u64, origin: String, ask: Ask },
    PageMenu { aid: u64, tab: u64, x: f32, y: f32, items: Vec<(i32, String, bool)>, link: String, selection: String },
    LinkHover(u64, String),
    SignInAnswer(u64, Option<(String, String)>),
    LowMemory,
    FullScreen(u64, bool),
    CloseMe(u64),
    Popup(u64, u64),
    PopupBlocked(u64, String, String),
    Hello(&'static str, u64, crate::hello::Verdict),
    HelloReady(bool),
    RedirectBlocked(u64, String, String),
    Download(Dl),
    Permission { pid: u64, tab: u64, origin: String, kind: &'static str },
    ExternalUri { tab: u64, origin: String, uri: String, user: bool },
    ExternalAnswer { origin: String, uri: String, answer: crate::extlink::Answer },
    Zoom(u64, f64),
    Upgrade(u64, String, String),
    Probed(u64, String, String, crate::sites::Probe),
    HttpChoice(u64, crate::sites::HttpChoice),
    Danger(u64, String),
    DangerChoice(u64, crate::sites::DangerChoice),
    ErrorChoice(u64, crate::sites::ErrorChoice),
    SiteCookies(u64, usize),
    SiteCleared(String, bool),
    Redirect(u64, String),
    Magnet(u64, String, bool),
    MagnetSettled(u64, String),
    Capture(u64, String),
    PageText(u64, String, String),
    TextHits(String, Vec<crate::recall::Hit>),
    ShowMe,
    Orphan(u64),
    Video(u64, crate::video::Up),
    Reader(u64, String, String),
    SponsorBlock(u64, String),
    AloudPrep(u64, String),
    AloudLine(u64, usize),
    VaultOpen(String, String),
    VaultReady(Result<Box<crate::kdbx::Vault>, String>),
    Handoff(u64, String, String, String),
    AriaAdded(Box<Added>),
    AriaFailed(u64, String, bool),
    AriaJobs(Vec<crate::aria::Job>),
    Agent(crate::agent::Call),
    Suspended(u64, bool),
    Key(Action),
    Usage(Vec<crate::monitor::Proc>),
    Crashed(u64, bool),
    Glance(u64, String),
    GlanceKey(&'static str),
    PaneFocus(u64),
    SplitDrag(f64),
    SplitDrop,
    SplitReset,
    TrPage(u64, String, String),
    Doctor(Box<crate::doctor::Report>),
    TrModel(u64, bool, String),
    TrMsg(String),
    #[cfg(feature = "servo")]
    ServoWake,
    #[cfg(feature = "servo")]
    ServoUrl(u64, String),
    #[cfg(feature = "servo")]
    ServoTitle(u64, String),
    #[cfg(feature = "servo")]
    ServoLoad(u64, bool),
    #[cfg(feature = "servo")]
    ServoGone(u64),
}

#[derive(Debug, Clone)]
pub struct Dl {
    pub id: u64,
    pub url: String,
    pub path: String,
    pub received: i64,
    pub total: i64,
    pub state: &'static str,
    pub speed: u64,
    pub note: String,
    pub private: bool,
    pub name: String,
    pub kind: &'static str,
    pub up: u64,
    pub peers: u64,
    pub seeds: u64,
    pub ratio: f64,
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
    Report,
    Dashboard,
    DevTools,
    Reader,
    Listen,
    DarkSite,
    Workspace(usize),
    Split,
    TranslatePage,
    TabSearch,
    Palette,
}

pub struct Ctx {
    pub window: Rc<Window>,
    pub proxy: EventLoopProxy<Ev>,
    pub env: RefCell<Option<ICoreWebView2Environment>>,
    pub next_id: Cell<u64>,
    pub popups: RefCell<Vec<(u64, WebView)>>,
    pub dark: Cell<bool>,
    pub on_built: RefCell<Option<Box<dyn Fn(&WebView, crate::privacy_stats::Who)>>>,
    pub web_context: RefCell<wry::WebContext>,
    pub args: String,
    pub tracking: RefCell<String>,
    pub downloads: RefCell<HashMap<u64, ICoreWebView2DownloadOperation>>,
    pub passwords: Cell<bool>,
    pub perms: RefCell<HashMap<u64, (ICoreWebView2PermissionRequestedEventArgs, ICoreWebView2Deferral)>>,
    pub waiting: RefCell<HashMap<u64, Waiting>>,
    pub aria_on: Cell<bool>,
    pub bypass: RefCell<std::collections::HashSet<String>>,
    pub dark_js: RefCell<Option<Rc<str>>>,
    pub rewrite: RefCell<Option<Rc<dyn Fn(&str, crate::privacy_stats::Who) -> Option<String>>>>,
    pub dark_ids: Rc<RefCell<HashMap<u64, String>>>,
    pub own_envs: RefCell<HashMap<String, ICoreWebView2Environment>>,
    pub pending_envs: RefCell<Vec<PendingEnv>>,
    pub own_env_failed: RefCell<std::collections::HashSet<String>>,
    pub profiles: RefCell<HashMap<String, ICoreWebView2Profile>>,
    pub https: Rc<RefCell<crate::sites::HttpsPolicy>>,
    pub gpc: Cell<bool>,
    pub userscripts: Cell<bool>,
    pub popup_info: Rc<RefCell<VecDeque<(bool, String, u64)>>>,
    pub popup_policy: RefCell<crate::popup::Policy>,
    pub redirect_policy: Rc<RefCell<crate::popup::RedirectPolicy>>,
    pub docs: Rc<RefCell<HashMap<u64, Doc>>>,
    pub redirect_pass: Rc<RefCell<Option<(u64, String)>>>,
    pub api_nav: Rc<RefCell<HashMap<u64, Instant>>>,
    pub clicks: Rc<RefCell<HashMap<u64, crate::popup::Page>>>,
    pub popup_js: String,
    pub popup_token: String,
}

pub struct PendingEnv {
    pub key: String,
    rx: std::sync::mpsc::Receiver<Result<ICoreWebView2Environment, String>>,
}

#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub url: String,
    pub loaded: Option<Instant>,
}

impl Ctx {
    pub fn alloc_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }
}

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

pub fn runtime_version() -> Option<String> {
    if cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_NO_WEBVIEW2").is_some() {
        return None;
    }
    let mut v = PWSTR::null();
    unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut v) }.ok()?;
    let text = take(v);
    (!text.is_empty()).then_some(text)
}

pub fn webview2_present() -> bool {
    runtime_version().is_some()
}

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

fn page_bg(dark: bool) -> (u8, u8, u8, u8) {
    if dark { (22, 20, 19, 255) } else { (247, 244, 240, 255) }
}

const UNNEEDED: &[&str] = &[
    "SpareRendererForSitePerProcess",
    "BackForwardCache",
    "Translate",
    "MediaRouter",
    "InterestFeedContentSuggestions",
    "AutofillServerCommunication",
    "OptimizationHints",
    "msWebView2EnableShoppingFeatures",
    "msEdgeCollections",
    "msEdgeRewards",
    "msEdgeWallet",
];

fn init_on(name: &str) -> bool {
    match std::env::var("LUMEN_TEST_NOINIT") {
        Ok(list) => !list.split(',').any(|s| s.trim().eq_ignore_ascii_case(name)),
        Err(_) => true,
    }
}

pub fn browser_args(gpu_in_process: bool, share_processes: bool) -> String {
    let mut a = String::from("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    for f in UNNEEDED {
        a.push(',');
        a.push_str(f);
    }
    if crate::headless() {
        a.push_str(",CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows");
    }
    if gpu_in_process {
        a.push_str(" --in-process-gpu --enable-features=NetworkServiceInProcess2");
    }
    if share_processes {
        a.push_str(" --process-per-site");
    }
    a.push_str(" --force-webrtc-ip-handling-policy=default_public_interface_only");
    if let Ok(extra) = std::env::var("LUMEN_ARGS") {
        a.push(' ');
        a.push_str(&extra);
    }
    a
}

#[derive(Clone, Debug, PartialEq)]
pub enum Profile {
    Main,
    Shared(String),
    Own(String),
}

impl Profile {
    #[allow(dead_code)]
    pub fn key(&self) -> &str {
        match self {
            Profile::Main => "",
            Profile::Shared(k) | Profile::Own(k) => k,
        }
    }

    pub fn agent(&self) -> bool {
        !matches!(self, Profile::Main)
    }
}

pub fn build_tab(ctx: &Rc<Ctx>, id: u64, url: Option<&str>, bounds: Rect, profile: Profile, private: bool, agent_tab: bool) -> wry::Result<WebView> {
    let env = ctx.env.borrow().clone();
    build_tab_in(ctx, id, url, bounds, env, &profile, private, agent_tab)
}

fn build_tab_in(
    ctx: &Rc<Ctx>,
    id: u64,
    url: Option<&str>,
    bounds: Rect,
    opener_env: Option<ICoreWebView2Environment>,
    profile: &Profile,
    private: bool,
    agent_tab: bool,
) -> wry::Result<WebView> {
    let dark = ctx.dark.get();
    let who = crate::privacy_stats::Who::new(id, private, profile.agent());
    let p_title = ctx.proxy.clone();
    let p_load = ctx.proxy.clone();
    let popup_ctx = ctx.clone();
    let popup_profile = profile.clone();

    let env = match profile {
        Profile::Own(key) => own_env(ctx, key),
        _ => opener_env.clone(),
    };
    if matches!(profile, Profile::Own(_)) && env.is_none() {
        return Err(wry::Error::InitScriptError);
    }
    let first = env.is_none();
    let mut wc = first.then(|| ctx.web_context.borrow_mut());
    let base = match (env, wc.as_mut()) {
        (Some(env), _) => WebViewBuilder::new().with_environment(env),
        (None, Some(wc)) => WebViewBuilder::new_with_web_context(wc).with_additional_browser_args(ctx.args.clone()),
        (None, None) => unreachable!(),
    };
    let base = match profile {
        Profile::Shared(key) => base.with_profile_name(format!("agent-{key}")),
        _ => base,
    };
    let user_js = if !agent_tab && !private && ctx.userscripts.get() { crate::userscripts::bootstrap() } else { Vec::new() };
    let mut b = base
        .with_bounds(bounds)
        .with_visible(false)
        .with_devtools(true)
        .with_hotkeys_zoom(true)
        .with_initialization_script(if init_on("COSMETIC") { crate::blocker::COSMETIC_JS } else { "" })
        .with_initialization_script(if init_on("GLANCE") { crate::glance::GLANCE_JS } else { "" })
        .with_initialization_script(if ctx.gpc.get() { GPC_JS } else { "" })
        .with_initialization_script(if init_on("WEBRTC") { WEBRTC_JS } else { "" })
        .with_initialization_script(if init_on("CALL") { CALL_JS } else { "" })
        .with_initialization_script(if init_on("POPUP") { ctx.popup_js.as_str() } else { "" })
        .with_theme(theme(dark))
        .with_background_color(page_bg(dark))
        .with_incognito(private)
        .with_document_title_changed_handler(title_pacer(id, p_title))
        .with_on_page_load_handler(move |e, _| {
            let _ = p_load.send_event(Ev::Loading(id, matches!(e, PageLoadEvent::Started)));
        })
        .with_new_window_req_handler(move |uri, features| {
            let ctx = &popup_ctx;
            let (gesture, site, tab) = ctx.popup_info.borrow_mut().pop_front().unwrap_or((false, String::new(), 0));
            let target = crate::privacy_stats::host(&uri);
            let page = ctx.clicks.borrow().get(&tab).cloned().unwrap_or_default();
            let ask = crate::popup::Ask {
                gesture,
                link: crate::popup::link_click(&page, &target),
                opener: site.clone(),
                target: target.clone(),
                popup_rule: crate::blocker::popup_rule(&uri),
            };
            let (block, trusted) = {
                let p = ctx.popup_policy.borrow();
                (p.block, p.trusted.clone())
            };
            if crate::popup::decide(&ask, block, &trusted) == crate::popup::Verdict::Deny {
                if !site.is_empty() {
                    crate::privacy_stats::popup(&site, who);
                }
                let _ = ctx.proxy.send_event(Ev::PopupBlocked(tab, target, site));
                return NewWindowResponse::Deny;
            }
            let new_id = ctx.alloc_id();
            let bounds = content_bounds_now(ctx);
            match build_tab_in(ctx, new_id, None, bounds, Some(features.opener.environment.clone()), &popup_profile, private, agent_tab) {
                Ok(wv) => {
                    let core = wv.webview();
                    ctx.popups.borrow_mut().push((new_id, wv));
                    let _ = ctx.proxy.send_event(Ev::Popup(new_id, id));
                    NewWindowResponse::Create { webview: core }
                }
                Err(_) => NewWindowResponse::Deny,
            }
        });
    for js in &user_js {
        b = b.with_initialization_script(js.as_str());
    }
    let agent_page = profile.agent();
    let dark_first = !agent_page && ctx.dark_js.borrow().is_some();
    if let Some(u) = url
        && !dark_first
    {
        b = b.with_url(u);
    }
    let wv = b.build_as_child(&*ctx.window)?;
    clip_siblings(&wv);
    drop(wc);
    if first {
        ctx.env.replace(Some(wv.environment()));
    }
    set_tracking(&wv, &ctx.tracking.borrow());
    hook_page(
        &wv,
        id,
        ctx.proxy.clone(),
        ctx.rewrite.borrow().clone(),
        NavRules {
            https: ctx.https.clone(),
            redirect: ctx.redirect_policy.clone(),
            docs: ctx.docs.clone(),
            pass: ctx.redirect_pass.clone(),
            api_nav: ctx.api_nav.clone(),
        },
        ctx.clicks.clone(),
        who,
    );
    hook_zoom(&wv, id, ctx.proxy.clone());
    crate::glance::hook(&wv, id, ctx.proxy.clone());
    hook_downloads(ctx, &wv, id, private);
    hook_crash(&wv, id, ctx.proxy.clone());
    hook_permissions(ctx, &wv, id, agent_page);
    if !agent_page {
        hook_page_asks(ctx, &wv, id);
    }
    hook_external(&wv, id, ctx.proxy.clone(), agent_page);
    hook_popup_sensor(&wv, ctx.popup_info.clone(), id);
    hook_popup_page(&wv, id, ctx);
    hook_rtc(&wv, who);
    hook_call(&wv, id, ctx.proxy.clone());
    set_passwords(&wv, ctx.passwords.get() && !agent_page && !private);
    hook_keys(&wv, ctx.proxy.clone());
    hook_focus(&wv, id, ctx.proxy.clone());
    if crate::headless() {
        set_muted(&wv, true);
    }
    ctx.dark_ids.borrow_mut().remove(&id);
    if !agent_page {
        set_dark(ctx, &wv, id, false, url.filter(|_| dark_first).map(str::to_string));
    }
    if let Some(f) = ctx.on_built.borrow().as_ref() {
        f(&wv, who);
    }
    Ok(wv)
}

fn safe_key(key: &str) -> String {
    let k: String = key.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).take(48).collect();
    if k.is_empty() || k == "." || k == ".." { "agent".into() } else { k }
}

pub fn own_env_dir(key: &str) -> PathBuf {
    crate::store::data_dir().join("agent_envs").join(safe_key(key))
}

pub fn own_env(ctx: &Rc<Ctx>, key: &str) -> Option<ICoreWebView2Environment> {
    ctx.own_envs.borrow().get(&safe_key(key)).cloned()
}

pub fn own_env_failed(ctx: &Rc<Ctx>, key: &str) -> bool {
    ctx.own_env_failed.borrow().contains(&safe_key(key))
}

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

pub fn webview_profile(wv: &WebView) -> Option<ICoreWebView2Profile> {
    unsafe { wv.webview().cast::<ICoreWebView2_13>().ok()?.Profile().ok() }
}

pub fn profile_name(profile: &ICoreWebView2Profile) -> String {
    let mut p = PWSTR::null();
    unsafe { profile.ProfileName(&mut p).ok() };
    take(p)
}

pub fn profile_path(profile: &ICoreWebView2Profile) -> Option<PathBuf> {
    let mut p = PWSTR::null();
    unsafe { profile.ProfilePath(&mut p).ok()? };
    let s = take(p);
    (!s.is_empty()).then(|| PathBuf::from(s))
}

pub fn set_profile_download_dir(wv: &WebView, dir: &std::path::Path) {
    if let Some(profile) = webview_profile(wv) {
        unsafe {
            let _ = profile.SetDefaultDownloadFolderPath(&HSTRING::from(dir.as_os_str()));
        }
    }
}

pub fn clear_profile(profile: &ICoreWebView2Profile) {
    let Ok(p2) = profile.cast::<ICoreWebView2Profile2>() else { return };
    let done = webview2_com::ClearBrowsingDataCompletedHandler::create(Box::new(|_| Ok(())));
    unsafe {
        let _ = p2.ClearBrowsingData(COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_PROFILE, &done);
    }
}

pub fn remember_profile(ctx: &Rc<Ctx>, key: &str, wv: &WebView) {
    if key.is_empty() {
        return;
    }
    if let Some(p) = webview_profile(wv) {
        ctx.profiles.borrow_mut().entry(safe_key(key)).or_insert(p);
    }
}

pub fn purge_profile(ctx: &Rc<Ctx>, key: &str, strict: bool) {
    if key.is_empty() {
        return;
    }
    let key = safe_key(key);
    if let Some(p) = ctx.profiles.borrow_mut().remove(&key) {
        clear_profile(&p);
    }
    if strict {
        ctx.own_envs.borrow_mut().remove(&key);
        own_env_dir(&key).is_dir().then(|| delete_profile_files(own_env_dir(&key)));
    }
    queue_purge_in(&crate::store::data_dir(), &key);
}

fn purge_queue_path(dir: &std::path::Path) -> PathBuf {
    dir.join("agent_purge.json")
}

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

fn content_bounds_now(ctx: &Ctx) -> Rect {
    let size = ctx.window.inner_size();
    Rect {
        position: wry::dpi::PhysicalPosition::new(0, 0).into(),
        size: wry::dpi::PhysicalSize::new(size.width, size.height).into(),
    }
}

fn title_pacer(id: u64, proxy: EventLoopProxy<Ev>) -> impl Fn(String) + 'static {
    use crate::pacing::{TITLE_GAP, Throttle};
    use std::sync::{Arc, Mutex};
    struct State {
        gate: Throttle<String>,
        sent: String,
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

fn history_nav(args: &ICoreWebView2NavigationStartingEventArgs) -> bool {
    let Ok(third) = args.cast::<ICoreWebView2NavigationStartingEventArgs3>() else {
        return true;
    };
    let mut kind = COREWEBVIEW2_NAVIGATION_KIND_NEW_DOCUMENT;
    if unsafe { third.NavigationKind(&mut kind) }.is_err() {
        return true;
    }
    kind == COREWEBVIEW2_NAVIGATION_KIND_BACK_OR_FORWARD || kind == COREWEBVIEW2_NAVIGATION_KIND_RELOAD
}

struct NavRules {
    https: Rc<RefCell<crate::sites::HttpsPolicy>>,
    redirect: Rc<RefCell<crate::popup::RedirectPolicy>>,
    docs: Rc<RefCell<HashMap<u64, Doc>>>,
    pass: Rc<RefCell<Option<(u64, String)>>>,
    api_nav: Rc<RefCell<HashMap<u64, Instant>>>,
}

const API_NAV: Duration = Duration::from_secs(2);

fn hook_page(
    wv: &WebView,
    id: u64,
    proxy: EventLoopProxy<Ev>,
    rewrite: Option<Rc<dyn Fn(&str, crate::privacy_stats::Who) -> Option<String>>>,
    rules: NavRules,
    clicks: Rc<RefCell<HashMap<u64, crate::popup::Page>>>,
    who: crate::privacy_stats::Who,
) {
    let NavRules { https, redirect, docs, pass, api_nav } = rules;
    let core = wv.webview();
    let mut token = 0i64;
    unsafe {
        let p = proxy.clone();
        let (d_start, pass_start) = (docs.clone(), pass.clone());
        let _ = core.add_NavigationStarting(
            &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut redirected = Default::default();
                let _ = args.IsRedirected(&mut redirected);
                if redirected.as_bool() {
                    clicks.borrow_mut().remove(&id);
                    let _ = p.send_event(Ev::NavRedirected(id));
                    return Ok(());
                }
                let mut u = PWSTR::null();
                args.Uri(&mut u)?;
                let u = take(u);
                if u.starts_with("magnet:") {
                    args.SetCancel(true)?;
                    let clicked = clicks.borrow().get(&id).is_some_and(|p| crate::popup::clicked_magnet(p, &u));
                    let _ = p.send_event(Ev::Magnet(id, u, clicked));
                    return Ok(());
                }
                if u.starts_with("lumen-http:") {
                    args.SetCancel(true)?;
                    if let Some(choice) = crate::sites::parse_http_choice(&u) {
                        let _ = p.send_event(Ev::HttpChoice(id, choice));
                    }
                    return Ok(());
                }
                if u.starts_with("lumen-error:") {
                    args.SetCancel(true)?;
                    if let Some(choice) = crate::sites::parse_error_choice(&u) {
                        let _ = p.send_event(Ev::ErrorChoice(id, choice));
                    }
                    return Ok(());
                }
                if u.starts_with("lumen-danger:") {
                    args.SetCancel(true)?;
                    if let Some(choice) = crate::sites::parse_danger_choice(&u) {
                        let _ = p.send_event(Ev::DangerChoice(id, choice));
                    }
                    return Ok(());
                }
                if crate::blocker::danger_host(&u) {
                    args.SetCancel(true)?;
                    let _ = p.send_event(Ev::Danger(id, u));
                    return Ok(());
                }
                let mut served = windows::core::BOOL::default();
                let _ = args.IsRedirected(&mut served);
                let doc = d_start.borrow().get(&id).cloned().unwrap_or_default();
                let from = crate::privacy_stats::host(&doc.url);
                let person = clicks.borrow().get(&id).cloned().unwrap_or_default();
                let ours = api_nav.borrow_mut().remove(&id).is_some_and(|t| t.elapsed() <= API_NAV);
                let step = crate::popup::Step {
                    gesture: ours || history_nav(&args) || crate::popup::asked_nav(&person, &u),
                    redirected: served.as_bool(),
                    from: doc.url,
                    to: u.clone(),
                    loaded: doc.loaded.map(|t| t.elapsed()).unwrap_or_default(),
                    agent: who.agent,
                };
                let mut once = pass_start.borrow_mut();
                let asked = matches!(once.as_ref(), Some((t, url)) if *t == id && *url == u);
                if asked {
                    *once = None;
                }
                drop(once);
                let (block, allowed) = {
                    let r = redirect.borrow();
                    (r.block, r.allowed.clone())
                };
                if !asked && crate::popup::blocks_redirect(&step, block, &allowed) {
                    args.SetCancel(true)?;
                    crate::privacy_stats::redirect(&from, who);
                    let _ = p.send_event(Ev::RedirectBlocked(id, u.clone(), from));
                    return Ok(());
                }
                if let Some(secure) = crate::sites::https_upgrade(&u, &https.borrow()) {
                    args.SetCancel(true)?;
                    crate::privacy_stats::https_upgrade(&crate::privacy_stats::host(&u), who);
                    let _ = p.send_event(Ev::Upgrade(id, secure, u));
                    return Ok(());
                }
                let _ = p.send_event(Ev::NavStart(id, u.clone()));
                clicks.borrow_mut().remove(&id);
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
        let d_src = docs.clone();
        let _ = core.add_SourceChanged(
            &SourceChangedEventHandler::create(Box::new(move |w, _| {
                if let Some(w) = w {
                    let mut s = PWSTR::null();
                    w.Source(&mut s)?;
                    let url = take(s);
                    let mut d = d_src.borrow_mut();
                    let doc = d.entry(id).or_default();
                    doc.url = url.clone();
                    doc.loaded = None;
                    let _ = p.send_event(Ev::Url(id, url));
                }
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let d_done = docs.clone();
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
                if ok.as_bool()
                    && let Some(doc) = d_done.borrow_mut().get_mut(&id)
                {
                    doc.loaded = Some(Instant::now());
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

        if init_on("MEDIA") {
            let _ = core.AddScriptToExecuteOnDocumentCreated(
                &HSTRING::from(crate::pagekit::MEDIA_SHIM),
                &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(|_, _| Ok(()))),
            );
        }
        if init_on("GATE") {
            let _ = core.AddScriptToExecuteOnDocumentCreated(
                &HSTRING::from(crate::pagekit::AV1_GATE),
                &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(|_, _| Ok(()))),
            );
        }
    }
}

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
                let down = |k: u16| GetAsyncKeyState(k as i32) < 0;
                let (ctrl, shift, alt) = (down(VK_CONTROL.0), down(VK_SHIFT.0), down(VK_MENU.0));
                if let Some(a) = map_key(vk, ctrl, shift, alt) {
                    args.SetHandled(true)?;
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

pub fn zoom_factor(wv: &WebView) -> Option<f64> {
    let mut f = 1.0f64;
    unsafe { wv.controller().ZoomFactor(&mut f).ok().map(|_| f) }
}

pub fn set_zoom_factor(wv: &WebView, f: f64) -> bool {
    unsafe { wv.controller().SetZoomFactor(f).is_ok() }
}

pub fn cookie_count(wv: &WebView, url: &str, done: impl FnOnce(usize) + 'static) {
    cookie_list(wv, url, move |list| done(list.len()));
}

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

const GPC_JS: &str = "try{Object.defineProperty(Navigator.prototype,'globalPrivacyControl',{get:()=>true,configurable:true});Object.defineProperty(Navigator.prototype,'doNotTrack',{get:()=>'1',configurable:true})}catch(e){}";

const WEBRTC_JS: &str = "try{const O=window.RTCPeerConnection;if(O){window.RTCPeerConnection=function(c,i){const pc=new O(c,i);let n=false;pc.addEventListener('icecandidate',e=>{if(!n&&e.candidate){n=true;try{window.chrome.webview.postMessage({lumen:'rtc'})}catch(_){}}});return pc};window.RTCPeerConnection.prototype=O.prototype}}catch(e){}";

const CALL_JS: &str = r#"try{(()=>{if(window!==window.top)return;const W=window.chrome&&window.chrome.webview;if(!W)return;
const live=new Set(),pcs=new Set();let last=false,tick=0;
const up=s=>s==='connected'||s==='connecting'||s==='checking'||s==='completed'||s==='new';
const say=()=>{let on=false;for(const t of live){if(t.readyState==='live'){on=true}else live.delete(t)}
 for(const p of pcs){const s=p.connectionState||p.iceConnectionState;if(s==='closed'||s==='failed'){pcs.delete(p)}else if(up(s)&&p.remoteDescription){on=true}}
 if(on!==last){last=on;try{W.postMessage({lumen:'call',on})}catch(_){}}
 if((live.size||pcs.size)&&!tick)tick=setInterval(say,15000);if(!live.size&&!pcs.size&&tick){clearInterval(tick);tick=0}};
const keep=st=>{try{for(const t of st.getTracks()){live.add(t);t.addEventListener('ended',say)}}catch(_){}say();return st};
const md=navigator.mediaDevices;
if(md)for(const k of['getUserMedia','getDisplayMedia']){const f=md[k];if(f)md[k]=function(...a){return f.apply(this,a).then(keep)}}
const T=window.MediaStreamTrack&&MediaStreamTrack.prototype;if(T&&T.stop){const f=T.stop;T.stop=function(){f.apply(this);say()}}
const P=window.RTCPeerConnection&&RTCPeerConnection.prototype;
if(P&&P.setRemoteDescription){const f=P.setRemoteDescription;P.setRemoteDescription=function(...a){if(!pcs.has(this)){pcs.add(this);this.addEventListener('connectionstatechange',say);this.addEventListener('iceconnectionstatechange',say)}const r=f.apply(this,a);Promise.resolve(r).then(say,say);return r}}
if(P&&P.close){const f=P.close;P.close=function(){f.apply(this);say()}}
})()}catch(e){}"#;

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
    const BACKSLASH: u32 = 0xDC;
    let k = |c: char| c as u32;
    Some(match (ctrl, shift, alt, vk) {
        (true, false, false, v) if v == k('T') => NewTab,
        (true, true, false, v) if v == k('N') => NewPrivate,
        (true, true, false, v) if v == k('T') => Reopen,
        (true, false, false, v) if v == k('W') => CloseTab,
        (true, false, false, v) if v == k('L') => Omnibox,
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
        (true, true, false, v) if v == k('A') => TabSearch,
        (true, true, false, v) if v == k('I') => DevTools,
        (true, false, true, v) if (k('1')..=k('4')).contains(&v) => Workspace((v - k('1')) as usize),
        _ => return None,
    })
}

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

fn hook_downloads(ctx: &Rc<Ctx>, wv: &WebView, tab: u64, private: bool) {
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

pub fn origin_of(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}", rest.split(['/', '?', '#']).next().unwrap_or("")),
        None => url.to_string(),
    }
}

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

static NEXT_ASK: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    Alert(String),
    Confirm(String),
    Prompt(String, String),
    Leave,
    SignIn(String),
}

pub enum Waiting {
    Script(ICoreWebView2ScriptDialogOpeningEventArgs, ICoreWebView2Deferral),
    Auth(ICoreWebView2BasicAuthenticationRequestedEventArgs, ICoreWebView2Deferral),
    Menu(ICoreWebView2ContextMenuRequestedEventArgs, ICoreWebView2Deferral),
}

fn realm_of(challenge: &str) -> String {
    challenge.split_once("realm=").map(|(_, r)| r.trim().trim_matches('"').split('"').next().unwrap_or("").to_string()).unwrap_or_default()
}

fn menu_label(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '&' {
            if it.peek() == Some(&'&') {
                out.push('&');
                it.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn menu_items(list: &ICoreWebView2ContextMenuItemCollection) -> Vec<(i32, String, bool)> {
    let mut out: Vec<(i32, String, bool)> = Vec::new();
    unsafe {
        let mut n = 0u32;
        if list.Count(&mut n).is_err() {
            return out;
        }
        for k in 0..n {
            let Ok(item) = list.GetValueAtIndex(k) else { continue };
            let mut kind = COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND::default();
            let _ = item.Kind(&mut kind);
            if kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SEPARATOR {
                if out.last().is_some_and(|l| l.0 != i32::MIN) {
                    out.push((i32::MIN, String::new(), false));
                }
                continue;
            }
            if kind == COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_SUBMENU {
                continue;
            }
            let (mut l, mut id, mut on) = (PWSTR::null(), 0i32, windows::core::BOOL::default());
            let _ = item.Label(&mut l);
            let _ = item.CommandId(&mut id);
            let _ = item.IsEnabled(&mut on);
            let label = menu_label(&take(l));
            if !label.is_empty() {
                out.push((id, label, on.as_bool()));
            }
        }
    }
    while out.last().is_some_and(|l| l.0 == i32::MIN) {
        out.pop();
    }
    out
}

fn hook_page_asks(ctx: &Rc<Ctx>, wv: &WebView, id: u64) {
    let core = wv.webview();
    unsafe {
        if let Ok(s) = core.Settings() {
            let _ = s.SetAreDefaultScriptDialogsEnabled(false);
            let _ = s.SetIsBuiltInErrorPageEnabled(false);
            let _ = s.SetIsStatusBarEnabled(false);
        }
        if let Ok(c12) = core.cast::<ICoreWebView2_12>() {
            let p = ctx.proxy.clone();
            let mut token = 0i64;
            let _ = c12.add_StatusBarTextChanged(
                &StatusBarTextChangedEventHandler::create(Box::new(move |w, _| {
                    let Some(w) = w.and_then(|w| w.cast::<ICoreWebView2_12>().ok()) else { return Ok(()) };
                    let mut t = PWSTR::null();
                    w.StatusBarText(&mut t)?;
                    let _ = p.send_event(Ev::LinkHover(id, take(t)));
                    Ok(())
                })),
                &mut token,
            );
        }
        let mut token = 0i64;
        let c = ctx.clone();
        let _ = core.add_ScriptDialogOpening(
            &ScriptDialogOpeningEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_SCRIPT_DIALOG_KIND::default();
                args.Kind(&mut kind)?;
                let (mut u, mut m, mut d) = (PWSTR::null(), PWSTR::null(), PWSTR::null());
                args.Uri(&mut u)?;
                args.Message(&mut m)?;
                let _ = args.DefaultText(&mut d);
                let msg = take(m);
                let ask = match kind {
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT => Ask::Alert(msg),
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM => Ask::Confirm(msg),
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT => Ask::Prompt(msg, take(d)),
                    _ => Ask::Leave,
                };
                let deferral = args.GetDeferral()?;
                let aid = NEXT_ASK.fetch_add(1, Ordering::Relaxed);
                c.waiting.borrow_mut().insert(aid, Waiting::Script(args.clone(), deferral));
                let _ = c.proxy.send_event(Ev::PageAsk { aid, tab: id, origin: origin_of(&take(u)), ask });
                Ok(())
            })),
            &mut token,
        );
        if let Ok(c10) = core.cast::<ICoreWebView2_10>() {
            let c = ctx.clone();
            let _ = c10.add_BasicAuthenticationRequested(
                &BasicAuthenticationRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let (mut u, mut ch) = (PWSTR::null(), PWSTR::null());
                    args.Uri(&mut u)?;
                    let _ = args.Challenge(&mut ch);
                    let deferral = args.GetDeferral()?;
                    let aid = NEXT_ASK.fetch_add(1, Ordering::Relaxed);
                    c.waiting.borrow_mut().insert(aid, Waiting::Auth(args.clone(), deferral));
                    let _ = c.proxy.send_event(Ev::PageAsk { aid, tab: id, origin: origin_of(&take(u)), ask: Ask::SignIn(realm_of(&take(ch))) });
                    Ok(())
                })),
                &mut token,
            );
        }
        if let Ok(c11) = core.cast::<ICoreWebView2_11>() {
            let c = ctx.clone();
            let _ = c11.add_ContextMenuRequested(
                &ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let items = args.MenuItems().map(|l| menu_items(&l)).unwrap_or_default();
                    let (mut link, mut selection) = (String::new(), String::new());
                    if let Ok(t) = args.ContextMenuTarget() {
                        let (mut has, mut p) = (windows::core::BOOL::default(), PWSTR::null());
                        if t.HasLinkUri(&mut has).is_ok() && has.as_bool() && t.LinkUri(&mut p).is_ok() {
                            link = take(p);
                        }
                        let (mut has, mut p) = (windows::core::BOOL::default(), PWSTR::null());
                        if t.HasSelection(&mut has).is_ok() && has.as_bool() && t.SelectionText(&mut p).is_ok() {
                            selection = take(p);
                        }
                    }
                    let mut pt = windows::Win32::Foundation::POINT::default();
                    let _ = args.Location(&mut pt);
                    args.SetHandled(true)?;
                    let deferral = args.GetDeferral()?;
                    let aid = NEXT_ASK.fetch_add(1, Ordering::Relaxed);
                    c.waiting.borrow_mut().insert(aid, Waiting::Menu(args.clone(), deferral));
                    let _ = c.proxy.send_event(Ev::PageMenu { aid, tab: id, x: pt.x as f32, y: pt.y as f32, items, link, selection });
                    Ok(())
                })),
                &mut token,
            );
        }
    }
}

pub fn answer_script(ctx: &Ctx, aid: u64, ok: bool, text: Option<&str>) {
    let Some(Waiting::Script(args, deferral)) = ctx.waiting.borrow_mut().remove(&aid) else { return };
    unsafe {
        if let Some(t) = text {
            let _ = args.SetResultText(&windows::core::HSTRING::from(t));
        }
        if ok {
            let _ = args.Accept();
        }
        let _ = deferral.Complete();
    }
}

pub fn answer_sign_in(ctx: &Ctx, aid: u64, with: Option<(String, String)>) {
    let Some(Waiting::Auth(args, deferral)) = ctx.waiting.borrow_mut().remove(&aid) else { return };
    unsafe {
        match with.as_ref().and_then(|w| args.Response().ok().map(|r| (r, w))) {
            Some((r, (user, pass))) => {
                let _ = r.SetUserName(&windows::core::HSTRING::from(user.as_str()));
                let _ = r.SetPassword(&windows::core::HSTRING::from(pass.as_str()));
            }
            None => {
                let _ = args.SetCancel(true);
            }
        }
        let _ = deferral.Complete();
    }
}

pub fn answer_menu(ctx: &Ctx, aid: u64, cmd: Option<i32>) {
    let Some(Waiting::Menu(args, deferral)) = ctx.waiting.borrow_mut().remove(&aid) else { return };
    unsafe {
        if let Some(c) = cmd {
            let _ = args.SetSelectedCommandId(c);
        }
        let _ = deferral.Complete();
    }
}

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

fn hook_popup_sensor(wv: &WebView, queue: Rc<RefCell<VecDeque<(bool, String, u64)>>>, id: u64) {
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
                if q.len() < 32 {
                    q.push_back((user.as_bool(), site, id));
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

fn hook_popup_page(wv: &WebView, id: u64, ctx: &Rc<Ctx>) {
    let core = wv.webview();
    let mut token = 0i64;
    let clicks = ctx.clicks.clone();
    let secret = ctx.popup_token.clone();
    unsafe {
        let _ = core.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut j = PWSTR::null();
                if args.WebMessageAsJson(&mut j).is_err() {
                    return Ok(());
                }
                let Ok(msg) = serde_json::from_str::<serde_json::Value>(&take(j)) else { return Ok(()) };
                if msg["t"].as_str() != Some(secret.as_str()) {
                    return Ok(());
                }
                let mut m = clicks.borrow_mut();
                let p = m.entry(id).or_default();
                match msg["lumen"].as_str().unwrap_or("") {
                    "click" => {
                        p.link = msg["link"].as_u64().unwrap_or(0) == 1;
                        p.url = msg["url"].as_str().unwrap_or("").to_string();
                        p.at = Some(Instant::now());
                    }
                    "key" => {
                        p.keyed = Some(Instant::now());
                    }
                    "open" => {
                        p.opened = Some((msg["url"].as_str().unwrap_or("").to_string(), Instant::now()));
                    }
                    _ => {}
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

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

fn hook_call(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>) {
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
                if msg["lumen"] == "call" {
                    let _ = proxy.send_event(Ev::Call(id, msg["on"].as_bool().unwrap_or(false)));
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

const TRACKERS: &[&str] = &[
    "fbclid", "gclid", "gclsrc", "dclid", "gbraid", "wbraid", "msclkid", "mc_cid", "mc_eid", "igshid", "yclid",
    "twclid", "ttclid", "li_fat_id", "_hsenc", "_hsmi", "mkt_tok", "oly_anon_id", "oly_enc_id", "vero_id", "wickedid",
];

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
    use super::{Action, clean_url, map_key, menu_label, origin_of, purge_at_start_in, purge_queue_path, queue_purge_in, realm_of, safe_key};

    #[test]
    fn a_realm_and_a_menu_label_read_plainly() {
        assert_eq!(realm_of("Basic realm=\"Lumen test\""), "Lumen test");
        assert_eq!(realm_of("Basic realm=\"Intranet\", charset=\"UTF-8\""), "Intranet");
        assert_eq!(realm_of("Basic"), "");
        assert_eq!(menu_label("&Back"), "Back");
        assert_eq!(menu_label("Save &as"), "Save as");
        assert_eq!(menu_label("Tom && Jerry"), "Tom & Jerry");
    }

    #[test]
    fn ctrl_shift_n_is_a_new_private_tab() {
        let n = 'N' as u32;
        assert!(matches!(map_key(n, true, true, false), Some(Action::NewPrivate)));
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
        queue_purge_in(&dir, key);
        assert_eq!(std::fs::read_to_string(purge_queue_path(&dir)).unwrap(), format!("[\"{key}\"]"));
        purge_at_start_in(&dir, &root);
        assert!(!mine.exists(), "the ended session's folder is gone");
        assert!(its.exists() && unrelated.exists(), "no other profile was touched");
        assert!(!purge_queue_path(&dir).exists(), "the queue is emptied");
        std::fs::write(purge_queue_path(&dir), "{not json").unwrap();
        purge_at_start_in(&dir, &root);
        assert!(its.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_profile_key_names_one_folder() {
        assert_eq!(safe_key("a1b2"), "a1b2");
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

pub fn user_agent(wv: &WebView) -> String {
    unsafe {
        let Ok(s2) = wv.webview().Settings().and_then(|s| s.cast::<ICoreWebView2Settings2>()) else { return String::new() };
        let mut p = PWSTR::null();
        if s2.UserAgent(&mut p).is_ok() { take(p) } else { String::new() }
    }
}

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
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ERASENOW);
    }
}

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
        SetWindowRgn(hwnd, Some(rgn), true);
    }
}

fn clip_siblings(wv: &WebView) {
    use windows::Win32::UI::WindowsAndMessaging::{GWL_STYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOZORDER, SetWindowLongPtrW, WS_CLIPSIBLINGS};
    unsafe {
        let mut hwnd = HWND::default();
        if wv.controller().ParentWindow(&mut hwnd).is_err() || hwnd.is_invalid() {
            return;
        }
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        if style & WS_CLIPSIBLINGS.0 as isize == 0 {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_CLIPSIBLINGS.0 as isize);
            let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
    }
}

pub fn raise(wv: &WebView) {
    unsafe {
        let mut hwnd = HWND::default();
        if wv.controller().ParentWindow(&mut hwnd).is_err() || hwnd.is_invalid() {
            return;
        }
        let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

pub fn memory(_env: &ICoreWebView2Environment) -> (u64, u32) {
    let (_, total, count, _) = memory_with_cpu(None, false);
    (total, count)
}

pub fn memory_with_cpu(_env: Option<&ICoreWebView2Environment>, cpu: bool) -> (u64, u64, u32, u64) {
    use crate::monitor::{cpu_time, private_ws, tree};
    let me = unsafe { GetCurrentProcess() };
    let own = private_ws(me);
    let mut total = own;
    let mut ticks = if cpu { cpu_time(me) } else { 0 };
    let mut count = 1u32;
    let mine = std::process::id();
    for (pid, _) in tree() {
        if pid == mine {
            continue;
        }
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
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

pub fn own_env_key(key: &str) -> String {
    safe_key(key)
}
