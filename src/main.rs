#![windows_subsystem = "windows"]

//! Lumen. A small browser shell.
//! One window. A slim UI page for the sidebar. Pages beside it.
//! Rust decides who lives, who sleeps, and when the UI hears about it.

mod aria;
mod agent;
mod agentkit;
mod blocker;
mod chrome;
mod engine;
mod library;
mod monitor;
mod pagekit;
mod recall;
mod video;
mod store;

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, Event, StartCause, WindowEvent},
    keyboard::{KeyCode, ModifiersState},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::windows::{IconExtWindows, WindowBuilderExtWindows, WindowExtWindows},
    window::{Fullscreen, Icon, ResizeDirection, Theme as WinTheme, WindowBuilder},
};
use windows::Win32::Foundation::HWND;
use wry::{MemoryUsageLevel, Rect, WebView, WebViewExtWindows};

use engine::{Action, Ctx, Ev};
use store::{SavedTab, Session, Settings};

const TICK: Duration = Duration::from_secs(3);
/// A hidden tab keeps its caches this long, so flipping back is instant.
const TRIM_AFTER: Duration = Duration::from_secs(60);
const MAX_CLOSED: usize = 25;
const COMPACT_W: f64 = 56.0;
/// Control page refresh. Only runs while the page is open.
const SAMPLE: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    None,
    Settings,
    Control,
    Downloads,
    History,
    Bookmarks,
}

impl Panel {
    fn name(self) -> &'static str {
        match self {
            Panel::None => "none",
            Panel::Settings => "settings",
            Panel::Control => "control",
            Panel::Downloads => "downloads",
            Panel::History => "history",
            Panel::Bookmarks => "bookmarks",
        }
    }
}

struct Tab {
    id: u64,
    url: String,
    title: String,
    favicon: String,
    loading: bool,
    back: bool,
    fwd: bool,
    pinned: bool,
    audio: bool,
    muted: bool,
    suspended: bool,
    trimmed: bool,
    /// Opened by an AI agent. Marked in the sidebar, never saved in the session.
    agent: bool,
    /// Showing reader view of this URL.
    reader: Option<String>,
    /// Opened by a page (window.open, target=_blank).
    popup: bool,
    /// InPrivate. Nothing here is written anywhere: no history, no page text,
    /// no session, no download row, no cookies once the last private page goes.
    private: bool,
    /// For agent tabs: the MCP session that owns it, and its client's name.
    owner: String,
    owner_name: String,
    view: Option<WebView>,
    hidden_at: Option<Instant>,
}

impl Tab {
    fn new(id: u64, url: String) -> Self {
        Self {
            id,
            url,
            title: String::new(),
            favicon: String::new(),
            loading: false,
            back: false,
            fwd: false,
            pinned: false,
            audio: false,
            muted: false,
            suspended: false,
            trimmed: false,
            agent: false,
            reader: None,
            popup: false,
            private: false,
            owner: String::new(),
            owner_name: String::new(),
            view: None,
            hidden_at: None,
        }
    }

    fn status(&self) -> &'static str {
        match (&self.view, self.url.is_empty(), self.suspended) {
            (None, true, _) => "new",
            (None, false, _) => "off",
            (Some(_), _, true) => "asleep",
            (Some(_), _, false) => "live",
        }
    }
}

struct App {
    ctx: Rc<Ctx>,
    chrome: chrome::Chrome,
    blocker: Rc<blocker::Blocker>,
    tabs: Vec<Tab>,
    active: u64,
    closed: Vec<SavedTab>,
    settings: Settings,
    panel: Panel,
    peek: bool,
    page_fs: bool,
    user_fs: bool,
    ui_ready: bool,
    /// Something changed. The UI hears once per frame, not once per event.
    ui_dirty: bool,
    /// Favicons are big. They travel alone, only when they change.
    fav_queue: Vec<u64>,
    /// Sidebar color reported by the UI. Paints the gaps around the page.
    bg: (u8, u8, u8),
    /// Last page size that got rounded corners. Skip the work when nothing moved.
    rounded: Option<(u64, u32, u32)>,
    was_max: bool,
    shown: bool,
    dirty: bool,
    mem: Option<(u64, u32)>,
    born: Instant,
    next_tick: Instant,
    exit: bool,
    /// The browser tree's Job Object. The CPU cap lives here.
    job: Option<monitor::Job>,
    /// Control page bookkeeping.
    next_sample: Option<Instant>,
    prev_cpu: HashMap<u32, u64>,
    prev_at: Instant,
    net: monitor::NetBytes,
    watches: HashMap<u64, monitor::NetWatch>,
    cores: f64,
    /// Newest first. Kept for this session.
    downloads: Vec<engine::Dl>,
    agent: agent::Agent,
    /// Last time an agent did something. Drives the "Agent working" chip.
    agent_seen: Option<Instant>,
    /// Last time any agent asked for anything. A Lumen started headless for agents
    /// leaves when they have been quiet a while.
    agent_last: Instant,
    /// Modifier keys as the main window last saw them.
    mods: ModifiersState,
    /// Remembered permission answers, and requests waiting for the user.
    rules: store::Rules,
    prompts: Vec<(u64, u64, String, &'static str)>,
    /// History and bookmarks.
    library: library::Library,
    /// aria2 and its jobs: gid -> download id in the list.
    aria: std::sync::Arc<aria::Aria>,
    aria_ids: HashMap<String, u64>,
    aria_poll: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Page text for searching history by content.
    recall: recall::Recall,
    /// Console, network and dialog records of agent tabs.
    agent_logs: agentkit::Logs,
    /// An agent waiting for the person: (tab, agent name, message, where to say Done).
    handoff: Option<(u64, String, String, std::sync::mpsc::Sender<Result<Value, String>>)>,
    /// What aria2 was given per gid, so a failed download can be tried again.
    aria_meta: HashMap<String, (String, Vec<String>, Option<String>)>,
    /// Next download id for aria2, video and remembered rows. Never reused.
    dl_seq: u64,
    /// Finished downloads changed; write downloads.json on the next tick.
    dl_dirty: bool,
    /// Video grabs: download id -> (page url, user agent, running yt-dlp).
    videos: HashMap<u64, (String, String, video::Slot)>,
    /// "Clear history on exit" owed a WebView2 clear it could not make in time.
    /// Done at the next start, as soon as a page exists to ask.
    clear_pending: bool,
}

/// Headless: a real Lumen whose window lives off-screen, never takes focus, has no taskbar
/// button and makes no sound. For tests and for agents working while someone uses the PC.
/// `--headless` or LUMEN_HEADLESS=1.
pub fn headless() -> bool {
    HEADLESS.load(std::sync::atomic::Ordering::Relaxed)
}

/// How long a headless Lumen waits for agents before it leaves. LUMEN_AGENT_IDLE seconds, default 600.
fn agent_idle() -> Duration {
    static D: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *D.get_or_init(|| Duration::from_secs(std::env::var("LUMEN_AGENT_IDLE").ok().and_then(|v| v.parse().ok()).unwrap_or(600)))
}

static HEADLESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// One Lumen per profile. The first one owns a named event; a later start sets it and
/// leaves, and the first one comes to the front. Returns the event if we are the first.
fn claim_instance() -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::Win32::{
        Foundation::{ERROR_ALREADY_EXISTS, GetLastError},
        System::Threading::{CreateEventW, SetEvent},
        UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow},
    };
    let dir = store::data_dir().to_string_lossy().to_lowercase();
    // FNV-1a: a short, stable name for this profile folder.
    let h = dir.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let name = windows::core::HSTRING::from(format!("Local\\Lumen-{h:016x}"));
    unsafe {
        let ev = CreateEventW(None, false, false, &name).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // A headless start (an agent asking) has nothing to show; a normal one does.
            if !headless() {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = SetEvent(ev);
            }
            return None;
        }
        Some(ev)
    }
}

/// Show the main window: normally, or, headless, out of sight and without stealing focus.
fn show_window(w: &tao::window::Window) {
    if !headless() {
        w.set_visible(true);
        return;
    }
    use windows::Win32::UI::WindowsAndMessaging::{HWND_BOTTOM, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SetWindowPos, ShowWindow};
    let h = HWND(w.hwnd() as _);
    unsafe {
        // Show first: showing puts a window back at its last place. Then move it out of sight.
        // (-32000 means "minimized" to Windows, so not that.)
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(h, Some(HWND_BOTTOM), -10000, -10000, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE);
    }
}

