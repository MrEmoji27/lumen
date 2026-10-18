#![windows_subsystem = "windows"]

//! Lumen. A small browser shell.
//! One window. A slim UI page for the sidebar. Pages beside it.
//! Rust decides who lives, who sleeps, and when the UI hears about it.

mod address;
mod aria;
mod agent;
mod agent_policy;
mod agentkit;
mod agent_result;
mod agent_tasks;
mod agent_consent;
mod aloud;
mod backdrop;
mod blocker;
mod bookmarks;
mod bugreport;
mod changelog;
mod chrome;
mod defaultapp;
mod engine;
mod extlink;
mod favicons;
mod glance;
mod journal;
mod kdbx;
mod launch;
mod library;
mod monitor;
mod mru;
mod pacing;
mod pagekit;
mod privacy_stats;
mod pins;
mod recall;
#[cfg(feature = "servo")]
mod servo_engine;
mod split;
mod stage;
mod watch;
mod translate;
mod upnp;
mod video;
mod workspaces;
mod store;
mod sites;
mod tabsearch;
mod sitefavs;
mod speeddial;
mod sponsorblock;
mod trackers;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
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
const STAGE_IDLE: Duration = Duration::from_secs(30);
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
    Drive,
    Report,
    Dashboard,
    /// What each site may do, and the zoom and http choices kept per site.
    Sites,
    /// What's new: the changelog built into the exe.
    Changes,
    /// Report a problem: a plain-text report to read, copy or save.
    Bug,
}

impl Panel {
    /// Pages that show live numbers: the process sampling runs only while one is open.
    fn live(self) -> bool {
        matches!(self, Panel::Control | Panel::Dashboard)
    }

    fn name(self) -> &'static str {
        match self {
            Panel::None => "none",
            Panel::Settings => "settings",
            Panel::Control => "control",
            Panel::Downloads => "downloads",
            Panel::History => "history",
            Panel::Bookmarks => "bookmarks",
            Panel::Drive => "drive",
            Panel::Report => "report",
            Panel::Dashboard => "dashboard",
            Panel::Sites => "sites",
            Panel::Changes => "changes",
            Panel::Bug => "bugreport",
        }
    }
}

/// One line of a Drive CLI's output, without the colors a terminal would read.
fn clean_line(line: &str) -> String {
    let mut out = String::new();
    let mut in_escape = false;
    for c in line.chars() {
        if c == '\u{1b}' {
            in_escape = true;
            continue;
        }
        if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
            continue;
        }
        if !c.is_control() || c == '\t' {
            out.push(c);
        }
    }
    out.trim_end().to_string()
}

/// One of a Drive CLI's pipes, read a line at a time into the run's log. The flag says
/// the pipe is done, which is how the run notices its CLI has ended.
fn drive_pipe(pipe: impl std::io::Read + Send + 'static, out: Arc<Mutex<std::collections::VecDeque<String>>>, done: Arc<AtomicBool>) {
    let reader = std::io::BufReader::new(pipe);
    for line in std::io::BufRead::lines(reader).map_while(Result::ok) {
        let line = clean_line(&line);
        if line.is_empty() {
            continue;
        }
        let mut q = out.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= 400 {
            q.pop_front();
        }
        q.push_back(line);
    }
    done.store(true, Ordering::Relaxed);
}

/// One Drive run: the agent CLI Lumen started with the person's task, its output, and
/// where the run is. Every run is its own MCP session, in its own profile.
struct Drive {
    /// The name this run's session keeps (`drive-3`): the CLI's label, its profile and
    /// the folder its downloads land in.
    name: String,
    task: String,
    child: std::process::Child,
    /// What the CLI printed, newest last: the agent's own messages.
    out: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
    /// The reader threads are done: the CLI has ended.
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
    started: Instant,
    /// The MCP session, once the CLI has opened one.
    session: Option<String>,
    /// The person took the tab over: only browser_handoff gets the agent back in.
    paused: bool,
    exit: Option<i32>,
}

