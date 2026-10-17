#![windows_subsystem = "windows"]

//! Lumen. A small browser shell.
//! One window. A slim UI page for the sidebar. Pages beside it.
//! Rust decides who lives, who sleeps, and when the UI hears about it.

mod address;
mod aria;
mod agent;
mod agentkit;
mod aloud;
mod blocker;
mod chrome;
mod engine;
mod glance;
mod journal;
mod kdbx;
mod library;
mod monitor;
mod pacing;
mod pagekit;
mod pins;
mod recall;
#[cfg(feature = "servo")]
mod servo_engine;
mod split;
mod translate;
mod upnp;
mod video;
mod workspaces;
mod store;
mod sponsorblock;
mod trackers;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, Event, StartCause, WindowEvent},
    keyboard::{KeyCode, ModifiersState},
    event_loop::{ControlFlow, DeviceEventFilter, EventLoopBuilder},
    platform::windows::{IconExtWindows, WindowBuilderExtWindows, WindowExtWindows},
    window::{Fullscreen, Icon, ResizeDirection, Theme as WinTheme, WindowBuilder},
};
use windows::Win32::Foundation::HWND;
use wry::{MemoryUsageLevel, Rect, WebView, WebViewExtWindows};

use engine::{Action, Ctx, Ev};
use store::{SavedTab, Session, Settings};

const TICK: Duration = Duration::from_secs(3);
/// A hidden tab keeps its caches this long, so flipping back is instant.
const TRIM_AFTER: Duration = Duration::from_secs(30);
/// Nothing waits: the loop sleeps this long instead of waking every TICK forever.
const IDLE_TICK: Duration = Duration::from_secs(30);
/// The sidebar memory readout, on its own slow clock. Reading it walks every
/// WebView2 process, so it does not run on the tick.
const MEM_TICK: Duration = Duration::from_secs(5);
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
    /// The page a pin was pinned on. Closing the tab unloads it back to this; the current
    /// `url` may have wandered. Empty when not pinned.
    pin_url: String,
    audio: bool,
    muted: bool,
    suspended: bool,
    trimmed: bool,
    /// Opened by an AI agent. Marked in the sidebar, never saved in the session.
    agent: bool,
    /// Its page died. Clicking the tab reloads it.
    crashed: bool,
    /// Showing reader view of this URL.
    reader: Option<String>,
    /// Showing the page in the target language. What it said before is in the page.
    translated: Option<String>,
    /// A translation is being worked on: the models, the engine or the words.
    translating: bool,
    /// Opened by a page (window.open, target=_blank).
    popup: bool,
    /// Which workspace this tab belongs to (index into `App::workspaces`).
    workspace: usize,
    /// InPrivate. Nothing here is written anywhere: no history, no page text,
    /// no session, no download row, no cookies once the last private page goes.
    private: bool,
    /// For agent tabs: the MCP session that owns it, and its client's name.
    owner: String,
    owner_name: String,
    view: Option<WebView>,
    /// A Servo page, for the hosts in the "Servo sites" list. The other engine.
    #[cfg(feature = "servo")]
    servo: Option<servo_engine::Page>,
    hidden_at: Option<Instant>,
    /// Its page window is on screen right now.
    visible: bool,
    /// Typed in the address bar as a bare host and not yet answered. See `Typed`.
    typed: Option<Typed>,
}

/// What the sleep policy needs to know about a hidden tab.
#[derive(Clone, Copy)]
struct SleepState {
    audio: bool,
    trimmed: bool,
    suspended: bool,
    pinned: bool,
}

/// One thing the sleep policy does to a hidden tab.
#[derive(Debug, PartialEq, Clone, Copy)]
enum SleepStep {
    /// Lower the memory target. The page keeps running.
    Trim,
    /// Freeze it.
    Suspend,
    /// Drop the page, keep the tab.
    Discard,
}

/// The step a hidden tab is owed `idle` after it was hidden, if any. A tab that
/// is playing audio is trimmed and nothing more: frozen or dropped, the music
/// would stop. Its clock restarts when the audio does (see `Ev::Audio`).
fn due_sleep_step(t: &SleepState, idle: Duration, sleep: Duration, discard: Duration) -> Option<SleepStep> {
    if t.audio {
        return (idle >= TRIM_AFTER && !t.trimmed).then_some(SleepStep::Trim);
    }
    if idle >= discard && !t.pinned {
        Some(SleepStep::Discard)
    } else if idle >= sleep && !t.suspended {
        Some(SleepStep::Suspend)
    } else if idle >= TRIM_AFTER && !t.trimmed {
        Some(SleepStep::Trim)
    } else {
        None
    }
}

/// How long after hiding the tab's next step comes, for the loop's next wake.
fn next_sleep_step_after(t: &SleepState, sleep: Duration, discard: Duration) -> Option<Duration> {
    if !t.trimmed {
        Some(TRIM_AFTER)
    } else if t.audio {
        None
    } else if !t.suspended {
        Some(sleep)
    } else if !t.pinned {
        Some(discard)
    } else {
        // Pinned and frozen: nothing left to do to it.
        None
    }
}

/// A bare host typed in the address bar (`youtube.musik`, not a link, not a
/// search). If the name does not exist, the person meant words: search for them.
struct Typed {
    /// What was typed, for the search.
    raw: String,
    /// The URL it was turned into.
    url: String,
    /// The page load for that URL has started. Anything else loading first
    /// (a link, a script) means the typed page was never the answer.
    started: bool,
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
            pin_url: String::new(),
            audio: false,
            muted: false,
            suspended: false,
            trimmed: false,
            agent: false,
            crashed: false,
            reader: None,
            translated: None,
            translating: false,
            popup: false,
            workspace: 0,
            private: false,
            owner: String::new(),
            owner_name: String::new(),
            view: None,
            #[cfg(feature = "servo")]
            servo: None,
            hidden_at: None,
            visible: false,
            typed: None,
        }
    }

    /// The session's record of this tab.
    fn saved(&self) -> SavedTab {
        SavedTab { url: self.url.clone(), title: self.title.clone(), favicon: self.favicon.clone(), pinned: self.pinned, pin_url: self.pin_url.clone() }
    }

    /// What the sleep policy looks at.
    fn sleep_state(&self) -> SleepState {
        SleepState { audio: self.audio, trimmed: self.trimmed, suspended: self.suspended, pinned: self.pinned }
    }

    /// The tab has a live page, of whichever engine. A Servo tab has no WebView2.
    fn has_page(&self) -> bool {
        if self.view.is_some() {
            return true;
        }
        #[cfg(feature = "servo")]
        if self.servo.is_some() {
            return true;
        }
        false
    }

    fn status(&self) -> &'static str {
        match (self.has_page(), self.url.is_empty(), self.crashed, self.suspended) {
            (false, true, _, _) => "new",
            (false, false, _, _) => "off",
            (true, _, true, _) => "crashed",
            (true, _, _, true) => "asleep",
            (true, _, _, false) => "live",
        }
    }
}

/// A reader-view article being read aloud: which tab, where each sentence
/// starts and ends in the page's text, and whether the voice is running.
struct Aloud {
    tab: u64,
    ranges: Vec<(usize, usize)>,
    playing: bool,
}

struct App {
    ctx: Rc<Ctx>,
    chrome: chrome::Chrome,
    blocker: Rc<blocker::Blocker>,
    tabs: Vec<Tab>,
    /// Named tab groups. `tab_ids` for the active workspace mirrors the live Tab IDs.
    workspaces: Vec<workspaces::WorkspaceData>,
    active_workspace: usize,
    /// Shift+clicked links open here: (tab id, url, view). Dropped on Escape.
    glance: Option<(u64, String, WebView)>,
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
    /// Two pages side by side: the tab in the other half of the stage.
    split: Option<u64>,
    /// The active tab sits in the right half of the split.
    pane_right: bool,
    /// Where the divider sits, as a fraction of the stage width.
    split_at: f64,
    /// The strip between the two panes.
    divider: split::Divider,
    /// Colors the divider was last given. Only re-sent when they change.
    div_colors: ((u8, u8, u8), (u8, u8, u8)),
    /// Last page sizes that got rounded corners. Skip the work when nothing moved.
    rounded: HashMap<u64, (u32, u32)>,
    was_max: bool,
    shown: bool,
    dirty: bool,
    mem: Option<(u64, u32)>,
    born: Instant,
    /// When the tick next wants to run. TICK apart, so a step that is not
    /// ready cannot wake the loop again on the same deadline.
    next_tick: Instant,
    /// When the sidebar memory readout next wants a fresh number.
    next_mem: Instant,
    /// When the journal's next `sample` is due. Not a deadline of its own: it is
    /// checked on the memory read the loop already makes, so an idle Lumen never
    /// wakes for the journal and the journal never adds a timer.
    journal_at: Instant,
    /// The window is minimized. Nobody is looking: the journal stands still.
    minimized: bool,
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
    /// Session-restored jobs have been put in the list once.
    aria_adopted: bool,
    /// The aria2 poll thread. Asleep until a download gives it work.
    aria_poll: std::sync::Arc<aria::Poll>,
    /// UPnP port mapping for the torrent port.
    upnp: upnp::Upnp,
    /// Page text for searching history by content.
    recall: recall::Recall,
    /// Console, network and dialog records of agent tabs.
    agent_logs: agentkit::Logs,
    /// An agent waiting for the person: (tab, agent name, message, where to say Done).
    handoff: Option<(u64, String, String, std::sync::mpsc::Sender<Result<Value, String>>)>,
    /// What aria2 was given per gid, so a failed download can be tried again.
    aria_meta: HashMap<String, (String, Vec<String>, Option<String>)>,
    /// Gids the person started: a paused one with its metadata in gets
    /// un-paused once (pause-metadata stops it after the metadata arrives).
    /// A Pause by the person removes the gid: their Pause is never undone.
    aria_person: HashSet<String>,
    /// Page-started adds (Handoff, page magnets): the torrent they spawn
    /// waits for Resume. Plain files never spawn one, so the mark is inert.
    aria_page: HashSet<String>,
    /// Why a page-started row waits: gid -> the note its paused row shows.
    aria_hold: HashMap<String, String>,
    /// The translator page is open. Only alive while a translation is in the air.
    translator: Option<translate::Page>,
    /// Servo, when the feature is compiled in and a Servo tab has been opened.
    #[cfg(feature = "servo")]
    servo: Option<servo_engine::Engine>,
    /// The tab being translated right now.
    tr_busy: Option<u64>,
    /// The pair chosen for it, and its text, waiting for the models and the engine.
    tr_pair: Option<&'static translate::P>,
    tr_texts: Option<String>,
    /// Next download id for aria2, video and remembered rows. Never reused.
    dl_seq: u64,
    /// Finished downloads changed; write downloads.json on the next tick.
    dl_dirty: bool,
    /// A download finished or started seeding while the Downloads page was closed.
    /// Drives the dot on the footer button until the page is opened.
    dl_unseen: bool,
    /// Debug test hook: LUMEN_TEST_MAGNET adds one magnet on the first aria2 tick.
    test_magnet: bool,
    /// The test magnet is on its way to aria2; its gid arrives in AriaAdded.
    test_pending: bool,
    /// Debug test hook: (gid, when to cancel it). LUMEN_TEST_CANCEL seconds, default 60.
    test_dl: Option<(String, Instant)>,
    /// Debug test hook: LUMEN_TEST_TRANSLATE translates the first page that loads.
    test_translate: bool,
    /// Video grabs: download id -> (page url, user agent, running yt-dlp).
    videos: HashMap<u64, (String, String, video::Slot)>,
    /// "Clear history on exit" owed a WebView2 clear it could not make in time.
    /// Done at the next start, as soon as a page exists to ask.
    clear_pending: bool,
    /// Read aloud: the reader-view tab being spoken, if any.
    aloud: Option<Aloud>,
    /// SponsorBlock: a fetched skip script waiting for its tab to finish
    /// loading, so it lands in the new document and not the one going away.
    spb: HashMap<u64, String>,
    /// The open KeePass database: memory only, dropped after ten idle minutes.
    vault: Option<kdbx::Vault>,
    /// When the vault was last used; the idle lock counts from here.
    vault_seen: Instant,
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
    install_panic_hook();
    // Before WebView2 starts: its processes must be born inside the job.
    let job = monitor::Job::adopt_self();
    let mut settings = store::load_settings();
    let session = store::load_session();
    // The journal opens before anything else runs, so a run that never gets a
    // window up still says hello. Also the one place old journals are cleaned.
    journal::start(&settings, restored_tabs(&session));