fn main() {
    HEADLESS.store(
        std::env::var_os("LUMEN_HEADLESS").is_some_and(|v| v != "0") || std::env::args().any(|a| a == "--headless"),
        std::sync::atomic::Ordering::Relaxed,
    );
    let Some(instance) = claim_instance() else { return };
    // Before WebView2 starts: its processes must be born inside the job.
    let job = monitor::Job::adopt_self();
    let settings = store::load_settings();
    let session = store::load_session();

    let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    let (w, h) = if session.width >= 720.0 && session.height >= 480.0 {
        (session.width, session.height)
    } else {
        (1360.0, 860.0)
    };
    let window = WindowBuilder::new()
        .with_title("Lumen")
        // Icon resource 1, baked in by build.rs from assets/lumen.ico.
        .with_window_icon(Icon::from_resource(1, None).ok())
        .with_decorations(false)
        // Opaque window. Glass mode gets its backdrop from the extended DWM frame and
        // black client pixels, not from window transparency, which costs a composite.
        .with_transparent(false)
        .with_background_color((0, 0, 0, 255))
        .with_undecorated_shadow(true)
        .with_visible(false)
        .with_skip_taskbar(headless())
        .with_inner_size(LogicalSize::new(w, h))
        .with_min_inner_size(LogicalSize::new(720.0, 480.0))
        .with_maximized(session.maximized)
        .build(&event_loop)
        .expect("window");

    let dark = resolve_dark(&settings, window.theme());
    let ctx = Rc::new(Ctx {
        window: Rc::new(window),
        proxy: event_loop.create_proxy(),
        env: RefCell::new(None),
        next_id: Cell::new(1),
        popups: RefCell::new(Vec::new()),
        dark: Cell::new(dark),
        on_built: RefCell::new(None),
        web_context: RefCell::new(wry::WebContext::new(Some(store::data_dir().join("WebView2")))),
        args: engine::browser_args(settings.gpu_in_process, settings.share_processes),
        tracking: RefCell::new(settings.tracking.clone()),
        downloads: RefCell::new(HashMap::new()),
        passwords: Cell::new(settings.save_passwords),
        perms: RefCell::new(HashMap::new()),
        aria_on: Cell::new(false),
        bypass: RefCell::new(Default::default()),
        dark_js: RefCell::new(None),
        rewrite: RefCell::new(None),
        dark_ids: Rc::new(RefCell::new(HashMap::new())),
    });

    {
        // A second lumen.exe sets the event: come forward.
        let (ev, proxy) = (instance.0 as isize, ctx.proxy.clone());
        std::thread::Builder::new()
            .name("lumen-instance".into())
            .spawn(move || loop {
                let h = windows::Win32::Foundation::HANDLE(ev as _);
                let r = unsafe { windows::Win32::System::Threading::WaitForSingleObject(h, windows::Win32::System::Threading::INFINITE) };
                if r != windows::Win32::Foundation::WAIT_OBJECT_0 || proxy.send_event(Ev::ShowMe).is_err() {
                    break;
                }
            })
            .ok();
    }

    let blocker = blocker::Blocker::start(store::data_dir());
    blocker.set_enabled(settings.adblock);
    blocker.set_cosmetic(settings.cosmetic);
    {
        let b = blocker.clone();
        ctx.rewrite.replace(Some(Rc::new(move |u: &str| b.rewrite(u))));
        let b = blocker.clone();
        ctx.on_built.replace(Some(Box::new(move |wv: &WebView| b.attach(wv))));
    }

    // The browser chrome: a native child window. No webview until a page needs one.
    let chrome = chrome::Chrome::new(HWND(ctx.window.hwnd() as _), ctx.proxy.clone());

    let bg = if dark { (24, 24, 26) } else { (243, 243, 244) };
    let mut app = App {
        ctx,
        chrome,
        blocker,
        tabs: Vec::new(),
        active: 0,
        closed: Vec::new(),
        settings,
        panel: Panel::None,
        peek: false,
        page_fs: false,
        user_fs: false,
        ui_ready: true,
        ui_dirty: false,
        fav_queue: Vec::new(),
        bg,
        rounded: None,
        was_max: session.maximized,
        shown: false,
        dirty: false,
        mem: None,
        born: Instant::now(),
        next_tick: Instant::now() + TICK,
        exit: false,
        job,
        next_sample: None,
        prev_cpu: HashMap::new(),
        prev_at: Instant::now(),
        net: Rc::new(RefCell::new(HashMap::new())),
        watches: HashMap::new(),
        cores: std::thread::available_parallelism().map_or(1.0, |n| n.get() as f64),
        downloads: Vec::new(),
        agent: agent::Agent::new(&store::data_dir()),
        agent_seen: None,
        agent_last: Instant::now(),
        mods: ModifiersState::empty(),
        rules: store::load_rules(),
        prompts: Vec::new(),
        library: library::Library::load(&store::data_dir()),
        aria: aria::Aria::start(&store::data_dir(), &dirs::download_dir().unwrap_or_else(store::data_dir)),
        aria_ids: HashMap::new(),
        aria_poll: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        videos: HashMap::new(),
        aria_meta: HashMap::new(),
        dl_seq: 0,
        dl_dirty: false,
        recall: recall::Recall::start(&store::data_dir()),
        agent_logs: Default::default(),
        handoff: None,
        clear_pending: false,
    };
    {
        // One small thread polls aria2 while it has work, and sleeps otherwise.
        let (aria, poll, proxy) = (app.aria.clone(), app.aria_poll.clone(), app.ctx.proxy.clone());
        std::thread::Builder::new()
            .name("lumen-aria-poll".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(700));
                if poll.load(std::sync::atomic::Ordering::Relaxed) && proxy.send_event(Ev::AriaJobs(aria.jobs())).is_err() {
                    break;
                }
            })
            .ok();
    }
    app.agent.set(app.settings.agent, app.settings.agent_port, &app.ctx.proxy);
    // The last run closed with "clear history on exit" and could not finish wiping
    // WebView2's own history. It happens as soon as there is a page to ask.
    app.clear_pending = store::clear_history_pending();
    app.apply_backdrop();
    app.apply_limits(None);
    app.refresh_dark();
    app.load_downloads();
    app.restore(session);
    app.flush();
    app.shown = true;
    show_window(&app.ctx.window);
    app.layout();
    if !headless() {
        app.focus_page();
    }
    // Test hook: LUMEN_TEST_PRIVATE=1 opens one normal and one private tab, so a
    // headless run can prove a private tab leaves no trace in session.json. The
    // private tab gets a url of its own: its Nav line in the log is the proof the
    // InPrivate page built.
    if cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_PRIVATE").is_some() {
        app.new_tab(Some("https://example.com/".into()), None);
        let id = app.new_private_tab();
        if let Some(i) = app.idx(id) {
            app.tabs[i].url = "https://private.example/".into();
            app.ensure_view(i);
        }
    }

    event_loop.run(move |event, _, cf| {
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                let now = Instant::now();
                if app.next_sample.is_some_and(|t| t <= now) {
                    app.sample();
                }
                if app.next_tick <= now {
                    app.tick();
                }
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::Resized(size) => {
                    if size.width == 0 || size.height == 0 {
                        // Minimized. Nobody is looking. Trim the active page too.
                        if let Some(v) = app.active_tab().and_then(|t| t.view.as_ref()) {
                            let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
                        }
                    } else {
                        app.layout();
                        let max = app.ctx.window.is_maximized();
                        if max != app.was_max {
                            app.was_max = max;
                            app.push();
                        }
                    }
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    app.rounded = None;
                    app.layout();
                }
                // Alt+Tab lands focus on the bare window. Hand it on, or keys go nowhere.
                WindowEvent::Focused(true) => app.focus_page(),
                WindowEvent::ThemeChanged(_) if app.settings.theme == "system" => {
                    app.retheme();
                    app.push();
                }
                WindowEvent::CloseRequested => app.exit = true,
                WindowEvent::ModifiersChanged(m) => app.mods = m,
                // Closing a tab drops focus onto the bare window until the next page takes it.
                // Keys typed in that gap land here; handle them instead of losing them.
                WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                    let m = app.mods;
                    if let Some(vk) = vk_of(event.physical_key)
                        && let Some(a) = engine::map_key(vk, m.control_key(), m.shift_key(), m.alt_key())
                        && (!event.repeat || matches!(a, Action::NextTab | Action::PrevTab))
                    {
                        app.on_key(a);
                    }
                    app.focus_page();
                }
                _ => {}
            },
            Event::UserEvent(ev) => app.on_event(ev),
            Event::MainEventsCleared => app.flush(),
            _ => {}
        }
        if app.exit {
            app.library.flush();
            app.clear_on_exit();
            app.save_session();
            *cf = ControlFlow::Exit;
        } else {
            let wake = app.next_sample.map_or(app.next_tick, |t| t.min(app.next_tick));
            *cf = ControlFlow::WaitUntil(wake);
        }
    });
}

// ------------------------------------------------------------------- helpers

/// yt-dlp part files are "name.f<format>.<ext>". Audio parts are m4a/webm audio.
fn part_kind(file: &str) -> Option<&'static str> {
    let (stem, ext) = file.rsplit_once('.')?;
    let fmt = stem.rsplit_once(".f")?.1;
    if fmt.is_empty() || !fmt.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(if matches!(ext, "m4a" | "opus" | "mp3" | "aac") { "audio" } else { "video" })
}

/// "https://a.b.c/x" -> "a.b.c"
fn host_of(url: &str) -> String {
    url.split("://").nth(1).and_then(|r| r.split(['/', '?', '#']).next()).map(|h| h.rsplit('@').next().unwrap_or(h)).unwrap_or("").to_string()
}

fn resolve_dark(s: &Settings, os: WinTheme) -> bool {
    match s.theme.as_str() {
        "light" => false,
        "system" => os == WinTheme::Dark,
        _ => true,
    }
}

fn logical_rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(x, y).into(),
        size: LogicalSize::new(w.max(1.0), h.max(1.0)).into(),
    }
}

fn logical_size(ctx: &Ctx) -> (f64, f64) {
    let s = ctx.window.inner_size();
    let f = ctx.window.scale_factor();
    (s.width as f64 / f, s.height as f64 / f)
}

/// Site searches, after DuckDuckGo's !bangs. `!yt cats` searches YouTube.
const BANGS: &[(&str, &str)] = &[
    ("g", "https://www.google.com/search?q=%s"),
    ("ddg", "https://duckduckgo.com/?q=%s"),
    ("b", "https://search.brave.com/search?q=%s"),
    ("yt", "https://www.youtube.com/results?search_query=%s"),
    ("w", "https://en.wikipedia.org/w/index.php?search=%s"),
    ("gh", "https://github.com/search?q=%s"),
    ("r", "https://www.reddit.com/search/?q=%s"),
    ("a", "https://www.amazon.com/s?k=%s"),
    ("m", "https://www.google.com/maps/search/%s"),
    ("so", "https://stackoverflow.com/search?q=%s"),
    ("mdn", "https://developer.mozilla.org/en-US/search?q=%s"),
    ("rs", "https://docs.rs/releases/search?query=%s"),
    ("crates", "https://crates.io/search?q=%s"),
    ("npm", "https://www.npmjs.com/search?q=%s"),
    ("x", "https://x.com/search?q=%s"),
    ("i", "https://www.google.com/search?tbm=isch&q=%s"),
    ("tr", "https://translate.google.com/?sl=auto&tl=en&text=%s"),
];

/// A known bang anywhere in the text, and the words around it.
fn bang(s: &str) -> Option<String> {
    let words: Vec<&str> = s.split_whitespace().collect();
    let (pos, url) = words.iter().enumerate().find_map(|(i, w)| {
        let name = w.strip_prefix('!')?.to_ascii_lowercase();
        BANGS.iter().find(|(b, _)| *b == name).map(|(_, u)| (i, *u))
    })?;
    let q: Vec<&str> = words.iter().enumerate().filter(|(i, _)| *i != pos).map(|(_, w)| *w).collect();
    Some(url.replace("%s", &url_encode(&q.join(" "))))
}

/// Omnibox text in, URL out. A bang: that site. Looks like an address: go there. Else: search.
fn resolve_input(raw: &str, search: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(u) = bang(s) {
        return Some(u);
    }
    let lower = s.to_ascii_lowercase();
    const SCHEMES: [&str; 7] = ["http://", "https://", "file:", "about:", "data:", "view-source:", "edge://"];
    if SCHEMES.iter().any(|p| lower.starts_with(p)) {
        return Some(s.to_string());
    }
    if !s.contains(char::is_whitespace) {
        let host = lower.split(['/', '?', '#']).next().unwrap_or("");
        let host_only = host.rsplit_once(':').map_or(host, |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) { h } else { host }
        });
        if host_only == "localhost" || host_only.parse::<std::net::Ipv4Addr>().is_ok() {
            return Some(format!("http://{s}"));
        }
        if let Some((_, tld)) = host_only.rsplit_once('.')
            && tld.len() >= 2
            && tld.chars().all(|c| c.is_ascii_alphabetic())
        {
            return Some(format!("https://{s}"));
        }
    }
    Some(search.replace("%s", &url_encode(s)))
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Events that fire in a stream (download progress, aria2 polls) would drown the
/// debug log in repeats. Everything else is worth one line.
fn quiet_event(ev: &Ev) -> bool {
    matches!(ev, Ev::Download(_) | Ev::AriaJobs(_) | Ev::Usage(_))
}

/// Debug builds only: append every engine event to %APPDATA%\Lumen\debug.log.
fn debug_log(ev: &Ev) {
    use std::io::Write;
    let mut line = format!("{ev:?}");
    if line.len() > 160 {
        let cut = (0..=160).rev().find(|&i| line.is_char_boundary(i)).unwrap_or(0);
        line.truncate(cut);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(store::data_dir().join("debug.log"))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// The tabs that go into session.json. Private tabs and agent tabs leave no
/// trace; an empty new tab page is only worth keeping while it is the active one.
fn keepable(tabs: &[Tab], active: u64) -> Vec<&Tab> {
    tabs.iter().filter(|t| !t.agent && !t.private && (!t.url.is_empty() || t.id == active)).collect()
}

/// Virtual-key code for the keys Lumen has shortcuts on.
fn vk_of(k: KeyCode) -> Option<u32> {
    use KeyCode::*;
    let letter = |c: char| Some(c as u32);
    match k {
        KeyT => letter('T'),
        KeyW => letter('W'),
        KeyL => letter('L'),
        KeyK => letter('K'),
        KeyR => letter('R'),
        KeyB => letter('B'),
        KeyJ => letter('J'),
        KeyD => letter('D'),
        KeyH => letter('H'),
        KeyO => letter('O'),
        KeyI => letter('I'),
        Digit1 => letter('1'),
        Digit2 => letter('2'),
        Digit3 => letter('3'),
        Digit4 => letter('4'),
        Digit5 => letter('5'),
        Digit6 => letter('6'),
        Digit7 => letter('7'),
        Digit8 => letter('8'),
        Digit9 => letter('9'),
        Tab => Some(0x09),
        Escape => Some(0x1B),
        ArrowLeft => Some(0x25),
        ArrowRight => Some(0x27),
        F5 => Some(0x74),
        F6 => Some(0x75),
        F11 => Some(0x7A),
        F12 => Some(0x7B),
        Comma => Some(0xBC),
        _ => None,
    }
}

/// Unix seconds to ("Today" / "Yesterday" / "Monday 12 October", "14:05"), local time.
fn local_day_time(secs: u64) -> (String, String) {
    use windows::Win32::{
        Foundation::{FILETIME, SYSTEMTIME},
        System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
    };
    let to_local = |s: u64| -> SYSTEMTIME {
        let ft100 = (s + 11_644_473_600) * 10_000_000;
        let ft = FILETIME { dwLowDateTime: ft100 as u32, dwHighDateTime: (ft100 >> 32) as u32 };
        let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
        unsafe {
            let _ = FileTimeToSystemTime(&ft, &mut utc);
            let _ = SystemTimeToTzSpecificLocalTime(None, &utc, &mut local);
        }
        local
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (t, today, yday) = (to_local(secs), to_local(now), to_local(now.saturating_sub(86_400)));
    let same = |a: &SYSTEMTIME, b: &SYSTEMTIME| (a.wYear, a.wMonth, a.wDay) == (b.wYear, b.wMonth, b.wDay);
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let day = if same(&t, &today) {
        "Today".to_string()
    } else if same(&t, &yday) {
        "Yesterday".to_string()
    } else {
        format!("{} {} {}", DAYS[t.wDayOfWeek as usize % 7], t.wDay, MONTHS[(t.wMonth as usize).saturating_sub(1) % 12])
    };
    (day, format!("{:02}:{:02}", t.wHour, t.wMinute))
}

/// Open a downloaded file, or show it selected in Explorer.
fn open_path(path: &str, folder: bool) {
    use windows::{Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL}, core::HSTRING};
    unsafe {
        if folder {
            let args = HSTRING::from(format!("/select,\"{path}\""));
            ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from("explorer.exe"), &args, None, SW_SHOWNORMAL);
        } else {
            ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(path), None, None, SW_SHOWNORMAL);
        }
    }
}