impl Drive {
    /// How long the run has been going, for the panel's title line.
    fn ago(&self) -> String {
        let s = self.started.elapsed().as_secs();
        if s < 60 { format!("{s}s") } else if s < 3600 { format!("{}m", s / 60) } else { format!("{}h {:02}m", s / 3600, (s % 3600) / 60) }
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
    /// For agent tabs: the profile key its own WebView2 profile is named after
    /// (`agent-<profile>`), so its cookies and storage never meet another agent's.
    profile: String,
    /// For agent tabs: its own folder under "Agent downloads".
    downloads: String,
    /// The session asked for strict isolation: its own environment, its own browser
    /// process. Cleared if WebView2 will not make one, so the tab still opens.
    own_env: bool,
    /// For a tab of the person's: the site (scheme://host) they said agents may use it on.
    /// Ends with a move to another site and with the tab.
    agent_ok: Option<String>,
    view: Option<WebView>,
    /// A Servo page, for the hosts in the "Servo sites" list. The other engine.
    #[cfg(feature = "servo")]
    servo: Option<servo_engine::Page>,
    hidden_at: Option<Instant>,
    /// Last navigation result: (ok, WebView2 error code, HTTP status).
    last_nav: Option<(bool, i32, i32)>,
    /// An agent tab's stage view, put to sleep by the idle clock: not painting.
    stage_idle: bool,
    /// A navigation is on its way: since an agent asked for one, or since the page began
    /// one (a click, a form). Ends at NavigationCompleted. Agents wait on this, not on a clock.
    nav_pending: Option<Instant>,
    /// The pending navigation has really started (NavigationStarting came).
    nav_started: bool,
    /// Its page window is on screen right now.
    visible: bool,
    /// Typed in the address bar as a bare host and not yet answered. See `Typed`.
    typed: Option<Typed>,
    /// The zoom the page is at, in percent.
    zoom: u32,
    /// A zoom Lumen just set itself: the change event that follows is not the person's.
    zoom_set: Option<u32>,
    /// Showing the "this site does not use https" page for this http address.
    no_https: Option<String>,
    /// Showing the danger warning for this address. Same shape as `no_https`.
    no_danger: Option<String>,
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
            profile: String::new(),
            downloads: String::new(),
            own_env: false,
            agent_ok: None,
            view: None,
            #[cfg(feature = "servo")]
            servo: None,
            hidden_at: None,
            last_nav: None,
            stage_idle: false,
            nav_pending: None,
            nav_started: false,
            visible: false,
            typed: None,
            zoom: 100,
            zoom_set: None,
            no_https: None,
            no_danger: None,
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
    /// Tab ids newest-first: where the person was, so closing a tab goes back
    /// there instead of to the neighbour in the row. See mru.rs.
    mru: mru::Mru,
    closed: Vec<SavedTab>,
    settings: Settings,
    panel: Panel,
    /// The report page's range tile: "today" | "7" | "30" | "all".
    report_range: String,
    /// The site whose row was clicked on the report page.
    report_site: Option<String>,
    peek: bool,
    page_fs: bool,
    user_fs: bool,
    ui_ready: bool,
    /// Something changed. The UI hears once per frame, not once per event.
    ui_dirty: bool,
    /// Favicons are big. They travel alone, only when they change.
    fav_queue: Vec<u64>,
    /// One favicon per site, for the speed dial's tiles. Kept in favicons.json.
    site_favs: sitefavs::Favs,
    site_favs_dirty: bool,
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
    /// The problem report, made when its page opens.
    bug_report: Option<String>,
    /// "problem", or "crash" when the page was opened from the start page's crash note.
    bug_kind: &'static str,
    /// This report has gone: Send does not send it twice.
    bug_sent: bool,
    /// The run before this one never said goodbye: the start page offers a report, once.
    unclean_last: bool,
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
    /// Off-screen stage window hosting agent tabs' views. Made with the first agent tab.
    stage: Option<stage::Stage>,
    /// The shield, the agent's cursor and the pill over a watched agent tab. Made with
    /// the first watch.
    watch: Option<watch::Watch>,
    /// Agent sessions whose tab the person took over from the watch pill: the shield is
    /// off and the agent may only ask for the tab back until the person gives it back.
    taken: std::collections::HashSet<String>,
    /// Modifier keys as the main window last saw them.
    mods: ModifiersState,
    /// Remembered permission answers, and requests waiting for the user.
    rules: store::Rules,
    /// The zoom each site is read at, in percent. Only sites that differ from the default.
    zooms: sites::Zooms,
    /// Hosts that answered on the https port this run, so the probe is made once.
    probed: HashSet<String>,
    /// How many cookies the active tab's site has: (tab, count), asked for when the lock opens.
    site_cookies: Option<(u64, usize)>,
    prompts: Vec<(u64, u64, String, &'static str)>,
    /// Which "open this app?" questions are on screen or were just refused.
    ext_gate: extlink::Gate,
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
    /// A consent the agent asked the person for: (tab, agent name, kind, message, reply).
    consent: Option<(u64, String, String, String, std::sync::mpsc::Sender<Result<Value, String>>)>,
    /// URLs agents asked to download, and the session folder each belongs to: they land
    /// in "Agent downloads\<folder>" and show in browser_downloads of that agent only.
    agent_dl: HashMap<String, String>,
    /// Strict-isolation agent tabs waiting for their browser to be made.
    pending_own: Vec<u64>,
    /// The Drive panel's run, if there is one.
    drive: Option<Drive>,
    /// How many Drive runs this Lumen has started: run N is `drive-N`.
    drive_n: u32,
    /// When the run next wants a look (its output, its session, its end).
    next_drive: Instant,
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
    /// The sidebar's width right now, and where it is gliding to (compact, peek,
    /// or the person's own width). `from, to, when it started`.
    side_w: f64,
    side_anim: Option<(f64, f64, Instant)>,
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
/// A later start that was given addresses leaves them in a note first (see `launch.rs`), and
/// the first one opens them when it wakes. A headless later start never brings a window up.
fn claim_instance(opens: &[launch::Open]) -> Option<windows::Win32::Foundation::HANDLE> {
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
            // Addresses always travel, and only a normal start asks for the window.
            let sent = !opens.is_empty() && launch::forward(opens, !headless());
            if !headless() || sent {
                if !headless() {
                    let _ = AllowSetForegroundWindow(ASFW_ANY);
                }
                let _ = SetEvent(ev);
            }
            return None;
        }
        Some(ev)
    }
}

/// `lumen.exe --register`: write the entries, tell the person what is left to do.
fn register_from_cli() -> String {
    let Some(reg) = defaultapp::Reg::current() else { return "Could not find the Lumen exe.".into() };
    match reg.ensure() {
        Ok(_) => "Lumen is registered as a browser. To finish, pick it in Settings > Apps > Default apps: Windows only lets you do that part.".into(),
        Err(e) => e,
    }
}

/// A windowless program has no stdout. Borrow the console of whoever started it, if any.
fn say_on_console(msg: &str) {
    use std::io::Write;
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
    }
    if let Ok(mut out) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        let _ = writeln!(out, "{msg}");
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

/// Which Lumen this is: the version, then the commit it was built from and its date.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("LUMEN_BUILD"), ")");

fn main() {
    // `lumen.exe --version`: say which build this is, then done. No window.
    if std::env::args().any(|a| a == "--version") {
        say_on_console(&format!("Lumen {VERSION}"));
        return;
    }
    HEADLESS.store(
        std::env::var_os("LUMEN_HEADLESS").is_some_and(|v| v != "0") || std::env::args().any(|a| a == "--headless"),
        std::sync::atomic::Ordering::Relaxed,
    );
    let cli = launch::from_process();
    // `lumen.exe --register` / `--unregister`: the default-browser entries, then done. No window.
    if cli.register || cli.unregister {
        let said = if cli.unregister { defaultapp::unregister() } else { register_from_cli() };
        say_on_console(&said);
        return;
    }
    let Some(instance) = claim_instance(&cli.opens) else { return };
    launch::sweep();
    defaultapp::startup_check();
    install_panic_hook();
    // Before WebView2 starts: its processes must be born inside the job.
    let job = monitor::Job::adopt_self();
    let mut settings = store::load_settings();
    // (Debug shots of the picker keep it with LUMEN_TEST_FIRSTRUN.)
    if headless() && !(cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_FIRSTRUN").is_some()) {
        settings.first_run_done = true;
    }
    let session = store::load_session();
    // Agent profiles whose sessions ended while Lumen was running: their folders are
    // deleted now, before WebView2 touches any profile.
    engine::purge_at_start();
    // The journal opens before anything else runs, so a run that never gets a
    // window up still says hello. Also the one place old journals are cleaned.
    let unclean = journal::start(&settings, restored_tabs(&session));

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
        own_envs: RefCell::new(HashMap::new()),
        pending_envs: RefCell::new(Vec::new()),
        own_env_failed: RefCell::new(HashSet::new()),
        profiles: RefCell::new(HashMap::new()),
        https: Rc::new(RefCell::new(https_policy(&settings))),
        gpc: Cell::new(settings.gpc),
        popup_info: Rc::new(RefCell::new(std::collections::VecDeque::new())),
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
    privacy_stats::set_enabled(settings.privacy_stats);
    blocker.set_gpc(settings.gpc);
    {
        let b = blocker.clone();
        ctx.rewrite.replace(Some(Rc::new(move |u: &str, who| b.rewrite(u, who))));
        let b = blocker.clone();
        ctx.on_built.replace(Some(Box::new(move |wv: &WebView, who| b.attach(wv, who))));
    }

    // The browser chrome: a native child window. No webview until a page needs one.
    let chrome = chrome::Chrome::new(HWND(ctx.window.hwnd() as _), ctx.proxy.clone());
    // The split divider: a strip of its own between the two panes.
    let divider = split::Divider::new(HWND(ctx.window.hwnd() as _), ctx.proxy.clone());

let bg = if dark { (24, 24, 26) } else { (243, 243, 244) };
    // Read before the struct literal moves `settings` into the app.
    let keep_seeding = settings.keep_seeding;
    let dl_limit = settings.download_limit_kbps;
    // The sidebar starts at its width; folding and peeking glide from here.
    let side_w = if settings.compact { COMPACT_W } else { settings.sidebar_width as f64 };
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
        mru: mru::Mru::new(),
        closed: Vec::new(),
        settings,
        panel: Panel::None,
        report_range: "today".into(),
        report_site: None,
        peek: false,
        page_fs: false,
        user_fs: false,
        ui_ready: true,
        ui_dirty: false,
        fav_queue: Vec::new(),
        site_favs: sitefavs::load(),
        site_favs_dirty: false,
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
        bug_report: None,
        bug_kind: "problem",
        bug_sent: false,
        unclean_last: false,
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
        stage: None,
        watch: None,
        taken: Default::default(),
        mods: ModifiersState::empty(),
        rules: store::load_rules(),
        zooms: store::load_zooms(),
        probed: HashSet::new(),
        site_cookies: None,
        prompts: Vec::new(),
        ext_gate: Default::default(),
        library: library::Library::load(&store::data_dir()),
        aria: aria::Aria::start(&store::data_dir(), &store::download_dir(), keep_seeding, dl_limit, bt_port),
        aria_ids: HashMap::new(),
        aria_adopted: false,
        aria_poll: aria::Poll::new(),
        upnp,
        recall: recall::Recall::start(&store::data_dir()),
        agent_logs: Default::default(),
        handoff: None,
        consent: None,
        agent_dl: HashMap::new(),
        pending_own: Vec::new(),
        drive: None,
        drive_n: 0,
        next_drive: Instant::now(),
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
        side_w,
        side_anim: None,
        spb: HashMap::new(),
        vault: None,
        vault_seen: Instant::now(),
    };
    app.unclean_last = unclean;
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
    // Test hook: LUMEN_TEST_AGENT_OFF=1 opens the door and shuts it again, the state of a Lumen
    // whose switch was turned off while it ran: it listens and answers 503 to everything.
    if cfg!(debug_assertions) && app.settings.agent && std::env::var_os("LUMEN_TEST_AGENT_OFF").is_some() {
        app.agent.set(false, app.settings.agent_port, &app.ctx.proxy);
    }
    // The last run closed with "clear history on exit" and could not finish wiping
    // WebView2's own history. It happens as soon as there is a page to ask.
    app.clear_pending = store::clear_history_pending();
    app.apply_backdrop();
    // Test hook for the backdrop measurement (docs/briefs/ui-backdrop.md):
    // LUMEN_TEST_BACKDROP=acrylic|mica puts Windows' own frost on the window at
    // start, so a headless run can be measured against one started without it.
    if cfg!(debug_assertions)
        && let Ok(v) = std::env::var("LUMEN_TEST_BACKDROP")
    {
        let b = match v.to_ascii_lowercase().as_str() {
            "mica" => Some(backdrop::Backdrop::Mica),
            "acrylic" => Some(backdrop::Backdrop::Acrylic),
            "none" => Some(backdrop::Backdrop::None),
            _ => None,
        };
        if let Some(b) = b {
            let took = backdrop::set(app.ctx.window.hwnd(), b, dark);
            eprintln!("lumen: backdrop {b:?} dark={dark} took={took}");
        }
    }
    app.apply_limits(None);
    app.refresh_dark();
    app.load_downloads();
    app.restore(session);
    // Addresses on the command line open on top of the restored tabs.
    for o in &cli.opens {
        app.open_launched(o);
    }
    app.flush();
    app.shown = true;
    show_window(&app.ctx.window);
    app.layout();
    if !headless() {
        app.focus_page();
    }
    // Test hooks for the chrome screenshots (LUMEN_TEST_SHOT, see chrome/shot.rs):
    // LUMEN_TEST_COMPACT=1 starts with the icon-only sidebar, LUMEN_TEST_PANEL=
    // downloads|control|history|bookmarks|settings|report opens that page.
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
            Ok("drive") => Panel::Drive,
            Ok("report") => Panel::Report,
            Ok("dashboard") => Panel::Dashboard,
            Ok("sites") => Panel::Sites,
            Ok("changes") => Panel::Changes,
            Ok("bugreport") => Panel::Bug,
            _ => Panel::None,
        };
        if panel != Panel::None || app.settings.compact {
            app.set_panel(panel);
        }
        // LUMEN_TEST_SCRIPT: timed key/click/shot steps, see chrome/shot.rs.
        #[cfg(debug_assertions)]
        chrome::test_script(app.ctx.proxy.clone(), app.chrome.hwnd);
    }
    // Test hook: LUMEN_TEST_EXT="spotify:track:1|spotify:track:2" makes the active page try to
    // open those apps, as if clicked, from LUMEN_TEST_EXT_ORIGIN (default open.spotify.com).
    if cfg!(debug_assertions)
        && let Ok(list) = std::env::var("LUMEN_TEST_EXT")
    {
        let origin = std::env::var("LUMEN_TEST_EXT_ORIGIN").unwrap_or_else(|_| "https://open.spotify.com".into());
        for uri in list.split('|') {
            let _ = app.ctx.proxy.send_event(Ev::ExternalUri { tab: app.active, origin: origin.clone(), uri: uri.to_string(), user: true });
        }
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
                // The sidebar's width glides: a step is what wakes for it.
                if app.side_anim.is_some() {
                    app.layout();
                }
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
                WindowEvent::Moved(_) => app.sync_watch(),
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
                WindowEvent::Focused(true) => {
                    app.focus_page();
                    // Back from Windows' Default apps page: the Settings page re-reads the pick.
                    if app.panel == Panel::Settings {
                        app.push();
                    }
                }
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
            Event::MainEventsCleared => {
                app.flush();
                // Whatever the blocker counted reaches the disk at most a minute
                // apart, and only when the loop had nothing left to do.
                privacy_stats::flush_if_due();
            }
            _ => {}
        }
        if app.exit {
            app.library.flush();
            app.clear_on_exit();
            app.save_session();
            app.upnp.release();
            // The journal's goodbye: a run with this line is a run that ended.
            journal::stop(true, app.born.elapsed());
            privacy_stats::flush();
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

/// The http-to-https rules the navigation hook reads, from the settings.
fn https_policy(s: &Settings) -> sites::HttpsPolicy {
    sites::HttpsPolicy { on: s.https_first, exceptions: s.http_ok.iter().cloned().collect(), passes: HashSet::new() }
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
fn ui_mark(b: library::Bookmark) -> chrome::UiMark {
    chrome::UiMark { id: b.id, url: b.url, title: b.title, folder: b.folder }
}

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

    /// The sidebar's width right now: folded, peeked open, or the person's own.
    /// The value glides (Compact, Ctrl+B, a peek) instead of jumping.
    fn sidebar_w(&self) -> f64 {
        self.side_w
    }

    /// Where the sidebar is headed.
    fn side_target(&self) -> f64 {
        if self.settings.compact && !self.peek { COMPACT_W } else { self.settings.sidebar_width as f64 }
    }

    /// Step the sidebar's width toward its target. Returns whether it is still
    /// moving, so the loop can keep waking for it and stop when it lands. Motion
    /// off (or reduced) means it lands at once.
    fn step_side(&mut self, now: Instant) -> bool {
        const SIDE_MS: f64 = 0.16;
        let target = self.side_target();
        if (target - self.side_w).abs() < 0.5 {
            self.side_w = target;
            self.side_anim = None;
            return false;
        }
        if !self.chrome.look().motion.enabled {
            // Motion off (or the system asking for less): it lands at once.
            self.side_w = target;
            self.side_anim = None;
            return false;
        }
        if self.side_anim.as_ref().is_none_or(|(_, to, _)| (*to - target).abs() > 0.01) {
            self.side_anim = Some((self.side_w, target, now));
        }
        let (from, to, at) = self.side_anim.unwrap_or((self.side_w, target, now));
        let t = ((now - at).as_secs_f64() / SIDE_MS).clamp(0.0, 1.0);
        self.side_w = from + (to - from) * chrome::ease_out(t as f32) as f64;
        if t >= 1.0 {
            self.side_w = to;
            self.side_anim = None;
            false
        } else {
            true
        }
    }

    /// The chrome's top rows: the tab strip, toolbar and bookmarks bar, for the
    /// layouts that have them. Zero for the vertical sidebar.
    fn top_h(&self) -> f64 {
        chrome::Metrics::new(self.layout_kind(), self.settings.compact, self.settings.bm_bar).top() as f64
    }

    fn layout_kind(&self) -> chrome::Kind {
        chrome::Kind::of(&self.settings.layout)
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
        let right = self.settings.sidebar_side == "right";
        let (x, y, sw, sh) = chrome::layout::stage(
            self.layout_kind(),
            w as f32,
            h as f32,
            side as f32,
            right,
            self.top_h() as f32,
            if self.settings.look == "glass" { self.settings.gap.max(10) } else { self.settings.gap } as f32,
        );
        (x as f64, y as f64, sw as f64, sh as f64)
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
    /// An agent asked this tab to navigate: it waits for that, from now.
    fn expect_nav(&mut self, i: usize) {
        self.tabs[i].nav_pending = Some(Instant::now());
        self.tabs[i].nav_started = false;
    }

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
            // An agent tab that is not being watched lives on the off-screen stage,
            // visible to Chromium. The idle clock in `tick` hides it, not this.
            if t.agent && t.id != active && split != Some(t.id) {
                continue;
            }
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
        // The sidebar's width glides; a step here is what moves it.
        let moving = self.step_side(Instant::now());
        let (w, h) = logical_size(&self.ctx);
        let fs = self.fullscreen_view();
        self.chrome.set_visible(!fs);
        if !fs {
            // The chrome's own rectangle for this layout. The horizontal layout
            // is a strip across the top; the hybrid one is an underlay the page
            // is drawn over; the vertical one is the sidebar itself.
            let (x, y, cw, ch) = chrome::Chrome::bounds_for(self.layout_kind(), w, h);
            self.chrome.set_bounds(x, y, cw, ch);
        }
        self.sync_panes();
        let (act, other, div) = self.panes();
        let scale = self.ctx.window.scale_factor();
        let r = if self.settings.look == "glass" { self.settings.radius.max(14) } else { self.settings.radius };
        let radius = if fs { 0 } else { (r as f64 * scale).round() as i32 };
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
        // A moving sidebar is a moving layout: keep the chrome drawing with it.
        if moving {
            self.push();
        }
        self.sync_watch();
    }

    /// The watch over the page: on when the active tab is an agent's and the page shows
    /// (no panel over it, not full-screen UI), off otherwise. Its pill says the last step.
    fn sync_watch(&mut self) {
        let t = self.active_tab().filter(|t| t.agent && t.view.is_some() && !t.owner.is_empty());
        let on = t.is_some() && self.panel == Panel::None && !self.ui_full() && !self.minimized && !headless();
        if !on {
            if let Some(w) = &self.watch {
                w.hide();
            }
            return;
        }
        let (owner, name) = t.map(|t| (t.owner.clone(), t.owner_name.clone())).unwrap_or_default();
        let (act, _, _) = self.panes();
        let step = self.agent.last_step(&owner);
        let taken = self.taken.contains(&owner);
        let c = chrome::agent_color(&owner);
        let rgb = ((c.0 * 255.0) as u8, (c.1 * 255.0) as u8, (c.2 * 255.0) as u8);
        let name = if name.trim().is_empty() { "agent".to_string() } else { name.trim().to_string() };
        let w = self.watch.get_or_insert_with(|| watch::Watch::new(self.ctx.window.hwnd() as isize, self.ctx.proxy.clone()));
        w.show(act, &name, rgb, &step, taken);
    }

    /// For each agent session, what became of the person's last message to it, in words.
    fn note_states(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for n in self.agent.notes() {
            let state = match &n.state {
                agent::NoteState::Waiting => "message waiting for its next call".to_string(),
                agent::NoteState::Delivered { tool, .. } => format!("message delivered with {}", tool.trim_start_matches("browser_")),
                agent::NoteState::Dropped => "message dropped: the agent left".to_string(),
            };
            out.retain(|(s, _)| *s != n.session);
            out.push((n.session.clone(), state));
        }
        out
    }

    /// The watched tab, if this is it: (zoom percent).
    fn watched_zoom(&self, tab: u64) -> Option<u32> {
        let t = self.active_tab().filter(|t| t.id == tab && t.agent)?;
        self.watch.as_ref().filter(|w| w.shown()).map(|_| t.zoom)
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
        // A watched agent tab is behind its shield: the keys stay with the window,
        // unless the person took the tab over.
        if self.active_tab().is_some_and(|t| t.agent && !self.taken.contains(&t.owner)) {
            self.chrome.focus();
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
            // Chrome coordinates: full mode starts at the window origin; a sidebar
            // on the right is the one place the stage sits left of the chrome.
            let (cx, _, _, _) = chrome::Chrome::bounds_for(self.layout_kind(), logical_size(&self.ctx).0, logical_size(&self.ctx).1);
            let off = cx;
            let bar_bookmarks = self.settings.bm_bar && self.layout_kind().top_toolbar();
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
                        agent_ok: self.agent_granted(t),
                        reader: t.reader.is_some(),
                        private: t.private,
                        split: self.split.is_some() && self.on_stage(t.id),
                        translated: t.translated.is_some(),
                        translating: t.translating,
                        servo: is_servo(t),
                        // The sidebar groups agent tabs per session: the id is what a
                        // Stop button ends, the name is what the group header shows.
                        owner: t.owner.clone(),
                        owner_name: t.owner_name.clone(),
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
                blocked: privacy_stats::tab_blocked(self.active),
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
                agent_notes: self.note_states(),
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
                bookmarks: if self.panel == Panel::Bookmarks || bar_bookmarks {
                    self.library.bookmarks().into_iter().map(ui_mark).collect()
                } else {
                    Vec::new()
                },
                // Filled only while the report page is open.
                privacy_today: (self.panel == Panel::Dashboard).then(privacy_stats::today),
                report: if self.panel == Panel::Report {
                    Some(privacy_stats::snapshot(&self.report_range, self.report_site.as_deref()))
                } else {
                    None
                },
                suggestions: self.speed_dial_suggestions(),
                mark: self.active_tab().filter(|t| !t.url.is_empty()).and_then(|t| self.library.mark_for_url(&t.url)).map(ui_mark),
                bm_folders: self.library.folders(),
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
                drive: self.drive_view(),
                bug_report: (self.panel == Panel::Bug).then(|| self.bug_report.clone()).flatten(),
                bug_sent: self.bug_sent,
                unclean_last: self.unclean_last && !headless(),
                agent_cmd: self.agent.setup_command(self.settings.agent_port),
                zoom: self.active_tab().map_or(100, |t| t.zoom),
                zooms: self.zooms.iter().map(|(h, p)| (h.clone(), *p)).collect(),
                site_cookies: self.site_cookies.filter(|(id, _)| *id == self.active).map(|(_, n)| n),
                other_tabs: self
                    .workspaces
                    .iter()
                    .enumerate()
                    .filter(|(w, _)| *w != self.active_workspace)
                    .flat_map(|(w, ws)| {
                        ws.saved.iter().enumerate().filter(|(_, t)| !t.url.is_empty()).map(move |(n, t)| chrome::UiOther {
                            ws: w,
                            idx: n,
                            ws_name: ws.name.clone(),
                            title: t.title.clone(),
                            url: t.url.clone(),
                        })
                    })
                    .collect(),
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

    /// The WebView2 world this tab belongs to: the person's own, or its agent session's.
    /// A session with strict isolation gets an environment of its own; until that one is
    /// ready (WebView2 makes it in its own time) the tab still has no page.
    fn tab_profile(&self, t: &Tab) -> engine::Profile {
        if t.agent && !self.settings.agent_shared_profile {
            if t.own_env { engine::Profile::Own(t.profile.clone()) } else { engine::Profile::Shared(t.profile.clone()) }
        } else {
            engine::Profile::Main
        }
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
        let profile = match self.tab_profile(&self.tabs[i]) {
            engine::Profile::Own(key) => {
                if engine::own_env(&self.ctx, &key).is_some() {
                    engine::Profile::Own(key)
                } else if engine::own_env_failed(&self.ctx, &key) {
                    // No browser of its own on this machine. The agent still gets a
                    // profile of its own in the shared one: isolated, just not separate.
                    self.tabs[i].own_env = false;
                    engine::Profile::Shared(key)
                } else {
                    // Its browser is being made right now. Come back for this tab when
                    // it is ready (`poll_own_envs`), never open it in the shared one.
                    engine::start_own_env(&self.ctx, &key);
                    if !self.pending_own.contains(&self.tabs[i].id) {
                        self.pending_own.push(self.tabs[i].id);
                    }
                    return;
                }
            }
            p => p,
        };
        let (x, y, w, h) = self.stage();
        let t = &mut self.tabs[i];
        // An agent's page starts blank, so the recording is on before the first request.
        let first = if t.agent { None } else { Some(t.url.as_str()) };
        if let Ok(v) = engine::build_tab(&self.ctx, t.id, first, logical_rect(x, y, w, h), profile.clone(), t.private) {
            if t.muted {
                engine::set_muted(&v, true);
            }
            if t.agent {
                agentkit::watch(&v, t.id, &self.agent_logs);
                engine::remember_profile(&self.ctx, &t.profile, &v);
                // WebView2's own downloads of this agent land in its own folder too.
                engine::set_profile_download_dir(&v, &agent_policy::agent_downloads_dir_for(&t.downloads));
                let _ = v.load_url(&t.url);
                // Onto the off-screen stage, where Chromium counts it as visible. A tab the
                // person is watching (a sleeping one they clicked) stays in the window.
                let watched = t.id == self.active || self.split == Some(t.id);
                if !watched {
                    let stage = self.stage.get_or_insert_with(stage::Stage::new);
                    let (sx, sy, sw, sh) = stage.slot(t.id, (w, h));
                    let hwnd = stage.hwnd();
                    let _ = v.reparent(hwnd);
                    let _ = v.set_bounds(logical_rect(sx, sy, sw, sh));
                    let _ = v.set_visible(true);
                }
            }
            t.view = Some(v);
            t.suspended = false;
            t.trimmed = false;
        }
        let want = self.wanted_zoom(i);
        self.put_zoom(i, want);
        self.limit_tab(i);
    }

    /// Strict-isolation browsers that came back since the last look: build the tabs
    /// that were waiting for them, and fall back to a named profile for the sessions
    /// WebView2 refused to give a process of their own.
    fn poll_own_envs(&mut self) {
        if self.pending_own.is_empty() {
            return;
        }
        let ready = engine::poll_own_envs(&self.ctx);
        if ready.is_empty() {
            return;
        }
        let waiting = std::mem::take(&mut self.pending_own);
        let mut retry = Vec::new();
        for id in waiting {
            let Some(i) = self.idx(id) else { continue };
            if !ready.contains(&self.tabs[i].profile) {
                retry.push(id);
                continue;
            }
            // It came back decided. With no environment, the agent still gets a profile
            // of its own in the shared browser: isolated, just not in its own process.
            if engine::own_env(&self.ctx, &self.tabs[i].profile).is_none() {
                self.tabs[i].own_env = false;
                journal::error("agent", "strict isolation unavailable; a named profile of the shared environment instead");
            }
            self.ensure_view(i);
            if self.tabs[i].view.is_none() && self.tabs[i].own_env {
                retry.push(id);
            }
        }
        self.pending_own = retry;
    }

    /// New page: give it the bandwidth cap and, if the Control page is open, a byte counter.
    fn limit_tab(&mut self, i: usize) {
        let t = &self.tabs[i];
        let Some(v) = &t.view else { return };
        if self.settings.net_limit {
            monitor::throttle(v, Some(self.settings.net_limit_kbps));
        }
        if self.panel.live()
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
        self.mru.touch(id);
        self.leave_panel();
        // Watching: an agent tab moves from the stage into the main window, awake.
        if self.tabs[i].agent
            && let Some(v) = self.tabs[i].view.as_ref()
        {
            let _ = v.reparent(self.ctx.window.hwnd() as isize);
            let _ = v.set_visible(true);
            self.tabs[i].stage_idle = false;
            self.tabs[i].visible = true;
        }
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
        if let Some(pidx) = prev {
            let pid = self.tabs[pidx].id;
            let was_agent = self.tabs[pidx].agent;
            if was_agent {
                // Send it back to the stage.
                if let Some(v) = self.tabs[pidx].view.as_ref() {
                    let page = self.stage();
                    // A tab that was born watched has never been on the stage, which may not exist yet.
                    let stage = self.stage.get_or_insert_with(stage::Stage::new);
                    let (sx, sy, sw, sh) = stage.slot(pid, (page.2, page.3));
                    let hwnd = stage.hwnd();
                    let _ = v.reparent(hwnd);
                    let _ = v.set_bounds(logical_rect(sx, sy, sw, sh));
                    let _ = v.set_visible(true);
                }
            }
            let p = &mut self.tabs[pidx];
            // It stops being the page being read unless it is the split's other half.
            if self.split != Some(p.id) {
                p.hidden_at = Some(Instant::now());
            }
        }
        self.focus_page();
        self.dirty = true;
        self.push();
        self.sync_watch();
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
            let slots: Vec<pins::Slot> = self.tabs.iter().map(|t| pins::Slot { pinned: t.pinned, loaded: t.has_page(), agent: t.agent }).collect();
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
        self.mru.remove(id);
        if t.agent
            && let Some(stage) = self.stage.as_mut()
        {
            stage.release(t.id);
        }
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
            // Back to the tab the person was on before this one, when it still
            // exists. An agent tab is never where the person lands, nor a pin
            // whose page is gone (landing there loads it). Failing that, the
            // other half of a split, else the nearest plain tab in the row, else
            // a fresh start page: pins restored at launch were never visited.
            let next = self
                .mru
                .back(id, |t| self.idx(t).is_some_and(|j| !self.tabs[j].agent && !(self.tabs[j].pinned && !self.tabs[j].has_page())))
                .or_else(|| keep.filter(|k| self.idx(*k).is_some()))
                .or_else(|| {
                    let slots: Vec<pins::Slot> = self.tabs.iter().map(|t| pins::Slot { pinned: t.pinned, loaded: t.has_page(), agent: t.agent }).collect();
                    pins::after_remove(&slots, i).map(|n| self.tabs[n].id)
                });
            match next {
                Some(next) => {
                    self.active = 0;
                    self.activate(next);
                }
                None => {
                    self.new_tab(None, None);
                }
            }
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
        let Ok(v) = engine::build_tab(&self.ctx, id, Some(url), rect, engine::Profile::Main, false) else { return };
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

    /// A tab for an address that came from outside: the command line, or a second start.
    fn open_launched(&mut self, o: &launch::Open) {
        let id = self.ctx.alloc_id();
        let mut t = Tab::new(id, o.url.clone());
        t.private = o.private;
        t.workspace = self.active_workspace;
        self.tabs.push(t);
        if cfg!(debug_assertions) {
            eprintln!("lumen: launch open {} private={}", o.url, o.private);
        }
        if o.private {
            journal::feature("private_tab", false);
        }
        self.activate(id);
    }

    /// A page tried to open an app (mailto:, spotify:...). Open it, ask, or let it go: see
    /// `extlink::decide`. Never for an agent's tab, never for a page nobody clicked on unless
    /// the person said "always" for that site.
    fn on_external(&mut self, tab: u64, origin: String, uri: String, user: bool) {
        let Some(scheme) = extlink::scheme_of(&uri) else { return };
        let Some(i) = self.idx(tab) else { return };
        let (agent, private) = (self.tabs[i].agent, self.tabs[i].private);
        match extlink::decide(&self.rules, &origin, &scheme, user, agent) {
            extlink::Decision::Drop => {}
            extlink::Decision::Open => self.open_external(&uri),
            extlink::Decision::Ask => {
                if !self.ext_gate.begin(&origin, &scheme, Instant::now()) {
                    return;
                }
                // A private tab, or a page with no real origin, can be let through once but
                // never remembered: a rule is a trace on disk.
                let remember = !private && origin.contains("://") && !origin.starts_with("file:");
                // Test hook (debug builds): answer without a box, and say what was asked.
                if cfg!(debug_assertions)
                    && let Ok(a) = std::env::var("LUMEN_TEST_EXT_ANSWER")
                {
                    eprintln!("lumen: ext asked {origin} {scheme} remember={remember} answer={a}");
                    let answer = match a.as_str() {
                        "always" if remember => extlink::Answer::Always,
                        "once" | "always" => extlink::Answer::Once,
                        _ => extlink::Answer::Cancel,
                    };
                    self.on_external_answer(origin, uri, answer);
                    return;
                }
                // Nobody is there to answer a headless Lumen.
                if headless() {
                    self.ext_gate.finish(&origin, &scheme, extlink::Answer::Cancel, Instant::now());
                    return;
                }
                let (title, text) = extlink::prompt_words(&origin, &scheme);
                let (near, proxy) = (self.ctx.window.hwnd() as isize, self.ctx.proxy.clone());
                // The box has a message loop of its own, so it lives on its own thread.
                std::thread::Builder::new()
                    .name("lumen-ext-ask".into())
                    .spawn(move || {
                        let buttons: &[&str] = if remember { &["Open", "Always for this site", "Cancel"] } else { &["Open", "Cancel"] };
                        let answer = match chrome::dialog::choose(&title, &text, buttons, near) {
                            Some(0) => extlink::Answer::Once,
                            Some(1) if remember => extlink::Answer::Always,
                            _ => extlink::Answer::Cancel,
                        };
                        let _ = proxy.send_event(Ev::ExternalAnswer { origin, uri, answer });
                    })
                    .ok();
            }
        }
    }

    fn on_external_answer(&mut self, origin: String, uri: String, answer: extlink::Answer) {
        let Some(scheme) = extlink::scheme_of(&uri) else { return };
        self.ext_gate.finish(&origin, &scheme, answer, Instant::now());
        match answer {
            extlink::Answer::Cancel => {}
            extlink::Answer::Once => self.open_external(&uri),
            extlink::Answer::Always => {
                self.rules.entry(origin).or_default().insert(extlink::rule_kind(&scheme), true);
                store::save_rules(&self.rules);
                self.open_external(&uri);
                self.push();
            }
        }
    }

    /// Hand an address to Windows. (A debug run with the test answer set only says it would.)
    fn open_external(&mut self, uri: &str) {
        if cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_EXT_ANSWER").is_some() {
            eprintln!("lumen: ext open {}", extlink::scheme_of(uri).unwrap_or_default());
            return;
        }
        if !extlink::launch(uri) {
            journal::error("external", "Windows had no app for a link");
        }
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
        // The report is made when its page opens, so it says how things are now. Opened
        // after a crash, it says so in its subject; either way the crash note has done its job.
        if p == Panel::Bug && was != Panel::Bug {
            self.bug_kind = if self.unclean_last { "crash" } else { "problem" };
            self.unclean_last = false;
            self.bug_sent = false;
            self.bug_report = Some(self.make_bug_report());
        }
        // The page is open now: the dot on the footer button has done its job.
        if p == Panel::Downloads {
            self.dl_unseen = false;
        }
        if was.live() && !p.live() {
            self.stop_monitor();
        }
        if p.live() && !was.live() {
            self.start_monitor();
        }
        self.layout();
        self.focus_page();
        self.push();
    }

    /// The problem report: facts about this Lumen and this PC, and the journal's crashes and
    /// stalls. No addresses, no titles, nothing from pages (bugreport.rs keeps it that way).
    fn make_bug_report(&self) -> String {
        let webview = self.ctx.env.borrow().as_ref().map(engine::browser_version).unwrap_or_default();
        let facts = bugreport::Facts {
            version: env!("CARGO_PKG_VERSION").to_string(),
            build: env!("LUMEN_BUILD").to_string(),
            os: bugreport::os_version(),
            webview,
            look: self.settings.look.clone(),
            layout: self.settings.layout.clone(),
            theme: self.settings.theme.clone(),
            blocker: self.settings.adblock,
            agent_door: self.settings.agent,
            uptime_s: self.born.elapsed().as_secs(),
            tabs: self.tabs.len(),
            mem_mb: self.mem.map(|(b, _)| b / (1024 * 1024)),
        };
        let dir = store::data_dir();
        let journal = bugreport::journal_digest(&dir.join("journal"), 3, 60);
        let crash = bugreport::crash_tail(&dir.join("crash.log"), 20);
        bugreport::build(&facts, &journal, &crash)
    }

    /// Switching tabs or navigating closes any panel, quietly.
    fn leave_panel(&mut self) {
        if self.panel.live() {
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
        if !self.panel.live() {
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
            // Tracking prevention is per profile, and private and agent pages have their own.
            for v in self.tabs.iter().filter_map(|t| t.view.as_ref()) {
                engine::set_tracking(v, &self.settings.tracking);
            }
        }
        if old.https_first != self.settings.https_first || old.http_ok != self.settings.http_ok {
            let mut p = self.ctx.https.borrow_mut();
            p.on = self.settings.https_first;
            p.exceptions = self.settings.http_ok.iter().cloned().collect();
        }
        if old.gpc != self.settings.gpc {
            self.blocker.set_gpc(self.settings.gpc);
            self.ctx.gpc.set(self.settings.gpc);
        }
        if old.perm_defaults != self.settings.perm_defaults {
        }
        if old.default_zoom != self.settings.default_zoom {
            // Sites with no zoom of their own follow the default.
            for i in 0..self.tabs.len() {
                let want = self.wanted_zoom(i);
                self.put_zoom(i, want);
            }
        }
        if old.radius != self.settings.radius {
            self.rounded.clear();
        }
        self.blocker.set_enabled(self.settings.adblock);
        self.blocker.set_cosmetic(self.settings.cosmetic);
        privacy_stats::set_enabled(self.settings.privacy_stats);
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
            if !self.settings.agent {
                // Door shut: nothing the person allowed to agents outlasts it.
                self.tabs.iter_mut().for_each(|t| t.agent_ok = None);
            }
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
                    // The warning for a site without https is a page made from a string: the tab
                    // keeps the address it was asked for, until a real page replaces it.
                    if self.tabs[i].no_https.is_some() {
                        if !u.starts_with("http") {
                            return;
                        }
                        self.tabs[i].no_https = None;
                    }
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
                                let who = privacy_stats::Who::new(tab_id, self.tabs[i].private, self.tabs[i].agent);
                                let site = privacy_stats::host(&u);
                                let proxy = self.ctx.proxy.clone();
                                std::thread::spawn(move || {
                                    let segs = sponsorblock::fetch_segments(&vid);
                                    // Counted where the segments are known: one
                                    // page load, however many segments it skipped.
                                    privacy_stats::sponsor(&site, segs.len() as u64, who);
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
                    // The person let agents use this tab on one site; another site ends it.
                    if self.tabs[i].agent_ok.as_deref().is_some_and(|site| site != engine::origin_of(&u)) {
                        self.tabs[i].agent_ok = None;
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
                    // A site the person zoomed keeps its zoom; any other gets the default.
                    let want = self.wanted_zoom(i);
                    self.put_zoom(i, want);
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
                    // The site's icon for the speed dial, kept past this tab. Not for private tabs:
                    // that would write down that the site was visited.
                    if !self.tabs[i].private && !self.tabs[i].agent {
                        if sitefavs::remember(&mut self.site_favs, &self.tabs[i].url, &f) {
                            self.site_favs_dirty = true;
                            self.chrome.set_site_fav(&speeddial::host_of(&self.tabs[i].url), &f);
                        }
                        // Same visit fills the on-disk icon cache the redesign draws from.
                        use base64::Engine as _;
                        if let Some(b64) = f.strip_prefix("data:image/png;base64,")
                            && let Ok(png) = base64::engine::general_purpose::STANDARD.decode(b64)
                        {
                            favicons::remember(&speeddial::host_of(&self.tabs[i].url), &png);
                        }
                    }
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
                    self.tabs[i].nav_pending = Some(Instant::now());
                    self.tabs[i].nav_started = true;
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
                if let Some(i) = self.idx(id) {
                    self.tabs[i].last_nav = Some((ok, err, http));
                    // Cancelled (14, OPERATION_CANCELED) is how a redirect, an https upgrade
                    // or a warning page begins: something else may follow. Expect it a moment.
                    if !ok && err == 14 {
                        self.expect_nav(i);
                    } else {
                        self.tabs[i].nav_pending = None;
                    }
                }
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
                match sites::verdict(&self.rules, &self.settings.perm_defaults, &origin, kind) {
                    sites::Verdict::Allow => engine::answer_permission(&self.ctx, pid, Some(true)),
                    sites::Verdict::Block => engine::answer_permission(&self.ctx, pid, Some(false)),
                    sites::Verdict::Ask => {
                        self.prompts.push((pid, tab, origin, kind));
                        self.push();
                    }
                }
            }
            Ev::Zoom(id, f) => {
                let Some(i) = self.idx(id) else { return };
                let pct = sites::pct_of(f);
                // The change Lumen made itself is not the person's choice.
                let ours = self.tabs[i].zoom_set.take() == Some(pct);
                self.tabs[i].zoom = pct;
                if !ours {
                    self.remember_zoom(i, pct);
                }
                self.push();
            }
            Ev::Upgrade(id, secure, plain) => self.upgrade(id, secure, plain),
            Ev::Probed(id, secure, plain, answer) => self.probed(id, secure, plain, answer),
            Ev::HttpChoice(id, choice) => self.http_choice(id, choice),
            Ev::Danger(id, url) => self.danger(id, url),
            Ev::DangerChoice(id, choice) => self.danger_choice(id, choice),
            Ev::SiteCookies(id, n) => {
                self.site_cookies = Some((id, n));
                self.push();
            }
            Ev::SiteCleared(origin, ok) => {
                self.site_cookies = None;
                let what = if ok { format!("Cleared the data of {}", host_of(&origin)) } else { "Could not clear that site".to_string() };
                self.chrome.flash(what);
                self.ask_site_cookies();
                self.push();
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
                // What an agent asked for, or an agent's page offered, goes in that
                // session's own folder: two agents never write over each other's files.
                let agent_dir = self
                    .agent_dl
                    .get(&url)
                    .cloned()
                    .or_else(|| self.idx(tab).filter(|&i| self.tabs[i].agent).map(|i| self.tabs[i].downloads.clone()))
                    .filter(|f| !f.is_empty())
                    .map(|folder| {
                        let dir = agent_policy::agent_downloads_dir_for(&folder);
                        let _ = std::fs::create_dir_all(&dir);
                        dir
                    });
                let start = move |cookie: String| {
                    if !cookie.is_empty() {
                        headers.push(format!("Cookie: {cookie}"));
                    }
                    std::thread::spawn(move || {
                        let name = (!name.is_empty()).then_some(name);
                        // A page's download: the .torrent file itself is fetched
                        // like any file, but the torrent it spawns waits for Resume.
                        let _ = match aria.add_in(&u, &headers, name.as_deref(), false, agent_dir.as_deref()) {
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
            Ev::ShowMe => {
                // A second start: addresses it left open as tabs; with none, or when it wants
                // the window, the window comes forward.
                let q = launch::drain();
                for o in &q.opens {
                    self.open_launched(o);
                }
                if q.opens.is_empty() || q.show {
                    self.show_me();
                }
            }
            Ev::ExternalUri { tab, origin, uri, user } => self.on_external(tab, origin, uri, user),
            Ev::ExternalAnswer { origin, uri, answer } => self.on_external_answer(origin, uri, answer),
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
                let (owner, owner_name, profile, downloads, own_env) = self
                    .idx(parent)
                    .map(|i| {
                        let t = &self.tabs[i];
                        (t.owner.clone(), t.owner_name.clone(), t.profile.clone(), t.downloads.clone(), t.own_env)
                    })
                    .unwrap_or_default();
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
                    // The popup is the same agent's, in the same profile and folder: an
                    // OAuth popup must see the cookies its opener is logged in with.
                    t.profile = profile;
                    t.downloads = downloads;
                    t.own_env = own_env;
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
            Action::Report => {
                let p = if self.panel == Panel::Report { Panel::None } else { Panel::Report };
                self.set_panel(p);
            }
            Action::Dashboard => {
                let p = if self.panel == Panel::Dashboard { Panel::None } else { Panel::Dashboard };
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
            // The switcher lives in the chrome: it owns the list box and the typing.
            Action::TabSearch => self.chrome.open_tab_search(),
        }
    }

    fn on_ui(&mut self, raw: &str) {
        let Ok(m) = serde_json::from_str::<Value>(raw) else { return };
        let id = m["id"].as_u64().unwrap_or(0);
        match m["t"].as_str().unwrap_or("") {
            "ready" => {
                self.ui_ready = true;
                self.fav_queue = self.tabs.iter().filter(|t| !t.favicon.is_empty()).map(|t| t.id).collect();
                for (host, uri) in &self.site_favs {
                    self.chrome.set_site_fav(host, uri);
                }
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
            "openTab" => {
                // A link of Lumen's own (the credit): a tab of its own, after this one.
                let url = m["url"].as_str().unwrap_or("").to_string();
                if url.starts_with("https://") {
                    let after = self.idx(self.active);
                    self.new_tab(Some(url), after);
                }
            }
            "bugSend" => {
                // Exactly what the page shows, with the person's words in it, posted off the
                // window's thread; the answer comes back as a "bugSent" message.
                if let Some(report) = self.bug_report.clone().filter(|_| !self.bug_sent) {
                    let text = bugreport::with_story(&report, m["what"].as_str().unwrap_or(""));
                    let (kind, proxy) = (self.bug_kind, self.ctx.proxy.clone());
                    self.bug_sent = true;
                    self.chrome.flash("Sending…".into());
                    self.push();
                    let _ = std::thread::Builder::new().name("lumen-report".into()).spawn(move || {
                        let r = bugreport::send(&text, kind, env!("CARGO_PKG_VERSION"));
                        let msg = json!({"t": "bugSent", "ok": r.is_ok(), "why": r.err().unwrap_or_default()});
                        let _ = proxy.send_event(Ev::Ui(msg.to_string()));
                    });
                }
            }
            "bugSent" => {
                if m["ok"].as_bool().unwrap_or(false) {
                    self.chrome.flash("Sent to zemo. Thank you.".into());
                } else {
                    // Not sent: it may be sent again, or copied and sent another way.
                    self.bug_sent = false;
                    let why = m["why"].as_str().unwrap_or("it did not go");
                    self.chrome.flash(format!("Not sent: {why}. Copy it instead, or try again."));
                }
                journal::feature("report_send", false);
                self.push();
            }
            "crashSeen" => {
                self.unclean_last = false;
                self.push();
            }
            "bugSave" => {
                // The report as a file in Downloads, named by the minute it was made.
                if let Some(text) = self.bug_report.clone() {
                    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
                    let name = format!("lumen-report-{:04}{:02}{:02}-{:02}{:02}.txt", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute);
                    let path = store::download_dir().join(&name);
                    let msg = match std::fs::write(&path, text) {
                        Ok(()) => format!("Saved {name} in Downloads"),
                        Err(_) => "could not write the report to Downloads".to_string(),
                    };
                    self.chrome.flash(msg);
                    self.push();
                }
            }
            "newtab" => {
                self.new_tab(None, None);
            }
            "newPrivate" => {
                self.new_private_tab();
            }
            "activate" => self.activate(id),
            "close" => self.close(id),
            "agentGrant" => {
                // Right-click > Let agents use this tab. Again: stop letting them. Only a tab
                // of the person's, only with a real page, and only for the site it is on now.
                self.toggle_agent_grant(id);
            }
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
            "dashboard" => {
                // The dashboard is Lumen's own home: the clock, the search box and
                // the speed dial. Opening it means an empty page on this tab.
                let i = self.active_idx();
                self.leave_panel();
                self.tabs[i].url.clear();
                self.tabs[i].title.clear();
                self.tabs[i].typed = None;
                // The page goes; the tab stays.
                self.tabs[i].view = None;
                self.tabs[i].visible = false;
                self.tabs[i].suspended = false;
                self.tabs[i].trimmed = false;
                self.tabs[i].crashed = false;
                self.tabs[i].reader = None;
                self.tabs[i].translated = None;
                self.layout();
                self.focus_page();
                self.dirty = true;
                self.push();
            }
            "panel" => {
                let p = match (m["open"].as_bool().unwrap_or(false), m["which"].as_str()) {
                    (false, _) => Panel::None,
                    (true, Some("control")) => Panel::Control,
                    (true, Some("downloads")) => Panel::Downloads,
                    (true, Some("history")) => Panel::History,
                    (true, Some("bookmarks")) => Panel::Bookmarks,
                    (true, Some("drive")) => Panel::Drive,
                    (true, Some("report")) => Panel::Report,
                    (true, Some("dashboard")) => Panel::Dashboard,
                    (true, Some("sites")) => Panel::Sites,
                    (true, Some("changes")) => Panel::Changes,
                    (true, Some("bugreport")) => Panel::Bug,
                    (true, _) => Panel::Settings,
                };
                self.set_panel(p);
            }
            "report" => {
                // The report page picked a range tile or a site row.
                if let Some(r) = m["range"].as_str()
                    && matches!(r, "today" | "7" | "30" | "all")
                {
                    self.report_range = r.to_string();
                }
                if let Some(host) = m["site"].as_str() {
                    self.report_site = (!host.is_empty()).then(|| host.to_string());
                }
                self.push();
            }
            "freeMemory" => {
                // The dashboard's button: every hidden page to its lowest memory level now,
                // the same trim the sleep policy gives a page hidden for 30 seconds.
                for t in self.tabs.iter().filter(|t| !self.on_stage(t.id)) {
                    if let Some(v) = &t.view {
                        let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
                    }
                }
                self.push();
            }
            "reportReset" => {
                privacy_stats::reset();
                self.report_site = None;
                self.push();
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
            "bmOpen" => {
                let url = m["url"].as_str().unwrap_or("").to_string();
                if url.is_empty() {
                    return;
                }
                if m["bg"].as_bool().unwrap_or(false) {
                    // A tab behind this one. It loads when it is first clicked, like any
                    // tab restored from a session: ten middle clicks must not start ten pages.
                    let mut t = Tab::new(self.ctx.alloc_id(), url.clone());
                    t.title = self.library.mark_for_url(&url).map(|b| b.title).unwrap_or_default();
                    t.workspace = self.active_workspace;
                    self.tabs.push(t);
                    self.dirty = true;
                    self.push();
                } else {
                    self.navigate(&url);
                }
            }
            "bmEdit" => {
                let s = |k: &str| m[k].as_str().unwrap_or("").to_string();
                if self.library.edit_bookmark(id, &s("title"), &s("url"), &s("folder")) {
                    self.library.flush();
                    self.push();
                }
            }
            "bmMove" => {
                self.library.move_bookmark(id, m["folder"].as_str().unwrap_or(""));
                self.library.flush();
                self.push();
            }
            "bmPlace" => {
                self.library.place(id, m["folder"].as_str().unwrap_or(""), m["before"].as_u64());
                self.library.flush();
                self.push();
            }
            "bmShift" => {
                self.library.shift_bookmark(id, m["d"].as_i64().unwrap_or(0).signum() as i32);
                self.library.flush();
                self.push();
            }
            "bmDelete" => {
                self.library.delete_bookmark(id);
                self.library.flush();
                self.push();
            }
            "bmFolderAdd" => {
                self.library.add_folder(m["name"].as_str().unwrap_or(""));
                self.library.flush();
                self.push();
            }
            "bmFolderRename" => {
                self.library.rename_folder(m["old"].as_str().unwrap_or(""), m["new"].as_str().unwrap_or(""));
                self.library.flush();
                self.push();
            }
            "bmFolderDelete" => {
                self.library.delete_folder(m["name"].as_str().unwrap_or(""));
                self.library.flush();
                self.push();
            }
            "bmImport" => {
                // Read-only: the other browsers' files are opened for reading and never touched.
                let want = m["browser"].as_str().unwrap_or("");
                let found: Vec<bookmarks::Found> = bookmarks::scan().into_iter().filter(|f| want == "all" || f.browser == want).collect();
                let (mut added, mut skipped) = (0, 0);
                for f in &found {
                    let (a, s) = self.library.import(&f.items);
                    added += a;
                    skipped += s;
                }
                self.library.flush();
                let msg = match found.as_slice() {
                    [one] => bookmarks::summary(one.browser, added, skipped),
                    [] => "That browser's bookmarks are gone".to_string(),
                    many => format!("Imported {added} from {} browsers, skipped {skipped} already here", many.len()),
                };
                self.chrome.flash(msg);
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
                self.forget_site_favs(since);
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
                self.forget_site_favs(since);
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
                    let (_, tab, origin, kind) = self.prompts.remove(k);
                    // A private tab's answer is for this once: it must not be written down.
                    let private = self.idx(tab).is_some_and(|i| self.off_record(&self.tabs[i]));
                    if !private {
                        let v = if allow { sites::Verdict::Allow } else { sites::Verdict::Block };
                        sites::set_rule(&mut self.rules, &origin, kind, v);
                        store::save_rules(&self.rules);
                    }
                }
                engine::answer_permission(&self.ctx, pid, Some(allow));
                self.push();
            }
            "permReset" => {
                let (origin, kind) = (m["origin"].as_str().unwrap_or(""), m["kind"].as_str().unwrap_or(""));
                sites::set_rule(&mut self.rules, origin, kind, sites::Verdict::Ask);
                store::save_rules(&self.rules);
                self.push();
            }
            // The Site settings page and the lock's popover: Allow, Block or Ask for one kind.
            "permSet" => {
                let (origin, kind) = (m["origin"].as_str().unwrap_or(""), m["kind"].as_str().unwrap_or(""));
                let v = match m["v"].as_str() {
                    Some("allow") => sites::Verdict::Allow,
                    Some("block") => sites::Verdict::Block,
                    _ => sites::Verdict::Ask,
                };
                // An answer given on a private or agent page, from the lock, is for this visit:
                // writing it down would record a site the person kept off the record.
                let keep = !(m["lock"].as_bool().unwrap_or(false) && self.active_tab().is_some_and(|t| self.off_record(t)));
                // Only kinds Lumen knows, on origins that are sites. The app launcher's own
                // per-scheme rules ("open mailto") are the Site settings page's to edit too.
                let known = sites::KINDS.iter().any(|k| k.key == kind) || kind.starts_with("open ");
                if keep && known && origin.starts_with("http") {
                    sites::set_rule(&mut self.rules, origin, kind, v);
                    store::save_rules(&self.rules);
                    self.push();
                }
            }
            // Every answer a site has: back to asking.
            "permResetSite" => {
                self.rules.remove(m["origin"].as_str().unwrap_or(""));
                store::save_rules(&self.rules);
                self.push();
            }
            // The chip in the address bar: back to 100%. The site keeps that choice.
            "zoomSet" => {
                let pct = sites::clamp_pct(m["pct"].as_u64().unwrap_or(100) as u32);
                let i = self.active_idx();
                self.put_zoom(i, pct);
                self.remember_zoom(i, pct);
                self.push();
            }
            // The Site settings page: this site goes back to the default zoom.
            "zoomForget" => {
                let host = m["host"].as_str().unwrap_or("").to_string();
                if self.zooms.remove(&host).is_some() {
                    store::save_zooms(&self.zooms);
                }
                self.follow_zoom(&host);
                self.push();
            }
            // The lock was pressed: ask what the site holds.
            "siteOpen" => self.ask_site_cookies(),
            "siteClear" => {
                let origin = m["origin"].as_str().unwrap_or("").to_string();
                // From the lock over a private or agent page, clearing would reach into the
                // person's own store for a site they never opened there. Those stores go away
                // with their last page anyway.
                if m["lock"].as_bool().unwrap_or(false) && self.active_tab().is_some_and(|t| self.off_record(t)) {
                    self.chrome.flash("Private tabs forget everything when they close".into());
                } else {
                    self.clear_site(&origin);
                }
            }
            // The tab search picked a row: a tab here, or one in a workspace that is put away.
            "switchTab" => {
                if m["other"].as_bool().unwrap_or(false) {
                    let ws = m["ws"].as_u64().unwrap_or(0) as usize;
                    let idx = m["idx"].as_u64().unwrap_or(0) as usize;
                    if ws < self.workspaces.len() && ws != self.active_workspace {
                        self.switch_workspace(ws);
                    }
                    // Pinned tabs sort to the front on load, so the index holds.
                    if let Some(t) = self.tabs.get(idx) {
                        let id = t.id;
                        self.activate(id);
                    }
                } else {
                    let id = m["id"].as_u64().unwrap_or(0);
                    if self.idx(id).is_some() {
                        self.activate(id);
                    }
                }
            }
            // Privacy: this host may stay on plain http no more.
            "httpForget" => {
                let host = m["host"].as_str().unwrap_or("").to_string();
                if self.settings.http_ok.iter().any(|h| *h == host) {
                    let mut st = self.settings.clone();
                    st.http_ok.retain(|h| *h != host);
                    self.apply_settings(st);
                }
                self.push();
            }
            // Per-kind default: ask, or block without asking.
            "permDefault" => {
                let kind = m["kind"].as_str().unwrap_or("");
                if sites::KINDS.iter().any(|k| k.key == kind) {
                    let v = if m["v"].as_str() == Some("block") { sites::Verdict::Block } else { sites::Verdict::Ask };
                    let mut st = self.settings.clone();
                    sites::set_default(&mut st.perm_defaults, kind, v);
                    self.apply_settings(st);
                }
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
                // A paused Drive run gets its tab back when the person presses Done:
                // that is what "pause and take over" is waiting for.
                if let Some(sid) = self.drive.as_ref().filter(|d| d.paused).and_then(|d| d.session.clone()) {
                    self.agent.set_paused(&sid, false);
                    if let Some(d) = self.drive.as_mut() {
                        d.paused = false;
                    }
                }
                self.push();
            }
            "driveStart" => self.drive_start(),
            "driveStop" => self.drive_stop("stopped by the person"),
            "drivePause" => self.drive_pause(),
            "handoffDecline" => {
                if let Some((_, _, _, r)) = self.handoff.take() {
                    let _ = r.send(Err("declined".into()));
                }
                self.push();
            }
            "consentApprove" => {
                if let Some((_, _, _, _, r)) = self.consent.take() {
                    let _ = r.send(Ok(Value::Null));
                }
                self.push();
            }
            "consentDecline" => {
                if let Some((_, _, _, _, r)) = self.consent.take() {
                    let _ = r.send(Err("declined".into()));
                }
                self.push();
            }
            "agentStop" => {
                if let Some((_, _, _, r)) = self.handoff.take() {
                    let _ = r.send(Err("the person stopped the agents".into()));
                }
                // The kill switch: door shut, agent tabs gone, their profile data wiped.
                let mut s = self.settings.clone();
                s.agent = false;
                self.apply_settings(s);
                self.end_all_agent_sessions();
                self.agent_seen = None;
                self.push();
            }
            "openMessage" => {
                let (session, name) = (m["session"].as_str().unwrap_or(""), m["name"].as_str().unwrap_or("agent"));
                self.chrome.open_message(session, name);
            }
            "watchMessage" => {
                // The pill's Message: the card over the sidebar, for the watched agent.
                if let Some((owner, name)) = self.active_tab().filter(|t| t.agent).map(|t| (t.owner.clone(), t.owner_name.clone())) {
                    let name = if name.trim().is_empty() { "agent".to_string() } else { name };
                    self.chrome.open_message(&owner, &name);
                }
            }
            "watchTakeOver" | "watchGiveBack" => {
                let Some(owner) = self.active_tab().filter(|t| t.agent).map(|t| t.owner.clone()) else { return };
                let take = m["t"] == "watchTakeOver";
                // Taken over: the agent may only ask for the tab back (browser_handoff) until the
                // person gives it back. The shield goes, the page takes the keyboard.
                self.agent.set_paused(&owner, take);
                if take {
                    self.taken.insert(owner);
                } else {
                    self.taken.remove(&owner);
                }
                self.sync_watch();
                self.focus_page();
                self.chrome.flash(if take { "The tab is yours; the agent waits until you give it back".into() } else { "Given back to the agent".into() });
                self.push();
            }
            "agentTell" => {
                let session = m["session"].as_str().unwrap_or("").to_string();
                let text = m["text"].as_str().unwrap_or("").trim().to_string();
                if text.is_empty() {
                    return;
                }
                match m["say"].as_str().unwrap_or("") {
                    // A new task is a new Drive run: Lumen cannot open a task inside a terminal
                    // session someone else started.
                    "new" => self.drive_start_task(text),
                    say => {
                        let say = if say == "later" { agent::Say::Later } else { agent::Say::Now };
                        // A Drive run that already ended takes a queued message as its next run.
                        let drive_over = self.drive.as_ref().is_some_and(|d| d.exit.is_some() && d.session.as_deref() == Some(session.as_str()));
                        if drive_over {
                            self.drive_start_task(text);
                        } else if self.agent.tell(&session, say, &text) {
                            self.chrome.flash(match say {
                                agent::Say::Now => "Sent: it reaches the agent with its next call".into(),
                                agent::Say::Later => "Queued: it reaches the agent with its next call, for after its task".into(),
                            });
                        } else {
                            self.chrome.flash("That agent is gone; the message was not sent".into());
                        }
                    }
                }
                self.push();
            }
            "agentStopSession" => {
                // The per-agent Stop in the sidebar: this session ends now. Its id stops
                // working, its tabs close, and what it left behind is wiped.
                let session = m["session"].as_str().unwrap_or("");
                if let Some((_, _, _, r)) = self.handoff.take() {
                    let _ = r.send(Err("the person stopped this agent".into()));
                }
                if let Some(who) = self.agent.end_session(session) {
                    self.end_agent_session(&who);
                    journal::agent_session(&who.session, &format!("{} (stopped)", who.name));
                }
                self.chrome.flash("Agent stopped, its data deleted".into());
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
        // A strictly isolated agent's browser is being made: the tick is what
        // notices it is ready and gives the waiting tabs their page.
        if !self.pending_own.is_empty() {
            return true;
        }
        // A stage view still painting: the tick is what puts it to sleep once its
        // agent goes quiet. After that nothing here wants a tick.
        if self.tabs.iter().any(|t| t.agent && t.view.is_some() && !t.stage_idle && !self.on_stage(t.id)) {
            return true;
        }
        // A Drive run is going: its output and its session want a look.
        if self.drive.is_some() && now >= self.next_drive {
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
        // A gliding sidebar wants the next frame, and only while it glides.
        if self.side_anim.is_some() {
            at = at.min(now + Duration::from_millis(16));
        }
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
        // Strictly isolated agents whose browser came back since the last look.
        self.poll_own_envs();
        // Filter lists past their own expiry are fetched again, in the background, hourly.
        self.blocker.refresh_if_due(now);
        // A Drive run's output, session and end.
        if self.drive.is_some() && now >= self.next_drive {
            self.drive_poll();
        }
        // Started headless for agents, never shown to a person, no agent tab left, quiet for
        // ten minutes: nobody needs this Lumen. lumen-agent starts a new one when asked.
        if headless() && self.handoff.is_none() && !self.tabs.iter().any(|t| t.agent) && now - self.agent_last > agent_idle() {
            self.library.flush();
            self.clear_on_exit();
            self.save_session();
            self.upnp.release();
            // Leaving on purpose, so the journal ends with a clean goodbye.
            journal::stop(true, self.born.elapsed());
            privacy_stats::flush();
            std::process::exit(0);
        }
        // A stage view whose agent has been quiet for STAGE_IDLE stops painting. Its next
        // call wakes it (`on_agent`). A watched tab is on screen and keeps painting.
        let stage_idle = if cfg!(debug_assertions) {
            std::env::var("LUMEN_STAGE_IDLE_MS").ok().and_then(|v| v.parse::<u64>().ok()).map(Duration::from_millis).unwrap_or(STAGE_IDLE)
        } else {
            STAGE_IDLE
        };
        for i in 0..self.tabs.len() {
            let t = &self.tabs[i];
            if !t.agent || t.stage_idle || self.on_stage(t.id) {
                continue;
            }
            let last = self.agent.session_last(&t.owner).unwrap_or(now);
            if now.duration_since(last) >= stage_idle
                && let Some(v) = t.view.as_ref()
            {
                let _ = v.set_visible(false);
                self.tabs[i].stage_idle = true;
            }
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
        let (own, total, procs, cpu) = engine::memory_with_cpu(env.as_ref(), journal_work);
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

    /// The person said agents may use this tab, and it is still on the site they said it for.
    fn agent_granted(&self, t: &Tab) -> bool {
        !t.agent && t.agent_ok.as_deref().is_some_and(|site| site == engine::origin_of(&t.url))
    }

    /// "Let agents use this tab", or take it back. Only a tab of the person's with a real page,
    /// never a private one, and only for the site it is on now.
    fn toggle_agent_grant(&mut self, id: u64) {
        let Some(i) = self.idx(id) else { return };
        let t = &mut self.tabs[i];
        t.agent_ok = if t.agent_ok.is_some() || t.agent || t.private || !t.url.starts_with("http") { None } else { Some(engine::origin_of(&t.url)) };
        self.push();
    }

    /// One primitive for an agent. An agent may use the tabs its session opened, and a tab of
    /// the person's only while they have granted that tab. Never another agent's. The tool-level
    /// rules (what may run where) are `agent_policy::authorize`, applied before this is reached.
    fn on_agent(&mut self, call: agent::Call) {
        use agent::Op;
        self.agent_last = Instant::now();
        // A strictly isolated browser made since the last call: its waiting tabs get
        // their page now, so the call below finds the tab it asked for.
        self.poll_own_envs();
        // A stage view the idle clock put to sleep paints again before the call runs.
        // Only then: a view that is awake costs nothing here.
        let wake_tab = match &call.op {
            Op::Navigate { tab, .. } | Op::Back { tab } | Op::Forward { tab } | Op::Reload { tab } | Op::NavInfo { tab }
            | Op::Eval { tab, .. } | Op::Cdp { tab, .. } | Op::Log { tab, .. } | Op::NetIdle { tab }
            | Op::Contexts { tab } | Op::CdpForSession { tab, .. } | Op::DragData { tab, .. }
            | Op::Dialogs { tab, .. } | Op::Show { tab } | Op::Handoff { tab, .. } | Op::Consent { tab, .. } => Some(*tab),
            _ => None,
        };
        if let Some(i) = wake_tab.and_then(|t| self.idx(t))
            && self.tabs[i].agent
            && self.tabs[i].stage_idle
        {
            self.tabs[i].stage_idle = false;
            if let Some(v) = self.tabs[i].view.as_ref() {
                let _ = v.set_visible(true);
                // A moment for Chromium to mark the page visible again, or the first
                // mouse event after a long quiet is dropped.
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        // The sidebar is drawn again only when a call can change what it shows: the chip
        // coming on, a tab opening, closing or moving. The calls in between (scripts,
        // DevTools, logs, the policy look-up) are nearly every call, and a whole redraw of
        // the window for each one was most of what a call cost.
        let reads = matches!(
            call.op,
            Op::Eval { .. } | Op::Cdp { .. } | Op::Log { .. } | Op::NetIdle { .. } | Op::Contexts { .. } | Op::CdpForSession { .. }
                | Op::DragData { .. } | Op::NavInfo { .. } | Op::Policy | Op::List | Op::Status | Op::Downloads | Op::Dialogs { .. }
        );
        if !reads || self.agent_seen.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.push();
        }
        self.agent_seen = Some(Instant::now());
        // The pill says what the watched agent did last; its step list grows per tool call.
        if self.watch.as_ref().is_some_and(|w| w.shown()) && self.active_tab().is_some_and(|t| t.agent && t.owner == call.who.session) {
            self.sync_watch();
        }
        let (reply, who) = (call.reply, call.who);
        // One line per session, not per call. No tab, no page, no content.
        journal::agent_session(&who.session, &who.name);
        let access = |app: &App, t: &Tab| agent_policy::access(&who.session, t.agent, &t.owner, app.agent_granted(t));
        let mine = |app: &App, tab: u64| app.idx(tab).filter(|&i| access(app, &app.tabs[i]).is_some());
        let no_tab = |tab: u64| Err(format!("no tab {tab} you may use; browser_tabs lists yours"));
        let crashed = "this tab's page crashed; browser_reload brings it back".to_string();
        match call.op {
            Op::List => {
                let list: Vec<Value> = self
                    .tabs
                    .iter()
                    .filter(|t| access(self, t).is_some())
                    .map(|t| json!({"tab": t.id, "title": t.title, "url": t.url, "front": t.id == self.active, "yours": t.agent, "crashed": t.crashed}))
                    .collect();
                let _ = reply.send(Ok(Value::Array(list)));
            }
            Op::Policy => {
                let tabs = self.tabs.iter().map(|t| agent_policy::TabInfo { id: t.id, agent: t.agent, owner: t.owner.clone(), granted: self.agent_granted(t) }).collect();
                let s = &self.settings;
                // The pause flag is read fresh here, so a Drive run paused from the panel
                // sees it on its very next call.
                let paused = self.agent.session(&who.session).is_some_and(|w| w.paused);
                let _ = reply.send(Ok(agent_policy::ctx_value(tabs, s.agent_shared_profile, s.agent_upload, &s.agent_upload_dir, paused)));
            }
            Op::TestState => {
                let all: Vec<Value> = self
                    .tabs
                    .iter()
                    .map(|t| json!({"id": t.id, "agent": t.agent, "owner": t.owner_name, "granted": self.agent_granted(t), "crashed": t.crashed, "url": t.url, "asleep": t.stage_idle}))
                    .collect();
                let _ = reply.send(if cfg!(debug_assertions) { Ok(Value::Array(all)) } else { Err("no such thing".into()) });
            }
            Op::TestGrant { tab } => {
                let r = match self.idx(tab) {
                    Some(i) if cfg!(debug_assertions) => {
                        self.toggle_agent_grant(tab);
                        Ok(json!({"url": self.tabs[i].url, "granted": self.agent_granted(&self.tabs[i])}))
                    }
                    _ => Err(format!("no tab {tab}")),
                };
                let _ = reply.send(r);
            }
            Op::TestProfile { tab } => {
                // Debug builds only: which WebView2 profile this tab lives in. The
                // integration test uses it to watch a profile folder appear and go.
                let r = match (self.idx(tab), cfg!(debug_assertions)) {
                    (Some(i), true) => match self.tabs[i].view.as_ref().and_then(engine::webview_profile) {
                        Some(p) => Ok(json!({"profile": engine::profile_name(&p), "path": engine::profile_path(&p).map(|d| d.display().to_string())})),
                        None => Err("the tab has no page yet".into()),
                    },
                    _ => Err(format!("no tab {tab}")),
                };
                let _ = reply.send(r);
            }
            Op::TestPause { paused } => {
                // Debug builds only: the same pause "Pause and take over" sets, so the
                // integration test can watch the door answer drive_paused.
                let r = if cfg!(debug_assertions) {
                    self.agent.set_paused(&who.session, paused);
                    Ok(json!({"paused": paused}))
                } else {
                    Err("no such thing".into())
                };
                let _ = reply.send(r);
            }
            Op::TestDecline => {
                if cfg!(debug_assertions) {
                    if let Some((_, _, _, r)) = self.handoff.take() {
                        let _ = r.send(Err("declined".into()));
                    }
                    let _ = reply.send(Ok(Value::Null));
                    self.push();
                } else {
                    let _ = reply.send(Err("no such thing".into()));
                }
            }
            Op::TestConsentApprove => {
                if cfg!(debug_assertions) {
                    if let Some((_, _, _, _, r)) = self.consent.take() {
                        let _ = r.send(Ok(Value::Null));
                    }
                    let _ = reply.send(Ok(Value::Null));
                    self.push();
                } else {
                    let _ = reply.send(Err("no such thing".into()));
                }
            }
            Op::TestConsentDecline => {
                if cfg!(debug_assertions) {
                    if let Some((_, _, _, _, r)) = self.consent.take() {
                        let _ = r.send(Err("declined".into()));
                    }
                    let _ = reply.send(Ok(Value::Null));
                    self.push();
                } else {
                    let _ = reply.send(Err("no such thing".into()));
                }
            }
            Op::TestActivate { tab } => {
                if cfg!(debug_assertions) {
                    self.activate(tab);
                    let _ = reply.send(Ok(Value::Null));
                } else {
                    let _ = reply.send(Err("no such thing".into()));
                }
            }
            Op::TestDone => {
                // Debug builds only: stands in for the person pressing Done.
                match self.handoff.take() {
                    Some((_, _, _, r)) if cfg!(debug_assertions) => {
                        let _ = r.send(Ok(Value::Null));
                        let _ = reply.send(Ok(Value::Null));
                        self.push();
                    }
                    other => {
                        self.handoff = other;
                        let _ = reply.send(Err("no handoff to finish".into()));
                    }
                }
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
                t.profile = who.profile.clone();
                t.downloads = who.downloads.clone();
                // Strict isolation, when the person turned it on, applies to the tabs a
                // session opens. A kept profile keeps: its own folder holds its cookies.
                t.own_env = self.settings.agent_strict_isolation;
                self.tabs.push(t);
                let i = self.tabs.len() - 1;
                self.expect_nav(i);
                if activate {
                    self.activate(id);
                } else {
                    self.ensure_view(i);
                }
                let _ = reply.send(Ok(json!({"tab": id})));
            }
            Op::Pointer { tab, x, y } => {
                if let Some(zoom) = self.watched_zoom(tab)
                    && let Some(w) = &self.watch
                {
                    w.point(x, y, zoom, "type");
                }
                let _ = reply.send(Ok(Value::Null));
            }
            Op::NavInfo { tab } => {
                if let Some(i) = mine(self, tab) {
                    let v = self.tabs[i].last_nav;
                    let (ok, err, http) = v.unwrap_or((true, 0, 0));
                    // unknown when no nav yet: return nulls
                    let mut resp = if v.is_none() {
                        json!({"ok": Value::Null, "err": Value::Null, "http": Value::Null})
                    } else {
                        json!({"ok": ok, "err": err, "http": http})
                    };
                    // Asked for and never started (history.back() with no history): after
                    // a moment it is not coming.
                    let t = &self.tabs[i];
                    resp["pending"] = json!(t.nav_pending.is_some_and(|at| t.nav_started || at.elapsed() < Duration::from_millis(1500)));
                    let _ = reply.send(Ok(resp));
                } else {
                    let _ = reply.send(no_tab(tab));
                }
            }
            Op::Navigate { tab, url } => match (mine(self, tab), resolve_input(&url, &self.settings.search)) {
                (Some(i), Some(url)) => {
                    match &self.tabs[i].view {
                        Some(v) => {
                            let _ = v.load_url(&url);
                            self.tabs[i].url = url;
                            self.tabs[i].crashed = false;
                        }
                        None => {
                            self.tabs[i].url = url;
                            self.ensure_view(i);
                        }
                    }
                    self.expect_nav(i);
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
                let Some(i) = mine(self, tab) else {
                    let _ = reply.send(no_tab(tab));
                    return;
                };
                self.expect_nav(i);
                // A dead renderer cannot run a script. Load the page again: that makes a new one.
                if self.tabs[i].crashed {
                    let url = self.tabs[i].url.clone();
                    let sent = match (&self.tabs[i].view, js) {
                        (Some(v), "location.reload()") => v.load_url(&url).is_ok(),
                        _ => false,
                    };
                    if sent {
                        self.tabs[i].crashed = false;
                        self.push();
                    }
                    let _ = reply.send(if sent { Ok(Value::Null) } else { Err(crashed) });
                    return;
                }
                let Some(v) = &self.tabs[i].view else {
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
                if self.tabs[i].crashed {
                    let _ = reply.send(Err(crashed));
                    return;
                }
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
                // The person's tab may be steered, not scripted: no code, no file picker.
                if !self.tabs[i].agent && !agent_policy::cdp_allowed_on_person_tab(&method) {
                    let _ = reply.send(Err(format!("{method} is not allowed in the person's tab")));
                    return;
                }
                if self.tabs[i].crashed {
                    let _ = reply.send(Err(crashed));
                    return;
                }
                if self.tabs[i].view.is_none() {
                    self.ensure_view(i);
                }
                // Watched: the agent's cursor goes where its mouse goes.
                if method == "Input.dispatchMouseEvent"
                    && let Some(zoom) = self.watched_zoom(tab)
                    && let Some(w) = &self.watch
                {
                    let kind = if params["type"] == "mousePressed" { "press" } else { "move" };
                    w.point(params["x"].as_f64().unwrap_or(0.0), params["y"].as_f64().unwrap_or(0.0), zoom, kind);
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
                    json!(
                        log.net
                            .iter()
                            .map(|n| json!({"id": n.id, "method": n.method, "url": n.url, "kind": n.kind, "status": n.status, "failed": n.failed, "mime": n.mime}))
                            .collect::<Vec<_>>()
                    )
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
            Op::NetIdle { tab } => {
                if mine(self, tab).is_none() {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                // Quiet for 250 ms: long enough for the fetch a script starts when the page
                // loads, short enough that a page with nothing more to fetch is not waited on.
                let idle = self.agent_logs.borrow().get(&tab).is_none_or(|log| log.net_idle(Duration::from_millis(250)));
                let _ = reply.send(Ok(json!(idle)));
            }
            Op::Contexts { tab } => {
                if mine(self, tab).is_none() {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                let out = self
                    .agent_logs
                    .borrow()
                    .get(&tab)
                    .map(|log| json!(log.contexts.iter().map(|(f, c)| json!({"frame": f, "context": c})).collect::<Vec<_>>()))
                    .unwrap_or_else(|| json!([]));
                let _ = reply.send(Ok(out));
            }
            Op::CdpForSession { tab, session, method, params } => {
                let Some(i) = mine(self, tab) else {
                    let _ = reply.send(no_tab(tab));
                    return;
                };
                if !self.tabs[i].agent && !agent_policy::cdp_allowed_on_person_tab(&method) {
                    let _ = reply.send(Err(format!("{method} is not allowed in the person's tab")));
                    return;
                }
                if self.tabs[i].view.is_none() {
                    self.ensure_view(i);
                }
                match &self.tabs[i].view {
                    Some(v) => agentkit::cdp_session(v, &session, &method, &params, move |r| {
                        let _ = reply.send(r);
                    }),
                    None => {
                        let _ = reply.send(Err("tab has no page".into()));
                    }
                }
            }
            Op::DragData { tab, take } => {
                if mine(self, tab).is_none() {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                let data = self.agent_logs.borrow_mut().get_mut(&tab).and_then(|log| if take { log.drag.take() } else { log.drag.clone() });
                let _ = reply.send(Ok(data.unwrap_or(Value::Null)));
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
            // Never in the way: no agent call brings a tab to the front. A handoff does,
            // because there the agent needs the person. This only checks the tab is theirs.
            Op::Show { tab } => {
                let _ = reply.send(if mine(self, tab).is_none() { no_tab(tab) } else { Ok(Value::Null) });
            }
            Op::Handoff { tab, message, .. } => {
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
            Op::Status => {
                let tabs: Vec<Value> = self.tabs.iter().map(|t| json!({"tab": t.id, "title": t.title, "url": t.url, "agent": t.agent, "owner": t.owner, "crashed": t.crashed, "granted": self.agent_granted(t)})).collect();                let handoff = self.handoff.as_ref().map(|h| json!({"tab": h.0, "who": h.1, "message": h.2}));
                let downloads: Vec<Value> = self.downloads.iter().filter(|d| !d.private).map(|d| json!({"url": d.url, "path": d.path, "state": d.state, "size": d.received})).collect();
                let _ = reply.send(Ok(json!({"tabs": tabs, "handoff": handoff, "downloads": downloads})));
            }
            Op::Consent { tab, kind, message } => {
                if mine(self, tab).is_none() {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                if self.handoff.is_some() || self.consent.is_some() {
                    let _ = reply.send(Err("the person is already helping another agent; try again in a minute".into()));
                    return;
                }
                self.activate(tab);
                self.consent = Some((tab, who.name.clone(), kind, message, reply));
                self.push();
            }
            Op::Close { tab } => {
                // Only a tab the agent opened. A granted tab of the person's stays the person's to close.
                if mine(self, tab).is_none_or(|i| !self.tabs[i].agent) {
                    let _ = reply.send(no_tab(tab));
                    return;
                }
                self.close(tab);
                let _ = reply.send(Ok(Value::Null));
            }
            Op::End => {
                self.end_agent_session(&who);
                let _ = reply.send(Ok(Value::Null));
            }
            Op::Download { url, tab } => {
                if !url.starts_with("http") {
                    let _ = reply.send(Err("only http(s) URLs".into()));
                    return;
                }
                let tab = tab.filter(|t| mine(self, *t).is_some()).unwrap_or(0);
                let page = self.idx(tab).map(|i| self.tabs[i].url.clone()).unwrap_or_default();
                // It goes to this agent's own folder under "Agent downloads", and is
                // the agent's to see.
                if self.agent_dl.len() > 500 {
                    self.agent_dl.clear();
                }
                self.agent_dl.insert(url.clone(), who.downloads.clone());
                let _ = self.ctx.proxy.send_event(Ev::Handoff(tab, url, page, String::new()));
                let _ = reply.send(Ok(Value::Null));
            }
            Op::Downloads => {
                // This agent's own files: its folder, or a download it asked for. The
                // person's downloads and other agents' files are not its business.
                let dir = agent_policy::agent_downloads_dir_for(&who.downloads);
                let list: Vec<Value> = self
                    .downloads
                    .iter()
                    .filter(|d| !d.private && agent_policy::is_agent_download(&dir, &d.path, &d.url, |u| self.agent_dl.get(u) == Some(&who.downloads)))
                    .map(|d| json!({"url": d.url, "path": d.path, "state": d.state, "size": d.received, "total": d.total, "note": d.note}))
                    .collect();
                let _ = reply.send(Ok(Value::Array(list)));
            }
        }
    }

    /// The end of one agent session: its tabs close and its profile data is wiped. An
    /// ephemeral session leaves nothing behind: not a cookie, not a stored page. A kept
    /// profile (`--session NAME`) keeps its cookies and its folder: that is the point.
    fn end_agent_session(&mut self, who: &agent::Who) {
        let ids: Vec<u64> = self.tabs.iter().filter(|t| t.agent && t.owner == who.session).map(|t| t.id).collect();
        for id in ids {
            self.close(id);
        }
        if who.profile.is_empty() || who.keeps {
            return;
        }
        engine::purge_profile(&self.ctx, &who.profile, self.settings.agent_strict_isolation);
    }

    /// Closing everything an agent opened, and wiping what each session left: the person
    /// shut the door, and the agents have no business keeping data after that.
    fn end_all_agent_sessions(&mut self) {
        // What each session left behind, before its tabs go: (profile, strict, kept).
        let sessions: Vec<(String, bool, bool)> = self
            .tabs
            .iter()
            .filter(|t| t.agent && !t.profile.is_empty())
            .map(|t| {
                let keeps = self.agent.session(&t.owner).is_some_and(|w| w.keeps);
                (t.profile.clone(), t.own_env, keeps)
            })
            .collect();
        let ids: Vec<u64> = self.tabs.iter().filter(|t| t.agent).map(|t| t.id).collect();
        for id in ids {
            self.close(id);
        }
        let strict = self.settings.agent_strict_isolation;
        for (profile, own, keeps) in sessions {
            // A kept profile is a place the person asked for; the door shutting does not
            // delete it. Everything else is wiped.
            if !keeps {
                engine::purge_profile(&self.ctx, &profile, strict && own);
            }
        }
    }

    // ---------------------------------------------------------------- drive

    /// Start a Drive run: the person's task goes to an agent CLI Lumen starts itself,
    /// with Lumen as its MCP server. The run gets its own session, its own profile and
    /// its own download folder; nothing of it touches the person's tabs or logins.
    fn drive_start(&mut self) {
        let task = self.chrome.drive_task();
        self.drive_start_task(task);
    }

    /// A Drive run for this task: from the Drive panel, or a "New task" message.
    fn drive_start_task(&mut self, task: String) {
        if self.drive.as_ref().is_some_and(|d| d.exit.is_none()) {
            self.chrome.flash("a Drive run is going; stop it first".into());
            self.push();
            return;
        }
        if task.trim().is_empty() {
            self.chrome.flash("type a task in the Drive panel first".into());
            self.push();
            return;
        }
        if !self.settings.agent {
            self.chrome.flash("the AI agent door is off: turn on Settings > AI agent first".into());
            self.push();
            return;
        }
        let Some((program, args)) = store::drive_command(&self.settings.drive_cmd, &task) else {
            self.chrome.flash("the Drive command in Settings needs to say where {task} goes".into());
            self.push();
            return;
        };
        self.drive_n += 1;
        let name = format!("drive-{}", self.drive_n);
        let out: Arc<Mutex<std::collections::VecDeque<String>>> = Arc::new(Mutex::new(Default::default()));
        let finished = Arc::new(AtomicBool::new(false));
        // The run's own session: the CLI labels itself cli-<name>, asks for a profile of
        // that name, and finds this Lumen through LUMEN_DATA. Nothing is shared.
        let env = |cmd: &mut std::process::Command| {
            cmd.env("LUMEN_SESSION", &name)
                .env("LUMEN_DATA", store::data_dir())
                .env("LUMEN_DOWNLOADS", store::download_dir())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashes up
        };
        let mut cmd = std::process::Command::new(&program);
        cmd.args(&args);
        env(&mut cmd);
        let child = cmd.spawn().or_else(|_| {
            // A CLI that is a .cmd shim (npm) is not a program Windows can start itself:
            // hand the whole line to cmd.
            let line = self.settings.drive_cmd.replace("{task}", &store::drive_prompt(&task));
            let mut c = std::process::Command::new("cmd");
            c.arg("/C").arg(&line);
            env(&mut c);
            c.spawn()
        });
        let child = match child {
            Ok(c) => c,
            Err(e) => {
                self.chrome.flash(format!("could not start the Drive command: {e}"));
                self.push();
                return;
            }
        };
        let mut child = child;
        if let Some(pipe) = child.stdout.take() {
            let (o, f) = (out.clone(), finished.clone());
            let _ = std::thread::Builder::new().name("lumen-drive-out".into()).spawn(move || drive_pipe(pipe, o, f));
        }
        if let Some(pipe) = child.stderr.take() {
            let o = out.clone();
            let _ = std::thread::Builder::new().name("lumen-drive-err".into()).spawn(move || drive_pipe(pipe, o, Arc::new(AtomicBool::new(false))));
        }
        journal::feature("drive", false);
        // A task the person typed does not belong in the journal: only that it ran.
        journal::error("agent", "drive run started");
        self.drive = Some(Drive {
            name,
            task,
            child,
            out,
            finished,
            started: Instant::now(),
            session: None,
            paused: false,
            exit: None,
        });
        self.next_drive = Instant::now();
        self.panel = Panel::Drive;
        self.chrome.flash("Drive run started: its tabs show up in the sidebar under its name".into());
        self.push();
    }

    /// Stop the run: the CLI is killed, its session ends, and its profile data goes.
    fn drive_stop(&mut self, why: &str) {
        let Some(d) = self.drive.as_mut() else { return };
        if d.exit.is_some() {
            self.chrome.flash("nothing is running".into());
            self.push();
            return;
        }
        let _ = d.child.kill();
        d.exit = Some(-1);
        let (name, session) = (d.name.clone(), d.session.clone());
        // Handoff is waiting on that tab: the run is over, so it ends too.
        if self.handoff.as_ref().is_some_and(|h| self.idx(h.0).is_some_and(|i| self.tabs[i].owner == session.clone().unwrap_or_default())) {
            if let Some((_, _, _, r)) = self.handoff.take() {
                let _ = r.send(Err("the Drive run was stopped".into()));
            }
        }
        if let Some(sid) = session
            && let Some(who) = self.agent.end_session(&sid)
        {
            self.drive_end_session(&who);
        }
        self.chrome.flash(format!("{name} stopped: {why}"));
        self.push();
    }

    /// A Drive run is over: its tabs close and its profile data is deleted. A run is not
    /// a worker to come back to; what it downloaded under its own name stays.
    fn drive_end_session(&mut self, who: &agent::Who) {
        let own = self.tabs.iter().find(|t| t.agent && t.owner == who.session).is_some_and(|t| t.own_env);
        self.end_agent_session(who);
        if !who.profile.is_empty() {
            engine::purge_profile(&self.ctx, &who.profile, own);
        }
    }

    /// "Pause and take over": the run's tab comes to the front, the agent may only ask
    /// for it back (browser_handoff), and Done hands it over again.
    fn drive_pause(&mut self) {
        let Some(d) = self.drive.as_mut() else { return };
        let Some(sid) = d.session.clone() else {
            self.chrome.flash("the run has not opened a tab yet".into());
            self.push();
            return;
        };
        d.paused = true;
        self.agent.set_paused(&sid, true);
        let tab = self.tabs.iter().find(|t| t.agent && t.owner == sid).map(|t| t.id);
        let name = d.name.clone();
        if let Some(id) = tab {
            self.activate(id);
            // The person holds the tab until they press Done; a pause is a handoff.
            let (tx, _rx) = std::sync::mpsc::channel();
            self.handoff = Some((id, name.clone(), "Paused: the tab is yours. Press Done when the run may have it back.".into(), tx));
        }
        self.chrome.flash(format!("{name} paused; it may only ask for the tab back now"));
        self.push();
    }

    /// A look at the run, while it is going: its output, its session, its end.
    fn drive_poll(&mut self) {
        self.next_drive = Instant::now() + Duration::from_millis(500);
        let Some(d) = self.drive.as_mut() else { return };
        if d.session.is_none() {
            // The CLI opens its session on its first call; its label is cli-<name>.
            d.session = self.agent.session_by_label(&format!("cli-{}", d.name)).map(|w| w.session);
        }
        if d.exit.is_some() || !d.finished.load(Ordering::Relaxed) {
            return;
        }
        let code = d.child.try_wait().ok().flatten().and_then(|s| s.code());
        d.exit = Some(code.unwrap_or(-1));
        let (name, session) = (d.name.clone(), d.session.clone());
        // What the person queued for after this run is the next run's task.
        let next = session.as_deref().map(|s| self.agent.take_later(s)).unwrap_or_default();
        // The run is over: its tabs close and its profile data goes.
        if let Some(sid) = session
            && let Some(who) = self.agent.end_session(&sid)
        {
            self.drive_end_session(&who);
        }
        // Nothing of the run is left to remember but its own step list, which the panel
        // still shows; the session's state goes when a new run starts.
        self.chrome.flash(format!("Drive run {name} finished"));
        if !next.is_empty() {
            self.drive_start_task(next.join("\n"));
        }
        self.push();
    }

    /// What the Drive panel shows: this run's steps and the tail of the CLI's words.
    /// Filled only while the panel is open.
    fn drive_view(&self) -> Option<chrome::DriveView> {
        if self.panel != Panel::Drive {
            return None;
        }
        let d = self.drive.as_ref()?;
        let steps = d.session.as_deref().map(|s| self.agent.steps(s)).unwrap_or_default();
        let log: Vec<String> = d.out.lock().unwrap_or_else(|e| e.into_inner()).iter().rev().take(12).rev().cloned().collect();
        Some(chrome::DriveView {
            name: d.name.clone(),
            running: d.exit.is_none(),
            paused: d.paused,
            task: d.task.clone(),
            elapsed: d.ago(),
            steps,
            log,
            exit: d.exit,
        })
    }

    /// Clearing everything also clears the site icons: they are a list of the sites that were
    /// visited. A partial clear leaves them (they carry no times to cut by).
    // ----------------------------------------------------------------- sites

    /// The zoom a tab's page should be at, in percent. Pages that are not websites stay at 100.
    fn wanted_zoom(&self, i: usize) -> u32 {
        sites::zoom_host(&self.tabs[i].url).map_or(100, |h| sites::zoom_for(&self.zooms, self.settings.default_zoom, &h))
    }

    /// Put a tab's page at `pct`. The change event this causes is Lumen's own, not the person's.
    fn put_zoom(&mut self, i: usize, pct: u32) {
        let t = &mut self.tabs[i];
        t.zoom = pct;
        let Some(v) = &t.view else { return };
        if engine::zoom_factor(v).map(sites::pct_of) != Some(pct) {
            t.zoom_set = Some(pct);
            engine::set_zoom_factor(v, sites::factor_of(pct));
        }
    }

    /// Every tab of the host `host` goes to the zoom the site has now (or the default).
    fn follow_zoom(&mut self, host: &str) {
        for i in 0..self.tabs.len() {
            if sites::zoom_host(&self.tabs[i].url).as_deref() == Some(host) {
                let want = self.wanted_zoom(i);
                self.put_zoom(i, want);
            }
        }
    }

    /// The person zoomed tab `i` to `pct`: the site keeps it. Private and agent tabs keep nothing.
    fn remember_zoom(&mut self, i: usize, pct: u32) {
        let t = &self.tabs[i];
        if self.off_record(t) {
            return;
        }
        let Some(host) = sites::zoom_host(&t.url) else { return };
        if sites::remember_zoom(&mut self.zooms, self.settings.default_zoom, &host, pct) {
            store::save_zooms(&self.zooms);
        }
        self.follow_zoom(&host);
    }

    /// A plain http address was stopped. If the site answers on the https port, go there; if
    /// it does not, say so and let the person choose. Asked once per site and run.
    fn upgrade(&mut self, id: u64, secure: String, plain: String) {
        let Some(i) = self.idx(id) else { return };
        let host = sites::exception_host(&plain).unwrap_or_default();
        self.tabs[i].no_https = None;
        self.tabs[i].url = secure.clone();
        if self.probed.contains(&host) {
            if let Some(v) = &self.tabs[i].view {
                let _ = v.load_url(&secure);
            }
        } else {
            // The spinner runs while the port is tried; the answer comes back as an event.
            self.tabs[i].loading = true;
            let proxy = self.ctx.proxy.clone();
            let target = format!("{}:443", host.trim_start_matches('['));
            std::thread::Builder::new()
                .name("lumen-https-probe".into())
                .spawn(move || {
                    let answer = sites::probe(&target, Duration::from_secs(4));
                    let _ = proxy.send_event(Ev::Probed(id, secure, plain, answer));
                })
                .ok();
        }
        self.dirty = true;
        self.push();
    }

    fn probed(&mut self, id: u64, secure: String, plain: String, answer: sites::Probe) {
        let Some(i) = self.idx(id) else { return };
        // The person typed something else while the port was tried: that wins.
        if self.tabs[i].url != secure || self.tabs[i].no_https.is_some() {
            return;
        }
        let host = sites::exception_host(&plain).unwrap_or_default();
        let t = &mut self.tabs[i];
        t.loading = false;
        match answer {
            sites::Probe::Shut => {
                // No page was loaded: the warning is the first thing this tab shows, so Back
                // goes to the page before it.
                t.no_https = Some(plain.clone());
                t.url = plain.clone();
                t.title = format!("{} does not use https", host);
                if let Some(v) = &t.view {
                    let _ = v.load_html(&sites::interstitial_html(&plain, self.ctx.dark.get()));
                }
                journal::feature("https_first_warning", t.private);
            }
            other => {
                if other == sites::Probe::Open {
                    self.probed.insert(host);
                }
                if let Some(v) = &t.view {
                    let _ = v.load_url(&secure);
                }
            }
        }
        self.dirty = true;
        self.push();
    }

    /// A click on the warning page. It only counts while this tab shows that page for that address.
    fn http_choice(&mut self, id: u64, choice: sites::HttpChoice) {
        use sites::HttpChoice::*;
        let Some(i) = self.idx(id) else { return };
        let Some(shown) = self.tabs[i].no_https.clone() else { return };
        let (url, always) = match choice {
            Back => {
                self.tabs[i].no_https = None;
                if self.tabs[i].back {
                    if let Some(v) = &self.tabs[i].view {
                        let _ = v.go_back();
                    }
                } else if self.tabs.len() > 1 {
                    self.close(id);
                }
                return;
            }
            Once(u) => (u, false),
            Always(u) => (u, true),
        };
        if url != shown {
            return;
        }
        let Some(host) = sites::exception_host(&url) else { return };
        // Agents and private tabs change nothing that outlives them.
        if always && !self.tabs[i].agent && !self.tabs[i].private {
            let mut st = self.settings.clone();
            st.http_ok.push(host);
            self.apply_settings(st);
        } else {
            self.ctx.https.borrow_mut().passes.insert(host);
        }
        self.tabs[i].no_https = None;
        self.tabs[i].url = url.clone();
        if let Some(v) = &self.tabs[i].view {
            let _ = v.load_url(&url);
        }
        self.dirty = true;
        self.push();
    }

    /// A navigation hit a host on the malware or phishing lists. Stop, show the
    /// warning, count it once per shown warning.
    fn danger(&mut self, id: u64, url: String) {
        let Some(i) = self.idx(id) else { return };
        let host = sites::host_of(&url);
        let t = &mut self.tabs[i];
        t.no_danger = Some(url.clone());
        t.url = url.clone();
        t.title = format!("{host} is a dangerous site");
        if let Some(v) = &t.view {
            let _ = v.load_html(&sites::danger_html(&url, self.ctx.dark.get()));
        }
        privacy_stats::danger(&host, privacy_stats::Who::new(id, t.private, t.agent));
        self.dirty = true;
        self.push();
    }

    /// A click on the danger page. It only counts while this tab shows that page.
    fn danger_choice(&mut self, id: u64, choice: sites::DangerChoice) {
        let Some(i) = self.idx(id) else { return };
        let Some(shown) = self.tabs[i].no_danger.clone() else { return };
        match choice {
            sites::DangerChoice::Back => {
                self.tabs[i].no_danger = None;
                if self.tabs[i].back {
                    if let Some(v) = &self.tabs[i].view {
                        let _ = v.go_back();
                    }
                } else if self.tabs.len() > 1 {
                    self.close(id);
                }
            }
            sites::DangerChoice::Continue(url) => {
                if url != shown {
                    return;
                }
                // Agents and private tabs change nothing that outlives them.
                if !self.tabs[i].agent && !self.tabs[i].private {
                    blocker::danger_pass(&url);
                }
                self.tabs[i].no_danger = None;
                self.tabs[i].url = url.clone();
                if let Some(v) = &self.tabs[i].view {
                    let _ = v.load_url(&url);
                }
            }
        }
        self.dirty = true;
        self.push();
    }

    /// Open the address bar's lock: ask how many cookies the site has.
    fn ask_site_cookies(&mut self) {
        let Some(t) = self.active_tab() else { return };
        let (id, url) = (t.id, t.url.clone());
        let Some(v) = t.view.as_ref() else { return };
        let proxy = self.ctx.proxy.clone();
        engine::cookie_count(v, &url, move |n| {
            let _ = proxy.send_event(Ev::SiteCookies(id, n));
        });
    }

    /// Forget one site's cookies and stored data. Never the InPrivate store: it ends with
    /// the last private tab anyway. Needs a page of the person's own profile to ask.
    fn clear_site(&mut self, origin: &str) {
        if !origin.starts_with("http") {
            return;
        }
        let Some(v) = self.plain_view() else {
            self.chrome.flash("Open a page first: clearing needs one".into());
            return;
        };
        let (proxy, o) = (self.ctx.proxy.clone(), origin.to_string());
        engine::clear_site(v, origin, move |ok| {
            let _ = proxy.send_event(Ev::SiteCleared(o, ok));
        });
    }

    fn forget_site_favs(&mut self, since: Option<u64>) {
        if since.is_none() && !self.site_favs.is_empty() {
            self.site_favs.clear();
            sitefavs::save(&self.site_favs);
        }
    }

    /// Sites to offer on the new tab page while it has no tiles: the ones seen most. Only worked
    /// out when the page is showing and has nothing else to show.
    fn speed_dial_suggestions(&self) -> Vec<store::Shortcut> {
        let showing = self.panel == Panel::None && self.active_tab().is_some_and(|t| t.status() == "new");
        if !showing || !self.settings.speed_dial || !self.settings.shortcuts.is_empty() {
            return Vec::new();
        }
        let seen: Vec<speeddial::Seen> =
            self.library.top_visits(60).into_iter().filter(|v| !v.url.is_empty()).map(|v| speeddial::Seen { url: v.url }).collect();
        speeddial::suggest(&seen, &self.settings.shortcuts, 8)
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
        self.library.clear_history(None);
        self.forget_site_favs(None);
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
        if std::mem::take(&mut self.site_favs_dirty) {
            sitefavs::save(&self.site_favs);
        }
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