    let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    // Lumen never reads a raw device event. By default tao registers for raw mouse
    // and keyboard input while the window has focus, and wakes the loop once per
    // mouse sample (125 to 1000 a second) for events nobody looks at. This is what
    // the journal's `wake` storms were: wakes follow the pointer, not the page.
    event_loop.set_device_event_filter(DeviceEventFilter::Always);
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
    // The split divider: a strip of its own between the two panes.
    let divider = split::Divider::new(HWND(ctx.window.hwnd() as _), ctx.proxy.clone());

let bg = if dark { (24, 24, 26) } else { (243, 243, 244) };
    // Read before the struct literal moves `settings` into the app.
    let keep_seeding = settings.keep_seeding;
    let dl_limit = settings.download_limit_kbps;
    // Ensure the torrent port is chosen (picks once, saves, returns it).
    let bt_port = settings.ensure_bt_port();
    // Start UPnP mapping for the torrent port if enabled.
    let upnp = upnp::Upnp::start(bt_port, settings.upnp);
    let mut app = App {
        ctx,
        chrome,
        blocker,
        tabs: Vec::new(),
        workspaces: Vec::new(),
        active_workspace: 0,
        glance: None,
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
        split: None,
        pane_right: false,
        split_at: 0.5,
        divider,
        div_colors: ((0, 0, 0), (0, 0, 0)),
        rounded: HashMap::new(),
        was_max: session.maximized,
        shown: false,
        dirty: false,
        mem: None,
        born: Instant::now(),
        next_tick: Instant::now() + TICK,
        next_mem: Instant::now() + MEM_TICK,
        journal_at: Instant::now() + journal::period(),
        minimized: false,
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
        aria: aria::Aria::start(&store::data_dir(), &store::download_dir(), keep_seeding, dl_limit, bt_port),
        aria_ids: HashMap::new(),
        aria_adopted: false,
        aria_poll: aria::Poll::new(),
        upnp,
        recall: recall::Recall::start(&store::data_dir()),
        agent_logs: Default::default(),
        handoff: None,
        aria_meta: HashMap::new(),
        aria_person: HashSet::new(),
        aria_page: HashSet::new(),
        aria_hold: HashMap::new(),
        translator: None,
        #[cfg(feature = "servo")]
        servo: None,
        tr_busy: None,
        tr_pair: None,
        tr_texts: None,
        videos: HashMap::new(),
        dl_seq: 0,
        dl_dirty: false,
        dl_unseen: false,
        test_magnet: false,
        test_pending: false,
        test_dl: None,
        test_translate: false,
        clear_pending: false,
        aloud: None,
        spb: HashMap::new(),
        vault: None,
        vault_seen: Instant::now(),
    };
    {
        // One small thread asks aria2 about its jobs. It sleeps on a condvar
        // until a download gives it something to ask about.
        let (aria, poll, proxy) = (app.aria.clone(), app.aria_poll.clone(), app.ctx.proxy.clone());
        std::thread::Builder::new()
            .name("lumen-aria-poll".into())
            .spawn(move || loop {
                poll.wait();
                std::thread::sleep(Duration::from_millis(700));
                if proxy.send_event(Ev::AriaJobs(aria.jobs())).is_err() {
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
    // Test hooks for the chrome screenshots (LUMEN_TEST_SHOT, see chrome/shot.rs):
    // LUMEN_TEST_COMPACT=1 starts with the icon-only sidebar, LUMEN_TEST_PANEL=
    // downloads|control|history|bookmarks|settings opens that page.
    if cfg!(debug_assertions) {
        if std::env::var_os("LUMEN_TEST_COMPACT").is_some() {
            app.settings.compact = true;
        }
        let panel = match std::env::var("LUMEN_TEST_PANEL").as_deref() {
            Ok("downloads") => Panel::Downloads,
            Ok("control") => Panel::Control,
            Ok("history") => Panel::History,
            Ok("bookmarks") => Panel::Bookmarks,
            Ok("settings") => Panel::Settings,
            _ => Panel::None,
        };
        if panel != Panel::None || app.settings.compact {
            app.set_panel(panel);
        }
        // LUMEN_TEST_SCRIPT: timed key/click/shot steps, see chrome/shot.rs.
        #[cfg(debug_assertions)]
        chrome::test_script(app.ctx.proxy.clone(), app.chrome.hwnd);
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
    // Test hook: LUMEN_TEST_SPLIT=1 opens two pages and splits the stage with them,
    // so a headless run can prove the panes were laid out and the divider placed.
    // LUMEN_TEST_TRANSLATE=1 translates the first page that finishes loading, and
    // prints what the page said afterwards.
    if cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_SPLIT").is_some() {
        app.new_tab(Some("https://example.com/".into()), None);
        app.new_tab(Some("https://example.org/".into()), None);
        app.on_key(Action::Split);
        let (act, other, div) = app.panes();
        eprintln!("lumen: split test: active {act:?} other {other:?} divider {div:?}");
    }
    if cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_TRANSLATE").is_some() {
        app.test_translate = true;
        app.new_tab(Some("https://es.wikipedia.org/wiki/Wikipedia:Portada".into()), None);
    }
    // Test hook: LUMEN_TEST_MAGNET=1 adds one public-domain torrent magnet on the
    // first aria2 tick, then cancels it after LUMEN_TEST_CANCEL seconds (default 60).
    if cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_MAGNET").is_some() {
        app.test_magnet = true;
    }

    event_loop.run(move |event, _, cf| {
        // The journal's wake count: one atomic add per loop turn, and only while
        // there is something to watch. An idle Lumen counts nothing.
        if matches!(&event, Event::NewEvents(_)) && app.journal_busy() {
            journal::wake();
        }
        // Servo works off its own clock: give it a turn whenever the loop wakes.
        #[cfg(feature = "servo")]
        if let Some(e) = &app.servo {
            e.spin();
        }
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                let now = Instant::now();
                if app.next_sample.is_some_and(|t| t <= now) {
                    app.sample();
                }
                if app.next_mem <= now && app.watches_memory() {
                    app.read_memory(now);
                }
                if app.tick_due(now) {
                    app.tick();
                }
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::Resized(size) => {
                    if size.width == 0 || size.height == 0 {
                        // Minimized. Nobody is looking. Trim the active page too.
                        app.minimized = true;
                        if let Some(v) = app.active_tab().and_then(|t| t.view.as_ref()) {
                            let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
                        }
                    } else {
                        app.minimized = false;
                        app.layout();
                        let max = app.ctx.window.is_maximized();
                        if max != app.was_max {
                            app.was_max = max;
                            app.push();
                        }
                    }
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    app.rounded.clear();
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
                    if let Some(vk) = vk_of(event.physical_key) {
                        // A bare-window glance takes Escape and Enter; the card
                        // view normally has focus and catches them itself.
                        if app.glance.is_some() {
                            match vk {
                                0x1B if !m.shift_key() => app.close_glance(),
                                0x0D => app.promote_glance(),
                                _ => {}
                            }
                        }
                        if let Some(a) = engine::map_key(vk, m.control_key(), m.shift_key(), m.alt_key())
                            && (!event.repeat || matches!(a, Action::NextTab | Action::PrevTab))
                        {
                            app.on_key(a);
                        }
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
            app.upnp.release();
            // The journal's goodbye: a run with this line is a run that ended.
            journal::stop(true, app.born.elapsed());
            *cf = ControlFlow::Exit;
        } else {
            let now = Instant::now();
            // A deadline that is already past must not turn into a spin.
            *cf = ControlFlow::WaitUntil(pacing::clamp_wake(now, app.next_wake(now)));
        }
    });
}

// ------------------------------------------------------------------- sleeping

/// The session as it was last written. A panic hook writes it back out before the
/// process goes, so a crash loses nothing the user had not already lost.
static LAST_SESSION: std::sync::Mutex<Option<Session>> = std::sync::Mutex::new(None);

/// Where a panic goes, and what it saves. Without this, a Rust panic in release is
/// an abort with no trace: the user sees Lumen vanish and learns nothing.
fn install_panic_hook() {
    let plain = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let what = info.payload().downcast_ref::<&str>().map(|s| (*s).to_string()).unwrap_or_else(|| "panic".to_string());
        let why = info.payload().downcast_ref::<String>().cloned().unwrap_or(what);
        let at = info.location().map_or_else(String::new, |l| format!("{}:{}", l.file().rsplit(['\\', '/']).next().unwrap_or(l.file()), l.line()));
        let _ = write_crash(&format!("{why}  at {at}"));
        // The journal is the one place the next session reads without a console.
        journal::panic(&format!("{why}  at {at}"));
        // The session on disk goes back to what we last had. It is the user's tabs.
        if let Ok(g) = LAST_SESSION.lock()
            && let Some(s) = g.as_ref()
        {
            store::save_session(s);
        }
        plain(info);
    }));
}

/// One line in `data_dir()/crash.log`. The file is kept small: it is a log, not a history.
fn write_crash(line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let (at, clock) = local_day_time(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    );
    let path = store::data_dir().join("crash.log");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(f, "{at} {clock}  {line}")?;
    // Past a page of it, keep only the tail: the newest crash is the one that matters.
    const MAX: u64 = 32 << 10;
    if f.metadata().map(|m| m.len()).unwrap_or(0) > 2 * MAX {
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let keep: Vec<&str> = old.lines().collect();
        let drop = keep.len().saturating_sub(MAX as usize / 64);
        let mut tail = keep[drop..].join("\n");
        tail.push('\n');
        let tmp = path.with_extension("part");
        if std::fs::write(&tmp, tail).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    Ok(())
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
    classify_input(raw, search).map(|(url, _)| url)
}

/// Like `resolve_input`, and says whether the text was a bare host (`github.com`,
/// `localhost:3000`): the one kind of typing that may turn out to be a typo, and
/// so the one that gets a search if the name does not exist.
fn classify_input(raw: &str, search: &str) -> Option<(String, bool)> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(u) = bang(s) {
        return Some((u, false));
    }
    let lower = s.to_ascii_lowercase();
    const SCHEMES: [&str; 8] = ["http://", "https://", "file:", "about:", "data:", "view-source:", "edge://", "magnet:"];
    if SCHEMES.iter().any(|p| lower.starts_with(p)) {
        return Some((s.to_string(), false));
    }
    if !s.contains(char::is_whitespace) && address::looks_like_host(s) {
        let host = lower.split(['/', '?', '#']).next().unwrap_or("");
        let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
        let plain = host.rsplit_once(':').map_or(host, |(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host });
        // Local and numeric hosts have no certificate to speak of: plain http.
        let local = plain == "localhost" || plain.parse::<std::net::Ipv4Addr>().is_ok() || plain.starts_with('[');
        return Some((format!("{}://{s}", if local { "http" } else { "https" }), true));
    }
    Some((search.replace("%s", &url_encode(s)), false))
}

/// The same page, give or take case and a trailing slash.
fn same_page(a: &str, b: &str) -> bool {
    a.trim_end_matches('/').eq_ignore_ascii_case(b.trim_end_matches('/'))
}

/// What a typed host's answer means.
#[derive(Debug, PartialEq)]
enum TypedVerdict {
    /// Not the answer to this typing (or not yet): keep waiting.
    Wait,
    /// It loaded, or failed some way a search would not mend: let go.
    Forget,
    /// The name does not exist: search for the words.
    Search,
}

/// WebView2's web error status for a name that did not resolve.
const ERR_NAME_NOT_RESOLVED: i32 = 13;
/// ... and for a load another load cut short.
const ERR_CANCELED: i32 = 14;