fn resize_dir(d: &str) -> Option<ResizeDirection> {
    Some(match d {
        "n" => ResizeDirection::North,
        "s" => ResizeDirection::South,
        "e" => ResizeDirection::East,
        "w" => ResizeDirection::West,
        "ne" => ResizeDirection::NorthEast,
        "nw" => ResizeDirection::NorthWest,
        "se" => ResizeDirection::SouthEast,
        "sw" => ResizeDirection::SouthWest,
        _ => return None,
    })
}

// ----------------------------------------------------------------------- app

impl App {
    fn idx(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn active_idx(&self) -> usize {
        self.idx(self.active).unwrap_or(0)
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.idx(self.active).map(|i| &self.tabs[i])
    }

    fn fullscreen(&self) -> bool {
        self.page_fs || self.user_fs
    }

    fn pinned_count(&self) -> usize {
        self.tabs.iter().filter(|t| t.pinned).count()
    }

    fn sidebar_w(&self) -> f64 {
        if self.settings.compact && !self.peek { COMPACT_W } else { self.settings.sidebar_width as f64 }
    }

    /// The UI needs the whole window only for the new tab page and settings.
    /// The rest of the time it is a strip. A small surface is a fast surface.
    fn ui_full(&self) -> bool {
        self.panel != Panel::None || self.active_tab().is_some_and(|t| t.status() == "new")
    }

    /// The page rectangle, in window coordinates.
    fn stage(&self) -> (f64, f64, f64, f64) {
        let (w, h) = logical_size(&self.ctx);
        if self.fullscreen() {
            return (0.0, 0.0, w, h);
        }
        let side = self.sidebar_w();
        let g = self.settings.gap as f64;
        let x = if self.settings.sidebar_side == "right" { g } else { side };
        (x, g, (w - side - g).max(1.0), (h - 2.0 * g).max(1.0))
    }

    fn layout(&mut self) {
        let (w, h) = logical_size(&self.ctx);
        let fs = self.fullscreen();
        self.chrome.set_visible(!fs);
        if !fs {
            let side = self.sidebar_w();
            let (x, cw) = if self.ui_full() {
                (0.0, w)
            } else if self.settings.sidebar_side == "right" {
                (w - side, side)
            } else {
                (0.0, side)
            };
            self.chrome.set_bounds(x, 0.0, cw, h);
        }
        let (x, y, sw, sh) = self.stage();
        let scale = self.ctx.window.scale_factor();
        let radius = if fs { 0 } else { (self.settings.radius as f64 * scale).round() as i32 };
        let i = self.active_idx();
        if let Some(t) = self.tabs.get(i)
            && let Some(v) = &t.view
        {
            let _ = v.set_bounds(logical_rect(x, y, sw, sh));
            // The corner clip is a region rebuild and a repaint. Only when size changed.
            let key = (t.id, (sw * scale) as u32, (sh * scale) as u32 + radius as u32);
            if self.rounded != Some(key) {
                self.rounded = Some(key);
                engine::round_corners(v, radius);
            }
        }
    }

    fn focus_page(&self) {
        // Headless never takes keyboard focus from whatever the person is doing.
        if headless() {
            return;
        }
        match self.active_tab().and_then(|t| t.view.as_ref()) {
            Some(v) if self.panel == Panel::None => {
                let _ = v.focus();
            }
            _ => self.chrome.focus(),
        }
    }

    /// Mark the UI stale. The real send happens once, after the event burst.
    fn push(&mut self) {
        self.ui_dirty = true;
    }

    fn flush(&mut self) {
        if !self.ui_ready {
            return;
        }
        if self.ui_dirty {
            self.ui_dirty = false;
            let (sx, sy, sw, sh) = self.stage();
            // Chrome coordinates: full mode starts at the window origin; strip mode is the sidebar only.
            let off = if !self.ui_full() && self.settings.sidebar_side == "right" { logical_size(&self.ctx).0 - self.sidebar_w() } else { 0.0 };
            let state = chrome::UiState {
                tabs: self
                    .tabs
                    .iter()
                    .map(|t| chrome::UiTab {
                        id: t.id,
                        title: t.title.clone(),
                        url: t.url.clone(),
                        loading: t.loading && t.view.is_some(),
                        back: t.back,
                        fwd: t.fwd,
                        pinned: t.pinned,
                        audio: t.audio,
                        muted: t.muted,
                        status: t.status(),
                        agent: t.agent,
                        reader: t.reader.is_some(),
                        private: t.private,
                    })
                    .collect(),
                active: self.active,
                settings: self.settings.clone(),
                dark: self.ctx.dark.get(),
                maximized: self.ctx.window.is_maximized(),
                panel: self.panel.name(),
                peek: self.peek,
                full: self.ui_full(),
                stage: ((sx - off) as f32, sy as f32, sw as f32, sh as f32),
                mem: self.mem,
                blocked: self.blocker.blocked(),
                cpu_cap: self.job.is_some(),
                downloads: self
                    .downloads
                    .iter()
                    .map(|d| chrome::UiDl {
                        id: d.id,
                        name: d.path.rsplit(['\\', '/']).next().unwrap_or(&d.path).to_string(),
                        url: d.url.clone(),
                        received: d.received,
                        total: d.total,
                        state: d.state,
                        speed: d.speed,
                        note: d.note.clone(),
                    })
                    .collect(),
                agent_busy: self.agent_seen.is_some(),
                bookmarked: self.active_tab().is_some_and(|t| !t.url.is_empty() && self.library.is_bookmarked(&t.url)),
                history: if self.panel == Panel::History {
                    self.library
                        .recent("", 0, 400)
                        .into_iter()
                        .map(|v| {
                            let (day, time) = local_day_time(v.last);
                            chrome::UiVisit { url: v.url, title: v.title, day, time }
                        })
                        .collect()
                } else {
                    Vec::new()
                },
                bookmarks: if self.panel == Panel::Bookmarks {
                    self.library.bookmarks().into_iter().map(|b| (b.url, b.title)).collect()
                } else {
                    Vec::new()
                },
                handoff: self.handoff.as_ref().map(|h| (h.1.clone(), h.2.clone())),
                prompt: self
                    .prompts
                    .iter()
                    .find(|p| p.1 == self.active)
                    .or(self.prompts.first())
                    .map(|p| (p.0, p.2.clone(), p.3)),
                rules: self
                    .rules
                    .iter()
                    .flat_map(|(o, r)| r.iter().map(move |(k, a)| (o.clone(), k.clone(), *a)))
                    .collect(),
                agent_cmd: self.agent.setup_command(self.settings.agent_port),
            };
            self.chrome.set_state(state);
            let bg = self.chrome.bg();
            if bg != self.bg {
                self.bg = bg;
                self.apply_backdrop();
            }
        }
        for id in std::mem::take(&mut self.fav_queue) {
            if let Some(i) = self.idx(id) {
                self.chrome.set_fav(id, &self.tabs[i].favicon);
            }
        }
    }

    // ---------------------------------------------------------------- tabs

    fn restore(&mut self, session: Session) {
        let mut active = None;
        for (i, saved) in session.tabs.into_iter().enumerate() {
            let mut t = Tab::new(self.ctx.alloc_id(), saved.url);
            t.title = saved.title;
            t.favicon = saved.favicon;
            t.pinned = saved.pinned;
            if i == session.active {
                active = Some(t.id);
            }
            self.tabs.push(t);
        }
        // Pinned tabs live at the front. The UI groups them; the order must agree.
        self.tabs.sort_by_key(|t| !t.pinned);
        if self.tabs.is_empty() {
            self.tabs.push(Tab::new(self.ctx.alloc_id(), String::new()));
        }
        // Only the active tab gets a page. The rest stay "off" until clicked.
        let id = active.filter(|id| self.idx(*id).is_some()).unwrap_or(self.tabs[0].id);
        self.activate(id);
    }

    fn ensure_view(&mut self, i: usize) {
        if self.tabs[i].view.is_some() || self.tabs[i].url.is_empty() {
            return;
        }
        let (x, y, w, h) = self.stage();
        let t = &mut self.tabs[i];
        let isolated = t.agent && !self.settings.agent_shared_profile;
        // An agent's page starts blank, so the recording is on before the first request.
        let first = if t.agent { None } else { Some(t.url.as_str()) };
        if let Ok(v) = engine::build_tab(&self.ctx, t.id, first, logical_rect(x, y, w, h), isolated, t.private) {
            if t.muted {
                engine::set_muted(&v, true);
            }
            if t.agent {
                agentkit::watch(&v, t.id, &self.agent_logs);
                let _ = v.load_url(&t.url);
            }
            t.view = Some(v);
            t.suspended = false;
            t.trimmed = false;
        }
        self.limit_tab(i);
    }

    /// New page: give it the bandwidth cap and, if the Control page is open, a byte counter.
    fn limit_tab(&mut self, i: usize) {
        let t = &self.tabs[i];
        let Some(v) = &t.view else { return };
        if self.settings.net_limit {
            monitor::throttle(v, Some(self.settings.net_limit_kbps));
        }
        if self.panel == Panel::Control
            && !self.watches.contains_key(&t.id)
            && let Some(w) = monitor::watch(v, t.id, self.net.clone())
        {
            self.watches.insert(t.id, w);
        }
    }

    fn activate(&mut self, id: u64) {
        let Some(i) = self.idx(id) else { return };
        let prev = (id != self.active).then(|| self.idx(self.active)).flatten();
        self.active = id;
        self.leave_panel();
        self.ensure_view(i);
        // Show the new page first, then hide the old. No flash of empty stage between.
        self.layout();
        let t = &mut self.tabs[i];
        t.hidden_at = None;
        t.suspended = false;
        if let Some(v) = &t.view {
            if t.trimmed {
                let _ = v.set_memory_usage_level(MemoryUsageLevel::Normal);
                t.trimmed = false;
            }
            let _ = v.set_visible(true);
        }
        if let Some(p) = prev {
            let p = &mut self.tabs[p];
            if let Some(v) = &p.view {
                let _ = v.set_visible(false);
            }
            p.hidden_at = Some(Instant::now());
        }
        self.focus_page();
        self.dirty = true;
        self.push();
    }

    fn new_tab(&mut self, url: Option<String>, after: Option<usize>) -> u64 {
        let id = self.ctx.alloc_id();
        let t = Tab::new(id, url.unwrap_or_default());
        let floor = self.pinned_count();
        match after {
            Some(i) if i < self.tabs.len() => self.tabs.insert((i + 1).max(floor), t),
            _ => self.tabs.push(t),
        }
        self.activate(id);
        id
    }

    /// Ctrl+Shift+N, or the tab menu. WebView2 InPrivate: no cookies outlive the
    /// last private page, and Lumen writes nothing about it anywhere.
    fn new_private_tab(&mut self) -> u64 {
        let id = self.ctx.alloc_id();
        let mut t = Tab::new(id, String::new());
        t.private = true;
        self.tabs.push(t);
        self.activate(id);
        id
    }

    /// Which cookie jar a tab talks to: 0 the user's, 1 an agent's own, 2 a private one.
    fn jar(&self, t: &Tab) -> u8 {
        if t.private {
            2
        } else if t.agent && !self.settings.agent_shared_profile {
            1
        } else {
            0
        }
    }

    /// A tab that leaves no trace: no history, no page text, no session, no reopen.
    fn off_record(&self, t: &Tab) -> bool {
        t.agent || t.private
    }

    /// A page of the user's own profile, to ask for cookies or clear data with.
    fn plain_view(&self) -> Option<&WebView> {
        self.tabs.iter().filter(|t| !self.off_record(t)).find_map(|t| t.view.as_ref())
    }

    fn close(&mut self, id: u64) {
        let Some(i) = self.idx(id) else { return };
        let t = self.tabs.remove(i);
        self.agent_logs.borrow_mut().remove(&id);
        if self.handoff.as_ref().is_some_and(|h| h.0 == id) {
            if let Some((_, _, _, r)) = self.handoff.take() {
                let _ = r.send(Err("the tab was closed".into()));
            }
        }
        // Requests from a closed tab go unanswered by the user; let WebView2 refuse them.
        for (pid, _, _, _) in self.prompts.iter().filter(|p| p.1 == id) {
            engine::answer_permission(&self.ctx, *pid, None);
        }
        self.prompts.retain(|p| p.1 != id);
        if let Some(w) = self.watches.remove(&id) {
            monitor::unwatch(None, w, true);
        }
        if !t.url.is_empty() && !t.private {
            self.closed.push(SavedTab { url: t.url, title: t.title, favicon: t.favicon, pinned: t.pinned });
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
        drop(t.view);
        if self.tabs.is_empty() {
            self.new_tab(None, None);
            return;
        }
        if id == self.active {
            let next = self.tabs[i.min(self.tabs.len() - 1)].id;
            self.active = 0;
            self.activate(next);
        } else {
            self.dirty = true;
            self.push();
        }
    }

    /// Drop the page, keep the tab. Frees the renderer process.
    fn discard(&mut self, i: usize) {
        let t = &mut self.tabs[i];
        if t.id == self.active || t.view.is_none() {
            return;
        }
        if let Some(w) = self.watches.remove(&t.id) {
            monitor::unwatch(None, w, true);
        }
        t.view = None;
        t.reader = None;
        t.suspended = false;
        t.trimmed = false;
        t.loading = false;
        t.audio = false;
    }

    /// Someone started lumen.exe while this one runs. Headless becomes a normal window:
    /// centred, in the taskbar, with sound. A normal one just comes to the front.
    fn show_me(&mut self) {
        let w = self.ctx.window.clone();
        if headless() {
            HEADLESS.store(false, std::sync::atomic::Ordering::Relaxed);
            use tao::platform::windows::WindowExtWindows as _;
            let _ = w.set_skip_taskbar(false);
            if let Some(m) = w.current_monitor().or_else(|| w.primary_monitor()) {
                let (ms, mp, ws) = (m.size(), m.position(), w.outer_size());
                let x = mp.x + (ms.width as i32 - ws.width as i32).max(0) / 2;
                let y = mp.y + (ms.height as i32 - ws.height as i32).max(0) / 2;
                w.set_outer_position(tao::dpi::PhysicalPosition::new(x, y));
            }
            for t in &self.tabs {
                if let Some(v) = &t.view {
                    engine::set_muted(v, t.muted);
                }
            }
        }
        w.set_visible(true);
        w.set_minimized(false);
        w.set_focus();
        self.layout();
        self.focus_page();
    }

    // ---------------------------------------------------------- downloads

    /// A download id in band `band` (1 aria2, 2 video, 3 remembered). Ids never repeat.
    fn next_dl(&mut self, band: u64) -> u64 {
        self.dl_seq += 1;
        (band << 40) + self.dl_seq
    }

    fn load_downloads(&mut self) {
        for s in store::load_downloads().into_iter().take(100) {
            let id = self.next_dl(3);
            let state = if s.state == "done" { "done" } else { "failed" };
            self.downloads.push(engine::Dl {
                id,
                url: s.url,
                path: s.path,
                received: s.size,
                total: s.size,
                state,
                speed: 0,
                note: s.note,
                private: false,
            });
        }
    }

    fn save_downloads(&mut self) {
        self.dl_dirty = false;
        let keep: Vec<store::SavedDl> = self
            .downloads
            .iter()
            .filter(|d| !d.private)
            .filter(|d| d.state == "done" || d.state == "failed")
            .take(100)
            .map(|d| store::SavedDl { url: d.url.clone(), path: d.path.clone(), state: d.state.to_string(), size: d.received, note: d.note.clone() })
            .collect();
        store::save_downloads(&keep);
    }

    /// Off the list, and out of whatever engine still remembers it. A running one is stopped.
    fn remove_download(&mut self, id: u64) {
        if let Some(gid) = self.aria_ids.iter().find(|(_, v)| **v == id).map(|(g, _)| g.clone()) {
            self.aria_ids.remove(&gid);
            self.aria_meta.remove(&gid);
            let aria = self.aria.clone();
            std::thread::spawn(move || {
                let _ = aria.remove(&gid);
            });
        } else if let Some((_, _, slot)) = self.videos.remove(&id) {
            video::stop(&slot);
        } else if self.downloads.iter().any(|d| d.id == id && (d.state == "progress" || d.state == "paused")) {
            engine::dl_control(&self.ctx, id, "cancel");
        }
        self.ctx.downloads.borrow_mut().remove(&id);
        self.downloads.retain(|d| d.id != id);
        self.dl_dirty = true;
        self.push();
    }

    /// Try a failed download again: same URL, same headers when Lumen still has them.
    fn retry_download(&mut self, id: u64) {
        let Some(d) = self.downloads.iter().find(|d| d.id == id) else { return };
        if self.videos.contains_key(&id) {
            if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                d.state = "progress";
                d.note = "trying again".into();
            }
            self.run_video(id);
            self.push();
            return;
        }
        let url = d.url.clone();
        if !url.starts_with("http") {
            return;
        }
        let private = d.private;
        let gid = self.aria_ids.iter().find(|(_, v)| **v == id).map(|(g, _)| g.clone());
        let (headers, name) = gid
            .as_ref()
            .and_then(|g| self.aria_meta.get(g))
            .map(|(_, h, n)| (h.clone(), n.clone()))
            .unwrap_or_else(|| (Vec::new(), d.path.rsplit(['\\', '/']).next().map(str::to_string).filter(|s| !s.is_empty())));
        self.remove_download(id);
        let (aria, proxy) = (self.aria.clone(), self.ctx.proxy.clone());
        let tab = self.active;
        std::thread::spawn(move || {
            let _ = match aria.add(&url, &headers, name.as_deref()) {
                Ok(gid) => proxy.send_event(Ev::AriaAdded(gid, url, headers, name, private)),
                Err(_) => proxy.send_event(Ev::AriaFailed(tab, url)),
            };
        });
    }

    // ---------------------------------------------------------- video

    /// Save the video on this tab's page (yt-dlp), into Downloads.
    fn save_video(&mut self, i: usize) {
        let t = &self.tabs[i];
        let url = t.reader.clone().unwrap_or_else(|| t.url.clone());
        if !url.starts_with("http") {
            return;
        }
        let title = if t.title.is_empty() { host_of(&url) } else { t.title.clone() };
        let ua = t.view.as_ref().map(engine::user_agent).unwrap_or_default();
        let private = t.private;
        let id = self.next_dl(2);
        self.downloads.insert(
            0,
            engine::Dl {
                id,
                url: url.clone(),
                path: format!("{title}.mp4"),
                received: 0,
                total: 0,
                state: "progress",
                speed: 0,
                note: "starting".into(),
                private,
            },
        );
        self.videos.insert(id, (url, ua, Default::default()));
        self.run_video(id);
        self.push();
    }

    fn run_video(&self, id: u64) {
        let Some((url, ua, slot)) = self.videos.get(&id).cloned() else { return };
        let tools = store::data_dir().join("tools");
        let out = dirs::download_dir().unwrap_or_else(store::data_dir);
        let proxy = self.ctx.proxy.clone();
        std::thread::Builder::new()
            .name("lumen-video".into())
            .spawn(move || {
                video::grab(&tools, &out, &url, &ua, &slot, |u| {
                    let _ = proxy.send_event(Ev::Video(id, u));
                })
            })
            .ok();
    }

    fn on_video(&mut self, id: u64, up: video::Up) {
        let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) else { return };
        if d.state != "progress" {
            return;
        }
        match up {
            video::Up::Note(n) => d.note = n,
            video::Up::Progress { done, total, speed, file } => {
                d.received = done as i64;
                d.total = total as i64;
                d.speed = speed;
                // Video and audio come as separate parts ("x.f399.mp4"); say which.
                d.note = match part_kind(&file) {
                    Some(k) => k.to_string(),
                    None => String::new(),
                };
            }
            video::Up::Done(f) => {
                self.dl_dirty = true;
                d.path = f;
                d.state = "done";
                d.speed = 0;
                d.note.clear();
                d.received = std::fs::metadata(&d.path).map(|m| m.len() as i64).unwrap_or(d.received);
            }
            video::Up::Failed(e) => {
                self.dl_dirty = true;
                d.state = "failed";
                d.speed = 0;
                d.note = e;
            }
        }
        self.push();
    }