fn typed_verdict(started: bool, ok: bool, err: i32) -> TypedVerdict {
    if !started || (!ok && err == ERR_CANCELED) {
        return TypedVerdict::Wait;
    }
    if !ok && err == ERR_NAME_NOT_RESOLVED { TypedVerdict::Search } else { TypedVerdict::Forget }
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

/// The `dn=` display name out of a magnet link, percent-decoded. Empty when absent.
fn magnet_name(url: &str) -> String {
    let Some(q) = url.split_once("?").map(|(_, q)| q) else { return String::new() };
    for kv in q.split('&') {
        if let Some(v) = kv.strip_prefix("dn=") {
            return percent_decode(v);
        }
    }
    String::new()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let h = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match h.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Why a page-started magnet waits, in words: which page asked for it.
fn magnet_hold(page_url: &str) -> String {
    let h = host_of(page_url);
    if h.is_empty() {
        "from a page: press Resume to start".to_string()
    } else {
        format!("from {h}: press Resume to start")
    }
}

/// A person-started torrent pause-metadata stopped once its metadata is in:
/// start it. `partial` keeps the rule off finished torrents, so an aria2
/// seed-limit pause is not undone. A page-started one (not in the person
/// set) waits for Resume; a Pause by the person removes the gid, so their
/// Pause is never undone either.
fn should_auto_resume(person: bool, status: &str, named: bool, partial: bool) -> bool {
    person && status == "paused" && named && partial
}

/// Events that fire in a stream (download progress, aria2 polls) would drown the
/// debug log in repeats. Everything else is worth one line.
fn quiet_event(ev: &Ev) -> bool {
    // Vault events carry a master password or the secrets behind it: never
    // let the debug log see them.
    matches!(ev, Ev::Download(_) | Ev::AriaJobs(_) | Ev::Usage(_) | Ev::VaultOpen(..) | Ev::VaultReady(_))
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

/// How many tabs the session is bringing back, for the journal's `start` line.
fn restored_tabs(session: &Session) -> usize {
    if session.workspaces.is_empty() {
        session.tabs.len()
    } else {
        session.workspaces.iter().map(|w| w.tabs.len()).sum()
    }
}

/// Whether this tab runs in Servo. Always false in a build without the feature.
fn is_servo(t: &Tab) -> bool {
    #[cfg(feature = "servo")]
    {
        t.servo.is_some()
    }
    #[cfg(not(feature = "servo"))]
    {
        let _ = t;
        false
    }
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
        // F8: translate the page, or put the original text back.
        F8 => Some(0x77),
        Comma => Some(0xBC),
        // Ctrl+\ puts two pages side by side.
        Backslash => Some(0xDC),
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

    /// Fullscreen as the layout sees it: the page fills the window. A panel (Downloads,
    /// Control...) is drawn by the chrome, which fullscreen hides, so with one open the
    /// window shows the chrome and its inset stage instead of a blank screen.
    fn fullscreen_view(&self) -> bool {
        self.fullscreen() && self.panel == Panel::None
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
        if self.fullscreen_view() {
            return (0.0, 0.0, w, h);
        }
        let side = self.sidebar_w();
        let g = self.settings.gap as f64;
        let x = if self.settings.sidebar_side == "right" { g } else { side };
        (x, g, (w - side - g).max(1.0), (h - 2.0 * g).max(1.0))
    }

    /// The stage cut in two: the active tab's rectangle, the split tab's, and the
    /// divider strip between them. With no split, both pages get the whole stage
    /// and there is no strip.
    fn panes(&self) -> ((f64, f64, f64, f64), (f64, f64, f64, f64), Option<(f64, f64, f64, f64)>) {
        let stage = self.stage();
        if self.split.is_none() {
            return (stage, stage, None);
        }
        let (l, d, r) = split::panes(stage, self.split_at);
        // The focused half is the one the person last worked in.
        if self.pane_right { (r, l, Some(d)) } else { (l, r, Some(d)) }
    }

    /// A tab that is on screen right now: the active one, or the other half of a split.
    /// Hidden-tab policies (freeze, discard, RAM) leave these alone.
    fn on_stage(&self, id: u64) -> bool {
        id == self.active || self.split == Some(id)
    }

    /// Which page windows are on screen: the two panes, and nothing else. A panel
    /// covers the stage, so nothing is.
    fn sync_panes(&mut self) {
        let on = self.panel == Panel::None && !self.ui_full();
        let split = self.split;
        let active = self.active;
        for i in 0..self.tabs.len() {
            let t = &mut self.tabs[i];
            let want = on && (t.id == active || split == Some(t.id)) && t.has_page();
            if want == t.visible {
                continue;
            }
            #[cfg(feature = "servo")]
            if let Some(p) = &t.servo {
                if want {
                    p.show();
                } else {
                    p.hide();
                }
                t.visible = want;
                continue;
            }
            if let Some(v) = &t.view {
                let _ = v.set_visible(want);
                t.visible = want;
            } else {
                t.visible = false;
            }
        }
    }

    /// The strip between the panes takes the gap color and a grip a step off it.
    fn divider_theme(&mut self) {
        let dark = self.ctx.dark.get();
        let (r, g, b) = self.bg;
        let step = |v: u8| if dark { v.saturating_add(36) } else { v.saturating_sub(26) };
        let c = ((r, g, b), (step(r), step(g), step(b)));
        if self.div_colors == c {
            return;
        }
        self.div_colors = c;
        self.divider.set_colors(c.0, c.1);
    }

    fn layout(&mut self) {
        let (w, h) = logical_size(&self.ctx);
        let fs = self.fullscreen_view();
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
        self.sync_panes();
        let (act, other, div) = self.panes();
        let scale = self.ctx.window.scale_factor();
        let radius = if fs { 0 } else { (self.settings.radius as f64 * scale).round() as i32 };
        // The divider only shows when the pages do.
        match div.filter(|_| !self.ui_full()) {
            Some((x, y, dw, dh)) => {
                self.divider_theme();
                self.divider.set_bounds(x, y, dw, dh);
                self.divider.set_visible(true);
            }
            None => self.divider.set_visible(false),
        }
        let mut place: Vec<(u64, (f64, f64, f64, f64))> = vec![(self.active, act)];
        if let Some(id) = self.split {
            place.push((id, other));
        }
        let mut clips: Vec<(u64, u32, u32)> = Vec::new();
        for (id, (x, y, pw, ph)) in place {
            if pw < 1.0 || ph < 1.0 {
                continue;
            }
            let Some(i) = self.idx(id) else { continue };
            #[cfg(feature = "servo")]
            if let Some(p) = &self.tabs[i].servo {
                p.set_bounds(x, y, pw, ph);
                continue;
            }
            let Some(v) = &self.tabs[i].view else { continue };
            let _ = v.set_bounds(logical_rect(x, y, pw, ph));
            clips.push((id, (pw * scale) as u32, (ph * scale) as u32));
        }
        // The corner clip is a region rebuild and a repaint. Only when size changed.
        for (id, cw, ch) in clips {
            let key = (cw, ch + radius as u32);
            if self.rounded.get(&id) == Some(&key) {
                continue;
            }
            self.rounded.insert(id, key);
            let v = self.idx(id).and_then(|i| self.tabs[i].view.as_ref());
            if let Some(v) = v {
                engine::round_corners(v, radius);
            }
        }
        // The glance card floats at 80% of the stage, centred, over a dimmed page.
        // With a split it covers both halves: the dim is one stage-sized window.
        if let Some((_, _, v)) = &self.glance {
            let (x, y, sw, sh) = self.stage();
            let gw = sw * 0.8;
            let gh = sh * 0.8;
            let _ = v.set_bounds(logical_rect(x + (sw - gw) / 2.0, y + (sh - gh) / 2.0, gw, gh));
            engine::round_corners(v, radius);
            // Dim over the page, card over the dim: both are sibling windows.
            self.chrome.set_glance_dim(x, y, sw, sh);
            engine::raise(v);
        }
    }

    fn focus_page(&self) {
        // Headless never takes keyboard focus from whatever the person is doing.
        if headless() {
            return;
        }
        if self.panel != Panel::None {
            self.chrome.focus();
            return;
        }
        // A Servo page takes the keyboard in its own child window.
        #[cfg(feature = "servo")]
        if let Some(p) = self.active_tab().and_then(|t| t.servo.as_ref()) {
            p.focus();
            return;
        }
        match self.active_tab().and_then(|t| t.view.as_ref()) {
            Some(v) => {
                let _ = v.focus();
            }
            None => self.chrome.focus(),
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
                        pin_away: pins::offers_back(t.pinned, &t.pin_url, &t.url),
                        audio: t.audio,
                        muted: t.muted,
                        status: t.status(),
                        agent: t.agent,
                        reader: t.reader.is_some(),
                        private: t.private,
                        split: self.split.is_some() && self.on_stage(t.id),
                        translated: t.translated.is_some(),
                        translating: t.translating,
                        servo: is_servo(t),
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
                        name: if !d.name.is_empty() {
                            d.name.clone()
                        } else {
                            d.path.rsplit(['\\', '/']).next().unwrap_or(&d.path).to_string()
                        },
                        url: d.url.clone(),
                        received: d.received,
                        total: d.total,
                        state: d.state,
                        speed: d.speed,
                        note: d.note.clone(),
                        kind: d.kind,
                        up: d.up,
                        peers: d.peers,
                        seeds: d.seeds,
                        ratio: d.ratio,
                    })
                    .collect(),
                dl_unseen: self.dl_unseen,
                upnp_port_open: self.upnp.is_mapped(),
                agent_busy: self.agent_seen.is_some(),
                bookmarked: self.active_tab().is_some_and(|t| !t.url.is_empty() && self.library.is_bookmarked(&t.url)),
                listen: match self.tabs.iter().find(|t| t.id == self.active) {
                    Some(t) if t.reader.is_some() && !t.agent => match &self.aloud {
                        Some(a) if a.tab == t.id && a.playing => "playing",
                        Some(a) if a.tab == t.id => "paused",
                        _ => "ready",
                    },
                    _ => "",
                },
                // Labels only; passwords never reach the chrome.
                kdbx: match (self.vault.as_ref(), self.active_tab()) {
                    (Some(v), Some(t)) if t.url.starts_with("http") => v.matches(&t.url).into_iter().take(8).map(|n| v.label(n)).collect(),
                    _ => Vec::new(),
                },
                vault: self.vault.as_ref().map_or(0, |v| v.len()),
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
                workspaces: self.workspaces.iter().map(|w| w.name.clone()).collect(),
                active_workspace: self.active_workspace,
                glance: self.glance.is_some(),
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
        let (workspaces, active_ws) = workspaces::build_from_session(&session);
        self.workspaces = workspaces;
        self.active_workspace = active_ws.min(self.workspaces.len().saturating_sub(1));
        // Load the active workspace's saved tabs into memory.
        let ws = self.workspaces[self.active_workspace].clone();
        let saved = ws.saved;
        let ws_active = ws.active;
        let mut active = None;
        for (i, saved) in saved.into_iter().enumerate() {
            let mut t = Tab::new(self.ctx.alloc_id(), saved.url);
            t.title = saved.title;
            t.favicon = saved.favicon;
            t.pinned = saved.pinned;
            t.pin_url = pins::restored_pin_url(saved.pinned, &saved.pin_url, &t.url);
            t.workspace = self.active_workspace;
            if i == ws_active {
                active = Some(t.id);
            }
            self.tabs.push(t);
        }
        // Pinned tabs live at the front. The UI groups them; the order must agree.
        self.tabs.sort_by_key(|t| !t.pinned);
        if self.tabs.is_empty() {
            let mut t = Tab::new(self.ctx.alloc_id(), String::new());
            t.workspace = self.active_workspace;
            self.tabs.push(t);
        }
        // Sync the active workspace's tab_ids with the live IDs.
        let ids: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        if let Some(ws) = self.workspaces.get_mut(self.active_workspace) {
            ws.tab_ids = ids;
            ws.saved.clear();
            ws.active = self.tabs.iter().position(|t| Some(t.id) == active).unwrap_or(0);
        }
        // Only the active tab gets a page. The rest stay "off" until clicked.
        let id = active.filter(|id| self.idx(*id).is_some()).unwrap_or(self.tabs[0].id);
        self.activate(id);
    }

    fn ensure_view(&mut self, i: usize) {
        #[cfg(feature = "servo")]
        {
            // A host from the "Servo sites" list opens in Servo, unless the tab
            // already has a WebView2 page (a reopened one) or the person put it back.
            let url = self.tabs[i].url.clone();
            if self.tabs[i].servo.is_none() && self.tabs[i].view.is_none() && self.wants_servo(&url) {
                self.open_servo(i, &url);
                if self.tabs[i].servo.is_some() {
                    return;
                }
                // Servo would not have it: carry on with WebView2.
            }
        }
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
        // The card covered the page being left; a new page under it means it is done.
        self.drop_glance();
        // A voice follows the tab it was reading; leaving that tab stops it.
        if self.aloud.as_ref().is_some_and(|a| a.tab != id) {
            self.listen_stop();
        }
        // The other half of a split: the pages stay where they are, the focus (and
        // the sidebar highlight) moves to this one.
        if self.split == Some(id) {
            self.split = Some(self.active);
            self.pane_right = !self.pane_right;
        }
        let prev = (id != self.active).then(|| self.idx(self.active)).flatten();
        self.active = id;
        self.leave_panel();
        self.ensure_view(i);
        // Show the new page first, then hide the old. No flash of empty stage between.
        self.wake(i);
        self.layout();
        // A dead page comes back on click: load it again. (A dead browser
        // process left no view; ensure_view already built a fresh one.)
        if self.tabs[i].crashed {
            self.tabs[i].crashed = false;
            if let Some(v) = &self.tabs[i].view {
                let _ = v.load_url(&self.tabs[i].url);
            }
        }
        if let Some(p) = prev {
            let p = &mut self.tabs[p];
            // It stops being the page being read unless it is the split's other half.
            if self.split != Some(p.id) {
                p.hidden_at = Some(Instant::now());
            }
        }
        self.focus_page();
        self.dirty = true;
        self.push();
    }

    /// A page that is on screen must be neither frozen nor trimmed, however long
    /// it has been since it was last the active tab.
    fn wake(&mut self, i: usize) {
        let t = &mut self.tabs[i];
        t.hidden_at = None;
        t.suspended = false;
        if let Some(v) = &t.view
            && t.trimmed
        {
            let _ = v.set_memory_usage_level(MemoryUsageLevel::Normal);
            t.trimmed = false;
        }
    }

    /// Two pages in one stage, `left` focused. Replaces any split there was.
    fn set_split(&mut self, left: u64, right: u64) {
        if left == right || self.idx(left).is_none() || self.idx(right).is_none() {
            return;
        }
        self.split = Some(right);
        self.pane_right = false;
        self.activate(left);
        // The page on the other side is on screen from now on: build it if it never
        // had one, and take it out of sleep if it had.
        if let Some(i) = self.idx(right) {
            self.ensure_view(i);
            self.limit_tab(i);
            self.wake(i);
        }
        self.layout();
        self.push();
    }

    /// Back to one page: the other half goes back to being a hidden tab.
    fn unsplit(&mut self) {
        let Some(id) = self.split.take() else { return };
        self.pane_right = false;
        if let Some(i) = self.idx(id) {
            self.tabs[i].hidden_at = Some(Instant::now());
        }
        self.layout();
        self.push();
    }

    /// Ctrl+\: the active tab and the next one share the stage, or one page again.
    /// A window with one tab has no next tab to share with.
    fn toggle_split(&mut self) {
        if self.split.is_some() {
            self.unsplit();
            return;
        }
        let n = self.tabs.len();
        if n < 2 {
            return;
        }
        let other = self.tabs[(self.active_idx() + 1) % n].id;
        let left = self.active;
        self.set_split(left, other);
        journal::feature("split", false);
    }

    /// The tab menu's "Split with next tab", on any tab: it shares the stage with
    /// the tab after it, or the split it is already half of goes away.
    fn split_next(&mut self, id: u64) {
        if self.split.is_some() && (self.split == Some(id) || id == self.active) {
            self.unsplit();
            return;
        }
        let n = self.tabs.len();
        let Some(i) = self.idx(id) else { return };
        if n < 2 {
            return;
        }
        let other = self.tabs[(i + 1) % n].id;
        self.set_split(id, other);
        journal::feature("split", false);
    }

    fn new_tab(&mut self, url: Option<String>, after: Option<usize>) -> u64 {
        let id = self.ctx.alloc_id();
        let mut t = Tab::new(id, url.unwrap_or_default());
        t.workspace = self.active_workspace;
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
        t.workspace = self.active_workspace;
        self.tabs.push(t);
        self.activate(id);
        // The opening itself is not private: one line, no content.
        journal::feature("private_tab", false);
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

    /// Closing a pinned tab unloads it instead: the page and its memory go, the pin stays
    /// in the row and loads its pinned URL when clicked. False: not a pin to keep.
    fn unload_pin(&mut self, id: u64) -> bool {
        let Some(i) = self.idx(id) else { return false };
        let t = &self.tabs[i];
        if pins::closing(t.pinned, self.off_record(t), &t.pin_url, &t.url) != pins::Closing::Unload {
            return false;
        }
        if self.aloud.as_ref().is_some_and(|a| a.tab == id) {
            self.listen_stop();
        }
        if id == self.active || self.split == Some(id) {
            self.unsplit();
        }
        if id == self.active {
            let slots: Vec<pins::Slot> = self.tabs.iter().map(|t| pins::Slot { pinned: t.pinned, loaded: t.has_page() }).collect();
            match pins::next_after_close(&slots, i) {
                Some(n) => {
                    let next = self.tabs[n].id;
                    self.activate(next);
                }
                None => {
                    self.new_tab(None, None);
                }
            }
        }
        let Some(i) = self.idx(id) else { return true };
        self.discard(i);
        // A pin comes back where it was pinned, with a clean slate: no history to walk, no reader.
        let t = &mut self.tabs[i];
        t.url = pins::target(&t.pin_url, &t.url);
        t.pin_url = t.url.clone();
        t.back = false;
        t.fwd = false;
        t.crashed = false;
        t.typed = None;
        t.translated = None;
        t.translating = false;
        self.dirty = true;
        self.push();
        true
    }

    fn close(&mut self, id: u64) {
        if self.unload_pin(id) {
            return;
        }
        let Some(i) = self.idx(id) else { return };
        if self.aloud.as_ref().is_some_and(|a| a.tab == id) {
            self.listen_stop();
        }
        self.spb.remove(&id);
        // Closing either half of a split leaves one page again. When the focused half
        // goes, the other one takes the whole stage instead of being left behind.
        let keep = if id == self.active { self.split.take() } else { None };
        if self.split == Some(id) {
            self.split = None;
        }
        if self.split.is_none() {
            self.pane_right = false;
        }
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
            self.closed.push(SavedTab { url: t.url, title: t.title, favicon: t.favicon, pinned: t.pinned, pin_url: t.pin_url });
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
            let next = keep.filter(|k| self.idx(*k).is_some()).unwrap_or_else(|| self.tabs[i.min(self.tabs.len() - 1)].id);
            self.active = 0;
            self.activate(next);
        } else {
            self.layout();
            self.dirty = true;
            self.push();
        }
    }

    /// Drop the page, keep the tab. Frees the renderer process.
    fn discard(&mut self, i: usize) {
        let t = &mut self.tabs[i];
        if t.id == self.active || !t.has_page() {
            return;
        }
        if let Some(w) = self.watches.remove(&t.id) {
            monitor::unwatch(None, w, true);
        }
        t.view = None;
        // A Servo page: closing the child window gives the tab's memory back.
        #[cfg(feature = "servo")]
        {
            t.servo = None;
        }
        t.reader = None;
        t.suspended = false;
        t.trimmed = false;
        t.loading = false;
        t.audio = false;
        t.visible = false;
    }

    // ---------------------------------------------------------- workspaces

    /// Serialize the live tabs into the active workspace's slot, so the
    /// workspace can come back later without keeping its views alive.
    fn stash_workspace(&mut self) {
        let keep: Vec<&Tab> = keepable(&self.tabs, self.active);
        let saved: Vec<SavedTab> = keep
            .iter()
            .map(|t| t.saved())
            .collect();
        let active_idx = keep.iter().position(|t| t.id == self.active).unwrap_or(0);
        let ids: Vec<u64> = keep.iter().map(|t| t.id).collect();
        if let Some(ws) = self.workspaces.get_mut(self.active_workspace) {
            ws.tab_ids = ids;
            ws.saved = saved;
            ws.active = active_idx;
        }
    }

    /// Drop every live tab and its view. Hidden workspaces cost no memory.
    fn drop_all_tabs(&mut self) {
        // Both halves of a split are going: the split goes with them, or the
        // stale id would leave the stage looking split with nothing beside it.
        self.split = None;
        self.pane_right = false;
        while let Some(t) = self.tabs.pop() {
            self.agent_logs.borrow_mut().remove(&t.id);
            if let Some(w) = self.watches.remove(&t.id) {
                monitor::unwatch(None, w, true);
            }
            self.prompts.retain(|p| p.1 != t.id);
            // A person waiting on a handoff from a discarded tab gets their answer back.
            if self.handoff.as_ref().is_some_and(|h| h.0 == t.id)
                && let Some((_, _, _, r)) = self.handoff.take()
            {
                let _ = r.send(Err("the tab was closed".into()));
            }
            drop(t);
        }
        // activate() looks the old id up; a dead id must not resolve.
        self.active = 0;
    }

    /// Rebuild live tabs for workspace `to` from its saved list and open one.
    fn load_workspace(&mut self, to: usize) {
        let ws = self
            .workspaces
            .get(to)
            .cloned()
            .unwrap_or_else(|| workspaces::WorkspaceData::new(workspaces::DEFAULT_NAME));
        let mut active = None;
        for (i, saved) in ws.saved.iter().enumerate() {
            let mut t = Tab::new(self.ctx.alloc_id(), saved.url.clone());
            t.title = saved.title.clone();
            t.favicon = saved.favicon.clone();
            t.pinned = saved.pinned;
            t.pin_url = pins::restored_pin_url(saved.pinned, &saved.pin_url, &t.url);
            t.workspace = to;
            if i == ws.active {
                active = Some(t.id);
            }
            self.tabs.push(t);
        }
        // Pinned tabs live at the front. The UI groups them; the order must agree.
        self.tabs.sort_by_key(|t| !t.pinned);
        if self.tabs.is_empty() {
            let mut t = Tab::new(self.ctx.alloc_id(), String::new());
            t.workspace = to;
            self.tabs.push(t);
        }
        let ids: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        let new_active = self.tabs.iter().position(|t| Some(t.id) == active).unwrap_or(0);
        if let Some(ws) = self.workspaces.get_mut(to) {
            ws.tab_ids = ids;
            ws.saved.clear();
            ws.active = new_active;
        }
        self.active_workspace = to;
        let id = active.filter(|id| self.idx(*id).is_some()).unwrap_or(self.tabs[0].id);
        self.activate(id);
    }

    /// Switch to workspace `to`. The current workspace's tabs are discarded
    /// (no memory) and the target's are restored.
    fn switch_workspace(&mut self, to: usize) {
        let to = to.min(self.workspaces.len().saturating_sub(1));
        if to == self.active_workspace {
            return;
        }
        self.stash_workspace();
        self.drop_all_tabs();
        self.load_workspace(to);
        journal::feature("workspace_switch", false);
    }

    // ---------------------------------------------------------------- glance

    /// Shift+click on a link: open it in a floating card over the current page.
    fn open_glance(&mut self, url: &str) {
        // One glance at a time. Drop the old one.
        self.glance = None;
        let id = self.ctx.alloc_id();
        let (x, y, w, h) = self.stage();
        // The card is 80% of the stage, centred; layout sizes the dim to match.
        let rect = logical_rect(x + w * 0.1, y + h * 0.1, w * 0.8, h * 0.8);
        let Ok(v) = engine::build_tab(&self.ctx, id, Some(url), rect, false, false) else { return };
        // The card takes Escape and Enter itself while it has focus.
        glance::hook_keys(&v, self.ctx.proxy.clone());
        if !headless() {
            let _ = v.focus();
        }
        journal::feature("glance", self.active_tab().is_some_and(|t| t.private));
        self.glance = Some((id, url.to_string(), v));
        self.chrome.set_glance(true);
        self.layout();
        self.push();
    }

    /// Escape: drop the floating card, back to the page below.
    fn close_glance(&mut self) {
        if self.drop_glance() {
            self.dirty = true;
            self.push();
            self.focus_page();
        }
    }

    /// The card goes down without the trailing work: the caller lays out and
    /// pushes anyway. True when a card was open.
    fn drop_glance(&mut self) -> bool {
        if self.glance.take().is_some() {
            self.chrome.set_glance(false);
            self.layout();
            true
        } else {
            false
        }
    }

    /// Enter or the "Open as tab" button: the glance becomes a normal tab.
    fn promote_glance(&mut self) {
        let Some((_, url, _v)) = self.glance.take() else { return };
        self.chrome.set_glance(false);
        self.new_tab(Some(url), None);
        self.layout();
        self.dirty = true;
        self.push();
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
            let name = s.path.rsplit(['\\', '/']).next().unwrap_or(&s.path).to_string();
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
                name,
                kind: "file",
                up: 0,
                peers: 0,
                seeds: 0,
                ratio: 0.0,
                files: 1,
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
            self.aria_person.remove(&gid);
            self.aria_page.remove(&gid);
            self.aria_hold.remove(&gid);
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
            let _ = match aria.add(&url, &headers, name.as_deref(), false) {
                Ok(gid) => proxy.send_event(Ev::AriaAdded(Box::new(engine::Added {
                    gid,
                    url,
                    headers,
                    name,
                    private,
                    person: true,
                    hold: None,
                }))),
                Err(_) => proxy.send_event(Ev::AriaFailed(tab, url)),
            };
        });
    }

    /// A magnet link goes straight to aria2. The display name comes from `dn=`
    /// until the torrent metadata arrives with the real one. `hold` is Some for
    /// a magnet a PAGE asked for: it goes in paused and waits for the person —
    /// a torrent tells the swarm the user's IP, so a page never starts one.
    fn add_magnet(&mut self, url: &str, private: bool, hold: Option<String>) {
        let name = magnet_name(url);
        let person = hold.is_none();
        let (aria, proxy) = (self.aria.clone(), self.ctx.proxy.clone());
        let u = url.to_string();
        let headers: Vec<String> = Vec::new();
        std::thread::spawn(move || {
            let _ = match aria.add(&u, &headers, None, !person) {
                Ok(gid) => proxy.send_event(Ev::AriaAdded(Box::new(engine::Added {
                    gid,
                    url: u,
                    headers,
                    name: Some(name),
                    private,
                    person,
                    hold,
                }))),
                Err(_) => proxy.send_event(Ev::AriaFailed(0, u)),
            };
        });
        self.aria_poll.store(true);
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
                name: format!("{title}.mp4"),
                kind: "video",
                up: 0,
                peers: 0,
                seeds: 0,
                ratio: 0.0,
                files: 1,
            },
        );
        self.videos.insert(id, (url, ua, Default::default()));
        journal::feature("save_video", private);
        self.run_video(id);
        self.push();
    }

    fn run_video(&self, id: u64) {
        let Some((url, ua, slot)) = self.videos.get(&id).cloned() else { return };
        let tools = store::data_dir().join("tools");
        let out = store::download_dir();
        let proxy = self.ctx.proxy.clone();
        let sponsorblock = self.settings.sponsorblock;
        std::thread::Builder::new()
            .name("lumen-video".into())
            .spawn(move || {
                video::grab(&tools, &out, &url, &ua, &slot, sponsorblock, |u| {
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
                if self.panel != Panel::Downloads {
                    self.dl_unseen = true;
                }
                d.path = f;
                d.state = "done";
                d.speed = 0;
                d.note.clear();
                d.received = std::fs::metadata(&d.path).map(|m| m.len() as i64).unwrap_or(d.received);
                journal::download(id, "video", "done", d.received, d.private);
            }
            video::Up::Failed(e) => {
                self.dl_dirty = true;
                if self.panel != Panel::Downloads {
                    self.dl_unseen = true;
                }
                d.state = "failed";
                d.speed = 0;
                journal::download(id, "video", "failed", d.received, d.private);
                journal::error("video", &e);
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
        let id = self.tabs[i].id;
        if let Some(orig) = self.tabs[i].reader.take() {
            if let Some(v) = &self.tabs[i].view {
                let _ = v.load_url(&orig);
            }
            // Out of reader view: the reading goes with it.
            if self.aloud.as_ref().is_some_and(|a| a.tab == id) {
                self.listen_stop();
            }
            self.push();
            return;
        }
        if !self.tabs[i].url.starts_with("http") {
            return;
        }
        let t = &self.tabs[i];
        let Some(v) = &t.view else { return };
        let (id, url, proxy) = (t.id, t.url.clone(), self.ctx.proxy.clone());
        let _ = v.evaluate_script_with_callback(&pagekit::extract_script(), move |out| {
            let _ = proxy.send_event(Ev::Reader(id, url.clone(), out));
        });
    }

    /// F7 or the chip: start, pause, or resume — whichever fits where it stands.
    fn listen_toggle(&mut self) {
        match self.aloud.as_mut() {
            Some(a) if a.playing => {
                aloud::pause();
                a.playing = false;
            }
            Some(a) => {
                aloud::resume();
                a.playing = true;
            }
            None => self.listen_start(),
        }
        self.push();
    }

    /// Reading starts from the article on screen: split it into sentences in
    /// the page, then hand the list to the speaker thread.
    fn listen_start(&mut self) {
        let i = self.active_idx();
        let t = &self.tabs[i];
        if t.reader.is_none() || t.agent {
            return;
        }
        let Some(v) = &t.view else { return };
        let (id, proxy, private) = (t.id, self.ctx.proxy.clone(), t.private);
        journal::feature("read_aloud", private);
        let _ = v.evaluate_script_with_callback(&aloud::prep_script(), move |out| {
            let _ = proxy.send_event(Ev::AloudPrep(id, out));
        });
    }

    /// Stop speaking and wipe the highlight. Called on tab switch, navigation,
    /// reader exit, close — anywhere the page under the voice is going away.
    fn listen_stop(&mut self) {
        let Some(a) = self.aloud.take() else { return };
        aloud::stop();
        if let Some(i) = self.idx(a.tab)
            && let Some(v) = &self.tabs[i].view
        {
            let _ = v.evaluate_script(&aloud::highlight_script(None));
        }
        self.push();
    }

    /// Put the chosen KeePass login into the password box on screen. The
    /// values exist only inside the script string, handed straight to the page.
    fn kdbx_fill(&mut self, n: usize) {
        let Some(i) = self.idx(self.active) else { return };
        let Some(v) = &self.vault else { return };
        let Some(k) = v.matches(&self.tabs[i].url).get(n).copied() else { return };
        let Some((user, pass)) = v.get(k) else { return };
        let js = kdbx::fill_js(user, pass);
        if let Some(view) = &self.tabs[i].view {
            let _ = view.evaluate_script(&js);
        }
        journal::feature("kdbx_fill", self.tabs[i].private);
        self.vault_seen = Instant::now();
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
            journal::feature("reader", t.private);
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
        journal::feature("dark_site", self.tabs[i].private);
        self.apply_settings(s);
    }

    fn navigate(&mut self, raw: &str) {
        let Some((url, bare)) = classify_input(raw, &self.settings.search) else { return };
        // A magnet link is not a page: hand it to aria2 instead of loading it.
        // The person typed it, so it runs at once.
        if url.starts_with("magnet:") {
            self.add_magnet(&url, self.active_tab().is_some_and(|t| t.private), None);
            return;
        }
        let i = self.active_idx();
        self.leave_panel();
        // A bare host is remembered until the page answers: if the name turns out
        // not to exist, the person gets a search for what they typed.
        self.tabs[i].typed = bare.then(|| Typed { raw: raw.trim().to_string(), url: url.clone(), started: false });
        // The other engine: a host from the "Servo sites" list is a Servo page, and
        // one that left the list goes back to WebView2.
        #[cfg(feature = "servo")]
        self.place_on_engine(i, &url);
        #[cfg(feature = "servo")]
        if self.tabs[i].servo.is_some() {
            if let Some(p) = &self.tabs[i].servo {
                p.load(&url);
            }
            self.tabs[i].url = url;
            self.layout();
            self.focus_page();
            self.dirty = true;
            self.push();
            return;
        }
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
        // The page is open now: the dot on the footer button has done its job.
        if p == Panel::Downloads {
            self.dl_unseen = false;
        }
        if was == Panel::Control && p != Panel::Control {
            self.stop_monitor();
        }
        if p == Panel::Control && was != Panel::Control {
            self.start_monitor();
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
        match self.ctx.env.borrow().as_ref() {
            Some(env) => monitor::sample(env, self.ctx.proxy.clone()),
            None => monitor::sample_host(self.ctx.proxy.clone()),
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
        self.rounded.clear();
        self.layout();
        self.push();
    }

    // ------------------------------------------------------------ settings

    /// The window behind the page gaps takes the sidebar's color.
    /// The native chrome is solid; an opaque window is the cheapest thing Windows draws.
    fn apply_backdrop(&mut self) {
        let (r, g, b) = self.bg;
        let w = self.ctx.window.clone();
        w.set_background_color(Some((r, g, b, 255)));
        engine::set_glass(w.hwnd(), false);
        self.divider_theme();
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
        // Switching compact either way (Ctrl+B, the toggle) is a fresh start for the peek.
        if old.compact != self.settings.compact {
            self.peek = false;
        }
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
            self.rounded.clear();
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
        if old.journal != self.settings.journal {
            journal::set_enabled(self.settings.journal);
        }
        if old.download_limit_kbps != self.settings.download_limit_kbps {
            self.aria.set_speed_limit(self.settings.download_limit_kbps as u64 * 1024);
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
                    // The page paces its own titles (engine.rs); one that is already shown is nothing.
                    if self.tabs[i].title != t {
                        self.show_title(i, t);
                    }
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
                        // The reader page is gone; so is the reading.
                        let id = self.tabs[i].id;
                        if self.aloud.as_ref().is_some_and(|a| a.tab == id) {
                            self.listen_stop();
                        }
                    }
                    // SponsorBlock: on a YouTube watch page, fetch segments and
                    // inject the skip script. One background fetch per video;
                    // whatever was pending for this tab is about the old page.
                    // Nothing is stored and the video ID never leaves the
                    // machine in the clear, so agent and private tabs get it too.
                    self.spb.remove(&self.tabs[i].id);
                    if self.settings.sponsorblock && sponsorblock::is_youtube_watch(&u) {
                        if let Some(vid) = sponsorblock::video_id(&u) {
                            if self.tabs[i].view.is_some() {
                                let tab_id = self.tabs[i].id;
                                let proxy = self.ctx.proxy.clone();
                                std::thread::spawn(move || {
                                    let segs = sponsorblock::fetch_segments(&vid);
                                    let json = serde_json::json!(segs.iter().map(|s| [s.start, s.end]).collect::<Vec<_>>());
                                    let script = sponsorblock::init_script(&json.to_string());
                                    let _ = proxy.send_event(Ev::SponsorBlock(tab_id, script));
                                });
                            }
                        }
                    }
                    // Agents browse off the record, and so do private tabs.
                    if !self.off_record(&self.tabs[i]) {
                        self.library.visit(&u, &self.tabs[i].title);
                    }
                    // Another page: the original text went with the old document, and
                    // a translation still in the air is for a page that is gone.
                    if self.tabs[i].url != u {
                        self.tabs[i].translated = None;
                        self.tabs[i].translating = false;
                        if self.tr_busy == Some(id) {
                            self.stop_translating(id);
                        }
                    }
                    self.tabs[i].url = u;
                    self.dirty = true;
                    self.push();
                }
            }
            Ev::Loading(id, on) => {
                if let Some(i) = self.idx(id) {
                    // A page navigating again is a page that came back from "crashed".
                    if on && self.tabs[i].crashed {
                        self.tabs[i].crashed = false;
                        self.push();
                    }
                    if self.tabs[i].loading != on {
                        self.tabs[i].loading = on;
                        self.push();
                        // The document is here: a skip script that was waiting
                        // for it can go in now.
                        if !on && let Some(script) = self.spb.remove(&id) {
                            if let Some(v) = &self.tabs[i].view {
                                let _ = v.evaluate_script(&script);
                            }
                        }
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
            // The journal's timing of a page. A private tab is not in it.
            Ev::NavStart(id, url) => {
                if let Some(i) = self.idx(id) {
                    // The page the person typed is the one now loading, or it never will be.
                    match self.tabs[i].typed.as_mut() {
                        Some(t) if same_page(&t.url, &url) => t.started = true,
                        Some(_) => self.tabs[i].typed = None,
                        None => {}
                    }
                }
                let (agent, private) = self.idx(id).map_or((false, true), |i| (self.tabs[i].agent, self.tabs[i].private));
                journal::nav_started(id, &url, agent, private, self.blocked_count());
            }
            Ev::NavDone(id, ok, err, http) => {
                journal::nav_done(id, ok, err, http, self.blocked_count());
                self.typed_done(id, ok, err);
            }
            // The server sent the typed page somewhere else: that is not a typo.
            Ev::NavRedirected(id) => {
                if let Some(i) = self.idx(id) {
                    self.tabs[i].typed = None;
                }
            }
            Ev::Audio(id, on) => {
                if let Some(i) = self.idx(id) {
                    self.tabs[i].audio = on;
                    if on {
                        // Sound from a page that was frozen a moment ago: let it go on.
                        if self.tabs[i].suspended
                            && let Some(v) = &self.tabs[i].view
                        {
                            engine::resume(v);
                            self.tabs[i].suspended = false;
                        }
                    } else if self.tabs[i].hidden_at.is_some() {
                        // The music stopped: the sleep timer starts now, not from the
                        // minute the tab was hidden.
                        self.tabs[i].hidden_at = Some(Instant::now());
                    }
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
            Ev::Crashed(id, browser) => {
                // The journal wants the death and, later, whether the reload took.
                let host = self.idx(id).map_or_else(String::new, |i| host_of(&self.tabs[i].url));
                let private = self.idx(id).is_some_and(|i| self.tabs[i].private);
                journal::crash(id, if browser { "browser" } else { "renderer" }, &host, private);
                if browser {
                    // The browser process is gone and every WebView2 object with it.
                    // Dropping them all lets the next tab activation build a fresh
                    // environment, the same way the first tab does at startup.
                    self.ctx.env.replace(None);
                    self.ctx.popups.borrow_mut().clear();
                    for t in &mut self.tabs {
                        t.view = None;
                    }
                } else if let Some(i) = self.idx(id) {
                    // The renderer died; the tab stays. Clicking it reloads the page.
                    self.tabs[i].crashed = true;
                }
                self.push();
            }
            Ev::PaneFocus(id) => {
                // A click landed in one half of the split. Anything else that takes
                // focus (a page loading on its own) leaves the panes where they are.
                if self.split == Some(id) {
                    self.activate(id);
                }
            }
            Ev::SplitDrag(x) => {
                if self.split.is_none() {
                    return;
                }
                let (sx, _, sw, _) = self.stage();
                let at = split::fraction(sx, sw, x);
                if (at - self.split_at).abs() > 0.002 {
                    self.split_at = at;
                    self.layout();
                }
            }
            Ev::SplitDrop => {}
            Ev::SplitReset => {
                if self.split.is_some() {
                    self.split_at = 0.5;
                    self.layout();
                }
            }
            Ev::TrPage(id, target, out) => self.on_tr_page(id, &target, &out),
            Ev::TrModel(id, ok, why) => self.on_tr_model(id, ok, why),
            Ev::TrMsg(msg) => self.on_tr_msg(&msg),
            // Servo's reports: the same shape as WebView2's, from the other engine.
            #[cfg(feature = "servo")]
            Ev::ServoWake => {}
            #[cfg(feature = "servo")]
            Ev::ServoUrl(id, u) => {
                if let Some(i) = self.idx(id) {
                    if !self.off_record(&self.tabs[i]) {
                        self.library.visit(&u, &self.tabs[i].title);
                    }
                    self.tabs[i].url = u;
                    self.dirty = true;
                    self.push();
                }
            }
            #[cfg(feature = "servo")]
            Ev::ServoTitle(id, title) => {
                if let Some(i) = self.idx(id) {
                    if !self.off_record(&self.tabs[i]) {
                        self.library.set_title(&self.tabs[i].url, &title);
                    }
                    self.tabs[i].title = title;
                    self.dirty = true;
                    self.push();
                }
            }
            #[cfg(feature = "servo")]
            Ev::ServoLoad(id, on) => {
                if let Some(i) = self.idx(id) {
                    if self.tabs[i].loading != on {
                        self.tabs[i].loading = on;
                        self.push();
                    }
                    // Test hook: the page finished loading, so say whether it painted.
                    #[cfg(debug_assertions)]
                    if !on
                        && let Some(p) = self.tabs[i].servo.as_ref()
                    {
                        p.proof_of_paint();
                    }
                }
            }
            #[cfg(feature = "servo")]
            Ev::ServoGone(id) => {
                // Servo gave up on the page, or it crashed: the tab stays, with its
                // URL, and a click on it opens it again (in WebView2, if it is not a
                // Servo host any more).
                if let Some(i) = self.idx(id) {
                    self.tabs[i].servo = None;
                    self.tabs[i].crashed = true;
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
                journal::download(d.id, d.kind, d.state, d.received, d.private);
                if matches!(d.state, "done" | "failed") {
                    self.dl_dirty = true;
                    if self.panel != Panel::Downloads {
                        self.dl_unseen = true;
                    }
                    // The reason WebView2 gives is usually none, so the row's own
                    // note, or the host, has to stand in for it.
                    let was = self.downloads.iter().find(|x| x.id == d.id).map(|x| x.state);
                    if d.state == "failed" && was.is_none_or(|w| w != "failed") {
                        let what = if d.name.is_empty() { host_of(&d.url) } else { d.name.clone() };
                        let why = if d.note.is_empty() { format!("{what} did not finish") } else { d.note.clone() };
                        journal::error("download", &why);
                    }
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
                        // A page's download: the .torrent file itself is fetched
                        // like any file, but the torrent it spawns waits for Resume.
                        let _ = match aria.add(&u, &headers, name.as_deref(), false) {
                            Ok(gid) => proxy.send_event(Ev::AriaAdded(Box::new(engine::Added {
                                gid,
                                url: u,
                                headers,
                                name,
                                private,
                                person: false,
                                hold: None,
                            }))),
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
            Ev::AriaAdded(a) => {
                let engine::Added { gid, url, headers, name, private, person, hold } = *a;
                if person {
                    self.aria_person.insert(gid.clone());
                } else if hold.is_none() {
                    // A page's download: any torrent it spawns waits for Resume.
                    self.aria_page.insert(gid.clone());
                }
                if let Some(note) = hold {
                    self.aria_hold.insert(gid.clone(), note);
                }
                // The row may already exist if a poll adopted this gid first; keep its id.
                if !self.aria_ids.contains_key(&gid) {
                    let id = self.next_dl(1);
                    self.aria_ids.insert(gid.clone(), id);
                    if private {
                        // Show it now, marked private, so it never reaches downloads.json.
                        let held = self.aria_hold.get(&gid).cloned();
                        self.downloads.insert(
                            0,
                            engine::Dl {
                                id,
                                url: url.clone(),
                                path: String::new(),
                                received: 0,
                                total: 0,
                                state: if held.is_some() { "paused" } else { "progress" },
                                speed: 0,
                                note: held.unwrap_or_else(|| "starting".into()),
                                private,
                                name: String::new(),
                                kind: "file",
                                up: 0,
                                peers: 0,
                                seeds: 0,
                                ratio: 0.0,
                                files: 1,
                            },
                        );
                        self.push();
                    }
                }
                self.aria_meta.insert(gid.clone(), (url, headers, name));
                if self.test_pending {
                    self.test_pending = false;
                    self.test_dl = Some((gid, Instant::now() + Duration::from_secs(
                        std::env::var("LUMEN_TEST_CANCEL").ok().and_then(|v| v.parse().ok()).unwrap_or(60),
                    )));
                }
                self.aria_poll.store(true);
            }
            Ev::AriaFailed(tab, url) => {
                if tab == 0 && !url.is_empty() {
                    // A manual add (Add link / magnet / torrent) that aria2 refused:
                    // show a failed row. There is no page to fall back to.
                    journal::error("download", "aria2 refused it");
                    let id = self.next_dl(1);
                    let name = if url.starts_with("magnet:") {
                        magnet_name(&url)
                    } else {
                        url.rsplit(['/', '?', '#']).next().unwrap_or("").to_string()
                    };
                    let name = if name.is_empty() { "download".to_string() } else { name };
                    self.downloads.insert(
                        0,
                        engine::Dl {
                            id,
                            url,
                            path: String::new(),
                            received: 0,
                            total: 0,
                            state: "failed",
                            speed: 0,
                            note: "aria2 refused it".into(),
                            private: false,
                            name,
                            kind: "file",
                            up: 0,
                            peers: 0,
                            seeds: 0,
                            ratio: 0.0,
                            files: 1,
                        },
                    );
                    self.dl_dirty = true;
                    if self.panel != Panel::Downloads {
                        self.dl_unseen = true;
                    }
                    self.push();
                    return;
                }
                // aria2 said no (auth, odd server): let WebView2 download it after all.
                self.ctx.bypass.borrow_mut().insert(url.clone());
                if let Some(v) = self.idx(tab).and_then(|i| self.tabs[i].view.as_ref()) {
                    let _ = v.load_url(&url);
                }
            }
            Ev::AriaJobs(jobs) => {
                // Adopt-on-start: aria2 restored unfinished downloads from its session
                // file, but Lumen forgot their gids. Put each one in the list once.
                if !jobs.is_empty() && !self.aria_adopted {
                    self.aria_adopted = true;
                    for j in &jobs {
                        if self.aria_ids.contains_key(&j.gid) {
                            continue;
                        }
                        let id = self.next_dl(1);
                        self.aria_ids.insert(j.gid.clone(), id);
                        let name = if !j.name.is_empty() {
                            j.name.clone()
                        } else {
                            j.path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
                        };
                        self.downloads.insert(
                            0,
                            engine::Dl {
                                id,
                                url: j.url.clone(),
                                path: j.path.clone(),
                                received: j.done as i64,
                                total: j.total as i64,
                                state: "progress",
                                speed: j.speed,
                                note: String::new(),
                                private: false,
                                name,
                                kind: if j.name.is_empty() { "file" } else { "torrent" },
                                up: 0,
                                peers: 0,
                                seeds: 0,
                                ratio: 0.0,
                                files: j.files.max(1),
                            },
                        );
                    }
                }
                // A finished .torrent file hands its row to the torrent it spawned.
                let fmap = aria::follow_map(&jobs);
                let mut busy_pending = false;
                for (old, new) in &fmap {
                    if let Some(&id) = self.aria_ids.get(old)
                        && !self.aria_ids.contains_key(new)
                    {
                        self.aria_ids.remove(old);
                        self.aria_meta.remove(old);
                        self.aria_ids.insert(new.clone(), id);
                        if self.aria_person.remove(old) {
                            // A person-started .torrent hands its consent to the
                            // torrent it spawned; pause-metadata stops the child,
                            // so start it now.
                            self.aria_person.insert(new.clone());
                            let gid = new.clone();
                            let aria = self.aria.clone();
                            std::thread::spawn(move || {
                                let _ = aria.resume(&gid);
                            });
                            busy_pending = true;
                        } else if self.aria_page.remove(old) {
                            // A page's .torrent: the torrent it spawned waits for
                            // the person, with a note saying so.
                            self.aria_hold.insert(new.clone(), "press Resume to start this torrent".into());
                            let gid = new.clone();
                            let aria = self.aria.clone();
                            std::thread::spawn(move || {
                                let _ = aria.pause(&gid);
                            });
                        }
                    }
                }
                let mut busy = busy_pending;
                let mut torrent_active = false;
                for j in jobs {
                    let Some(&id) = self.aria_ids.get(&j.gid) else { continue };
                    // A person-started torrent pause-metadata stopped once its
                    // metadata arrived: start it. Keep the poll awake for one
                    // more round to see it run.
                    if should_auto_resume(self.aria_person.contains(&j.gid), &j.status, !j.name.is_empty(), j.done < j.total) {
                        let gid = j.gid.clone();
                        let aria = self.aria.clone();
                        std::thread::spawn(move || {
                            let _ = aria.resume(&gid);
                        });
                        busy = true;
                    }
                    let state = match j.status {
                        "active" | "waiting" => {
                            if j.seeder { "seeding" } else { "progress" }
                        }
                        "paused" => "paused",
                        "complete" => {
                            if j.seeder { "seeding" } else { "done" }
                        }
                        _ => "failed",
                    };
                    busy |= matches!(state, "progress" | "seeding");
                    let note = match j.status {
                        "active" if j.seeder => format!("{} peers", j.connections),
                        "active" => format!("{} connections", j.connections),
                        "waiting" => "queued".to_string(),
                        "paused" => self.aria_hold.get(&j.gid).cloned().unwrap_or_default(),
                        "removed" => "cancelled".to_string(),
                        "error" if !j.error.is_empty() => j.error.trim().trim_end_matches('.').to_string(),
                        _ => String::new(),
                    };
                    let was = self.downloads.iter().find(|x| x.id == id).map(|x| x.state);
                    if matches!(state, "done" | "failed" | "seeding")
                        && was.is_some_and(|w| w != state)
                    {
                        if state != "seeding" {
                            self.dl_dirty = true;
                        }
                        // Something ended while the page was closed: show the dot.
                        if self.panel != Panel::Downloads {
                            self.dl_unseen = true;
                        }
                    }
                    let private = self.downloads.iter().find(|x| x.id == id).is_some_and(|x| x.private);
                    let name = if !j.name.is_empty() {
                        j.name.clone()
                    } else {
                        j.path.rsplit(['\\', '/']).next().unwrap_or("").to_string()
                    };
                    let ratio = if j.done > 0 { j.upload_length as f64 / j.done as f64 } else { 0.0 };
                    let kind = if j.name.is_empty() { "file" } else { "torrent" };
                    // The journal's view of it: one `download` line when it ends,
                    // and the reason the person was shown when aria2 gives up.
                    journal::download(id, kind, state, j.done as i64, private);
                    if state == "failed" && was.is_some_and(|w| w != "failed") && !note.is_empty() {
                        journal::error("download", &note);
                    }
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
                        name,
                        kind,
                        up: j.upload_speed,
                        peers: j.connections as u64,
                        seeds: j.num_seeders,
                        ratio,
                        files: j.files.max(1),
                    };
                    if kind == "torrent" && matches!(state, "progress" | "seeding") {
                        torrent_active = true;
                    }
                    match self.downloads.iter_mut().find(|x| x.id == id) {
                        Some(x) => *x = d,
                        None => self.downloads.insert(0, d),
                    }
                }
                if torrent_active {
                    self.upnp.torrent_active();
                } else {
                    self.upnp.torrent_idle();
                }
                if !busy && !self.downloads.iter().any(|d| d.state == "paused" && d.id >= 1 << 40) {
                    self.aria_poll.store(false);
                }
                self.push();
            }
            Ev::Reader(id, url, out) => self.show_reader(id, &url, &out),
            Ev::SponsorBlock(id, script) => {
                // Used only when there was something to skip: an empty list is
                // a page SponsorBlock knows nothing about.
                if !script.contains("const SEGMENTS = [];") {
                    let private = self.idx(id).is_some_and(|i| self.tabs[i].private);
                    journal::feature("sponsorblock_skip", private);
                }
                // A page still loading is about to become another document:
                // hold the script until it is there, or it runs in the old one
                // and the player never hears of it.
                let loading = self.idx(id).is_some_and(|i| self.tabs[i].loading);
                if loading {
                    self.spb.insert(id, script);
                } else {
                    self.spb.remove(&id);
                    if let Some(i) = self.idx(id) {
                        if let Some(v) = &self.tabs[i].view {
                            let _ = v.evaluate_script(&script);
                        }
                    }
                }
            }
            Ev::AloudPrep(id, out) => {
                let Ok(rows) = serde_json::from_str::<Vec<(usize, usize, String)>>(&out) else { return };
                if rows.is_empty() {
                    return;
                }
                let ranges: Vec<(usize, usize)> = rows.iter().map(|r| (r.0, r.1)).collect();
                let texts: Vec<String> = rows.into_iter().map(|r| r.2).collect();
                let (tab, proxy) = (id, self.ctx.proxy.clone());
                let back = proxy.clone();
                let ok = aloud::speak(
                    texts,
                    move |n| {
                        let _ = proxy.send_event(Ev::AloudLine(tab, n));
                    },
                    move || {
                        let _ = back.send_event(Ev::AloudLine(tab, usize::MAX));
                    },
                );
                if ok {
                    self.aloud = Some(Aloud {
                        tab,
                        ranges,
                        playing: true,
                    });
                    self.push();
                }
            }
            Ev::AloudLine(tab, n) => {
                if !self.aloud.as_ref().is_some_and(|a| a.tab == tab) {
                    return;
                }
                let Some(i) = self.idx(tab) else { return };
                if n == usize::MAX {
                    // Spoken through: no state left, no highlight left.
                    self.aloud = None;
                    if let Some(v) = &self.tabs[i].view {
                        let _ = v.evaluate_script(&aloud::highlight_script(None));
                    }
                } else if self.aloud.as_ref().is_some_and(|a| a.ranges.get(n).is_some())
                    && let Some(v) = &self.tabs[i].view
                {
                    let _ = v.evaluate_script(&aloud::highlight_script(Some(n)));
                }
                self.push();
            }
            Ev::VaultOpen(path, pw) => {
                // Parsing is slow and must stay off the UI thread. The pair
                // rides on a quiet event: never logged, never written.
                let proxy = self.ctx.proxy.clone();
                std::thread::spawn(move || {
                    let r = kdbx::Vault::open(std::path::Path::new(&path), &pw).map(Box::new);
                    let _ = proxy.send_event(Ev::VaultReady(r));
                });
            }
            Ev::VaultReady(r) => match r {
                Ok(v) => {
                    self.vault = Some(*v);
                    self.vault_seen = Instant::now();
                    self.push();
                }
                Err(e) => {
                    // Wrong password or not a .kdbx: say it where the button was.
                    let msg = format!("Couldn't open that file: {e}");
                    journal::error("kdbx", &msg);
                    self.chrome.flash(msg);
                }
            },
            Ev::Video(id, up) => self.on_video(id, up),
            Ev::ShowMe => self.show_me(),
            Ev::Glance(id, url) => {
                if id == self.active {
                    self.open_glance(&url);
                } else if let Some((gid, _, v)) = &self.glance
                    && *gid == id
                {
                    // A shift+click inside the card moves the card, not the page.
                    let _ = v.load_url(&url);
                }
            }
            Ev::GlanceKey(k) => match k {
                "esc" => self.close_glance(),
                _ => self.promote_glance(),
            },
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
                        t.visible = false;
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
            Ev::Magnet(tab, url) => {
                // A page (or an agent tab — those count as pages) asked for a
                // magnet: it goes in paused, waiting for the person. The note
                // says which page asked.
                let private = self.idx(tab).is_some_and(|i| self.tabs[i].private);
                let page = self.idx(tab).map(|i| self.tabs[i].url.clone()).unwrap_or_default();
                self.add_magnet(&url, private, Some(magnet_hold(&page)));
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
            Action::Reload => {
                #[cfg(feature = "servo")]
                if let Some(p) = &self.tabs[i].servo {
                    p.reload();
                    self.dirty = true;
                    self.push();
                    return;
                }
                match &self.tabs[i].view {
                    Some(v) => {
                        let _ = v.reload();
                    }
                    None => self.activate(self.active),
                }
            }
            Action::Back => {
                #[cfg(feature = "servo")]
                if let Some(p) = &self.tabs[i].servo {
                    p.back();
                    return;
                }
                if let Some(v) = &self.tabs[i].view {
                    let _ = v.go_back();
                }
            }
            Action::Forward => {
                #[cfg(feature = "servo")]
                if let Some(p) = &self.tabs[i].servo {
                    p.forward();
                    return;
                }
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
            Action::Listen => self.listen_toggle(),
            Action::DarkSite => self.toggle_dark_site(i),
            Action::Workspace(n) => {
                if n < self.workspaces.len() {
                    self.switch_workspace(n);
                }
            }
            Action::Split => self.toggle_split(),
            Action::TranslatePage => self.toggle_translate(i),
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
                    // The pin remembers the page it was made on; unpinning forgets it.
                    t.pin_url = if t.pinned { t.url.clone() } else { String::new() };
                    // Pinned: end of the pinned block. Unpinned: first after it.
                    let at = self.pinned_count();
                    self.tabs.insert(at, t);
                    self.dirty = true;
                    self.push();
                }
            }
            "pinBack" => {
                // A pin that wandered goes back to the page it was pinned on.
                if let Some(i) = self.idx(id) {
                    let t = &mut self.tabs[i];
                    let url = t.pin_url.clone();
                    if t.pinned && !url.is_empty() {
                        t.url = url.clone();
                        if let Some(v) = &t.view {
                            let _ = v.load_url(&url);
                        }
                        self.dirty = true;
                        self.push();
                    }
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
            "split" => self.split_next(id),
            "webview2" => {
                // "Reopen in WebView2": the host leaves the Servo list, so it opens
                // here from now on, and this tab is handed over right away.
                #[cfg(feature = "servo")]
                if let Some(i) = self.idx(id) {
                    let host = host_of(&self.tabs[i].url);
                    if !host.is_empty() {
                        let mut s = self.settings.clone();
                        s.servo_sites.retain(|h| !host.eq_ignore_ascii_case(h.trim().trim_start_matches("www.")));
                        self.apply_settings(s);
                    }
                    self.tabs[i].servo = None;
                    self.tabs[i].visible = false;
                    self.ensure_view(i);
                    self.layout();
                    self.focus_page();
                    self.push();
                }
            }
            "translate" => {
                if let Some(i) = self.idx(id) {
                    self.toggle_translate(i);
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
            "workspaceSwitch" => {
                if let Some(n) = m["to"].as_u64() {
                    self.switch_workspace(n as usize);
                }
            }
            "workspaceAdd" => {
                // A new, empty workspace; switch into it. Ctrl+Alt+1..4 reaches it.
                let n = self.workspaces.len();
                if n < workspaces::MAX_WORKSPACES {
                    journal::feature("workspace_add", false);
                    self.workspaces.push(workspaces::WorkspaceData::new(&format!("Workspace {}", n + 1)));
                    self.switch_workspace(n);
                }
            }
            "workspaceRename" => {
                let i = m["i"].as_u64().unwrap_or(0) as usize;
                let name = m["name"].as_str().unwrap_or("").trim().to_string();
                if let Some(ws) = self.workspaces.get_mut(i) {
                    if !name.is_empty() {
                        ws.name = name;
                    }
                    self.dirty = true;
                    self.push();
                }
            }
            "workspaceDuplicate" => {
                let i = m["i"].as_u64().unwrap_or(0) as usize;
                // The active workspace's tabs are live; everyone else's are stored.
                let dup = self.workspaces.get(i).map(|ws| {
                    let saved = if i == self.active_workspace {
                        keepable(&self.tabs, self.active)
                            .iter()
                            .map(|t| t.saved())
                            .collect()
                    } else {
                        ws.saved.clone()
                    };
                    (format!("{} copy", ws.name), saved)
                });
                if let Some((name, saved)) = dup
                    && self.workspaces.len() < workspaces::MAX_WORKSPACES
                {
                    let mut ws = workspaces::WorkspaceData::new(&name);
                    ws.saved = saved;
                    self.workspaces.push(ws);
                    self.dirty = true;
                    self.push();
                }
            }
            "workspaceClose" => {
                let i = m["i"].as_u64().unwrap_or(0) as usize;
                if self.workspaces.len() > 1 && i < self.workspaces.len() {
                    let was_active = i == self.active_workspace;
                    self.workspaces.remove(i);
                    if was_active {
                        // The closed workspace's live tabs go with it; open the neighbour.
                        self.active_workspace = self.active_workspace.min(self.workspaces.len() - 1);
                        self.drop_all_tabs();
                        self.load_workspace(self.active_workspace);
                    } else {
                        if i < self.active_workspace {
                            self.active_workspace -= 1;
                        }
                        // Live tabs all belong to the active workspace; their index moved.
                        for t in &mut self.tabs {
                            t.workspace = self.active_workspace;
                        }
                        self.dirty = true;
                        self.push();
                    }
                }
            }
            "moveTabToWorkspace" => {
                let to = m["to"].as_u64().map(|n| n as usize).unwrap_or(usize::MAX);
                if let Some(id) = m["id"].as_u64()
                    && to < self.workspaces.len()
                    && to != self.active_workspace
                    && let Some(i) = self.idx(id)
                {
                    // Not closed: it lives on in the other workspace, so no reopen entry.
                    // It does leave the stage, so a split half ends the split here too.
                    let keep = if id == self.active { self.split.take() } else { None };
                    if self.split == Some(id) {
                        self.split = None;
                    }
                    if self.split.is_none() {
                        self.pane_right = false;
                    }
                    let t = self.tabs.remove(i);
                    let saved = t.saved();
                    self.agent_logs.borrow_mut().remove(&t.id);
                    self.prompts.retain(|p| p.1 != t.id);
                    if let Some(w) = self.watches.remove(&t.id) {
                        monitor::unwatch(None, w, true);
                    }
                    drop(t);
                    if let Some(ws) = self.workspaces.get_mut(to) {
                        ws.saved.push(saved);
                    }
                    if id == self.active {
                        if self.tabs.is_empty() {
                            self.new_tab(None, None);
                        } else {
                            // The other pane, when there was one, takes the stage.
                            let next = keep.filter(|k| self.idx(*k).is_some()).unwrap_or_else(|| self.tabs[i.min(self.tabs.len() - 1)].id);
                            self.active = 0;
                            self.activate(next);
                        }
                    } else {
                        self.layout();
                        self.dirty = true;
                        self.push();
                    }
                }
            }
            "glanceOpen" => self.promote_glance(),
            "glanceClose" => self.close_glance(),
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
                            // Resume on a held row is consent: it runs now, and
                            // any torrent it spawns inherits that consent.
                            // Pause takes a person-started one out of auto-start.
                            if a == "resume" {
                                self.aria_hold.remove(&gid);
                                self.aria_person.insert(gid.clone());
                            } else if a == "pause" {
                                self.aria_person.remove(&gid);
                            }
                            let aria = self.aria.clone();
                            let act = a.to_string();
                            self.aria_poll.store(true);
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
                            // A multi-file torrent has no single thing to open: show it in its folder.
                            let folder = a == "folder" || (a == "open" && d.kind == "torrent" && d.files > 1);
                            open_path(&d.path, folder);
                        }
                    }
                    "clear" => {
                        let gone: Vec<u64> = self.downloads.iter().filter(|d| d.state == "done" || d.state == "failed").map(|d| d.id).collect();
                        for id in gone {
                            self.remove_download(id);
                        }
                    }
                    "addUrl" => {
                        // The chrome read the clipboard and classified it; take it from there.
                        let url = m["url"].as_str().unwrap_or("").to_string();
                        if url.starts_with("magnet:") {
                            let private = self.active_tab().is_some_and(|t| t.private);
                            self.add_magnet(&url, private, None);
                        } else if url.starts_with("http") {
                            let private = self.active_tab().is_some_and(|t| t.private);
                            let (aria, proxy) = (self.aria.clone(), self.ctx.proxy.clone());
                            let u = url.clone();
                            let headers: Vec<String> = Vec::new();
                            std::thread::spawn(move || {
                                let _ = match aria.add(&u, &headers, None, false) {
                                    Ok(gid) => proxy.send_event(Ev::AriaAdded(Box::new(engine::Added {
                                        gid,
                                        url: u,
                                        headers,
                                        name: None,
                                        private,
                                        person: true,
                                        hold: None,
                                    }))),
                                    Err(_) => proxy.send_event(Ev::AriaFailed(0, u)),
                                };
                            });
                            self.aria_poll.store(true);
                        }
                    }
                    "addTorrent" => {
                        // The chrome opened the file dialog and read the file.
                        let b64 = m["b64"].as_str().unwrap_or("");
                        if b64.is_empty() {
                            return;
                        }
                        let private = self.active_tab().is_some_and(|t| t.private);
                        let b64 = b64.to_string();
                        let (aria, proxy) = (self.aria.clone(), self.ctx.proxy.clone());
                        std::thread::spawn(move || {
                            let _ = match aria.add_torrent(&b64, &[]) {
                                Ok(gid) => proxy.send_event(Ev::AriaAdded(Box::new(engine::Added {
                                    gid,
                                    url: String::new(),
                                    headers: Vec::new(),
                                    name: None,
                                    private,
                                    person: true,
                                    hold: None,
                                }))),
                                Err(_) => proxy.send_event(Ev::AriaFailed(0, String::new())),
                            };
                        });
                        self.aria_poll.store(true);
                    }
                    "openFolder" => open_path(&store::download_dir().to_string_lossy(), false),
                    "limit" => {
                        let kbps = m["kbps"].as_u64().unwrap_or(0) as u32;
                        self.aria.set_speed_limit(kbps as u64 * 1024);
                        let mut s = self.settings.clone();
                        s.download_limit_kbps = kbps;
                        self.apply_settings(s);
                    }
                    "stopSeed" => {
                        // Keep the files, drop the torrent out of aria2.
                        if let Some(gid) = self.aria_ids.iter().find(|(_, v)| **v == id).map(|(g, _)| g.clone()) {
                            self.aria_ids.remove(&gid);
                            self.aria_meta.remove(&gid);
                            self.aria_person.remove(&gid);
                            self.aria_page.remove(&gid);
                            self.aria_hold.remove(&gid);
                            let aria = self.aria.clone();
                            std::thread::spawn(move || {
                                let _ = aria.remove(&gid);
                            });
                        }
                        if let Some(d) = self.downloads.iter_mut().find(|d| d.id == id) {
                            d.state = "done";
                            d.speed = 0;
                            d.up = 0;
                        }
                        self.dl_dirty = true;
                        self.push();
                    }
                    "note" => {
                        // The firewall note is dismissed for good.
                        let mut s = self.settings.clone();
                        s.torrent_note = false;
                        self.apply_settings(s);
                    }
                    _ => {}
                }
            }
            "suggest" => {
                let q = m["q"].as_str().unwrap_or("").to_string();
                // The journal sees that history was searched, never what for.
                journal::history_search(&q);
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
            "listen" => self.listen_toggle(),
            "listenStop" => self.listen_stop(),
            "kdbxFill" => self.kdbx_fill(m["n"].as_u64().unwrap_or(0) as usize),
            "kdbxLock" => {
                self.vault = None;
                self.push();
            }
            "unbookmark" => {
                self.library.unbookmark(m["url"].as_str().unwrap_or(""));
                // Written now, not on a later tick: a bookmark is worth a write.
                self.library.flush();
                self.push();
            }
            "forget" => {
                self.library.forget(m["url"].as_str().unwrap_or(""));
                self.library.flush();
                self.recall.forget(m["url"].as_str().unwrap_or(""));
                self.push();
            }
            "clearHistory" => {
                // Seconds since the epoch; missing or null means everything.
                let since = m["since"].as_u64();
                self.library.clear_history(since);
                self.recall.clear(since);
                self.library.flush();
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
                self.library.flush();
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

    // ---------------------------------------------------------- translation

    /// F8, or "Translate page" in the tab menu: the page's text in the target
    /// language, or the page as it was.
    fn toggle_translate(&mut self, i: usize) {
        // Already translated: the original text is still in the page.
        if self.tabs[i].translated.take().is_some() {
            if let Some(v) = &self.tabs[i].view {
                let _ = v.evaluate_script(translate::RESTORE);
            }
            self.tabs[i].translating = false;
            self.push();
            return;
        }
        if self.tabs[i].translating || self.tr_busy.is_some() {
            return;
        }
        let url = self.tabs[i].url.clone();
        if !url.starts_with("http") || self.tabs[i].view.is_none() {
            return;
        }
        let id = self.tabs[i].id;
        let target = self.settings.translate_to.clone();
        self.tabs[i].translating = true;
        journal::feature("translate", self.tabs[i].private);
        self.tr_busy = Some(id);
        self.push();
        // Ask the page what language it is in and what its text is.
        let proxy = self.ctx.proxy.clone();
        if let Some(v) = &self.tabs[i].view {
            let _ = v.evaluate_script_with_callback(translate::COLLECT, move |out| {
                let _ = proxy.send_event(Ev::TrPage(id, target.clone(), out));
            });
        }
    }

    /// The page came back with its language and its text. Find the pair that
    /// translates it into what the person reads, and get its model.
    fn on_tr_page(&mut self, id: u64, target: &str, out: &str) {
        if self.tr_busy != Some(id) {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(out) else {
            self.stop_translating(id);
            return;
        };
        let texts = v.get("texts").cloned().unwrap_or(Value::Null);
        if !texts.as_array().is_some_and(|a| !a.is_empty()) {
            self.stop_translating(id);
            return;
        }
        let src = translate::page_lang(v.get("lang").and_then(|l| l.as_str()).unwrap_or(""));
        let Some(p) = translate::pair(&src, target) else {
            eprintln!("lumen: no translation model for {src} -> {target}");
            self.stop_translating(id);
            return;
        };
        self.tr_pair = Some(p);
        self.tr_texts = Some(texts.to_string());
        // 15-25 MB the first time a pair is asked for. The rest of the app keeps going.
        let proxy = self.ctx.proxy.clone();
        let note = format!("{} -> {}", p.src, p.dst);
        std::thread::Builder::new()
            .name("lumen-translate".into())
            .spawn(move || {
                let (ok, why) = match translate::fetch(p) {
                    Ok(()) => (true, String::new()),
                    Err(e) => (false, format!("{note}: {e}")),
                };
                let _ = proxy.send_event(Ev::TrModel(id, ok, why));
            })
            .ok();
    }

    /// The model is on disk (or is not). Open the translator page and let it load.
    fn on_tr_model(&mut self, id: u64, ok: bool, why: String) {
        if self.tr_busy != Some(id) {
            return;
        }
        if !ok {
            eprintln!("lumen: translation model: {why}");
            self.stop_translating(id);
            return;
        }
        let Some(p) = self.tr_pair else { return };
        if self.idx(id).is_none() {
            self.stop_translating(id);
            return;
        }
        match translate::Page::open(&self.ctx, p) {
            Some(page) => self.translator = Some(page),
            None => {
                eprintln!("lumen: the translator page would not open");
                self.stop_translating(id);
            }
        }
    }

    /// Word from the translator page: it is up, it is working, or it is done.
    fn on_tr_msg(&mut self, msg: &str) {
        let Ok(m) = serde_json::from_str::<Value>(msg) else { return };
        if m["lumen"] != "tr" {
            return;
        }
        if m["ready"] == 1 {
            // The page is up: hand it the text it is to work on.
            let texts = self.tr_texts.take();
            if let (Some(texts), Some(page)) = (texts, self.translator.as_ref()) {
                page.run(&texts);
            }
            return;
        }
        if let Some(err) = m["err"].as_str() {
            eprintln!("lumen: translation: {err}");
            if let Some(id) = self.tr_busy {
                self.stop_translating(id);
            }
            return;
        }
        if let Some(texts) = m["texts"].as_array() {
            let texts = Value::Array(texts.clone()).to_string();
            self.show_translation(&texts);
        }
    }

    /// The translations came back. Put them in the page; the page keeps the
    /// originals, so F8 goes back. The translator page goes away with them.
    fn show_translation(&mut self, texts_json: &str) {
        let Some(id) = self.tr_busy.take() else { return };
        // The wasm, the worker, the model in memory and a renderer: all of it goes.
        self.translator = None;
        self.tr_pair = None;
        self.tr_texts = None;
        let Some(i) = self.idx(id) else { return };
        self.tabs[i].translating = false;
        self.tabs[i].translated = Some(self.settings.translate_to.clone());
        if let Some(v) = &self.tabs[i].view {
            let _ = v.evaluate_script(&translate::apply(texts_json));
            // Test hook: say what the page reads now, and that going back works.
            if cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_TRANSLATE").is_some() {
                let _ = v.evaluate_script_with_callback("document.body.innerText.slice(0,200)", |out| {
                    eprintln!("lumen: translated page text: {out}");
                });
                let _ = v.evaluate_script_with_callback("(window.__lumenTrRestore(),document.body.innerText.slice(0,120))", |out| {
                    eprintln!("lumen: page text after restore: {out}");
                });
            }
        }
        self.push();
    }

    /// Give up on a translation that cannot be finished.
    fn stop_translating(&mut self, id: u64) {
        self.tr_busy = None;
        self.tr_pair = None;
        self.tr_texts = None;
        self.translator = None;
        if let Some(i) = self.idx(id) {
            self.tabs[i].translating = false;
        }
        self.push();
    }

    // ----------------------------------------------------------------- servo

    /// A host from the "Servo sites" list: this tab is meant to run in Servo. The
    /// list is exact hosts, with or without the "www.".
    #[cfg(feature = "servo")]
    fn wants_servo(&self, url: &str) -> bool {
        let host = host_of(url);
        if host.is_empty() {
            return false;
        }
        self.settings.servo_sites.iter().any(|s| {
            let s = s.trim().trim_start_matches("www.");
            !s.is_empty() && (host.eq_ignore_ascii_case(s) || host.eq_ignore_ascii_case(&format!("www.{s}")))
        })
    }

    /// Open a Servo page for this tab. Servo itself is made the first time one is
    /// asked for, so a Lumen that never opens one never pays for it.
    #[cfg(feature = "servo")]
    fn open_servo(&mut self, i: usize, url: &str) {
        let id = self.tabs[i].id;
        if self.servo.is_none() {
            self.servo = Some(servo_engine::Engine::new(self.ctx.proxy.clone()));
        }
        let parent = HWND(self.ctx.window.hwnd() as _);
        let proxy = self.ctx.proxy.clone();
        let page = self.servo.as_ref().and_then(|e| e.page(parent, id, url, proxy));
        match page {
            Some(page) => {
                let (x, y, w, h) = self.stage();
                page.set_bounds(x, y, w, h);
                self.tabs[i].servo = Some(page);
            }
            None => eprintln!("lumen: servo would not open {url}"),
        }
    }

    /// Put this tab on the engine its host asks for, dropping the other one's page.
    /// Nothing happens when it is already on the right one.
    #[cfg(feature = "servo")]
    fn place_on_engine(&mut self, i: usize, url: &str) {
        let wants = self.wants_servo(url);
        if wants == self.tabs[i].servo.is_some() {
            return;
        }
        if wants {
            // Servo's turn: the WebView2 page goes.
            self.tabs[i].view = None;
            self.tabs[i].visible = false;
            self.tabs[i].trimmed = false;
            self.tabs[i].suspended = false;
        } else {
            self.tabs[i].servo = None;
        }
    }

    // ---------------------------------------------------------------- tick

    /// The tick runs when something waits for it, at most once per TICK. The
    /// rate limit matters: a step whose time has come but whose turn has not
    /// must not wake the loop again on the same instant.
    fn tick_due(&self, now: Instant) -> bool {
        self.next_tick <= now && self.tick_waiting(now)
    }

    /// What wants a tick: pending saves, the agent chip, the RAM cap, a sleep
    /// step gone due, and aria2's boot (the tick is what notices it answered
    /// and turns accelerate_downloads on).
    fn tick_waiting(&self, now: Instant) -> bool {
        if self.dirty || self.dl_dirty || self.agent_seen.is_some() || self.settings.ram_limit {
            return true;
        }
        // The window not shown yet (the 3 s fallback), and a headless Lumen, which
        // must notice when agents have gone quiet and leave.
        if !self.shown || headless() {
            return true;
        }
        if self.settings.accelerate_downloads && !self.ctx.aria_on.get() && self.aria.ready() {
            return true;
        }
        self.next_tab_wake().is_some_and(|t| t <= now)
    }

    /// When the loop owes the next wake: a deadline it owns, the tick's next
    /// turn when something waits for it, IDLE_TICK when nothing does.
    fn next_wake(&self, now: Instant) -> Instant {
        let mut at = now + IDLE_TICK;
        // The memory read, which the journal's minute rides on. Idle means idle:
        // neither clock is armed when nothing is on screen and nothing is loading,
        // so the journal never wakes an idle Lumen.
        let watch = self.settings.show_memory || (journal::on() && self.journal_busy());
        let mem = watch.then_some(self.next_mem);
        for t in [self.next_sample, mem].into_iter().flatten() {
            if t < at {
                at = t;
            }
        }
        if self.tick_waiting(now) {
            if self.next_tick < at {
                at = self.next_tick;
            }
        } else if let Some(t) = self.next_tab_wake()
            && t < at
        {
            // Not due yet: arrive for it exactly.
            at = t;
        }
        at
    }

    /// The next sleep-policy step of any hidden tab, in the past if one is due:
    /// trim at TRIM_AFTER, freeze at sleep_after_min, discard at discard_after_min.
    /// A crashed page is already dead; a page playing audio is only ever trimmed.
    fn next_tab_wake(&self) -> Option<Instant> {
        let sleep = Duration::from_secs(self.settings.sleep_after_min as u64 * 60);
        let discard = Duration::from_secs(self.settings.discard_after_min as u64 * 60);
        self.tabs
            .iter()
            .filter(|t| t.has_page() && !t.crashed && !self.on_stage(t.id))
            .filter_map(|t| Some(t.hidden_at? + next_sleep_step_after(&t.sleep_state(), sleep, discard)?))
            .min()
    }

    /// The page a typed host led to has answered. A name that does not exist
    /// becomes a search for the words; anything else lets the typing go.
    fn typed_done(&mut self, id: u64, ok: bool, err: i32) {
        let Some(i) = self.idx(id) else { return };
        let Some(t) = &self.tabs[i].typed else { return };
        match typed_verdict(t.started, ok, err) {
            TypedVerdict::Wait => {}
            TypedVerdict::Forget => self.tabs[i].typed = None,
            TypedVerdict::Search => {
                let Some(t) = self.tabs[i].typed.take() else { return };
                let url = self.settings.search.replace("%s", &url_encode(&t.raw));
                journal::feature("typed_name_search", self.tabs[i].private);
                if let Some(v) = &self.tabs[i].view {
                    let _ = v.load_url(&url);
                }
                self.tabs[i].url = url;
                self.dirty = true;
                self.push();
            }
        }
    }

    /// A title reaches the chrome, the history and the session.
    fn show_title(&mut self, i: usize, t: String) {
        if !self.off_record(&self.tabs[i]) {
            self.library.set_title(&self.tabs[i].url, &t);
        }
        self.tabs[i].title = t;
        self.dirty = true;
        self.push();
    }

    /// Trim, freeze and discard hidden tabs, enforce the RAM cap, save what changed.
    fn tick(&mut self) {
        let now = Instant::now();
        // Started headless for agents, never shown to a person, no agent tab left, quiet for
        // ten minutes: nobody needs this Lumen. lumen-agent starts a new one when asked.
        if headless() && self.handoff.is_none() && !self.tabs.iter().any(|t| t.agent) && now - self.agent_last > agent_idle() {
            self.library.flush();
            self.clear_on_exit();
            self.save_session();
            self.upnp.release();
            // Leaving on purpose, so the journal ends with a clean goodbye.
            journal::stop(true, self.born.elapsed());
            std::process::exit(0);
        }
        self.next_tick = now + TICK;

        let sleep = Duration::from_secs(self.settings.sleep_after_min as u64 * 60);
        let discard = Duration::from_secs(self.settings.discard_after_min as u64 * 60);
        for i in 0..self.tabs.len() {
            if self.on_stage(self.tabs[i].id) {
                continue;
            }
            let t = &mut self.tabs[i];
            if !t.has_page() {
                continue;
            }
            let Some(at) = t.hidden_at else { continue };
            match due_sleep_step(&t.sleep_state(), now - at, sleep, discard) {
                Some(SleepStep::Discard) => {
                    self.discard(i);
                    self.push();
                }
                Some(SleepStep::Suspend) => {
                    if let Some(v) = &t.view {
                        engine::try_suspend(v, t.id, self.ctx.proxy.clone());
                    }
                }
                Some(SleepStep::Trim) => {
                    if let Some(v) = &t.view {
                        let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
                    }
                    t.trimmed = true;
                }
                None => {}
            }
        }

        // The sidebar memory readout has its own clock in the event loop.
        self.enforce_ram(now);
        self.settle_clear();
        self.ctx.aria_on.set(self.settings.accelerate_downloads && self.aria.ready());
        self.library.flush();
        if self.agent_seen.is_some_and(|t| now - t > Duration::from_secs(6)) {
            self.agent_seen = None;
            self.push();
        }

        // KeePass: ten minutes untouched and the database leaves memory.
        if self.vault.is_some() && now - self.vault_seen > Duration::from_secs(600) {
            self.vault = None;
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
        // Debug test hook: add one magnet once aria2 is up, cancel it on schedule.
        if cfg!(debug_assertions) {
            // Translate the first page that has settled, once.
            if self.test_translate && self.tr_busy.is_none() {
                let settled = self
                    .tabs
                    .iter()
                    .position(|t| t.url.starts_with("http") && !t.loading && t.view.is_some() && t.translated.is_none());
                if let Some(i) = settled {
                    self.test_translate = false;
                    self.toggle_translate(i);
                }
            }
            if self.test_magnet && self.aria.ready() && !self.test_pending && self.test_dl.is_none() {
                self.test_magnet = false;
                self.test_pending = true;
                const MAGNET: &str = "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=Big+Buck+Bunny&tr=udp%3A%2F%2Fexplodie.org%3A6969&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=wss%3A%2F%2Ftracker.webtorrent.dev";
                self.add_magnet(MAGNET, false, None);
            }
            if let Some((gid, _)) = self.test_dl.take_if(|(_, at)| now >= *at) {
                if let Some(&id) = self.aria_ids.get(&gid) {
                    self.remove_download(id);
                }
            }
        }
    }

    /// Something is on screen, or on its way: the journal works only then. Idle
    /// (hidden or minimized, nothing loading) means it does nothing at all:
    /// no sample, no write to disk, no wake of its own.
    fn journal_busy(&self) -> bool {
        self.tabs.iter().any(|t| t.loading) || (self.shown && !self.minimized)
    }

    /// Whether the memory clock is worth arming: the sidebar shows it, or the
    /// journal needs the numbers behind it.
    fn watches_memory(&self) -> bool {
        self.settings.show_memory || (journal::on() && self.journal_busy())
    }

    /// The blocker's running total, for a page's share of it. None when blocking
    /// is off: there is no count to speak of then.
    fn blocked_count(&self) -> Option<u64> {
        self.settings.adblock.then(|| self.blocker.blocked())
    }

    /// What the journal needs to know about the tabs right now.
    fn journal_sample(&self) -> journal::Sample {
        let count = |want: [&'static str; 1]| self.tabs.iter().filter(|t| want.contains(&t.status())).count();
        journal::Sample {
            tabs: self.tabs.len(),
            live: count(["live"]),
            asleep: count(["asleep"]),
            // A tab with no page left (unloaded, or waiting to load) is discarded.
            discarded: count(["off"]),
            workspaces: self.workspaces.len(),
            panel: self.panel.name(),
        }
    }

    /// The sidebar memory readout, on its own slow clock. Reading it walks every
    /// WebView2 process, so it does not run on the tick. The journal's minute
    /// rides on the same walk: one enumeration, two readers, and nothing at all
    /// while the window is hidden or minimized and no page is loading.
    fn read_memory(&mut self, now: Instant) {
        self.next_mem = now + MEM_TICK;
        let journal_work = journal::on() && self.journal_busy();
        let env = self.ctx.env.borrow();
        let Some(env) = env.as_ref() else { return };
        let (own, total, procs, cpu) = engine::memory_with_cpu(env, journal_work);
        if journal_work {
            // The 60 s line, on the minute, and the write to disk behind it.
            journal::mem_read(own, total, cpu);
            if now >= self.journal_at {
                self.journal_at = now + journal::period();
                journal::sample(&self.journal_sample());
            }
            journal::flush_due();
        }
        // Only redraw when it moved by more than 1 MB.
        if self.settings.show_memory && self.mem.is_none_or(|(b, _)| b.abs_diff(total) > 1 << 20) {
            self.mem = Some((total, procs));
            self.ui_dirty = true;
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
        // One line per session, not per call. No tab, no page, no content.
        journal::agent_session(&who.session, &who.name);
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
            .filter(|(_, t)| !self.on_stage(t.id) && t.has_page() && !t.pinned && !t.audio)
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
        let saved: Vec<SavedTab> = keep
            .iter()
            .map(|t| t.saved())
            .collect();
        let active_tab_idx = keep.iter().position(|t| t.id == self.active).unwrap_or(0);
        // to_session stamps the live tabs into the active workspace's slot.
        let s = workspaces::to_session(
            &self.workspaces,
            self.active_workspace,
            &saved,
            active_tab_idx,
            if maximized || self.fullscreen() { 0.0 } else { width },
            if maximized || self.fullscreen() { 0.0 } else { height },
            maximized,
        );
        // Remember it for the panic hook: a panic must not lose these tabs.
        if let Ok(mut g) = LAST_SESSION.lock() {
            *g = Some(s.clone());
        }
        store::save_session(&s);
    }
}

#[cfg(test)]
mod tests {
    use super::{SleepState, SleepStep, TRIM_AFTER, Tab, classify_input, due_sleep_step, next_sleep_step_after, keepable, magnet_hold, magnet_name, resolve_input, same_page, should_auto_resume, typed_verdict};

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

    /// The dn= display name comes out of a magnet link, percent-decoded.
    #[test]
    fn magnet_name_reads_dn() {
        let m = "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=Big+Buck+Bunny&tr=udp%3A%2F%2Ftracker.example%3A1337";
        assert_eq!(magnet_name(m), "Big Buck Bunny");
        // No dn= at all: empty, not a crash.
        assert_eq!(magnet_name("magnet:?xt=urn:btih:abc"), "");
        assert_eq!(magnet_name("https://example.com/"), "");
    }

    /// A page-started magnet's hold note names the page that asked for it.
    #[test]
    fn magnet_hold_names_the_page() {
        assert_eq!(magnet_hold("https://example.com/files?x=1"), "from example.com: press Resume to start");
        // No host to name: still says a page asked.
        assert_eq!(magnet_hold("about:blank"), "from a page: press Resume to start");
    }

    /// Only a person-started, unfinished, known-name torrent gets auto-started.
    #[test]
    fn only_person_torrents_auto_resume() {
        // The rule fires for a person's torrent that pause-metadata stopped.
        assert!(should_auto_resume(true, "paused", true, true));
        // A page-started one waits for Resume.
        assert!(!should_auto_resume(false, "paused", true, true));
        // Still fetching metadata: nothing to resume into yet.
        assert!(!should_auto_resume(true, "paused", false, true));
        // A finished torrent paused by aria2's seed limit is left alone.
        assert!(!should_auto_resume(true, "paused", true, false));
        // Running torrents need nothing.
        assert!(!should_auto_resume(true, "active", true, true));
    }

    /// A magnet typed in the address bar reaches the main loop, not the renderer.
    #[test]
    fn resolve_input_passes_magnets_through() {
        let m = "magnet:?xt=urn:btih:abc&dn=x";
        assert_eq!(resolve_input(m, "https://duckduckgo.com/?q=%s").as_deref(), Some(m));
        // And an ordinary https URL is still just itself.
        assert_eq!(resolve_input("https://a.test/", "x").as_deref(), Some("https://a.test/"));
    }

    const DDG: &str = "https://duckduckgo.com/?q=%s";

    /// What the address bar does with a word: go there, or search for it.
    #[test]
    fn address_bar_classifies_typing() {
        let url = |s: &str| classify_input(s, DDG).unwrap();
        // Not a real TLD: a search, not a dead page.
        assert_eq!(url("youtube.music"), ("https://duckduckgo.com/?q=youtube.music".into(), false));
        assert_eq!(url("index.html").0, "https://duckduckgo.com/?q=index.html");
        assert_eq!(url("rust lang"), ("https://duckduckgo.com/?q=rust+lang".into(), false));
        // Places.
        assert_eq!(url("music.youtube.com"), ("https://music.youtube.com".into(), true));
        assert_eq!(url("localhost:3000"), ("http://localhost:3000".into(), true));
        assert_eq!(url("192.168.1.1"), ("http://192.168.1.1".into(), true));
        assert_eq!(url("example.co.uk"), ("https://example.co.uk".into(), true));
        assert_eq!(url("github.com/rust-lang/rust").0, "https://github.com/rust-lang/rust");
        // A scheme says it all, even for a name that is not a TLD.
        assert_eq!(url("https://youtube.music"), ("https://youtube.music".into(), false));
        assert_eq!(classify_input("  ", DDG), None);
    }

    /// A typed host that does not exist becomes a search, once, and only if it
    /// was the page that was loading.
    #[test]
    fn typed_host_falls_back_to_search_only_for_missing_names() {
        use crate::TypedVerdict::*;
        assert_eq!(typed_verdict(true, false, 13), Search);
        // Not started: an older page's ending is not the answer.
        assert_eq!(typed_verdict(false, false, 13), Wait);
        // Cut short by another load: keep waiting for the real one.
        assert_eq!(typed_verdict(true, false, 14), Wait);
        // Loaded, or failed in a way a search would not mend.
        assert_eq!(typed_verdict(true, true, 0), Forget);
        assert_eq!(typed_verdict(true, false, 9), Forget);
        assert_eq!(typed_verdict(true, false, 6), Forget);
        assert!(same_page("https://a.com", "https://A.com/"));
        assert!(!same_page("https://a.com", "https://b.com/"));
    }

    fn st(audio: bool, trimmed: bool, suspended: bool, pinned: bool) -> SleepState {
        SleepState { audio, trimmed, suspended, pinned }
    }

    /// A tab playing audio is trimmed, never frozen and never dropped, however
    /// long it has been hidden.
    #[test]
    fn sleep_policy_never_freezes_or_drops_a_playing_tab() {
        use std::time::Duration;
        let (sleep, discard) = (Duration::from_secs(300), Duration::from_secs(1800));
        let hours = Duration::from_secs(5 * 3600);
        // Playing and not yet trimmed: trim once, after the usual wait, and not before.
        assert_eq!(due_sleep_step(&st(true, false, false, false), Duration::from_secs(1), sleep, discard), None);
        assert_eq!(due_sleep_step(&st(true, false, false, false), TRIM_AFTER, sleep, discard), Some(SleepStep::Trim));
        // An hour in, trimmed, still playing: nothing, ever.
        assert_eq!(due_sleep_step(&st(true, true, false, false), hours, sleep, discard), None);
        // Even straight from untrimmed after a very long hide: trim, not discard.
        assert_eq!(due_sleep_step(&st(true, false, false, false), hours, sleep, discard), Some(SleepStep::Trim));
        assert_eq!(next_sleep_step_after(&st(true, true, false, false), sleep, discard), None);
        assert_eq!(next_sleep_step_after(&st(true, false, false, false), sleep, discard), Some(TRIM_AFTER));
    }

    /// Silent tabs keep the policy they always had; a tab whose audio just
    /// stopped is silent, so its steps come back.
    #[test]
    fn sleep_policy_for_silent_tabs_is_unchanged() {
        use std::time::Duration;
        let (sleep, discard) = (Duration::from_secs(300), Duration::from_secs(1800));
        let s = |x| Duration::from_secs(x);
        assert_eq!(due_sleep_step(&st(false, false, false, false), s(10), sleep, discard), None);
        assert_eq!(due_sleep_step(&st(false, false, false, false), s(31), sleep, discard), Some(SleepStep::Trim));
        assert_eq!(due_sleep_step(&st(false, true, false, false), s(301), sleep, discard), Some(SleepStep::Suspend));
        assert_eq!(due_sleep_step(&st(false, true, true, false), s(301), sleep, discard), None);
        assert_eq!(due_sleep_step(&st(false, true, true, false), s(1801), sleep, discard), Some(SleepStep::Discard));
        // Pinned tabs freeze but are never dropped.
        assert_eq!(due_sleep_step(&st(false, true, true, true), s(99999), sleep, discard), None);
        assert_eq!(next_sleep_step_after(&st(false, true, true, true), sleep, discard), None);
        assert_eq!(next_sleep_step_after(&st(false, true, false, false), sleep, discard), Some(sleep));
        assert_eq!(next_sleep_step_after(&st(false, true, true, false), sleep, discard), Some(discard));
    }
}