    /// Pause stops yt-dlp (it keeps its .part files); resume runs it again and it carries on.
    fn video_control(&mut self, id: u64, a: &str) {
        let Some((_, _, slot)) = self.videos.get(&id) else { return };
        video::stop(slot);
        let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) else { return };
        match a {
            "pause" => {
                d.state = "paused";
                d.speed = 0;
            }
            "resume" => {
                d.state = "progress";
                d.note = "resuming".into();
                self.run_video(id);
            }
            _ => {
                d.state = "failed";
                d.speed = 0;
                d.note = "cancelled".into();
            }
        }
        self.push();
    }

    // ---------------------------------------------------------- page kit

    /// Reader view on, or back to the page.
    fn toggle_reader(&mut self, i: usize) {
        let t = &mut self.tabs[i];
        let Some(v) = &t.view else { return };
        if let Some(orig) = t.reader.take() {
            let _ = v.load_url(&orig);
            self.push();
            return;
        }
        if !t.url.starts_with("http") {
            return;
        }
        let (id, url, proxy) = (t.id, t.url.clone(), self.ctx.proxy.clone());
        let _ = v.evaluate_script_with_callback(&pagekit::extract_script(), move |out| {
            let _ = proxy.send_event(Ev::Reader(id, url.clone(), out));
        });
    }

    fn show_reader(&mut self, id: u64, url: &str, out: &str) {
        let Some(i) = self.idx(id) else { return };
        let Ok(article) = serde_json::from_str::<Value>(out) else { return };
        // The page moved on while Readability worked, or there is no article.
        if article.is_null() || self.tabs[i].url != url {
            return;
        }
        let th = chrome::theme::Theme::new(&self.settings.palette, self.ctx.dark.get(), &self.settings.accent);
        let st = pagekit::Style { dark: th.dark, bg: th.bg.hex(), text: th.text.hex(), dim: th.dim.hex(), accent: th.accent.hex() };
        let html = pagekit::render(&article, url, &st);
        let t = &mut self.tabs[i];
        if let Some(v) = &t.view
            && v.load_html(&html).is_ok()
        {
            t.reader = Some(url.to_string());
            self.push();
        }
    }

    /// The Dark Reader script for the current settings, pushed to every page.
    fn refresh_dark(&mut self) {
        let on = self.settings.dark_pages && self.ctx.dark.get();
        let js = on.then(|| {
            let th = chrome::theme::Theme::new(&self.settings.palette, true, &self.settings.accent);
            let look = pagekit::Look { bg: th.bg.hex(), text: th.text.hex() };
            Rc::<str>::from(pagekit::dark_script(&look, &self.settings.dark_skip))
        });
        self.ctx.dark_js.replace(js);
        for t in self.tabs.iter().filter(|t| !t.agent) {
            if let Some(v) = &t.view {
                engine::set_dark(&self.ctx, v, t.id, true, None);
            }
        }
    }

    /// Shift+F9: this site keeps its own colors, or gets darkened again.
    fn toggle_dark_site(&mut self, i: usize) {
        let h = host_of(&self.tabs[i].url);
        if h.is_empty() {
            return;
        }
        let mut s = self.settings.clone();
        if !s.dark_pages {
            s.dark_pages = true;
            s.dark_skip.retain(|x| *x != h);
        } else if let Some(k) = s.dark_skip.iter().position(|x| *x == h) {
            s.dark_skip.remove(k);
        } else {
            s.dark_skip.push(h);
        }
        self.apply_settings(s);
    }

    fn navigate(&mut self, raw: &str) {
        let Some(url) = resolve_input(raw, &self.settings.search) else { return };
        let i = self.active_idx();
        self.leave_panel();
        if let Some(v) = &self.tabs[i].view {
            let _ = v.load_url(&url);
            self.tabs[i].url = url;
        } else {
            self.tabs[i].url = url;
            self.ensure_view(i);
            if let Some(v) = &self.tabs[i].view {
                let _ = v.set_visible(true);
            }
        }
        self.layout();
        self.focus_page();
        self.dirty = true;
        self.push();
    }

    fn cycle(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        let i = (self.active_idx() as isize + step).rem_euclid(n) as usize;
        let id = self.tabs[i].id;
        self.activate(id);
    }

    fn set_panel(&mut self, p: Panel) {
        let was = self.panel;
        self.panel = p;
        if was == Panel::Control && p != Panel::Control {
            self.stop_monitor();
        }
        if p == Panel::Control && was != Panel::Control {
            self.start_monitor();
        }
        if let Some(v) = self.active_tab().and_then(|t| t.view.as_ref()) {
            let _ = v.set_visible(p == Panel::None);
        }
        self.layout();
        self.focus_page();
        self.push();
    }

    /// Switching tabs or navigating closes any panel, quietly.
    fn leave_panel(&mut self) {
        if self.panel == Panel::Control {
            self.stop_monitor();
        }
        self.panel = Panel::None;
    }

    fn start_monitor(&mut self) {
        self.net.borrow_mut().clear();
        self.prev_cpu.clear();
        for i in 0..self.tabs.len() {
            self.limit_tab(i);
        }
        self.sample();
    }

    fn stop_monitor(&mut self) {
        self.next_sample = None;
        let keep = self.settings.net_limit;
        for (id, w) in std::mem::take(&mut self.watches) {
            let v = self.idx(id).and_then(|i| self.tabs[i].view.as_ref());
            monitor::unwatch(v, w, keep);
        }
    }

    fn sample(&mut self) {
        self.next_sample = Some(Instant::now() + SAMPLE);
        if let Some(env) = self.ctx.env.borrow().as_ref() {
            monitor::sample(env, self.ctx.proxy.clone());
        }
    }

    /// Raw process numbers in; per-tab memory, CPU and network out, straight to the UI.
    fn on_usage(&mut self, procs: Vec<monitor::Proc>) {
        if self.panel != Panel::Control {
            return;
        }
        let now = Instant::now();
        let secs = (now - self.prev_at).as_secs_f64().max(0.001);
        self.prev_at = now;
        let first = self.prev_cpu.is_empty();
        // Main-frame id -> tab.
        let mut owner: HashMap<u32, u64> = HashMap::new();
        for t in &self.tabs {
            if let Some(f) = t.view.as_ref().and_then(monitor::frame_id) {
                owner.insert(f, t.id);
            }
        }
        let mut tabs: HashMap<u64, (f64, f64)> = HashMap::new();
        let (mut ui, mut core, mut total) = ((0.0, 0.0), (0.0, 0.0), (0.0, 0.0));
        let mut next_cpu = HashMap::new();
        let mut parts: Vec<Value> = Vec::new();
        for p in &procs {
            let prev = self.prev_cpu.get(&p.pid).copied().unwrap_or(p.cpu);
            next_cpu.insert(p.pid, p.cpu);
            // 100 ns ticks to percent of the whole machine.
            let cpu = if first { 0.0 } else { (p.cpu.saturating_sub(prev) as f64 / 1e7) / secs / self.cores * 100.0 };
            let mem = p.mem as f64;
            total.0 += mem;
            total.1 += cpu;
            let mine: Vec<u64> = p.frames.iter().filter_map(|f| owner.get(f).copied()).collect();
            if !mine.is_empty() {
                // A process shared by several tabs: split it evenly. Honest enough.
                let n = mine.len() as f64;
                for id in mine {
                    let e = tabs.entry(id).or_default();
                    e.0 += mem / n;
                    e.1 += cpu / n;
                }
            } else if p.kind == "host" {
                // Lumen itself: the native chrome lives here.
                ui = (ui.0 + mem, ui.1 + cpu);
            } else {
                core = (core.0 + mem, core.1 + cpu);
                parts.push(json!({"kind": p.kind, "mem": mem, "cpu": cpu}));
            }
        }
        self.prev_cpu = next_cpu;
        let net: HashMap<u64, u64> = std::mem::take(&mut *self.net.borrow_mut());
        let net_total: u64 = net.values().sum();
        let per_tab: serde_json::Map<String, Value> = self
            .tabs
            .iter()
            .map(|t| {
                let (m, c) = tabs.get(&t.id).copied().unwrap_or_default();
                let n = net.get(&t.id).copied().unwrap_or(0) as f64 / secs;
                (t.id.to_string(), json!({"mem": m, "cpu": c, "net": n}))
            })
            .collect();
        let report = json!({
            "tabs": per_tab,
            "ui": {"mem": ui.0, "cpu": ui.1},
            "core": {"mem": core.0, "cpu": core.1, "parts": parts},
            "total": {"mem": total.0, "cpu": total.1, "net": net_total as f64 / secs, "procs": procs.len()},
        });
        self.chrome.usage(report);
    }

    /// CPU cap and bandwidth cap. The RAM limit is enforced in tick().
    fn apply_limits(&mut self, old: Option<&Settings>) {
        let s = &self.settings;
        let cpu_changed = old.is_none_or(|o| (o.cpu_limit, o.cpu_limit_pct) != (s.cpu_limit, s.cpu_limit_pct));
        if cpu_changed && let Some(job) = &self.job {
            job.cap_cpu(s.cpu_limit.then_some(s.cpu_limit_pct));
        }
        let net_changed = old.is_some_and(|o| (o.net_limit, o.net_limit_kbps) != (s.net_limit, s.net_limit_kbps));
        if net_changed {
            let cap = s.net_limit.then_some(s.net_limit_kbps);
            for v in self.tabs.iter().filter_map(|t| t.view.as_ref()) {
                monitor::throttle(v, cap);
            }
        }
    }

    fn set_fullscreen(&mut self) {
        let on = self.fullscreen();
        self.ctx.window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        self.rounded = None;
        self.layout();
        self.push();
    }

    // ------------------------------------------------------------ settings

    /// The window behind the page gaps takes the sidebar's color.
    /// The native chrome is solid; an opaque window is the cheapest thing Windows draws.
    fn apply_backdrop(&self) {
        let (r, g, b) = self.bg;
        let w = &*self.ctx.window;
        w.set_background_color(Some((r, g, b, 255)));
        engine::set_glass(w.hwnd(), false);
    }

    fn retheme(&mut self) {
        let dark = resolve_dark(&self.settings, self.ctx.window.theme());
        self.ctx.dark.set(dark);
        let theme = engine::theme(dark);
        for v in self.tabs.iter().filter_map(|t| t.view.as_ref()) {
            let _ = v.set_theme(theme);
        }
        self.apply_backdrop();
        self.refresh_dark();
    }

    fn apply_settings(&mut self, mut new: Settings) {
        new.clamp();
        let old = std::mem::replace(&mut self.settings, new);
        if old.theme != self.settings.theme {
            self.retheme();
        } else if old.backdrop != self.settings.backdrop {
            self.apply_backdrop();
        }
        if old.tracking != self.settings.tracking {
            self.ctx.tracking.replace(self.settings.tracking.clone());
            // Tracking prevention is per profile; any page reaches it.
            if let Some(v) = self.tabs.iter().find_map(|t| t.view.as_ref()) {
                engine::set_tracking(v, &self.settings.tracking);
            }
        }
        if old.radius != self.settings.radius {
            self.rounded = None;
        }
        self.blocker.set_enabled(self.settings.adblock);
        self.blocker.set_cosmetic(self.settings.cosmetic);
        self.apply_limits(Some(&old));
        if old.save_passwords != self.settings.save_passwords {
            self.ctx.passwords.set(self.settings.save_passwords);
            for t in self.tabs.iter().filter(|t| !t.agent || self.settings.agent_shared_profile) {
                if let Some(v) = &t.view {
                    engine::set_passwords(v, self.settings.save_passwords);
                }
            }
        }
        if old.dark_pages != self.settings.dark_pages
            || old.dark_skip != self.settings.dark_skip
            || old.palette != self.settings.palette
            || old.accent != self.settings.accent
        {
            self.refresh_dark();
        }
        if old.agent != self.settings.agent || old.agent_port != self.settings.agent_port {
            self.agent.set(self.settings.agent, self.settings.agent_port, &self.ctx.proxy);
        }
        store::save_settings(&self.settings);
        self.layout();
        self.push();
    }

    // -------------------------------------------------------------- events

    fn on_event(&mut self, ev: Ev) {
        if cfg!(debug_assertions) && !quiet_event(&ev) {
            debug_log(&ev);
        }
        match ev {
            Ev::Ui(msg) => self.on_ui(&msg),
            Ev::Key(a) => self.on_key(a),
            Ev::Title(id, t) => {
                if let Some(i) = self.idx(id) {
                    if !self.off_record(&self.tabs[i]) {
                        self.library.set_title(&self.tabs[i].url, &t);
                    }
                    self.tabs[i].title = t;
                    self.dirty = true;
                    self.push();
                }
            }
            Ev::Url(id, u) => {
                if let Some(i) = self.idx(id) {
                    // Reader view is a page made from a string; the tab keeps the real URL.
                    if self.tabs[i].reader.is_some() {
                        if !u.starts_with("http") {
                            return;
                        }
                        self.tabs[i].reader = None;
                    }
                    // Agents browse off the record, and so do private tabs.
                    if !self.off_record(&self.tabs[i]) {
                        self.library.visit(&u, &self.tabs[i].title);
                    }
                    self.tabs[i].url = u;
                    self.dirty = true;
                    self.push();
                }
            }
            Ev::Loading(id, on) => {
                if let Some(i) = self.idx(id)
                    && self.tabs[i].loading != on
                {
                    self.tabs[i].loading = on;
                    self.push();
                    // Done loading: read the page a little later, once scripts have filled it in.
                    let t = &self.tabs[i];
                    if !on && self.settings.remember_text && !self.off_record(t) && t.reader.is_none() && t.url.starts_with("http") {
                        let (proxy, url) = (self.ctx.proxy.clone(), t.url.clone());
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(1500));
                            let _ = proxy.send_event(Ev::Capture(id, url));
                        });
                    }
                }
            }
            Ev::Capture(id, url) => {
                if let Some(i) = self.idx(id)
                    && self.tabs[i].url == url
                    && self.tabs[i].reader.is_none()
                    && let Some(v) = &self.tabs[i].view
                {
                    // Pages that ask for a password are not kept.
                    let js = format!(
                        "(()=>{{if(document.querySelector('input[type=password]'))return null;const b=document.body;if(!b)return null;return {{t:document.title,x:b.innerText.slice(0,{})}}}})()",
                        recall::MAX_TEXT
                    );
                    let proxy = self.ctx.proxy.clone();
                    let _ = v.evaluate_script_with_callback(&js, move |out| {
                        let _ = proxy.send_event(Ev::PageText(id, url.clone(), out));
                    });
                }
            }
            Ev::PageText(id, url, out) => {
                if self.idx(id).is_some_and(|i| !self.off_record(&self.tabs[i]) && self.tabs[i].url == url)
                    && let Ok(v) = serde_json::from_str::<Value>(&out)
                    && let Some(text) = v["x"].as_str()
                    && text.trim().len() > 40
                {
                    self.recall.add(&url, v["t"].as_str().unwrap_or(""), text);
                }
            }
            Ev::TextHits(q, hits) => {
                let hits = hits
                    .into_iter()
                    .map(|(url, title, snip)| {
                        let name = if title.is_empty() { host_of(&url) } else { title };
                        (url, name, false, snip)
                    })
                    .collect();
                self.chrome.set_suggest(&q, hits);
            }
            Ev::Favicon(id, f) => {
                if let Some(i) = self.idx(id)
                    && self.tabs[i].reader.is_none()
                    && self.tabs[i].favicon != f
                {
                    self.tabs[i].favicon = f;
                    self.fav_queue.push(id);
                    self.dirty = true;
                }
            }
            Ev::Nav(id, b, f) => {
                if let Some(i) = self.idx(id)
                    && (self.tabs[i].back, self.tabs[i].fwd) != (b, f)
                {
                    self.tabs[i].back = b;
                    self.tabs[i].fwd = f;
                    self.push();
                }
            }
            Ev::Audio(id, on) => {
                if let Some(i) = self.idx(id) {
                    self.tabs[i].audio = on;
                    self.push();
                }
            }
            Ev::Suspended(id, ok) => {
                if let Some(i) = self.idx(id) {
                    // Only count it if the tab is still hidden. User may have clicked back.
                    self.tabs[i].suspended = ok && id != self.active;
                    self.push();
                }
            }
            Ev::FullScreen(id, on) => {
                if id == self.active {
                    self.page_fs = on;
                    self.set_fullscreen();
                }
            }
            Ev::CloseMe(id) => self.close(id),
            Ev::Usage(procs) => self.on_usage(procs),
            Ev::Download(d) => {
                if matches!(d.state, "done" | "failed") {
                    self.dl_dirty = true;
                }
                match self.downloads.iter_mut().find(|x| x.id == d.id) {
                    Some(x) => *x = d,
                    None => {
                        self.downloads.insert(0, d);
                        self.downloads.truncate(100);
                    }
                }
                self.push();
            }
            Ev::Agent(call) => self.on_agent(call),
            Ev::Permission { pid, tab, origin, kind } => {
                match self.rules.get(&origin).and_then(|r| r.get(kind)).copied() {
                    Some(allow) => engine::answer_permission(&self.ctx, pid, Some(allow)),
                    None => {
                        self.prompts.push((pid, tab, origin, kind));
                        self.push();
                    }
                }
            }
            Ev::Handoff(tab, url, page, name) => {
                let mut headers = Vec::new();
                let view = self.idx(tab).and_then(|i| self.tabs[i].view.as_ref());
                if let Some(v) = view {
                    let ua = engine::user_agent(v);
                    if !ua.is_empty() {
                        headers.push(format!("User-Agent: {ua}"));
                    }
                }
                if !page.is_empty() && page != "about:blank" {
                    headers.push(format!("Referer: {page}"));
                }
                let (aria, proxy) = (self.aria.clone(), self.ctx.proxy.clone());
                let u = url.clone();
                // A download started by a private tab leaves no row on disk either.
                let jar = self.idx(tab).map_or(0, |i| self.jar(&self.tabs[i]));
                let private = jar == 2;
                let start = move |cookie: String| {
                    if !cookie.is_empty() {
                        headers.push(format!("Cookie: {cookie}"));
                    }
                    std::thread::spawn(move || {
                        let name = (!name.is_empty()).then_some(name);
                        let _ = match aria.add(&u, &headers, name.as_deref()) {
                            Ok(gid) => proxy.send_event(Ev::AriaAdded(gid, u, headers, name, private)),
                            Err(_) => proxy.send_event(Ev::AriaFailed(tab, u)),
                        };
                    });
                };
                // Cookies live in the profile, so ask a settled page of the same jar:
                // a popup that is turning into a download may never answer. A private
                // tab's jar is its own; an agent's is its own; never mix them.
                let asker = self
                    .tabs
                    .iter()
                    .filter(|t| t.id != tab && !t.popup && self.jar(t) == jar)
                    .find_map(|t| t.view.as_ref())
                    .or(view);
                match asker {
                    Some(v) => engine::cookie_header(v, &url, start),
                    None => start(String::new()),
                }
            }
            Ev::AriaAdded(gid, url, headers, name, private) => {
                let id = self.next_dl(1);
                self.aria_ids.insert(gid.clone(), id);
                self.aria_meta.insert(gid, (url.clone(), headers, name));
                self.aria_poll.store(true, std::sync::atomic::Ordering::Relaxed);
                if private {
                    // Show it now, marked private, so it never reaches downloads.json.
                    self.downloads.insert(
                        0,
                        engine::Dl { id, url, path: String::new(), received: 0, total: 0, state: "progress", speed: 0, note: "starting".into(), private },
                    );
                    self.push();
                }
            }
            Ev::AriaFailed(tab, url) => {
                // aria2 said no (auth, odd server): let WebView2 download it after all.
                self.ctx.bypass.borrow_mut().insert(url.clone());
                if let Some(v) = self.idx(tab).and_then(|i| self.tabs[i].view.as_ref()) {
                    let _ = v.load_url(&url);
                }
            }
            Ev::AriaJobs(jobs) => {
                let mut busy = false;
                for j in jobs {
                    let Some(&id) = self.aria_ids.get(&j.gid) else { continue };
                    let state = match j.status {
                        "active" | "waiting" => "progress",
                        "paused" => "paused",
                        "complete" => "done",
                        _ => "failed",
                    };
                    busy |= state == "progress";
                    let note = match j.status {
                        "active" => format!("{} connections", j.connections),
                        "waiting" => "queued".to_string(),
                        "removed" => "cancelled".to_string(),
                        "error" if !j.error.is_empty() => j.error.trim().trim_end_matches('.').to_string(),
                        _ => String::new(),
                    };
                    if matches!(state, "done" | "failed") && self.downloads.iter().any(|x| x.id == id && x.state != state) {
                        self.dl_dirty = true;
                    }
                    let private = self.downloads.iter().find(|x| x.id == id).is_some_and(|x| x.private);
                    let d = engine::Dl {
                        id,
                        url: j.url,
                        path: j.path,
                        received: j.done as i64,
                        total: j.total as i64,
                        state,
                        speed: j.speed,
                        note,
                        private,
                    };
                    match self.downloads.iter_mut().find(|x| x.id == id) {
                        Some(x) => *x = d,
                        None => self.downloads.insert(0, d),
                    }
                }
                if !busy && !self.downloads.iter().any(|d| d.state == "paused" && d.id >= 1 << 40) {
                    self.aria_poll.store(false, std::sync::atomic::Ordering::Relaxed);
                }
                self.push();
            }
            Ev::Reader(id, url, out) => self.show_reader(id, &url, &out),
            Ev::Video(id, up) => self.on_video(id, up),
            Ev::ShowMe => self.show_me(),
            Ev::Orphan(id) => {
                // Like Chrome: a tab a page opened just for a download goes away.
                // A tab the user typed it into goes back to the new tab page.
                if let Some(i) = self.idx(id) {
                    if self.tabs[i].popup {
                        self.close(id);
                    } else {
                        let t = &mut self.tabs[i];
                        t.view = None;
                        t.url.clear();
                        t.title.clear();
                        t.favicon.clear();
                        t.loading = false;
                        self.dirty = true;
                        self.layout();
                        self.push();
                    }
                }
            }
            Ev::Redirect(id, url) => {
                if let Some(i) = self.idx(id)
                    && let Some(v) = &self.tabs[i].view
                {
                    let _ = v.load_url(&url);
                    self.tabs[i].url = url;
                }
            }
            Ev::Popup(id, parent) => {
                let private = self.idx(parent).is_some_and(|i| self.tabs[i].private);
                let agent = self.idx(parent).is_some_and(|i| self.tabs[i].agent);
                let (owner, owner_name) = self.idx(parent).map(|i| (self.tabs[i].owner.clone(), self.tabs[i].owner_name.clone())).unwrap_or_default();
                let found = {
                    let mut p = self.ctx.popups.borrow_mut();
                    p.iter().position(|(pid, _)| *pid == id).map(|k| p.remove(k).1)
                };
                if let Some(v) = found {
                    let mut t = Tab::new(id, String::new());
                    t.agent = agent;
                    t.private = private;
                    t.owner = owner;
                    t.owner_name = owner_name;
                    t.popup = true;
                    if agent {
                        agentkit::watch(&v, id, &self.agent_logs);
                    }
                    t.view = Some(v);
                    t.title = "Loading…".into();
                    let at = (self.active_idx() + 1).max(self.pinned_count()).min(self.tabs.len());
                    self.tabs.insert(at, t);
                    self.limit_tab(at);
                    self.activate(id);
                }
            }
        }
    }

    fn on_key(&mut self, a: Action) {
        let i = self.active_idx();
        match a {
            Action::NewTab => {
                // Like Chrome: a new tab means "I want to type an address".
                self.new_tab(None, None);
                self.on_key(Action::Omnibox);
            }
            Action::NewPrivate => {
                self.new_private_tab();
            }
            Action::CloseTab => self.close(self.active),
            Action::Reopen => {
                if let Some(t) = self.closed.pop() {
                    self.new_tab(Some(t.url), Some(i));
                }
            }
            Action::NextTab => self.cycle(1),
            Action::PrevTab => self.cycle(-1),
            Action::Nth(n) => {
                if let Some(t) = self.tabs.get(n) {
                    let id = t.id;
                    self.activate(id);
                }
            }
            Action::LastTab => {
                let id = self.tabs[self.tabs.len() - 1].id;
                self.activate(id);
            }
            Action::Omnibox => {
                self.chrome.focus_omnibox();
            }
            Action::Reload => match &self.tabs[i].view {
                Some(v) => {
                    let _ = v.reload();
                }
                None => self.activate(self.active),
            },
            Action::Back => {
                if let Some(v) = &self.tabs[i].view {
                    let _ = v.go_back();
                }
            }
            Action::Forward => {
                if let Some(v) = &self.tabs[i].view {
                    let _ = v.go_forward();
                }
            }
            Action::Sidebar => {
                let mut s = self.settings.clone();
                s.compact = !s.compact;
                self.apply_settings(s);
            }
            Action::Fullscreen => {
                self.user_fs = !self.user_fs;
                self.set_fullscreen();
            }
            Action::Settings => {
                let p = if self.panel == Panel::Settings { Panel::None } else { Panel::Settings };
                self.set_panel(p);
            }
            Action::Bookmark => self.toggle_bookmark(),
            Action::History => {
                let p = if self.panel == Panel::History { Panel::None } else { Panel::History };
                self.set_panel(p);
            }
            Action::Bookmarks => {
                let p = if self.panel == Panel::Bookmarks { Panel::None } else { Panel::Bookmarks };
                self.set_panel(p);
            }
            Action::Downloads => {
                let p = if self.panel == Panel::Downloads { Panel::None } else { Panel::Downloads };
                self.set_panel(p);
            }
            Action::Control => {
                let p = if self.panel == Panel::Control { Panel::None } else { Panel::Control };
                self.set_panel(p);
            }
            Action::DevTools => match &self.tabs[i].view {
                Some(v) => v.open_devtools(),
                None => {}
            },
            Action::Reader => self.toggle_reader(i),
            Action::DarkSite => self.toggle_dark_site(i),
        }
    }

    fn on_ui(&mut self, raw: &str) {
        let Ok(m) = serde_json::from_str::<Value>(raw) else { return };
        let id = m["id"].as_u64().unwrap_or(0);
        match m["t"].as_str().unwrap_or("") {
            "ready" => {
                self.ui_ready = true;
                self.fav_queue = self.tabs.iter().filter(|t| !t.favicon.is_empty()).map(|t| t.id).collect();
                self.push();
                if !self.shown {
                    self.shown = true;
                    show_window(&self.ctx.window);
                    self.layout();
                    if !headless() {
                        self.focus_page();
                    }
                }
            }
            "nav" => self.navigate(m["url"].as_str().unwrap_or("")),
            "newtab" => {
                self.new_tab(None, None);
            }
            "newPrivate" => {
                self.new_private_tab();
            }
            "activate" => self.activate(id),
            "close" => self.close(id),
            "pin" => {
                if let Some(i) = self.idx(id) {
                    let mut t = self.tabs.remove(i);
                    t.pinned = !t.pinned;
                    // Pinned: end of the pinned block. Unpinned: first after it.
                    let at = self.pinned_count();
                    self.tabs.insert(at, t);
                    self.dirty = true;
                    self.push();
                }
            }
            "reader" => {
                if let Some(i) = self.idx(id) {
                    self.toggle_reader(i);
                }
            }
            "video" => {
                if let Some(i) = self.idx(id) {
                    self.save_video(i);
                }
            }
            "darksite" => {
                if let Some(i) = self.idx(id) {
                    self.toggle_dark_site(i);
                }
            }
            "mute" => {
                if let Some(i) = self.idx(id) {
                    let t = &mut self.tabs[i];
                    t.muted = !t.muted;
                    if let Some(v) = &t.view {
                        engine::set_muted(v, t.muted);
                    }
                    self.push();
                }
            }
            "sleep" => {
                if let Some(i) = self.idx(id) {
                    self.discard(i);
                    self.push();
                }
            }
            "duplicate" => {
                if let Some(i) = self.idx(id) {
                    let url = self.tabs[i].url.clone();
                    if !url.is_empty() {
                        self.new_tab(Some(url), Some(i));
                    }
                }
            }
            "closeOthers" => {
                let others: Vec<u64> =
                    self.tabs.iter().filter(|t| t.id != id && !t.pinned).map(|t| t.id).collect();
                if self.idx(id).is_some() {
                    self.activate(id);
                }
                for o in others {
                    self.close(o);
                }
            }
            "move" => {
                if let Some(i) = self.idx(id) {
                    let t = self.tabs.remove(i);
                    // Stay inside your own group: pinned with pinned, the rest with the rest.
                    let pins = self.pinned_count();
                    let (lo, hi) = if t.pinned { (0, pins) } else { (pins, self.tabs.len()) };
                    let to = (m["to"].as_u64().unwrap_or(0) as usize).clamp(lo, hi);
                    self.tabs.insert(to, t);
                    self.dirty = true;
                    self.push();
                }
            }
            "back" => self.on_key(Action::Back),
            "fwd" => self.on_key(Action::Forward),
            "reload" => self.on_key(Action::Reload),
            "stop" => {
                if let Some(v) = self.active_tab().and_then(|t| t.view.as_ref()) {
                    let _ = v.evaluate_script("window.stop()");
                }
            }
            "reopen" => self.on_key(Action::Reopen),
            "devtools" => self.on_key(Action::DevTools),
            "win" => {
                let w = &self.ctx.window;
                match m["a"].as_str().unwrap_or("") {
                    "min" => w.set_minimized(true),
                    "max" => w.set_maximized(!w.is_maximized()),
                    "close" => self.exit = true,
                    _ => {}
                }
            }
            "drag" => {
                let _ = self.ctx.window.drag_window();
            }
            "resize" => {
                if let Some(d) = resize_dir(m["d"].as_str().unwrap_or("")) {
                    let _ = self.ctx.window.drag_resize_window(d);
                }
            }
            "settings" => {
                if let Ok(s) = serde_json::from_value::<Settings>(m["v"].clone()) {
                    self.apply_settings(s);
                }
            }
            "panel" => {
                let p = match (m["open"].as_bool().unwrap_or(false), m["which"].as_str()) {
                    (false, _) => Panel::None,
                    (true, Some("control")) => Panel::Control,
                    (true, Some("downloads")) => Panel::Downloads,
                    (true, Some("history")) => Panel::History,
                    (true, Some("bookmarks")) => Panel::Bookmarks,
                    (true, _) => Panel::Settings,
                };
                self.set_panel(p);
            }
            "peek" => {
                self.peek = m["on"].as_bool().unwrap_or(false);
                self.layout();
                self.push();
            }
            "focusPage" => self.focus_page(),
            "dl" => {
                let a = m["a"].as_str().unwrap_or("");
                match a {
                    "retry" => self.retry_download(id),
                    "remove" => self.remove_download(id),
                    "pause" | "resume" | "cancel" if self.videos.contains_key(&id) => self.video_control(id, a),
                    "pause" | "resume" | "cancel" => match self.aria_ids.iter().find(|(_, v)| **v == id).map(|(g, _)| g.clone()) {
                        Some(gid) => {
                            let aria = self.aria.clone();
                            let act = a.to_string();
                            self.aria_poll.store(true, std::sync::atomic::Ordering::Relaxed);
                            std::thread::spawn(move || {
                                let _ = match act.as_str() {
                                    "pause" => aria.pause(&gid),
                                    "resume" => aria.resume(&gid),
                                    _ => aria.remove(&gid),
                                };
                            });
                        }
                        None => engine::dl_control(&self.ctx, id, a),
                    },
                    "open" | "folder" => {
                        if let Some(d) = self.downloads.iter().find(|d| d.id == id) {
                            open_path(&d.path, a == "folder");
                        }
                    }
                    "clear" => {
                        let gone: Vec<u64> = self.downloads.iter().filter(|d| d.state == "done" || d.state == "failed").map(|d| d.id).collect();
                        for id in gone {
                            self.remove_download(id);
                        }
                    }
                    _ => {}
                }
            }
            "suggest" => {
                let q = m["q"].as_str().unwrap_or("").to_string();
                let hits = self
                    .library
                    .search(&q, 6)
                    .into_iter()
                    .map(|h| (h.url, h.title, h.kind == library::Kind::Bookmark, String::new()))
                    .collect();
                self.chrome.set_suggest(&q, hits);
                // Then what pages said, from the worker thread, when it has looked.
                if self.settings.remember_text && q.trim().len() >= 3 && !q.contains("://") && !(q.contains('.') && !q.contains(' ')) {
                    let proxy = self.ctx.proxy.clone();
                    self.recall.search(&q, 4, move |q, hits| {
                        if !hits.is_empty() {
                            let _ = proxy.send_event(Ev::TextHits(q, hits));
                        }
                    });
                }
            }
            "bookmark" => self.toggle_bookmark(),
            "unbookmark" => {
                self.library.unbookmark(m["url"].as_str().unwrap_or(""));
                self.push();
            }
            "forget" => {
                self.library.forget(m["url"].as_str().unwrap_or(""));
                self.recall.forget(m["url"].as_str().unwrap_or(""));
                self.push();
            }
            "clearHistory" => {
                // Seconds since the epoch; missing or null means everything.
                let since = m["since"].as_u64();
                self.library.clear_history(since);
                self.recall.clear(since);
                // WebView2 keeps a history of its own in the profile; it goes too.
                if let Some(v) = self.plain_view() {
                    engine::clear_data(v, since, false);
                }
                self.push();
            }
            "clearData" => {
                let since = m["since"].as_u64();
                let more = m["more"].as_bool().unwrap_or(false);
                self.library.clear_history(since);
                self.recall.clear(since);
                if let Some(v) = self.plain_view() {
                    engine::clear_data(v, since, more);
                }
                self.push();
            }
            "perm" => {
                let pid = m["pid"].as_u64().unwrap_or(0);
                let allow = m["allow"].as_bool().unwrap_or(false);
                if let Some(k) = self.prompts.iter().position(|p| p.0 == pid) {
                    let (_, _, origin, kind) = self.prompts.remove(k);
                    self.rules.entry(origin).or_default().insert(kind.to_string(), allow);
                    store::save_rules(&self.rules);
                }
                engine::answer_permission(&self.ctx, pid, Some(allow));
                self.push();
            }
            "permReset" => {
                let (origin, kind) = (m["origin"].as_str().unwrap_or(""), m["kind"].as_str().unwrap_or(""));
                if let Some(r) = self.rules.get_mut(origin) {
                    r.remove(kind);
                    if r.is_empty() {
                        self.rules.remove(origin);
                    }
                }
                store::save_rules(&self.rules);
                self.push();
            }
            "clearPasswords" => {
                if let Some(v) = self.tabs.iter().filter(|t| !t.agent).find_map(|t| t.view.as_ref()) {
                    engine::clear_passwords(v);
                }
            }
            "handoffDone" => {
                if let Some((_, _, _, r)) = self.handoff.take() {
                    let _ = r.send(Ok(Value::Null));
                }
                self.push();
            }
            "agentStop" => {
                if let Some((_, _, _, r)) = self.handoff.take() {
                    let _ = r.send(Err("the person stopped the agents".into()));
                }
                // The kill switch: door shut, agent tabs gone.
                let mut s = self.settings.clone();
                s.agent = false;
                self.apply_settings(s);
                let ids: Vec<u64> = self.tabs.iter().filter(|t| t.agent).map(|t| t.id).collect();
                for id in ids {
                    self.close(id);
                }
                self.agent_seen = None;
                self.push();
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- tick

    /// Every few seconds: trim, freeze and discard hidden tabs, read memory, save.
    fn tick(&mut self) {
        let now = Instant::now();
        // Started headless for agents, never shown to a person, no agent tab left, quiet for
        // ten minutes: nobody needs this Lumen. lumen-agent starts a new one when asked.
        if headless() && self.handoff.is_none() && !self.tabs.iter().any(|t| t.agent) && now - self.agent_last > agent_idle() {
            self.library.flush();
            self.clear_on_exit();
            self.save_session();
            std::process::exit(0);
        }
        self.next_tick = now + TICK;

        let sleep = Duration::from_secs(self.settings.sleep_after_min as u64 * 60);
        let discard = Duration::from_secs(self.settings.discard_after_min as u64 * 60);
        for i in 0..self.tabs.len() {
            let t = &mut self.tabs[i];
            if t.id == self.active || t.view.is_none() || t.audio {
                continue;
            }
            let Some(at) = t.hidden_at else { continue };
            let idle = now - at;
            if idle >= discard && !t.pinned {
                self.discard(i);
                self.push();
            } else if idle >= sleep && !t.suspended {
                if let Some(v) = &t.view {
                    engine::try_suspend(v, t.id, self.ctx.proxy.clone());
                }
            } else if idle >= TRIM_AFTER && !t.trimmed {
                if let Some(v) = &t.view {
                    let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
                }
                t.trimmed = true;
            }
        }

        if self.settings.show_memory
            && let Some(env) = self.ctx.env.borrow().as_ref()
        {
            let m = engine::memory(env);
            // Only redraw when it moved by more than 1 MB.
            if self.mem.is_none_or(|(b, _)| b.abs_diff(m.0) > 1 << 20) {
                self.mem = Some(m);
                self.ui_dirty = true;
            }
        }
        self.enforce_ram(now);
        self.settle_clear();
        self.ctx.aria_on.set(self.settings.accelerate_downloads && self.aria.ready());
        self.library.flush();
        if self.agent_seen.is_some_and(|t| now - t > Duration::from_secs(6)) {
            self.agent_seen = None;
            self.push();
        }

        // The UI never said ready. Show the window anyway so the user is not stuck.
        if !self.shown && now - self.born > Duration::from_secs(3) {
            self.shown = true;
            show_window(&self.ctx.window);
        }

        if self.dirty {
            self.save_session();
        }
        if self.dl_dirty {
            self.save_downloads();
        }
    }

    /// One primitive for an agent. An agent may use the tabs its session opened, and, if the
    /// person shared their profile, the person's own tabs. Never another agent's.
    fn on_agent(&mut self, call: agent::Call) {
        use agent::Op;
        self.agent_seen = Some(Instant::now());
        self.agent_last = Instant::now();
        self.push();
        let (reply, who) = (call.reply, call.who);
        let shared = self.settings.agent_shared_profile;
        let mine = |app: &App, tab: u64| app.idx(tab).filter(|&i| if app.tabs[i].agent { app.tabs[i].owner == who.session } else { shared });
        let no_tab = |tab: u64| Err(format!("no tab {tab} you may use; browser_tabs lists yours"));
        match call.op {
            Op::List => {
                let list: Vec<Value> = self
                    .tabs
                    .iter()
                    .filter(|t| if t.agent { t.owner == who.session } else { shared })
                    .map(|t| json!({"tab": t.id, "title": t.title, "url": t.url, "front": t.id == self.active, "yours": t.agent}))
                    .collect();
                let _ = reply.send(Ok(Value::Array(list)));
            }
            Op::Open { url, activate } => {
                let Some(url) = resolve_input(&url, &self.settings.search) else {
                    let _ = reply.send(Err("empty url".into()));
                    return;
                };
                let id = self.ctx.alloc_id();
                let mut t = Tab::new(id, url);
                t.agent = true;
                t.owner = who.session.clone();
                t.owner_name = who.name.clone();
                self.tabs.push(t);
                if activate {
                    self.activate(id);
                } else {
                    let i = self.tabs.len() - 1;
                    self.ensure_view(i);
                }
                let _ = reply.send(Ok(json!({"tab": id})));
            }
            Op::Navigate { tab, url } => match (mine(self, tab), resolve_input(&url, &self.settings.search)) {
                (Some(i), Some(url)) => {
                    match &self.tabs[i].view {
                        Some(v) => {
                            let _ = v.load_url(&url);
                            self.tabs[i].url = url;
                        }
                        None => {
                            self.tabs[i].url = url;
                            self.ensure_view(i);
                        }
                    }
                    let _ = reply.send(Ok(Value::Null));
                }
                _ => {
                    let _ = reply.send(no_tab(tab));
                }
            },
            op @ (Op::Back { .. } | Op::Forward { .. } | Op::Reload { .. }) => {
                let (tab, js) = match op {
                    Op::Back { tab } => (tab, "history.back()"),
                    Op::Forward { tab } => (tab, "history.forward()"),
                    Op::Reload { tab } => (tab, "location.reload()"),
                    _ => return,
                };
                let Some(v) = mine(self, tab).and_then(|i| self.tabs[i].view.as_ref()) else {
                    let _ = reply.send(no_tab(tab));
                    return;
                };
                let _ = v.evaluate_script(js);
                let _ = reply.send(Ok(Value::Null));
            }
            Op::Eval { tab, js } => {
                let Some(i) = mine(self, tab) else {
                    let _ = reply.send(no_tab(tab));
                    return;
                };
                if self.tabs[i].view.is_none() {
                    self.ensure_view(i);
                }
                match &self.tabs[i].view {
                    Some(v) => {
                        let r = reply.clone();
                        let ok = v.evaluate_script_with_callback(&js, move |out| {
                            let val = serde_json::from_str(&out).unwrap_or(Value::String(out));
                            let _ = r.send(Ok(val));
                        });
                        if ok.is_err() {
                            let _ = reply.send(Err("script failed to start".into()));
                        }
                    }
                    None => {
                        let _ = reply.send(Err("tab has no page".into()));
                    }
                }
            }
            Op::Cdp { tab, method, params } => {
                let Some(i) = mine(self, tab) else {
                    let _ = reply.send(no_tab(tab));
                    return;
                };
                if self.tabs[i].view.is_none() {
                    self.ensure_view(i);
                }
                match &self.tabs[i].view {
                    Some(v) => agentkit::cdp(v, &method, &params, move |r| {
                        let _ = reply.send(r);
                    }),
                    None => {
                        let _ = reply.send(Err("tab has no page".into()));
                    }
                }
            }
            Op::Log { tab, network, clear } => {
                if mine(self, tab).is_none() {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                let mut logs = self.agent_logs.borrow_mut();
                let Some(log) = logs.get_mut(&tab) else {
                    let _ = reply.send(Ok(json!([])));
                    return;
                };
                let out = if network {
                    json!(log.net.iter().map(|n| json!({"method": n.method, "url": n.url, "kind": n.kind, "status": n.status, "failed": n.failed})).collect::<Vec<_>>())
                } else {
                    json!(log.console.iter().collect::<Vec<_>>())
                };
                if clear {
                    if network {
                        log.net.clear();
                    } else {
                        log.console.clear();
                    }
                }
                let _ = reply.send(Ok(out));
            }
            Op::Dialogs { tab, accept, text } => {
                if mine(self, tab).is_none() {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                let mut logs = self.agent_logs.borrow_mut();
                let log = logs.entry(tab).or_default();
                log.accept = accept;
                log.prompt_text = text;
                let _ = reply.send(Ok(Value::Null));
            }
            Op::Show { tab } => {
                if mine(self, tab).is_none() {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                self.activate(tab);
                let _ = reply.send(Ok(Value::Null));
            }
            Op::Handoff { tab, message } => {
                if mine(self, tab).is_none() {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                // One at a time. A second agent asking waits its turn with an error it can retry.
                if self.handoff.is_some() {
                    let _ = reply.send(Err("the person is already helping another agent; try again in a minute".into()));
                    return;
                }
                self.activate(tab);
                self.handoff = Some((tab, who.name.clone(), message, reply));
                self.push();
            }
            Op::Close { tab } => {
                if mine(self, tab).is_none() {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                self.close(tab);
                let _ = reply.send(Ok(Value::Null));
            }
            Op::End => {
                let ids: Vec<u64> = self.tabs.iter().filter(|t| t.agent && t.owner == who.session).map(|t| t.id).collect();
                for id in ids {
                    self.close(id);
                }
                let _ = reply.send(Ok(Value::Null));
            }
            Op::Download { url, tab } => {
                if !url.starts_with("http") {
                    let _ = reply.send(Err("only http(s) URLs".into()));
                    return;
                }
                let tab = tab.filter(|t| mine(self, *t).is_some()).unwrap_or(0);
                let page = self.idx(tab).map(|i| self.tabs[i].url.clone()).unwrap_or_default();
                let _ = self.ctx.proxy.send_event(Ev::Handoff(tab, url, page, String::new()));
                let _ = reply.send(Ok(Value::Null));
            }
            Op::Downloads => {
                let list: Vec<Value> = self
                    .downloads
                    .iter()
                    .map(|d| json!({"url": d.url, "path": d.path, "state": d.state, "size": d.received, "total": d.total, "note": d.note}))
                    .collect();
                let _ = reply.send(Ok(Value::Array(list)));
            }
        }
    }

    fn toggle_bookmark(&mut self) {
        let Some(t) = self.active_tab() else { return };
        if t.url.is_empty() || t.agent {
            return;
        }
        let (url, title) = (t.url.clone(), t.title.clone());
        if self.library.is_bookmarked(&url) {
            self.library.unbookmark(&url);
        } else {
            self.library.bookmark(&url, &title);
        }
        self.push();
    }

    /// Soft RAM limit. Over it: unload the hidden tab looked at longest ago.
    /// One per tick, so a spike never empties the window.
    fn enforce_ram(&mut self, now: Instant) {
        if !self.settings.ram_limit {
            return;
        }
        let used = if self.settings.show_memory {
            self.mem.map_or(0, |m| m.0)
        } else {
            self.ctx.env.borrow().as_ref().map_or(0, |e| engine::memory(e).0)
        };
        if used <= self.settings.ram_limit_mb as u64 * 1024 * 1024 {
            return;
        }
        let victim = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.id != self.active && t.view.is_some() && !t.pinned && !t.audio)
            .min_by_key(|(_, t)| t.hidden_at.unwrap_or(now))
            .map(|(i, _)| i);
        if let Some(i) = victim {
            self.discard(i);
            self.push();
        }
    }

    /// Clean close with "clear history on exit" on. Lumen's history and the page
    /// text go right now; WebView2's own history goes now too when a page is there
    /// to ask, and at the next start when there is not (the call is WebView2's to
    /// finish, and this app is on its way out).
    fn clear_on_exit(&mut self) {
        if !self.settings.clear_on_exit {
            return;
        }
        self.library.clear_history(None);
        self.recall.clear(None);
        self.library.flush();
        // WebView2 finishes the wipe on its own time and Lumen is closing, so ask now
        // and again at the next start: clearing twice costs nothing, missing it would.
        if let Some(v) = self.plain_view() {
            engine::clear_data(v, None, false);
        }
        store::set_clear_history_pending(true);
    }

    /// A clear-on-exit the last run could not finish, done at the first chance.
    fn settle_clear(&mut self) {
        if !self.clear_pending {
            return;
        }
        // Take the flag first: the view borrow and the field write must not overlap.
        self.clear_pending = false;
        let Some(v) = self.plain_view() else {
            self.clear_pending = true;
            return;
        };
        store::set_clear_history_pending(false);
        engine::clear_data(v, None, false);
    }

    fn save_session(&mut self) {
        self.dirty = false;
        let (width, height) = logical_size(&self.ctx);
        let maximized = self.ctx.window.is_maximized();
        let keep: Vec<&Tab> = keepable(&self.tabs, self.active);
        let s = Session {
            tabs: keep
                .iter()
                .map(|t| SavedTab { url: t.url.clone(), title: t.title.clone(), favicon: t.favicon.clone(), pinned: t.pinned })
                .collect(),
            active: keep.iter().position(|t| t.id == self.active).unwrap_or(0),
            width: if maximized || self.fullscreen() { 0.0 } else { width },
            height: if maximized || self.fullscreen() { 0.0 } else { height },
            maximized,
        };
        store::save_session(&s);
    }
}

#[cfg(test)]
mod tests {
    use super::{Tab, keepable};

    /// A normal tab, a private one and an agent one; only the normal one may be saved.
    #[test]
    fn session_holds_normal_tabs_only() {
        let mut tabs = vec![Tab::new(1, "https://plain.example/".into())];
        tabs.push(Tab::new(2, String::new()));
        let mut private = Tab::new(3, "https://secret.example/login".into());
        private.private = true;
        tabs.push(private);
        let mut agent = Tab::new(4, "https://agent.example/".into());
        agent.agent = true;
        tabs.push(agent);

        let keep = keepable(&tabs, 1);
        // The private tab, the agent tab and the empty new tab page are all out.
        assert_eq!(keep.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1]);

        // An empty tab is kept while it is the one being looked at.
        let keep = keepable(&tabs, 2);
        assert_eq!(keep.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 2]);
    }

    /// A pinned private tab still leaves no trace.
    #[test]
    fn a_pinned_private_tab_is_not_saved_either() {
        let mut private = Tab::new(7, "https://secret.example/".into());
        private.private = true;
        private.pinned = true;
        assert!(keepable(&[private], 7).is_empty());
    }
}
