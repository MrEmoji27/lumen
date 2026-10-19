//! Native browser chrome. One child window, drawn with Direct2D.
//!
//! Rust state comes in through `set_state` (at most once a frame). User actions go
//! out as the same small JSON messages the old HTML UI sent, so main.rs handles
//! them unchanged. The address bar and new-tab search are real Win32 edit boxes;
//! everything else is drawn.

mod gfx;
pub mod layout;
mod motion;
mod pages;
mod report;
mod dashboard;
mod color;
mod form;
mod confirm;
mod menu;
pub mod nowebview;
mod files;
#[cfg(debug_assertions)]
mod shot;
mod style;
#[cfg(debug_assertions)]
pub use shot::script as test_script;
pub mod theme;

/// The layouts and their chrome heights, for main.rs: it lays the window out and
/// the chrome draws with the same numbers.
pub use layout::{Kind, Metrics};
/// The chrome's easing, for the things main.rs moves itself (the sidebar width).
pub use motion::ease_out;

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Mutex, time::Instant};

use serde_json::{Value, json};
use tao::event_loop::EventLoopProxy;
use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CLEARTYPE_QUALITY, CombineRgn, CreateRectRgn, CreateRoundRectRgn, RGN_DIFF, RGN_OR, SetWindowRgn, ScreenToClient, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateFontW,
            BLACKNESS, CreateSolidBrush, DEFAULT_CHARSET, DeleteDC, DeleteObject, EndPaint, FF_DONTCARE, FW_NORMAL, GetTextFaceW,
            HBRUSH, HDC, HFONT, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PatBlt, SelectObject, SetBkColor, SetTextColor,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{EM_GETSEL, EM_SETCUEBANNER, EM_SETMARGINS, EM_SETSEL, WM_MOUSELEAVE},
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{
                GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_CONTROL,
                VK_BACK, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_F10, VK_MENU, VK_RETURN, VK_SHIFT, VK_TAB, VK_UP,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::{
    engine::Ev,
    speeddial,
    store::{Settings, Shortcut},
};
use gfx::{Align, Font, Gfx, R};

use motion::{Ease, Id, Motion, Prop};
use style::Look;
use theme::Theme;

// ------------------------------------------------------------------ memory

/// The memory figure where the number warms toward red and a glow starts to
/// grow behind it: one gigabyte.
const MEM_WARN: u64 = 1 << 30;
/// The figure where the glow is full and breathes: two gigabytes.
const MEM_HOT: u64 = 2 << 30;
/// How long one breath of that glow takes.
const MEM_BREATH: f32 = 4.0;

/// The memory figure in short form: "312M", "1.2G". The compact column has no
/// room for the long one.
fn mem_short(bytes: u64) -> String {
    let mb = bytes as f64 / 1048576.0;
    if mb >= 1024.0 {
        format!("{:.1}G", mb / 1024.0)
    } else {
        format!("{}M", mb.round() as u64)
    }
}

// ------------------------------------------------------------------- state

#[derive(Clone, Debug)]
pub struct UiTab {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub back: bool,
    pub fwd: bool,
    pub pinned: bool,
    /// A pin that is not on the page it was pinned on: the menu offers the way back.
    pub pin_away: bool,
    pub audio: bool,
    pub muted: bool,
    /// In a call; drawn as a microphone where the sound mark goes.
    pub call: bool,
    pub status: &'static str,
    pub agent: bool,
    /// The person's tab, which they let agents use (right-click). Marked in the sidebar.
    pub agent_ok: bool,
    pub reader: bool,
    /// InPrivate tab. Marked in the sidebar, never saved anywhere.
    pub private: bool,
    /// This tab is one half of a split: the stage shows it beside another.
    pub split: bool,
    /// The page is showing in the target language.
    pub translated: bool,
    /// A translation of it is being worked on.
    pub translating: bool,
    /// The page runs in Servo rather than WebView2.
    pub servo: bool,
    /// Agent tabs: the MCP session that owns the tab, and the name its client gave.
    /// The sidebar groups agent tabs by owner and puts a Stop on each group.
    pub owner: String,
    pub owner_name: String,
}

#[derive(Clone, Debug)]
pub struct UiDl {
    pub id: u64,
    pub name: String,
    pub url: String,
    pub received: i64,
    pub total: i64,
    pub state: &'static str,
    pub speed: u64,
    pub note: String,
    /// "file" | "torrent" | "video"
    pub kind: &'static str,
    /// Upload speed, bytes per second.
    pub up: u64,
    /// Connected peers (torrents) or connections (files).
    pub peers: u64,
    pub seeds: u64,
    pub ratio: f64,
}

#[derive(Clone, Debug)]
pub struct UiState {
    pub tabs: Vec<UiTab>,
    pub active: u64,
    pub settings: Settings,
    pub dark: bool,
    pub maximized: bool,
    pub panel: &'static str,
    pub peek: bool,
    /// The chrome window covers the whole window (new tab page or a panel).
    pub full: bool,
    /// Page rectangle in chrome coordinates (DIPs).
    pub stage: (f32, f32, f32, f32),
    pub mem: Option<(u64, u32)>,
    /// The tab whose sound the footer's now-playing card follows.
    pub media: Option<u64>,
    pub blocked: u64,
    pub cpu_cap: bool,
    /// Battery saver is in force: hidden tabs sleep sooner, motion is reduced, a leaf shows.
    pub battery: bool,
    /// The active tab's page is asking the person something: the card over the page.
    pub page_ask: Option<UiAsk>,
    pub downloads: Vec<UiDl>,
    pub agent_busy: bool,
    /// Per agent session: what became of the person's last message to it, in words.
    pub agent_notes: Vec<(String, String)>,
    pub agent_cmd: String,
    /// A site waiting for a permission answer: (request id, origin, what).
    pub prompt: Option<(u64, String, &'static str)>,
    /// An agent waiting for the person: (agent name, message).
    pub handoff: Option<(String, String)>,
    /// Remembered answers: (origin, what, allowed).
    pub rules: Vec<(String, String, bool)>,
    /// Workspace names, for the switcher at the top of the sidebar.
    pub workspaces: Vec<String>,
    /// Index into `workspaces` of the active one.
    pub active_workspace: usize,
    /// A glance card is open over the page.
    pub glance: bool,
    /// The active page is bookmarked.
    pub bookmarked: bool,
    /// Sites seen most, for the new tab page while it has no tiles. Filled only then.
    pub suggestions: Vec<Shortcut>,
    /// The active page's bookmark, for the star's menu.
    pub mark: Option<UiMark>,
    /// Bookmark folder names, in order. "Unsorted" is not one: it is the empty name.
    pub bm_folders: Vec<String>,
    /// Read aloud in reader view: "" | "ready" | "playing" | "paused".
    pub listen: &'static str,
    /// KeePass entries matching the active site: labels only, empty when locked.
    pub kdbx: Vec<String>,
    /// Entries in the open KeePass database; 0 = no database open.
    pub vault: usize,
    /// Filled only while the History page is open.
    pub history: Vec<UiVisit>,
    /// Filled only while the Bookmarks page is open.
    pub bookmarks: Vec<UiMark>,
    /// Filled only while the report page is open.
    pub report: Option<crate::privacy_stats::Snapshot>,
    /// Filled only while the dashboard is open: today's privacy counts.
    pub privacy_today: Option<crate::privacy_stats::Counts>,
    /// A download ended while the page was closed: a small dot on the footer button.
    pub dl_unseen: bool,
    /// The Drive panel's run: steps, the agent's own words. Filled only while it is open.
    pub drive: Option<DriveView>,
    /// The problem report, plain text, while its page is open.
    pub bug_report: Option<String>,
    /// That report has been sent.
    pub bug_sent: bool,
    /// The run before this one never said goodbye: the start page offers a report.
    pub unclean_last: bool,
    /// Whether the torrent port is mapped via UPnP (only meaningful while torrents run).
    pub upnp_port_open: bool,
    /// The active page's zoom, in percent.
    pub zoom: u32,
    /// Sites with a zoom of their own: (host, percent).
    pub zooms: Vec<(String, u32)>,
    /// Cookies the active page's site has, once the lock has asked.
    pub site_cookies: Option<usize>,
    /// Tabs of the other workspaces, for the tab search. Private tabs never appear here:
    /// they are not kept when a workspace is put away.
    pub other_tabs: Vec<UiOther>,
}

/// A tab in a workspace that is not showing: enough to find it and bring it back.
#[derive(Clone, Debug)]
pub struct UiOther {
    pub ws: usize,
    /// Its place among that workspace's saved tabs.
    pub idx: usize,
    pub ws_name: String,
    pub title: String,
    pub url: String,
}

/// What the Drive panel shows of one run: whether it is going, its step list (the tool
/// calls, newest last), the tail of what the agent said, and how it ended.
#[derive(Clone, Debug, Default)]
pub struct DriveView {
    pub name: String,
    pub running: bool,
    pub paused: bool,
    /// The person's words, as typed.
    pub task: String,
    /// How long the run has been going, e.g. "4m".
    pub elapsed: String,
    /// The steps in plain words, each with its time in ms once it is over.
    pub steps: Vec<(String, Option<u64>)>,
    pub log: Vec<String>,
    /// The CLI's exit code, once it is over.
    pub exit: Option<i32>,
}

/// One agent's block in the sidebar list: its group header (a tab of that session, for
/// the owner id and the name) and the tabs it opened.
struct AgentGroup<'a> {
    head: &'a UiTab,
    tabs: Vec<&'a UiTab>,
}

/// One line of the sidebar list: a tab, or the header of an agent's block.
enum Row<'a> {
    Tab(&'a UiTab),
    Header(&'a UiTab),
}

/// The agent tabs, grouped by session, in the order the sessions first appear.
fn agent_groups(s: &UiState) -> Vec<AgentGroup<'_>> {
    let mut out: Vec<AgentGroup> = Vec::new();
    for t in s.tabs.iter().filter(|t| t.agent && !t.pinned) {
        match out.iter_mut().find(|g| g.head.owner == t.owner) {
            Some(g) => g.tabs.push(t),
            None => out.push(AgentGroup { head: t, tabs: vec![t] }),
        }
    }
    out
}

/// A colour per agent, from the session id: the dot on its group header, and nothing
/// else. Two agents in the sidebar are told apart at a glance.
pub(crate) fn agent_color(owner: &str) -> theme::Color {
    const DOTS: [(f32, f32, f32); 6] = [
        (0.47, 0.67, 1.00),
        (1.00, 0.67, 0.47),
        (0.59, 0.86, 0.59),
        (0.90, 0.59, 0.90),
        (0.94, 0.82, 0.47),
        (0.55, 0.84, 0.88),
    ];
    let h = owner.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let (r, g, b) = DOTS[(h % DOTS.len() as u32) as usize];
    theme::Color(r, g, b, 1.0)
}

#[derive(Clone, Debug)]
pub struct UiMark {
    pub id: u64,
    pub url: String,
    pub title: String,
    /// Empty is Unsorted.
    pub folder: String,
}

#[derive(Clone, Debug)]
pub struct UiVisit {
    pub url: String,
    pub title: String,
    pub day: String,
    pub time: String,
}

impl UiState {
    fn active_tab(&self) -> Option<&UiTab> {
        self.tabs.iter().find(|t| t.id == self.active)
    }
    fn compact(&self) -> bool {
        self.settings.compact && !self.peek
    }
    /// Height the workspace row takes. One workspace has nothing to switch
    /// to, so the row only shows for two or more, or to hold "Open as tab".
    fn ws_h(&self) -> f32 {
        if self.compact() || (self.workspaces.len() < 2 && !self.glance) { 0.0 } else { 32.0 }
    }
    pub fn sidebar_w(&self) -> f32 {
        if self.compact() { 56.0 } else { self.settings.sidebar_width as f32 }
    }
}

/// Everything clickable. Drawn regions register one of these; the mouse looks them up.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Drag,
    Back,
    Fwd,
    Reload,
    Min,
    Max,
    Close,
    Omni,
    Tab(u64),
    TabX(u64),
    TabSnd(u64),
    /// Picture-in-picture for the tab's biggest playing video.
    TabPip(u64),
    /// The footer's now-playing card: pause, next, and the title that switches to the tab.
    MediaPlay,
    MediaNext,
    MediaTitle(u64),
    NewTab,
    Gear,
    /// The sidebar button: compact on or off, same as Ctrl+B.
    Sidebar,
    Mem,
    DlFooter,
    /// The arrow in the footer: the Drive panel.
    DriveFooter,
    AgentStop,
    /// The Stop button on one agent's group header, for that MCP session.
    AgentStopSession(String),
    /// The speech button on an agent's group row: a message for it. (session, its name)
    AgentMessage(String, String),
    PermAllow(u64),
    HandoffDone,
    PermBlock(u64),
    Star,
    Listen,
    ListenStop,
    KdbxFill,
    Sugg(usize),
    Shortcut(usize),
    /// A suggested site on the speed dial, and its "keep it" button.
    DialSugg(usize),
    DialPin(usize),
    /// The "+" cell of the speed dial.
    DialAdd,
    NtpSearch,
    /// Page widgets (settings and Control live in pages.rs).
    Page(pages::PageHit),
    /// A widget of the report page (report.rs).
    Report(report::Click),
    /// A tile or button of the dashboard (dashboard.rs).
    Dash(dashboard::Click),
    /// The grid button in the footer: the dashboard.
    DashFooter,
    /// Clicked a workspace name in the switcher.
    Workspace(usize),
    /// "Open as tab" button on the glance card.
    GlanceOpen,
    /// The "+" button next to workspaces to add a new one.
    WorkspaceAdd,
    /// The toolbar's menu button: settings, history, bookmarks, control, dashboard.
    Menu,
    /// The small workspaces switcher in the horizontal tab strip.
    WsChip,
    /// The lock (or globe) in the address bar: the site info popover.
    SiteInfo,
    /// A permission answer in the site popover: (kind, "allow" | "block" | "ask").
    SitePerm(String, &'static str),
    /// "Clear data for this site" in the site popover.
    SiteClear,
    /// "Site settings…" in the site popover: open the Sites page.
    SiteSettings,
    /// The ad-block switch in the site popover: this site is in or out of the off list.
    SiteAdblock,
    /// The zoom chip in the address bar: click takes the page back to 100%.
    ZoomChip,
    /// A row of the tab search card.
    TabSearch(usize),
    /// The tab search card itself: clicks on it are not clicks outside it.
    SearchCard,
    /// A row of the command palette.
    Pal(usize),
    /// The command palette card itself: clicks on it are not clicks outside it.
    PaletteCard,
    /// The message card itself, its kinds, Send and its close button.
    SayCard,
    SayMode(usize),
    SaySend,
    SayClose,
    /// The colour picker's card: the card, its square, its strip, its swatches.
    ColorCard,
    ColorArea,
    ColorHue,
    ColorPreset(usize),
    ColorRecent(usize),
    ColorDone,
    /// The form card: the card itself, and its two buttons.
    FormCard,
    FormSave,
    FormCancel,
    /// The question card: the card itself, and its buttons.
    ConfirmCard,
    ConfirmButton(usize),
    /// The menu card: the card itself, and its rows.
    MenuCard,
    MenuRow(usize),
    /// The file card: the card itself, its rows, places, drives and buttons.
    FileCard,
    FileRow(usize),
    FilePlace(usize),
    FileDrive(usize),
    FileUp,
    FileGo,
    FileCancel,
    FilePath,
    FileName,
}

#[derive(Clone, Debug)]
struct Sugg {
    kind: SuggKind,
    label: String,
    /// A second line: the words a page-text match was found in.
    detail: String,
}

#[derive(Clone, Debug, PartialEq)]
enum SuggKind {
    Go(String),
    Search(String),
    Tab(u64),
    /// A tab in a workspace that is put away: (workspace, index among its saved tabs).
    TabOther(usize, usize),
    /// From history (false) or bookmarks (true).
    Url(String, bool),
}

/// One cell of the speed dial.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Cell {
    /// A tile you keep (index into the settings' shortcuts).
    Tile(usize),
    /// A site you visit a lot, offered while you keep none (index into the suggestions).
    Sugg(usize),
    /// The "+" cell.
    Add,
}

/// 1284 as "1,284": a count read at a glance.
fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What a full page sits on: the look's ground, or in Glass a dark see-through pane over
/// the wallpaper.
fn page_ground(lo: &Look) -> theme::Color {
    if lo.style == theme::Style::Glass { theme::Color(0.05, 0.04, 0.09, 0.55) } else { lo.theme.bg }
}

/// A start-page site card's height (the cell is this and a 12 DIP gap).
const NTP_CARD_H: f32 = 112.0;

/// The start page's search bar fill: ink in Signal, a frosted pane in Glass, a card in Precise.
fn ntp_bar_fill(lo: &Look) -> theme::Color {
    match lo.style {
        theme::Style::Signal => lo.theme.pill,
        theme::Style::Glass => theme::Color(1.0, 1.0, 1.0, 0.12),
        theme::Style::Precise => lo.theme.raise,
    }
}

/// Where a speed-dial cell goes. The default sizes are `speeddial`'s own grid;
/// a tile size or column count the person chose gets the same shapes with their
/// numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
enum DialGrid {
    /// As many columns as fit, the tuned cell size.
    Classic(speeddial::Grid),
    /// A size the settings asked for.
    Sized { cols: usize, cell_w: f32, n: usize, x0: f32, row_h: f32 },
}

impl DialGrid {
    /// `left`/`width` is the room; `tile` the tile size; `cols` 0 means "as the
    /// width allows".
    fn new(left: f32, width: f32, n: usize, tile: f32, cols: u32) -> DialGrid {
        if cols == 0 && (tile - 60.0).abs() < 0.5 {
            return DialGrid::Classic(speeddial::grid_in(left, width, n));
        }
        const GAP: f32 = 12.0;
        let cell_min = tile + 28.0;
        let fit = ((width + GAP) / (cell_min + GAP)).floor().max(1.0) as usize;
        let cols = if cols > 0 { (cols as usize).clamp(1, 10).min(n.max(1)) } else { fit.clamp(3, 8).min(n.max(1)) };
        let cell_w = ((width - GAP * (cols as f32 - 1.0)) / cols as f32).clamp(48.0, 200.0);
        let full = cols as f32 * cell_w + (cols as f32 - 1.0) * GAP;
        DialGrid::Sized { cols, cell_w, n, x0: left + (width - full).max(0.0) / 2.0, row_h: tile + 44.0 }
    }

    fn cell_w(&self) -> f32 {
        match self {
            DialGrid::Classic(g) => g.cell_w,
            DialGrid::Sized { cell_w, .. } => *cell_w,
        }
    }

    fn row_h(&self) -> f32 {
        match self {
            DialGrid::Classic(_) => speeddial::ROW_H,
            DialGrid::Sized { row_h, .. } => *row_h,
        }
    }

    fn cell_count(&self) -> usize {
        match self {
            DialGrid::Classic(g) => g.n,
            DialGrid::Sized { n, .. } => *n,
        }
    }

    fn rows(&self) -> usize {
        match self {
            DialGrid::Classic(g) => g.rows(),
            DialGrid::Sized { cols, n, .. } => n.div_ceil((*cols).max(1)),
        }
    }

    fn height(&self) -> f32 {
        match self {
            DialGrid::Classic(g) => g.height(),
            DialGrid::Sized { .. } => self.rows() as f32 * self.row_h(),
        }
    }

    fn origin(&self, i: usize) -> (f32, f32) {
        match self {
            DialGrid::Classic(g) => g.origin(i),
            DialGrid::Sized { cols, cell_w, n, x0, row_h } => {
                const GAP: f32 = 12.0;
                let (row, col) = (i / (*cols).max(1), i % (*cols).max(1));
                let in_row = if row + 1 == self.rows() { *n - row * *cols } else { *cols };
                let row_w = in_row as f32 * *cell_w + in_row.saturating_sub(1) as f32 * GAP;
                let full_w = *cols as f32 * *cell_w + (*cols as f32 - 1.0) * GAP;
                let lead = *x0 + (full_w - row_w) / 2.0;
                (lead + col as f32 * (*cell_w + GAP), row as f32 * *row_h)
            }
        }
    }

    fn index_at(&self, x: f32, y: f32) -> Option<usize> {
        match self {
            DialGrid::Classic(g) => g.index_at(x, y),
            DialGrid::Sized { .. } => {
                if y < 0.0 {
                    return None;
                }
                (0..self.cell_count()).find(|&i| {
                    let (cx, cy) = self.origin(i);
                    x >= cx && x < cx + self.cell_w() && y >= cy && y < cy + self.row_h()
                })
            }
        }
    }
}

struct NtpGeo {
    field: R,
    grid: DialGrid,
    /// Where the first row of cells starts.
    top: f32,
    cells: Vec<Cell>,
}

/// What the speed dial shows: nothing when it is switched off, your tiles, or, with none, the
/// sites seen most; then the "+" while there is room for one more.
fn dial_cells(s: &UiState) -> Vec<Cell> {
    if !s.settings.speed_dial {
        return Vec::new();
    }
    let mine = s.settings.shortcuts.len().min(speeddial::MAX_TILES);
    // Five suggestions and the "+" make one full row of six.
    let mut cells: Vec<Cell> = if mine > 0 { (0..mine).map(Cell::Tile).collect() } else { (0..s.suggestions.len().min(5)).map(Cell::Sugg).collect() };
    if mine < speeddial::MAX_TILES {
        cells.push(Cell::Add);
    }
    cells
}

/// A tile picked up on the new tab page: it moves when let go over another one.
struct DialDrag {
    from: usize,
    x0: f32,
    y0: f32,
    moved: bool,
}

struct Drag {
    id: u64,
    x0: f32,
    y0: f32,
    dx: f32,
    dy: f32,
    moved: bool,
    pinned: bool,
}

pub(crate) struct Inner {
    hwnd: HWND,
    proxy: EventLoopProxy<Ev>,
    gfx: Gfx,
    pub(crate) theme: Theme,
    /// The style of the moment: theme, fonts, spacing, layout, motion. Rebuilt
    /// from the settings on every state push.
    pub(crate) look: Look,
    pub(crate) state: Option<UiState>,
    hits: Vec<(R, Hit)>,
    hover: Option<Hit>,
    press: Option<Hit>,
    scroll: f32,
    list_max: f32,
    drag: Option<Drag>,
    favs: HashMap<u64, Vec<u8>>,
    /// One icon per site (png bytes, by host), for the speed dial's tiles.
    site_favs: HashMap<String, Vec<u8>>,
    /// The floating cards drawn this frame (rect, corner radius), and the ones the chrome's
    /// window region was last cut to, in device pixels. Empty: the chrome is not raised.
    floats: Vec<(R, f32)>,
    float_layer: Vec<[i32; 5]>,
    /// The disk cache's answer per host, read once: hits stay, misses are asked again
    /// after a while, since a fetch may have landed the icon since.
    disk_favs: HashMap<String, (Option<Vec<u8>>, std::time::Instant, std::time::Instant)>,
    /// Hosts whose icon has been fetched from the site this run, so each is asked once.
    fav_asked: std::collections::HashSet<String>,
    dial_drag: Option<DialDrag>,
    edit: HWND,
    ntp_edit: HWND,
    /// The Bookmarks page's search box.
    bm_edit: HWND,
    /// The Drive panel's task box.
    drive_edit: HWND,
    /// The Drive page just opened: give its task box the cursor once.
    drive_focus: bool,
    edit_font: HFONT,
    /// The family the edit boxes were made with, to remake them when it changes.
    edit_face: String,
    ntp_font: HFONT,
    edit_brush: HBRUSH,
    ntp_brush: HBRUSH,
    /// The start page's search bar: its own fill (ink, in Signal).
    bar_brush: HBRUSH,
    editing: bool,
    /// The address bar was opened while compact hid its edit box: focus it once the peek shows it.
    focus_edit: bool,
    sugg: Vec<Sugg>,
    sel: usize,
    /// What was typed in the address bar and in the start page's box, apart from what
    /// Lumen filled in.
    omni_typed: Typed,
    ntp_typed: Typed,
    /// The page question whose card is up, if any.
    page_ask_shown: Option<u64>,
    /// Where the link under the pointer goes, drawn as a pill at the page's foot.
    link: String,
    born: Instant,
    spinning: bool,
    /// The memory glow's timer is armed: the number is over the hard line and
    /// the glow breathes.
    mem_pulse: bool,
    /// Every animated value of the chrome.
    motion: Motion,
    animating: bool,
    pub(crate) page: pages::Page,
    theme_key: String,
    mouse: (f32, f32),
    /// Dimmed overlay window shown behind the glance card.
    glance_dim: Option<HWND>,
    /// Workspace index being renamed via the omnibox edit, if any.
    renaming: Option<usize>,
    /// Tabs that just closed: their rows collapse and fade in place for a moment.
    closing: Vec<Closing>,
    /// Tab ids the last frame drew, so an entrance (a new row) can be told apart.
    seen_tabs: std::collections::HashSet<u64>,
    /// The workspace the tab list was last drawn for: a switch cross-fades.
    last_ws: usize,
    /// The new tab page's wallpaper: the file path and its bytes, read once.
    wallpaper: Option<(String, Vec<u8>)>,
    /// The fonts Windows has installed, for the settings' picker.
    fonts: Rc<Vec<String>>,
    /// The clock timer's period, so it is only reset when it changes.
    clock_period: u32,
    /// The site info popover (the lock) is open.
    site_open: bool,
    /// The tab search card is open (Ctrl+Shift+A).
    search_open: bool,
    /// The filter box of the tab search; a real edit window.
    search_edit: HWND,
    /// The row the tab search has picked.
    search_sel: usize,
    /// What the last tab-search frame listed, so a click knows what it lands on.
    search_rows: Vec<SwitchRow>,
    /// The message card over the sidebar: to which agent, and which kind (0 steer now,
    /// 1 queue, 2 new task). Its text box is a real edit window.
    say: Option<Say>,
    say_edit: HWND,
    /// The command palette (Ctrl+K) is open.
    pal_open: bool,
    /// The filter box of the command palette; a real edit window.
    palette_edit: HWND,
    /// The row the command palette has picked.
    pal_sel: usize,
    /// What the last palette frame listed, so a click knows what it lands on.
    pal_rows: Vec<PalRow>,
    /// Words the palette left for the Settings page's own search box.
    pal_jump: Option<String>,
    /// The Settings page's search box.
    set_edit: HWND,
    /// The report page's "what happened" box.
    bug_edit: HWND,
    /// The colour picker's hex box.
    color_edit: HWND,
    /// The colour picker's card, open under the swatch it was asked from.
    color: Option<color::ColorCard>,
    /// The form card's text boxes, one per field, and the one that hides its text.
    form_edits: [HWND; 3],
    form_secret_edit: HWND,
    /// The form card, open while it waits for Save or Cancel.
    form: Option<form::FormCard>,
    /// The question card, open while it waits for a button.
    confirm: Option<confirm::ConfirmCard>,
    /// The menu card, open at the click point while it waits for a choice.
    menu: Option<menu::MenuCard>,
    /// The file card's path and name boxes.
    file_path_edit: HWND,
    file_name_edit: HWND,
    /// The file card, open while it waits for a pick.
    files: Option<files::FileCard>,
}

/// A message being written to an agent: the card over the sidebar.
struct Say {
    session: String,
    name: String,
    mode: usize,
}

/// The three kinds of message, as the card offers them, and what each means.
const SAY_MODES: [(&str, &str, &str); 3] = [
    ("Steer now", "now", "It reads this first, on its next call."),
    ("Queue", "later", "For after its current task."),
    ("New task", "new", "Starts a new Drive run."),
];

/// The narrowest a floating card gets: a compact sidebar is an icon column, and a card
/// that wide could not say anything.
const FLOAT_MIN_W: f32 = 248.0;

/// A floating card's column in its area: the area less a margin, never narrower than
/// `FLOAT_MIN_W`. Wider than a sidebar on the right, it grows leftwards, over the page.
fn float_col(area: R, right: bool) -> (f32, f32) {
    let w = (area.w - 16.0).max(FLOAT_MIN_W);
    (if right { area.right() - 8.0 - w } else { area.x + 8.0 }, w)
}

/// The message card's rectangle: at the top of the cards' area.
fn say_card(area: R, right: bool) -> R {
    let (x, w) = float_col(area, right);
    R::new(x, area.y + 8.0, w, 168.0)
}

/// A tab that just closed. The row is drawn once more, collapsing.
struct Closing {
    tab: UiTab,
    /// Its row's place in the list, in chrome coordinates.
    y: f32,
    /// When it closed, so a ghost never outlives its moment.
    at: Instant,
}

/// One row of the tab search: enough to switch to the tab it names. `other` is (workspace,
/// index among that workspace's saved tabs); without it, the tab is in this workspace.
#[derive(Clone, Debug)]
struct SwitchRow {
    id: u64,
    other: Option<(usize, usize)>,
    ws_name: String,
    title: String,
    url: String,
    private: bool,
}

impl SwitchRow {
    fn label(&self) -> String {
        match (self.title.is_empty(), host(&self.url)) {
            (false, _) => self.title.clone(),
            (true, h) if h.is_empty() => "New tab".into(),
            (true, h) => h,
        }
    }
}

/// The native chrome. Cheap to clone the handle; state lives behind the RefCell.
pub struct Chrome {
    pub hwnd: HWND,
    inner: Rc<RefCell<Inner>>,
}

const CLASS: PCWSTR = w!("LumenChrome");
const ID_EDIT: usize = 101;
const ID_NTP: usize = 102;
const ID_BM: usize = 103;
const ID_DRIVE: usize = 104;
const ID_SRCH: usize = 105;
const ID_SAY: usize = 106;
const ID_SET: usize = 107;
const ID_BUG: usize = 108;
const ID_COLOR: usize = 109;
const ID_FORM0: usize = 110;
const ID_FORM1: usize = 111;
const ID_FORM2: usize = 112;
const ID_FORMPW: usize = 113;
const ID_FILEPATH: usize = 114;
const ID_FILENAME: usize = 115;
const ID_PAL: usize = 116;
const TIMER_SPIN: usize = 1;
const TIMER_CLOCK: usize = 2;
const TIMER_ANIM: usize = 3;
const TIMER_MEM: usize = 5;
#[cfg(debug_assertions)]
const TIMER_SHOT: usize = 4;

fn send(proxy: &EventLoopProxy<Ev>, v: Value) {
    let _ = proxy.send_event(Ev::Ui(v.to_string()));
}

// ------------------------------------------------------------- media keys
//
// Play/pause, next and previous from the keyboard or the earbuds arrive as
// WM_APPCOMMAND on the window with focus. When a page has focus Chromium answers
// them itself, through its own Media Session: those never come here. These are the
// presses that land while Lumen's own chrome has focus, and they go to the tab
// that last played. The subclass chains onto whatever window proc is already on
// the top-level window (tao's), so nothing else changes hands.

/// The old window procedure, and where a press is sent. One window, one subclass.
static MEDIA_KEYS: Mutex<Option<(usize, EventLoopProxy<Ev>)>> = Mutex::new(None);

unsafe extern "system" fn media_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_APPCOMMAND {
            // The command sits in the low word of the high word of lParam.
            let cmd = ((lp.0 >> 16) & 0xffff) as u32 & 0x7fff;
            let which = match cmd {
                11 => "next",
                12 => "prev",
                14 => "toggle",
                _ => "",
            };
            if !which.is_empty() {
                if let Ok(g) = MEDIA_KEYS.lock()
                    && let Some((_, px)) = g.as_ref()
                {
                    let _ = px.send_event(Ev::Ui(format!(r#"{{"t":"media","cmd":"{which}"}}"#)));
                }
                return LRESULT(0);
            }
        }
        let old: WNDPROC = match MEDIA_KEYS.lock() {
            Ok(g) => g.as_ref().map(|(o, _)| std::mem::transmute(*o)),
            Err(_) => None,
        };
        match old {
            Some(old) => CallWindowProcW(Some(old), hwnd, msg, wp, lp),
            None => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Subclass the top-level window once, for the media keys. `parent` is that window.
fn install_media_keys(parent: HWND, proxy: EventLoopProxy<Ev>) {
    unsafe {
        let root = GetAncestor(parent, GA_ROOT);
        let old = SetWindowLongPtrW(root, GWLP_WNDPROC, media_wndproc as *const () as isize);
        if old != 0
            && let Ok(mut g) = MEDIA_KEYS.lock()
        {
            *g = Some((old as usize, proxy));
        }
    }
}

/// What a permission prompt says a site wants, in words.
fn ask_text(kind: &str) -> String {
    match kind {
        "multiple downloads" => "wants to download several files".into(),
        "notifications" => "wants to show notifications".into(),
        "autoplay" => "wants to play sound on its own".into(),
        "files" => "wants to edit files on this PC".into(),
        "window placement" => "wants to place windows on your screens".into(),
        "something" => "asks for a permission".into(),
        k => format!("wants your {k}"),
    }
}

/// The bitmap cache key of a site's icon. Tab ids are small; a site's key has the top bit set,
/// so the two never meet in the cache.
fn site_key(host: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    host.hash(&mut h);
    h.finish() | (1 << 63)
}

fn host(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or("");
    let h = rest.split(['/', '?', '#']).next().unwrap_or("");
    let h = h.rsplit('@').next().unwrap_or(h);
    h.strip_prefix("www.").unwrap_or(h).to_string()
}

/// How many tab search rows show before the card says how many are left.
const SEARCH_ROWS: usize = 9;

/// The tab search's rows: this workspace's tabs as they stand, then the tabs of the workspaces
/// that are put away, filtered by `q` and ranked best first. Equal scores keep the sidebar's
/// order, so an empty query lists the tabs as they are.
fn switch_rows(s: &UiState, q: &str) -> Vec<SwitchRow> {
    let mut all = switch_candidates(&s.tabs, &s.other_tabs);
    let ws_name = s.workspaces.get(s.active_workspace).cloned().unwrap_or_default();
    for row in &mut all {
        if row.other.is_none() {
            row.ws_name = ws_name.clone();
        }
    }
    rank_rows(&all, q)
}

/// The candidates of the tab search, in list order.
fn switch_candidates(tabs: &[UiTab], other: &[UiOther]) -> Vec<SwitchRow> {
    let mut all: Vec<SwitchRow> = tabs
        .iter()
        .map(|t| SwitchRow { id: t.id, other: None, ws_name: String::new(), title: t.title.clone(), url: t.url.clone(), private: t.private })
        .collect();
    for o in other {
        all.push(SwitchRow {
            id: 0,
            other: Some((o.ws, o.idx)),
            ws_name: o.ws_name.clone(),
            title: o.title.clone(),
            url: o.url.clone(),
            private: false,
        });
    }
    all
}

/// The candidates that fit `q`, best first.
fn rank_rows(all: &[SwitchRow], q: &str) -> Vec<SwitchRow> {
    let order = {
        let entries: Vec<crate::tabsearch::Entry> = all.iter().map(|r| crate::tabsearch::Entry { title: &r.title, url: &r.url }).collect();
        crate::tabsearch::rank(q, &entries)
    };
    order.into_iter().map(|i| all[i].clone()).collect()
}

/// The tab search card's rectangle: at the top of the cards' area.
fn search_card(area: R, right: bool, rows: usize, more: bool) -> R {
    let h = 52.0 + rows as f32 * 32.0 + if rows == 0 { 34.0 } else { 8.0 } + if more { 20.0 } else { 0.0 };
    let (x, w) = float_col(area, right);
    R::new(x, area.y + 8.0, w, h.min(area.h - 16.0))
}

/// How many rows of the command palette show before the card says how many are left.
const PAL_ROWS: usize = 12;

/// One line of the command palette: which group it heads, what it says, and what Enter does.
#[derive(Clone)]
struct PalRow {
    /// Some(name) on a group's first row, so the label draws once.
    group: Option<&'static str>,
    label: String,
    detail: String,
    run: PalRun,
}

/// What a palette row does when it is picked.
#[derive(Clone)]
enum PalRun {
    /// Anything the main loop already knows how to do.
    Msg(Value),
    /// Open Settings with the page's own search box holding these words.
    Set(String),
    /// Bring a tab back, the way the tab search does.
    Switch { id: u64, other: Option<(usize, usize)> },
    /// Open an address in a new tab.
    Open(String),
}

/// Every word typed is in the row's label and detail.
fn pal_words(row: &PalRow, q: &str) -> bool {
    let hay = format!("{} {}", row.label, row.detail).to_lowercase();
    q.split_whitespace().all(|w| hay.contains(w))
}

/// The palette's rows for `q`: the groups in order, each cut to what fits the card.
/// With no words typed it is the actions and the open tabs; settings, bookmarks and
/// history appear once the person starts narrowing.
fn pal_rows(s: &UiState, q: &str) -> Vec<PalRow> {
    let mut out: Vec<PalRow> = Vec::new();
    let mut add = |group: &'static str, mut rows: Vec<PalRow>| {
        rows.truncate(if q.is_empty() {
            match group {
                "ACTIONS" => 7,
                "TABS" => 5,
                _ => 0,
            }
        } else {
            match group {
                "ACTIONS" => 3,
                "SETTINGS" => 6,
                "TABS" => 4,
                _ => 3,
            }
        });
        for (n, r) in rows.into_iter().enumerate() {
            out.push(PalRow { group: if n == 0 { Some(group) } else { None }, ..r });
        }
    };
    // ACTIONS: the things a person does, and the pages they open.
    let active = s.active;
    let mut next = s.settings.clone();
    next.compact = !next.compact;
    let which = |w: &'static str| json!({"t": "panel", "open": true, "which": w});
    let actions = [
        ("New tab", PalRun::Msg(json!({"t": "newtab"}))),
        ("New private tab", PalRun::Msg(json!({"t": "newPrivate"}))),
        ("Close tab", PalRun::Msg(json!({"t": "close", "id": active}))),
        ("Reload", PalRun::Msg(json!({"t": "reload"}))),
        ("Settings", PalRun::Msg(json!({"t": "panel", "open": true}))),
        ("Settings: Look", PalRun::Set("look".into())),
        ("Toggle compact", PalRun::Msg(json!({"t": "settings", "v": next}))),
        ("History", PalRun::Msg(which("history"))),
        ("Bookmarks", PalRun::Msg(which("bookmarks"))),
        ("Downloads", PalRun::Msg(which("downloads"))),
        ("Control", PalRun::Msg(which("control"))),
        ("Privacy report", PalRun::Msg(which("report"))),
        ("Dashboard", PalRun::Msg(which("dashboard"))),
    ];
    let rows: Vec<PalRow> = actions
        .into_iter()
        .map(|(label, run)| PalRow { group: None, label: label.into(), detail: String::new(), run })
        .filter(|r| q.is_empty() || pal_words(r, q))
        .collect();
    add("ACTIONS", rows);
    // SETTINGS: only once the person is narrowing, and never every row at once.
    if !q.is_empty() {
        let mut rows = Vec::new();
        for (section, items) in pages::settings_schema() {
            for it in &items {
                if pages::item_matches(q, section, it) {
                    rows.push(PalRow { group: None, label: pages::pal_label(it), detail: section.into(), run: PalRun::Set(pages::pal_label(it)) });
                }
            }
        }
        add("SETTINGS", rows);
    }
    // TABS: the tab search's own list, filtered and ranked the same way.
    let rows: Vec<PalRow> = switch_rows(s, q)
        .into_iter()
        .map(|row| {
            let tag = if row.private {
                "Private".to_string()
            } else if row.other.is_some() {
                row.ws_name.clone()
            } else {
                String::new()
            };
            PalRow { group: None, label: row.label(), detail: tag, run: PalRun::Switch { id: row.id, other: row.other } }
        })
        .collect();
    add("TABS", rows);
    // BOOKMARKS and HISTORY: open the address in a new tab.
    if !q.is_empty() {
        let rows: Vec<PalRow> = s
            .bookmarks
            .iter()
            .map(|m| PalRow {
                group: None,
                label: if m.title.is_empty() { m.url.clone() } else { m.title.clone() },
                detail: m.folder.clone(),
                run: PalRun::Open(m.url.clone()),
            })
            .filter(|r| pal_words(r, q))
            .collect();
        add("BOOKMARKS", rows);
        let rows: Vec<PalRow> = s
            .history
            .iter()
            .map(|v| PalRow {
                group: None,
                label: if v.title.is_empty() { v.url.clone() } else { v.title.clone() },
                detail: host(&v.url),
                run: PalRun::Open(v.url.clone()),
            })
            .filter(|r| pal_words(r, q))
            .collect();
        add("HISTORY", rows);
    }
    out
}

/// The rows that show, the groups those head, and how many the filter found in all.
fn pal_view(s: &UiState, q: &str) -> (Vec<PalRow>, usize, usize) {
    let all = pal_rows(s, q);
    let total = all.len();
    let mut rows = all;
    rows.truncate(PAL_ROWS);
    let groups = rows.iter().filter(|r| r.group.is_some()).count();
    (rows, groups, total)
}

/// The command palette card's rectangle: at the top of the cards' area, where the
/// tab search sits, so the two read as one machine.
fn pal_card(area: R, right: bool, rows: usize, groups: usize, more: bool) -> R {
    let h = 56.0 + groups as f32 * 18.0 + rows as f32 * 30.0 + if rows == 0 { 34.0 } else { 0.0 } + if more { 20.0 } else { 0.0 };
    let (x, w) = float_col(area, right);
    R::new(x, area.y + 8.0, w, h.min(area.h - 16.0))
}

/// Put history and bookmark hits into a suggestion list: under the "Switch to tab" rows (an
/// open tab is always the better answer) and over what is already there, skipping a URL the
/// list already shows.
fn merge_hits(sugg: &mut Vec<Sugg>, hits: Vec<(String, String, bool, String)>) {
    let at = sugg
        .iter()
        .rposition(|s| matches!(s.kind, SuggKind::Tab(_) | SuggKind::TabOther(..)))
        .map_or(sugg.len(), |n| n + 1);
    let mut add = Vec::new();
    for (url, title, bm, detail) in hits {
        if sugg.iter().any(|s| matches!(&s.kind, SuggKind::Go(u) | SuggKind::Url(u, _) if *u == url)) {
            continue;
        }
        let label = if title.is_empty() { url.clone() } else { title };
        add.push(Sugg { kind: SuggKind::Url(url, bm), label, detail });
    }
    for (n, s) in add.into_iter().enumerate() {
        sugg.insert(at + n, s);
    }
}

/// A page's question for the card: which one, whose, what, and whether it has asked so often
/// that the card offers "Stop this page asking".
#[derive(Clone, Debug)]
pub struct UiAsk {
    pub aid: u64,
    pub host: String,
    pub ask: crate::engine::Ask,
    pub many: bool,
}

/// What the person typed into a box Lumen fills in, kept apart from what Lumen added.
#[derive(Default)]
struct Typed {
    text: String,
    /// The next change is a deletion (Backspace, Delete): fill nothing back in for it.
    no_fill: bool,
    /// The last change may be filled in: it added to the end of what was typed.
    ok: bool,
}

impl Typed {
    /// The box changed: remember what is in it now, and whether it may be filled in. Only a
    /// change that added to the end may: not a deletion, not an edit in the middle.
    fn changed(&mut self, edit: HWND) -> String {
        let raw = wide_text(edit);
        let (start, end) = edit_sel(edit);
        let at_end = start == end && end == raw.encode_utf16().count();
        self.ok = !self.no_fill && at_end && raw.chars().count() > self.text.chars().count();
        self.no_fill = false;
        self.text = raw.clone();
        raw
    }

    /// "yout" becomes "youtube.com" with "ube.com" selected, so the next letter types over
    /// it, Backspace takes it away and Enter goes there. Only when the typed text is exactly
    /// the start of the site's name. Our own change: the EN_CHANGE it raises finds the chrome
    /// borrowed and is dropped, so `text` keeps what the person typed.
    fn fill(&self, edit: HWND, q: &str, fill: &str) -> bool {
        if !self.ok || self.text != q || !q.is_ascii() || fill.len() <= q.len() || !fill.starts_with(&q.to_ascii_lowercase()) {
            return false;
        }
        let at = q.encode_utf16().count();
        unsafe {
            SetWindowTextW(edit, &HSTRING::from(format!("{q}{}", &fill[q.len()..]))).ok();
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(at)), Some(LPARAM(-1)));
        }
        true
    }
}

/// An edit control's selection, start and end, in UTF-16 units.
fn edit_sel(hwnd: HWND) -> (usize, usize) {
    let (mut start, mut end) = (0u32, 0u32);
    unsafe {
        SendMessageW(hwnd, EM_GETSEL, Some(WPARAM(&mut start as *mut u32 as usize)), Some(LPARAM(&mut end as *mut u32 as isize)));
    }
    (start as usize, end as usize)
}

/// The mark beside a tab that makes sound: muted, in a call, or playing.
fn sound_icon(t: &UiTab) -> &'static str {
    if t.muted {
        "muted"
    } else if t.call && !t.audio {
        "mic"
    } else {
        "sound"
    }
}

fn wide_text(hwnd: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hwnd);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..got as usize])
    }
}

fn lparam_xy(l: LPARAM) -> (i32, i32) {
    ((l.0 & 0xffff) as i16 as i32, ((l.0 >> 16) & 0xffff) as i16 as i32)
}

impl Chrome {
    pub fn new(parent: HWND, proxy: EventLoopProxy<Ev>) -> Chrome {
        unsafe {
            let hinst: HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS,
                w!(""),
                // Clip siblings: below the page, the chrome's repaints never land on it.
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                240,
                600,
                Some(parent),
                None,
                Some(hinst),
                None,
            )
            .expect("chrome window");
            let edit_style = WINDOW_STYLE(ES_AUTOHSCROLL as u32) | WS_CHILD;
            // The glance card's dimmed backdrop: a translucent child of the main
            // window, raised over the page while a card is open.
            let dim = CreateWindowExW(
                WS_EX_LAYERED,
                w!("STATIC"),
                w!(""),
                WS_CHILD | WS_CLIPSIBLINGS,
                0, 0, 1, 1,
                Some(parent),
                None,
                Some(hinst),
                None,
            ).unwrap_or_default();
            if !dim.is_invalid() {
                let old = SetWindowLongPtrW(dim, GWLP_WNDPROC, dim_proc as *const () as isize);
                SetWindowLongPtrW(dim, GWLP_USERDATA, old);
            }
            let _ = ShowWindow(dim, SW_HIDE);
            let _ = SetLayeredWindowAttributes(dim, COLORREF(0), 75, LWA_ALPHA);
            let edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_EDIT as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let ntp_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_NTP as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let bm_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_BM as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let drive_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_DRIVE as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let search_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_SRCH as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let say_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_SAY as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let set_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_SET as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let bug_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_BUG as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let color_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_COLOR as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let mk_form = |id: usize| {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("EDIT"),
                    w!(""),
                    edit_style,
                    0,
                    0,
                    10,
                    10,
                    Some(hwnd),
                    Some(HMENU(id as _)),
                    Some(hinst),
                    None,
                )
                .unwrap_or_default()
            };
            let form_edits = [mk_form(ID_FORM0), mk_form(ID_FORM1), mk_form(ID_FORM2)];
            let form_secret_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style | WINDOW_STYLE(ES_PASSWORD as u32),
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_FORMPW as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let file_path_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_FILEPATH as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let file_name_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_FILENAME as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let palette_edit = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                edit_style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                Some(HMENU(ID_PAL as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            for e in [edit, ntp_edit, bm_edit, drive_edit, search_edit, say_edit, set_edit, bug_edit, color_edit, form_secret_edit, file_path_edit, file_name_edit, palette_edit]
                .into_iter()
                .chain(form_edits)
            {
                let old = SetWindowLongPtrW(e, GWLP_WNDPROC, edit_proc as *const () as isize);
                SetWindowLongPtrW(e, GWLP_USERDATA, old);
            }
            // The Drive panel's task box says what it is for, in the box itself: an empty
            // edit box shows nothing of the drawn placeholder behind it.
            let cue = HSTRING::from("Find the cheapest X and put it in the cart, stop before paying");
            let _ = SendMessageW(drive_edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
            let dpi = GetDpiForWindow(hwnd) as f32;
            let gfx = Gfx::new(hwnd, dpi).expect("direct2d");
            let fonts = Rc::new(gfx.installed_families().to_vec());
            // The media keys (play/pause, next, previous) while Lumen's own chrome
            // has focus: one subclass on the top-level window.
            install_media_keys(parent, proxy.clone());
            let inner = Rc::new(RefCell::new(Inner {
                hwnd,
                proxy,
                gfx,
                theme: Theme::new("graphite", true, ""),
                look: Look::default(),
                state: None,
                hits: Vec::new(),
                hover: None,
                press: None,
                scroll: 0.0,
                list_max: 0.0,
                drag: None,
                favs: HashMap::new(),
                site_favs: HashMap::new(),
                floats: Vec::new(),
                float_layer: Vec::new(),
                disk_favs: HashMap::new(),
                fav_asked: std::collections::HashSet::new(),
                dial_drag: None,
                edit,
                ntp_edit,
                bm_edit,
                drive_edit,
                drive_focus: false,
                edit_font: HFONT::default(),
                edit_face: String::new(),
                ntp_font: HFONT::default(),
                edit_brush: HBRUSH::default(),
                ntp_brush: HBRUSH::default(),
                bar_brush: HBRUSH::default(),
                editing: false,
                focus_edit: false,
                sugg: Vec::new(),
                omni_typed: Typed::default(),
                ntp_typed: Typed::default(),
                page_ask_shown: None,
                link: String::new(),
                sel: 0,
                born: Instant::now(),
                spinning: false,
                mem_pulse: false,
                motion: Motion::default(),
                animating: false,
                page: pages::Page::default(),
                theme_key: String::new(),
                mouse: (-1.0, -1.0),
                glance_dim: Some(dim),
                renaming: None,
                closing: Vec::new(),
                seen_tabs: std::collections::HashSet::new(),
                last_ws: 0,
                wallpaper: None,
                fonts,
                clock_period: 0,
                site_open: false,
                search_open: false,
                search_edit,
                search_sel: 0,
                search_rows: Vec::new(),
                say: None,
                say_edit,
                pal_open: false,
                palette_edit,
                pal_sel: 0,
                pal_rows: Vec::new(),
                pal_jump: None,
                set_edit,
                bug_edit,
                color_edit,
                color: None,
                form_edits,
                form_secret_edit,
                form: None,
                confirm: None,
                menu: None,
                file_path_edit,
                file_name_edit,
                files: None,
            }));
            // The window proc finds its state through this pointer. The Rc lives as long as App.
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(&inner) as isize);
            inner.borrow_mut().make_fonts();
            Chrome { hwnd, inner }
        }
    }

    /// Place the chrome window, logical (DIP) coordinates relative to the parent.
    pub fn set_bounds(&self, x: f64, y: f64, w: f64, h: f64) {
        unsafe {
            let k = GetDpiForWindow(self.hwnd) as f64 / 96.0;
            let _ = SetWindowPos(
                self.hwnd,
                None,
                (x * k).round() as i32,
                (y * k).round() as i32,
                (w * k).round().max(1.0) as i32,
                (h * k).round().max(1.0) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// The chrome's own rectangle: the whole window, in every layout. The page is
    /// a sibling window drawn over the chrome, so what shows around the page is
    /// the chrome's background: one window, three shapes, no seams.
    pub fn bounds_for(_kind: Kind, w: f64, h: f64) -> (f64, f64, f64, f64) {
        (0.0, 0.0, w, h)
    }

    /// Sidebar color, for the window behind the page gaps.
    pub fn bg(&self) -> (u8, u8, u8) {
        self.inner.try_borrow().map(|i| i.look.theme.bg.rgb8()).unwrap_or((24, 24, 26))
    }

    /// The chrome's look. For main.rs: the page's backdrop colors and the
    /// sidebar's animated width are read from the same style the chrome draws.
    pub fn look(&self) -> Look {
        self.inner.try_borrow().map(|i| i.look.clone()).unwrap_or_default()
    }

    /// Show or reposition the dimmed backdrop behind the glance card,
    /// covering the page area (logical coords: page origin + size).
    pub fn set_glance_dim(&self, x: f64, y: f64, w: f64, h: f64) {
        if let Some(dim) = self.inner.try_borrow().ok().and_then(|i| i.glance_dim) {
            unsafe {
                let k = GetDpiForWindow(self.hwnd) as f64 / 96.0;
                let _ = SetWindowPos(
                    dim,
                    Some(HWND_TOP),
                    (x * k).round() as i32,
                    (y * k).round() as i32,
                    (w * k).round().max(1.0) as i32,
                    (h * k).round().max(1.0) as i32,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
    }

    /// A glance card opened or closed. The dim overlay follows over a moment, in
    /// the frame draw; the next flush repaints the switcher with (or without) its
    /// "Open as tab" button.
    pub fn set_glance(&self, on: bool) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if let Some(s) = i.state.as_mut() {
            s.glance = on;
        }
        if on {
            // Show it at nothing and let the fade raise it: no flash of full dim.
            if let Some(dim) = i.glance_dim {
                unsafe {
                    let _ = SetLayeredWindowAttributes(dim, COLORREF(0), 0, LWA_ALPHA);
                    let _ = ShowWindow(dim, SW_SHOWNA);
                }
            }
        }
        i.invalidate();
    }

    pub fn set_visible(&self, on: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if on { SW_SHOWNA } else { SW_HIDE });
        }
    }

    pub fn set_state(&self, s: UiState) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        i.apply(s);
        i.sync_page_ask();
    }

    /// A right-click on the active page, at (x, y) in page DIPs: Lumen's own menu with
    /// WebView2's items for that spot and Lumen's. False when the chrome cannot show it now.
    pub fn page_menu(&self, aid: u64, x: f32, y: f32, items: Vec<(i32, String, bool)>, link: String, selection: String) -> bool {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return false };
        let Some((sx, sy, _, _)) = i.state.as_ref().map(|s| s.stage) else { return false };
        let mut rows: Vec<(u32, String, bool)> = Vec::new();
        for (n, (cmd, label, on)) in items.iter().enumerate() {
            if *cmd == i32::MIN {
                rows.push((0, String::new(), true));
            } else {
                rows.push((1000 + n as u32, label.clone(), *on));
            }
        }
        let quote: String = selection.split_whitespace().collect::<Vec<_>>().join(" ");
        if !link.is_empty() || !quote.is_empty() {
            if !rows.is_empty() {
                rows.push((0, String::new(), true));
            }
            if !link.is_empty() {
                rows.push((1, "Open link in a private tab".into(), true));
            }
            if !quote.is_empty() {
                let short: String = if quote.chars().count() > 28 { quote.chars().take(27).collect::<String>() + "…" } else { quote.clone() };
                rows.push((2, format!("Search for \u{201c}{short}\u{201d}"), true));
            }
        }
        if rows.is_empty() {
            return false;
        }
        let cmds: Vec<i32> = items.iter().map(|c| c.0).collect();
        menu::open_rows_or(
            &mut i,
            &rows,
            sx + x,
            sy + y,
            move |i, id| {
                let cmd = match id {
                    1 => {
                        send(&i.proxy, json!({"t": "openPrivateUrl", "url": link}));
                        None
                    }
                    2 => {
                        send(&i.proxy, json!({"t": "searchNewTab", "q": quote}));
                        None
                    }
                    n => cmds.get((n as usize).wrapping_sub(1000)).copied(),
                };
                send(&i.proxy, json!({"t": "pageMenuPick", "aid": aid, "cmd": cmd}));
            },
            move |i| send(&i.proxy, json!({"t": "pageMenuPick", "aid": aid, "cmd": null})),
        );
        true
    }

    /// What the person typed in the Drive panel's task box. Empty when it was never opened.
    pub fn drive_task(&self) -> String {
        self.inner.try_borrow().map(|i| wide_text(i.drive_edit)).unwrap_or_default()
    }

    /// A short-lived note: a failed KeePass unlock, shown where it happened.
    /// Where the link under the pointer goes; "" takes the pill away.
    pub fn set_link(&self, text: String) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if i.link != text {
            i.link = text;
            i.invalidate();
        }
    }

    pub fn flash(&self, msg: String) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        i.page.flash = Some((msg, Instant::now()));
        i.invalidate();
    }

    pub fn set_fav(&self, id: u64, data_uri: &str) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        let png = data_uri
            .split_once("base64,")
            .and_then(|(_, b)| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b).ok())
            .unwrap_or_default();
        i.gfx.bitmaps.remove(&id);
        i.favs.insert(id, png);
        i.invalidate();
    }

    /// A site's icon for the speed dial's tiles.
    pub fn set_site_fav(&self, host: &str, data_uri: &str) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        let png = data_uri
            .split_once("base64,")
            .and_then(|(_, b)| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b).ok())
            .unwrap_or_default();
        i.gfx.bitmaps.remove(&site_key(host));
        i.site_favs.insert(host.to_string(), png);
        if i.ntp_visible() {
            i.invalidate();
        }
    }

    /// History and bookmark matches for `q`, from the main loop. Merged if still current.
    /// Hits are (url, title, bookmarked, detail). Detail, when not empty, is a second line.
    /// History's answer for what was typed, and the site to fill in, if any. Late answers
    /// for text that has moved on are dropped.
    pub fn set_suggest(&self, q: &str, hits: Vec<(String, String, bool, String)>, fill: Option<String>) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if !i.editing || i.omni_typed.text.trim() != q {
            return;
        }
        merge_hits(&mut i.sugg, hits);
        i.sugg.truncate(9);
        if let Some(f) = fill
            && i.omni_typed.fill(i.edit, q, &f)
        {
            // The filled-in site is the first row, chosen, so Enter goes there.
            i.sugg.retain(|s| !matches!(&s.kind, SuggKind::Go(_)));
            i.sugg.insert(0, Sugg { kind: SuggKind::Go(f.clone()), label: f, detail: String::new() });
            i.sel = 0;
        }
        i.invalidate();
    }

    /// The site to fill into the start page's box for what was typed there.
    pub fn set_ntp_fill(&self, q: &str, fill: Option<String>) {
        let Ok(i) = self.inner.try_borrow_mut() else { return };
        if let Some(f) = fill
            && i.ntp_typed.text.trim() == q
        {
            i.ntp_typed.fill(i.ntp_edit, q, &f);
        }
    }

    pub fn usage(&self, v: Value) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        i.page.usage = Some(v);
        // Both pages show these numbers. The dashboard used to wait for some other
        // repaint, so its tiles sat on an old reading.
        if i.state.as_ref().is_some_and(|s| s.panel == "control" || s.panel == "dashboard") {
            i.invalidate();
        }
    }

    /// Keyboard focus to the chrome: the new-tab search box if it is showing.
    pub fn focus(&self) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if i.ntp_visible() {
            i.focus_ntp();
        } else {
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
            }
        }
    }

    pub fn focus_omnibox(&self) {
        let edit = {
            let Ok(mut i) = self.inner.try_borrow_mut() else { return };
            i.close_palette();
            i.begin_edit();
            i.edit
        };
        unsafe {
            let _ = SetFocus(Some(edit));
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
    }

    /// The message card for one agent, from the watch pill's Message.
    pub fn open_message(&self, session: &str, name: &str) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        i.open_say(session.to_string(), name.to_string());
    }

    /// A folder's read landed from the worker thread: the file card takes it, if it
    /// is still the read that card is waiting for.
    pub fn file_list(&self, m: &serde_json::Value) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        files::list_landed(&mut i, m);
    }

    /// A site wants to open an app: the drawn question. The answer comes back to the
    /// main loop as an ExternalAnswer event, which is what finishes the ask.
    pub fn ask_external(&self, origin: &str, scheme: &str, remember: bool, uri: &str) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        confirm::open(&mut i, confirm::ConfirmKind::External {
            origin: origin.to_string(),
            uri: uri.to_string(),
            remember,
            scheme: scheme.to_string(),
        });
    }

    /// Ctrl+Shift+A: the quick switcher over the sidebar. A second press closes it.
    pub fn open_tab_search(&self) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if i.search_open {
            i.close_search();
            return;
        }
        i.close_palette();
        // The card lives in the sidebar; in the icon-only sidebar show the full one.
        if i.state.as_ref().is_some_and(|s| s.settings.compact) {
            send(&i.proxy, json!({"t": "peek", "on": true}));
        }
        i.search_open = true;
        i.search_sel = 0;
        unsafe {
            SetWindowTextW(i.search_edit, &HSTRING::from("")).ok();
        }
        i.layout_edits();
        unsafe {
            let _ = SetFocus(Some(i.search_edit));
        }
        // The rows are rebuilt by the paint this asks for.
        i.invalidate();
    }

    /// Ctrl+K: the command palette over the sidebar. A second press closes it.
    pub fn open_palette(&self) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if i.pal_open {
            i.close_palette();
            return;
        }
        i.close_search();
        i.close_say();
        i.site_open = false;
        // The card lives in the sidebar; in the icon-only sidebar show the full one.
        if i.state.as_ref().is_some_and(|s| s.settings.compact) {
            send(&i.proxy, json!({"t": "peek", "on": true}));
        }
        i.pal_open = true;
        i.pal_sel = 0;
        unsafe {
            SetWindowTextW(i.palette_edit, &HSTRING::from("")).ok();
        }
        // The rows are rebuilt by the paint this asks for, and the pages that a row
        // opens need their lists ready: the main loop fills them while this is open.
        send(&i.proxy, json!({"t": "palOpen", "open": true}));
        i.layout_edits();
        unsafe {
            let _ = SetFocus(Some(i.palette_edit));
        }
        i.invalidate();
    }
}

// ---------------------------------------------------------------- the guts

impl Inner {
    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn k(&self) -> f32 {
        self.gfx.dpi / 96.0
    }

    fn make_fonts(&mut self) {
        unsafe {
            for f in [self.edit_font, self.ntp_font] {
                if !f.is_invalid() {
                    let _ = DeleteObject(f.into());
                }
            }
            let k = self.k();
            self.edit_face = self.gfx.ui_family().to_string();
            let face = HSTRING::from(self.edit_face.as_str());
            let mk = |px: f32, face: &HSTRING| {
                CreateFontW(
                    -(px * k).round() as i32,
                    0,
                    0,
                    0,
                    FW_NORMAL.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY,
                    FF_DONTCARE.0 as u32,
                    face,
                )
            };
            // GDI quietly swaps in another font when it lacks the name. Ask what it gave.
            let real = |f: HFONT| -> String {
                let dc = CreateCompatibleDC(None);
                let old = SelectObject(dc, f.into());
                let mut buf = [0u16; 64];
                let n = GetTextFaceW(dc, Some(&mut buf));
                SelectObject(dc, old);
                let _ = DeleteDC(dc);
                String::from_utf16_lossy(&buf[..(n.max(1) as usize - 1).min(63)])
            };
            let mut edit = mk(13.0, &face);
            if real(edit) != self.edit_face {
                let _ = DeleteObject(edit.into());
                edit = mk(13.0, &HSTRING::from("Segoe UI Variable Text"));
            }
            let face = if real(edit) == self.edit_face { face } else { HSTRING::from("Segoe UI Variable Text") };
            self.edit_font = edit;
            self.ntp_font = mk(15.0, &face);
            SendMessageW(self.edit, WM_SETFONT, Some(WPARAM(self.edit_font.0 as usize)), Some(LPARAM(1)));
            SendMessageW(self.ntp_edit, WM_SETFONT, Some(WPARAM(self.ntp_font.0 as usize)), Some(LPARAM(1)));
            SendMessageW(self.bm_edit, WM_SETFONT, Some(WPARAM(self.edit_font.0 as usize)), Some(LPARAM(1)));
            SendMessageW(self.drive_edit, WM_SETFONT, Some(WPARAM(self.edit_font.0 as usize)), Some(LPARAM(1)));
            SendMessageW(self.set_edit, WM_SETFONT, Some(WPARAM(self.edit_font.0 as usize)), Some(LPARAM(1)));
            SendMessageW(self.bug_edit, WM_SETFONT, Some(WPARAM(self.edit_font.0 as usize)), Some(LPARAM(1)));
            SendMessageW(self.color_edit, WM_SETFONT, Some(WPARAM(self.edit_font.0 as usize)), Some(LPARAM(1)));
            for e in [self.form_secret_edit, self.file_path_edit, self.file_name_edit].into_iter().chain(self.form_edits) {
                SendMessageW(e, WM_SETFONT, Some(WPARAM(self.edit_font.0 as usize)), Some(LPARAM(1)));
            }
            for e in [self.edit, self.ntp_edit, self.bm_edit, self.drive_edit, self.set_edit, self.bug_edit, self.color_edit, self.form_secret_edit, self.file_path_edit, self.file_name_edit]
                .into_iter()
                .chain(self.form_edits)
            {
                SendMessageW(e, EM_SETMARGINS, Some(WPARAM((EC_LEFTMARGIN | EC_RIGHTMARGIN) as usize)), Some(LPARAM(0)));
            }
        }
    }

    fn make_brushes(&mut self) {
        unsafe {
            for b in [self.edit_brush, self.ntp_brush] {
                if !b.is_invalid() {
                    let _ = DeleteObject(b.into());
                }
            }
            self.edit_brush = CreateSolidBrush(COLORREF(self.theme.solid(self.theme.raise).colorref()));
            self.ntp_brush = CreateSolidBrush(COLORREF(self.theme.bg.colorref()));
            if !self.bar_brush.is_invalid() {
                let _ = DeleteObject(self.bar_brush.into());
            }
            self.bar_brush = CreateSolidBrush(COLORREF(self.theme.solid(ntp_bar_fill(&self.look)).colorref()));
        }
    }

    fn apply(&mut self, #[cfg_attr(not(debug_assertions), allow(unused_mut))] mut s: UiState) {
        #[cfg(debug_assertions)]
        shot::fake(&mut s);
        // The whole style of this state, in one place. Reduced-motion is read
        // here too, so a system that turns animations off is felt at once.
        // Battery saver: the chrome moves less, whatever the motion setting says.
        let look = if s.battery && s.settings.motion == "full" {
            let mut quiet = s.settings.clone();
            quiet.motion = "reduced".into();
            Look::of(&quiet, s.dark, motion::system_allows())
        } else {
            Look::of(&s.settings, s.dark, motion::system_allows())
        };
        self.gfx.set_families(&s.settings.ui_font, &s.settings.display_font, &s.settings.mono_font);
        if self.gfx.ui_family() != self.edit_face && !self.edit.is_invalid() {
            self.make_fonts();
        }
        let key = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            s.settings.palette,
            s.dark,
            s.settings.accent,
            s.settings.bg_color,
            s.settings.side_color,
            s.settings.surface_color,
            s.settings.text_color,
            s.settings.dim_color
        );
        self.look = look.clone();
        self.theme = look.theme;
        if key != self.theme_key {
            self.theme_key = key;
            self.make_brushes();
        }
        // The wallpaper file is read once per choice, not per frame.
        let want_wp = s.settings.ntp_bg == "wallpaper" && !s.settings.wallpaper.is_empty();
        let held = self.wallpaper.as_ref().map(|(p, _)| p.clone());
        if !want_wp {
            self.wallpaper = None;
        } else if held.as_deref() != Some(s.settings.wallpaper.as_str()) {
            let path = s.settings.wallpaper.clone();
            self.gfx.bitmaps.remove(&wallpaper_key(&path));
            self.wallpaper = std::fs::read(&path).ok().filter(|b| !b.is_empty()).map(|bytes| (path, bytes));
        }
        // A tab that is gone from the new state leaves its row behind for a
        // moment: the row collapses and fades instead of blinking out.
        let before: Vec<UiTab> = self.state.as_ref().map(|o| o.tabs.clone()).unwrap_or_default();
        let now: std::collections::HashSet<u64> = s.tabs.iter().map(|t| t.id).collect();
        if !before.is_empty() {
            let rects: HashMap<u64, R> = self.hits.iter().filter_map(|(r, h)| match h {
                Hit::Tab(id) => Some((*id, *r)),
                _ => None,
            }).collect();
            for t in before.into_iter().filter(|t| !now.contains(&t.id)) {
                let y = rects.get(&t.id).map_or(0.0, |r| r.y);
                let already = self.closing.iter().any(|c| c.tab.id == t.id);
                if !already {
                    self.closing.push(Closing { tab: t, y, at: Instant::now() });
                }
            }
        }
        let was_ntp = self.ntp_visible();
        // A page opens at its top, also when it is the one that was open before.
        let page_changed = self.state.as_ref().map_or("none", |o| o.panel) != s.panel;
        if page_changed {
            self.page.scroll = 0.0;
            // The search box starts empty each time the Bookmarks page opens.
            unsafe {
                SetWindowTextW(self.bm_edit, &HSTRING::new()).ok();
                SetWindowTextW(self.set_edit, &HSTRING::new()).ok();
                SetWindowTextW(self.bug_edit, &HSTRING::new()).ok();
            }
            self.page.bm_collapsed.clear();
            // The Drive page opens with the cursor in its task box: typing is the point.
            if s.panel == "drive" {
                self.drive_focus = true;
            }
            self.page.font_pick = None;
            self.page.theme_name.clear();
        }
        // The palette's Settings row: its words land in the page's own search box,
        // right after the clear above emptied it.
        if s.panel == "settings" {
            if let Some(words) = self.pal_jump.take() {
                unsafe {
                    SetWindowTextW(self.set_edit, &HSTRING::from(words)).ok();
                }
            }
        } else if page_changed {
            self.pal_jump = None;
        }
        // A run started from a message has a task the box never saw: the box shows it, so the
        // page says what is being done. Never over what the person is typing.
        if let Some(d) = s.drive.as_ref().filter(|d| !d.task.is_empty())
            && wide_text(self.drive_edit).is_empty()
            && unsafe { GetFocus() } != self.drive_edit
        {
            unsafe {
                SetWindowTextW(self.drive_edit, &HSTRING::from(d.task.as_str())).ok();
            }
        }
        // Debug: LUMEN_TEST_SCROLL starts a run's page scrolled, so a screenshot
        // can show what is further down without a wheel script.
        #[cfg(debug_assertions)]
        if page_changed
            && std::env::var("LUMEN_TEST_PANEL").map(|p| p == s.panel).unwrap_or(false)
            && let Some(px) = std::env::var("LUMEN_TEST_SCROLL").ok().and_then(|v| v.parse::<f32>().ok())
            && px > 0.0
        {
            // As if that page had been open already: the draw pass must not take
            // the offset for a fresh page and start over, nor clamp it against a
            // height that is only known after the first draw.
            self.page.which = s.panel;
            self.page.test_scroll(px);
        }
        self.state = Some(s);
        if !self.editing {
            self.sugg.clear();
        }
        let now_ntp = self.ntp_visible();
        if now_ntp != was_ntp {
            unsafe {
                if now_ntp {
                    SetWindowTextW(self.ntp_edit, &HSTRING::new()).ok();
                    self.clock_timer();
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_CLOCK);
                }
            }
        } else if now_ntp && self.clock_period != self.clock_wanted() {
            // The clock's own tick: a second when seconds show, else ten.
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_CLOCK);
            }
            self.clock_timer();
        }
        self.layout_edits();
        // Compact hid the edit box when the address bar opened; the peek has shown it now.
        if self.focus_edit && self.editing && self.state.as_ref().is_some_and(|s| !s.compact()) {
            self.focus_edit = false;
            unsafe {
                let _ = SetFocus(Some(self.edit));
                SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            }
        }
        self.spin();
        self.mem_timer();
        // Debug: LUMEN_TEST_FONT_PICK="ui_font:segoe" opens the font list with a
        // filter typed, so a screenshot can show it without clicking through.
        #[cfg(debug_assertions)]
        if let Some(spec) = std::env::var("LUMEN_TEST_FONT_PICK").ok()
            && self.page.font_pick.is_none()
        {
            let (k, filter) = spec.split_once(':').unwrap_or((spec.as_str(), ""));
            let key: &'static str = match k {
                "display_font" => "display_font",
                "mono_font" => "mono_font",
                _ => "ui_font",
            };
            self.page.font_pick = Some(pages::FontPick { key, filter: filter.to_string(), scroll: 0.0, shown: Vec::new() });
        }
        // Debug: LUMEN_TEST_CARD="color", "form", "confirm", "files" or "files-open"
        // opens that card without a click, so a screenshot can show it.
        #[cfg(debug_assertions)]
        if self.color.is_none() && self.form.is_none() && self.confirm.is_none() && self.files.is_none() && self.menu.is_none() {
            match std::env::var("LUMEN_TEST_CARD").as_deref() {
                // The anchorless card sits at the right edge.
                Ok("color") => {
                    let color = self.theme.accent;
                    color::open(self, "accent", String::new(), color, None);
                }
                Ok("form") => form::open(
                    self,
                    "Edit bookmark",
                    &[("Title", "The Rust Programming Language"), ("Address", "https://doc.rust-lang.org/book/")],
                    form::FormAct::EditBookmark { id: 1, folder: String::new() },
                ),
                Ok("confirm") => confirm::open(self, confirm::ConfirmKind::ResetStats),
                Ok("files") | Ok("files-open") | Ok("files-kdbx") | Ok("files-torrent") | Ok("files-image") => {
                    // A tidy folder of Lumen's own under LUMEN_DATA, so a shot shows
                    // sample names, never the person's real files.
                    let dir = crate::store::data_dir().join("files-demo");
                    let _ = std::fs::create_dir_all(dir.join("Projects"));
                    let _ = std::fs::create_dir_all(dir.join("Saved pages"));
                    for (name, bytes) in [
                        ("notes.txt", 4096usize),
                        ("my-look.json", 512),
                        ("holiday.png", 812_000),
                        ("kettle-report.pdf", 2_300_000),
                        ("vault.kdbx", 64_000),
                        ("archive.torrent", 48_000),
                    ] {
                        let _ = std::fs::write(dir.join(name), vec![0u8; bytes]);
                    }
                    let what = match std::env::var("LUMEN_TEST_CARD").as_deref() {
                        Ok("files-open") => files::FileWhat::ThemeOpen,
                        Ok("files-kdbx") => files::FileWhat::Kdbx,
                        Ok("files-torrent") => files::FileWhat::Torrent,
                        Ok("files-image") => files::FileWhat::Image,
                        _ => files::FileWhat::ThemeSave { text: "{}".into(), suggested: "my-look.json".into() },
                    };
                    files::open_at(self, what, dir);
                }
                Ok("confirm3") => confirm::open(
                    self,
                    confirm::ConfirmKind::External {
                        origin: "https://open.spotify.com".into(),
                        uri: "spotify:track:1".into(),
                        remember: true,
                        scheme: "spotify".into(),
                    },
                ),
                _ => {}
            }
        }
        self.invalidate();
    }

    fn ntp_visible(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| s.full && s.panel == "none" && s.active_tab().is_some_and(|t| t.status == "new"))
    }

    /// The clock's tick: a second when the seconds show, else ten.
    fn clock_wanted(&self) -> u32 {
        if self.look.clock_seconds { 1_000 } else { 10_000 }
    }

    fn clock_timer(&mut self) {
        let want = self.clock_wanted();
        self.clock_period = want;
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_CLOCK, want, None);
        }
    }

    /// Spinner timer runs only while something is loading.
    fn spin(&mut self) {
        let loading = self.state.as_ref().is_some_and(|s| s.tabs.iter().any(|t| t.loading));
        if loading != self.spinning {
            self.spinning = loading;
            unsafe {
                if loading {
                    SetTimer(Some(self.hwnd), TIMER_SPIN, 33, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_SPIN);
                }
            }
        }
    }

    /// The glow behind the memory number breathes only over the hard line, and
    /// only in the compact column, where the number is drawn. Below that line
    /// there is no timer and no repaint: an idle Lumen stays idle.
    fn mem_timer(&mut self) {
        let want = self.look.motion.enabled
            && self.look.show_memory
            && self
                .state
                .as_ref()
                .is_some_and(|s| s.compact() && s.mem.is_some_and(|(b, _)| b >= MEM_HOT));
        if want != self.mem_pulse {
            self.mem_pulse = want;
            unsafe {
                if want {
                    SetTimer(Some(self.hwnd), TIMER_MEM, 80, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_MEM);
                }
            }
        }
    }

    // ------------------------------------------------------------ layout

    /// The chrome's own heights for the current look.
    fn metrics(&self) -> layout::Metrics {
        self.look.metrics()
    }

    /// Where the floating cards live: the sidebar, or with none (horizontal) a column at the
    /// left under the top rows, over the page.
    fn float_area(&self) -> Option<R> {
        if let Some(sb) = self.sidebar() {
            return Some(sb);
        }
        self.state.as_ref()?;
        let (w, h) = self.size_dip();
        let top = self.metrics().top();
        Some(R::new(0.0, top, (FLOAT_MIN_W + 16.0).min(w), (h - top).max(1.0)))
    }

    /// The cards sit against the right edge when the sidebar does.
    fn cards_right(&self) -> bool {
        self.look.kind.sidebar() && self.state.as_ref().is_some_and(|s| s.settings.sidebar_side == "right")
    }

    /// The page is a window over the chrome, so a card drawn over the page would be hidden
    /// under it. While cards reach over the page, the chrome rises above it, cut down to its
    /// own strips and those cards; when they close it sinks back, whole, to the bottom.
    fn sync_float_layer(&mut self, s: &UiState) {
        let k = self.k();
        let stage = R::new(s.stage.0, s.stage.1, s.stage.2, s.stage.3);
        let meets = |r: &R| r.x < stage.right() && r.right() > stage.x && r.y < stage.bottom() && r.bottom() > stage.y;
        let key: Vec<[i32; 5]> = if s.full {
            Vec::new()
        } else {
            self.floats
                .iter()
                .filter(|(r, _)| meets(r))
                .map(|(r, rad)| [(r.x * k) as i32, (r.y * k) as i32, (r.right() * k).ceil() as i32, (r.bottom() * k).ceil() as i32, (rad * k) as i32])
                .collect()
        };
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        if key == self.float_layer {
            // Same cards: only stay on top. A page made since (a new tab) starts above us.
            if !key.is_empty() {
                unsafe {
                    let _ = SetWindowPos(self.hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags);
                }
            }
            return;
        }
        self.float_layer = key;
        unsafe {
            if self.float_layer.is_empty() {
                let _ = SetWindowRgn(self.hwnd, None, true);
                let _ = SetWindowPos(self.hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, flags);
                return;
            }
            let mut rc = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut rc);
            let rgn = CreateRectRgn(0, 0, rc.right, rc.bottom);
            let hole = CreateRectRgn((stage.x * k) as i32, (stage.y * k) as i32, (stage.right() * k).ceil() as i32, (stage.bottom() * k).ceil() as i32);
            CombineRgn(Some(rgn), Some(rgn), Some(hole), RGN_DIFF);
            let _ = DeleteObject(hole.into());
            for c in &self.float_layer {
                let card = CreateRoundRectRgn(c[0], c[1], c[2] + 1, c[3] + 1, 2 * c[4], 2 * c[4]);
                CombineRgn(Some(rgn), Some(rgn), Some(card), RGN_OR);
                let _ = DeleteObject(card.into());
            }
            // The window owns the region from here on.
            SetWindowRgn(self.hwnd, Some(rgn), true);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags);
        }
    }

    /// The sidebar's rectangle, in chrome coordinates. None for a layout with no
    /// sidebar. The chrome covers the whole window, so this is always the edge
    /// strip, with or without a panel open.
    fn sidebar(&self) -> Option<R> {
        let s = self.state.as_ref()?;
        if !self.look.kind.sidebar() {
            return None;
        }
        let (w, h) = self.size_dip();
        let sw = s.sidebar_w();
        let right = s.settings.sidebar_side == "right";
        let x = if right { (w - sw).max(0.0) } else { 0.0 };
        match self.look.kind {
            Kind::Hybrid => {
                let top = self.metrics().top();
                Some(R::new(x, top, sw, (h - top).max(1.0)))
            }
            _ => Some(R::new(x, 0.0, sw, h)),
        }
    }

    /// A layout's top rows, in chrome coordinates: the tab strip (horizontal
    /// only), the toolbar, and the bookmarks bar when it is on.
    fn top_rows(&self) -> Option<(Option<R>, R, Option<R>)> {
        if !self.look.kind.top_toolbar() {
            return None;
        }
        let (w, _) = self.size_dip();
        let m = self.metrics();
        let strip = self.look.kind.tab_strip().then(|| R::new(0.0, 0.0, w, m.strip_h));
        let tool = R::new(0.0, m.strip_h, w, m.tool_h);
        let bm = (m.bm_h > 0.0).then(|| R::new(0.0, m.strip_h + m.tool_h, w, m.bm_h));
        Some((strip, tool, bm))
    }

    fn size_dip(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        (rc.right as f32 / self.k(), rc.bottom as f32 / self.k())
    }

    fn omni_rect(&self) -> R {
        let Some(s) = &self.state else { return R::new(0.0, 0.0, 0.0, 0.0) };
        // A top toolbar: navigation at the left, downloads and the menu at the
        // right, the address bar taking everything between.
        if let Some((_, tool, _)) = self.top_rows() {
            let pad = 8.0;
            let nav = 3.0 * 32.0 + pad;
            let right = 2.0 * 34.0 + pad;
            let x = tool.x + nav;
            let w = (tool.right() - right - x).max(120.0);
            let h = (tool.h - 10.0).max(24.0);
            return R::new(x, tool.y + (tool.h - h) / 2.0, w, h);
        }
        let sb = self.sidebar().unwrap_or_default();
        let ws_h = self.ws_h(s);
        if s.compact() {
            R::new(sb.x + (sb.w - 36.0) / 2.0, sb.y + 140.0 + ws_h, 36.0, 34.0)
        } else {
            R::new(sb.x + 8.0, sb.y + 46.0 + ws_h, sb.w - 16.0, 34.0)
        }
    }

    fn ntp_field(&self) -> R {
        self.ntp_geometry().field
    }

    /// The new tab page's parts: the search field, and the speed dial's cells under it. With
    /// many rows of tiles the whole composition rises, so the last row is not cut off, but never
    /// so far that the clock above the field runs out of room.
    fn ntp_geometry(&self) -> NtpGeo {
        let s = self.state.as_ref().unwrap();
        let lo = &self.look;
        let (x, y, w, h) = s.stage;
        // One column: the clock and the numbers, the search bar, the site cards.
        let cw = (w - 112.0).clamp(300.0, 1000.0);
        let x0 = x + (w - cw) / 2.0;
        let cells = dial_cells(s);
        let cols = if lo.dial_cols > 0 { lo.dial_cols } else if cw >= 760.0 { 6 } else { 4 };
        let grid = DialGrid::new(x0, cw, cells.len(), NTP_CARD_H - 44.0 + 12.0, cols);
        let head = if lo.clock { self.ntp_head_h() } else { 40.0 };
        let total = head + 18.0 + 56.0 + 18.0 + grid.height();
        let top = (y + (h - total) * 0.42).max(y + 40.0);
        let field = R::new(x0, top + head + 18.0, cw, 56.0);
        NtpGeo { field, grid, top: field.bottom() + 18.0, cells }
    }

    /// How tall the clock and its date are: the clock's own size, its date line, air.
    fn ntp_head_h(&self) -> f32 {
        let s = self.state.as_ref().unwrap();
        let big = self.ntp_clock_font(s.stage.2).size10 as f32 / 10.0;
        big * 0.92 + 40.0
    }

    /// The clock's face at the size the settings or the look give it.
    fn ntp_clock_font(&self, stage_w: f32) -> Font {
        let lo = &self.look;
        let mut f = lo.clock_font;
        if !lo.clock_display_face {
            f = Font { face: gfx::Face::Ui, ..f };
        }
        if lo.clock_size > 0 {
            f.size10 = lo.clock_size * 10;
        } else if stage_w < 760.0 {
            f.size10 = (f.size10 as f32 * 0.72) as u32;
        }
        f
    }

    /// Edit boxes are real windows; keep them on top of what is drawn under them.
    fn layout_edits(&mut self) {
        let Some(s) = &self.state else { return };
        let k = self.k();
        let place = |e: HWND, r: R, show: bool| unsafe {
            if show {
                let _ = MoveWindow(e, (r.x * k) as i32, (r.y * k) as i32, (r.w * k) as i32, (r.h * k) as i32, true);
                let _ = ShowWindow(e, SW_SHOWNA);
            } else {
                let _ = ShowWindow(e, SW_HIDE);
            }
        };
        let o = self.omni_rect();
        let text_h = 20.0;
        // A compact sidebar hides the box until the address bar is opened; a top
        // toolbar always shows it, since it is the layout's whole point.
        let edit_on = self.editing && (!s.compact() || self.look.kind.top_toolbar());
        place(self.edit, R::new(o.x + 32.0, o.y + (o.h - text_h) / 2.0, o.w - 42.0, text_h), edit_on);
        let f = self.ntp_field();
        // The box hides when idle and empty, so the drawn placeholder shows through.
        let ntp_live = unsafe { GetFocus() } == self.ntp_edit || !wide_text(self.ntp_edit).is_empty();
        let box_on = self.ntp_visible() && self.look.ntp_search;
        place(self.ntp_edit, R::new(f.x + 52.0, f.y + (f.h - 22.0) / 2.0, f.w - 130.0, 22.0), box_on && ntp_live);
        let b = pages::bm_search_rect(R::new(s.stage.0, s.stage.1, s.stage.2, s.stage.3), self.look.s5);
        place(self.bm_edit, R::new(b.x + 34.0, b.y + (b.h - 20.0) / 2.0, b.w - 46.0, 20.0), s.full && s.panel == "bookmarks");
        // The Drive panel's task box, always shown while that page is: it is where the
        // task is typed, so it holds the focus as soon as the panel opens.
        let d = pages::drive_task_rect(R::new(s.stage.0, s.stage.1, s.stage.2, s.stage.3), self.look.s5);
        let show = s.full && s.panel == "drive";
        place(self.drive_edit, R::new(d.x + 8.0, d.y + (d.h - 22.0) / 2.0, d.w - 16.0, 22.0), show);
        let q = pages::set_search_rect(R::new(s.stage.0, s.stage.1, s.stage.2, s.stage.3));
        place(self.set_edit, R::new(q.x + 34.0, q.y + (q.h - 20.0) / 2.0, q.w - 46.0, 20.0), s.full && s.panel == "settings" && self.page.font_pick.is_none());
        let b = pages::bug_story_rect(R::new(s.stage.0, s.stage.1, s.stage.2, s.stage.3), self.look.s5);
        place(self.bug_edit, R::new(b.x + 16.0, b.y + (b.h - 22.0) / 2.0, b.w - 32.0, 22.0), s.full && s.panel == "bugreport");
        // The colour picker's hex box, inside the box the card draws for it.
        match &self.color {
            Some(c) => place(self.color_edit, R::new(c.hex_box().x + 8.0, c.hex_box().y + 4.0, c.hex_box().w - 16.0, 18.0), true),
            None => place(self.color_edit, R::default(), false),
        }
        // The form card's boxes, one per field, inside the boxes it draws for them.
        // A field whose text hides uses the password box.
        let form_at = |n: usize| -> (HWND, Option<R>) {
            let secret = self.form.as_ref().and_then(|f| f.fields.get(n)).map(|(_, _, s)| *s).unwrap_or(false);
            let e = if secret { self.form_secret_edit } else { self.form_edits[n.min(self.form_edits.len() - 1)] };
            (e, self.form.as_ref().and_then(|f| f.boxes.get(n)).copied())
        };
        for n in 0..(self.form.as_ref().map(|f| f.fields.len()).unwrap_or(0)) {
            let (e, b) = form_at(n);
            match b {
                Some(b) => place(e, R::new(b.x + 8.0, b.y + (b.h - 18.0) / 2.0, b.w - 16.0, 18.0), true),
                None => place(e, R::default(), false),
            }
        }
        for n in 0..self.form_edits.len().max(1) {
            let (e, b) = form_at(n);
            if b.is_none() {
                place(e, R::default(), false);
            }
        }
        if self.form.is_none() {
            place(self.form_secret_edit, R::default(), false);
        }
        // The file card's two boxes: the path, and the name in save mode.
        match &self.files {
            Some(f) => {
                place(self.file_path_edit, R::new(f.path_box.x + 8.0, f.path_box.y + 4.0, f.path_box.w - 16.0, 20.0), true);
                let save = matches!(f.what, files::FileWhat::ThemeSave { .. });
                place(self.file_name_edit, R::new(f.name_box.x + 8.0, f.name_box.y + 4.0, f.name_box.w - 16.0, 18.0), save);
            }
            None => {
                place(self.file_path_edit, R::default(), false);
                place(self.file_name_edit, R::default(), false);
            }
        }
        // The Drive page takes the cursor once, when it opens.
        if show && std::mem::take(&mut self.drive_focus) {
            unsafe {
                let _ = SetFocus(Some(self.drive_edit));
            }
        }
        // The tab search's filter, inside the card over the sidebar.
        if let Some(sb) = self.float_area() {
            let right = self.cards_right();
            let q = wide_text(self.search_edit).trim().to_lowercase();
            let n = switch_rows(s, &q).len();
            let card = search_card(sb, right, n.min(SEARCH_ROWS), n > SEARCH_ROWS);
            place(self.search_edit, R::new(card.x + 38.0, card.y + 13.0, card.w - 52.0, 22.0), self.search_open);
            // The palette's filter, inside its own card over the sidebar.
            if self.pal_open {
                let (rows, groups, total) = pal_view(s, &wide_text(self.palette_edit).trim().to_lowercase());
                let card = pal_card(sb, right, rows.len(), groups, total > rows.len());
                place(self.palette_edit, R::new(card.x + 38.0, card.y + 13.0, card.w - 52.0, 22.0), true);
            } else {
                place(self.palette_edit, R::default(), false);
            }
            let card = say_card(sb, right);
            place(self.say_edit, R::new(card.x + 22.0, card.y + 50.0, card.w - 44.0, 22.0), self.say.is_some());
        } else {
            place(self.say_edit, R::default(), false);
        }
    }

    fn focus_ntp(&mut self) {
        unsafe {
            let _ = ShowWindow(self.ntp_edit, SW_SHOWNA);
            let _ = SetFocus(Some(self.ntp_edit));
        }
        self.layout_edits();
        self.invalidate();
    }

    /// Open the message card for this agent, the text box empty and focused.
    fn open_say(&mut self, session: String, name: String) {
        self.close_search();
        self.close_palette();
        if self.state.as_ref().is_some_and(|s| s.settings.compact) {
            send(&self.proxy, json!({"t": "peek", "on": true}));
        }
        self.say = Some(Say { session, name, mode: 0 });
        unsafe {
            SetWindowTextW(self.say_edit, &HSTRING::from("")).ok();
        }
        self.layout_edits();
        unsafe {
            let _ = SetFocus(Some(self.say_edit));
        }
        self.invalidate();
    }

    /// Send what is in the card, as the kind picked, and close it. Nothing typed, nothing sent.
    fn send_say(&mut self) {
        let text = wide_text(self.say_edit).trim().to_string();
        let Some(say) = self.say.as_ref() else { return };
        if text.is_empty() {
            return;
        }
        let kind = SAY_MODES[say.mode.min(2)].1;
        send(&self.proxy, json!({"t": "agentTell", "session": say.session, "say": kind, "text": text}));
        self.close_say();
    }

    fn close_say(&mut self) {
        if self.say.take().is_none() {
            return;
        }
        if self.state.as_ref().is_some_and(|s| s.settings.compact || s.peek) {
            send(&self.proxy, json!({"t": "peek", "on": false}));
        }
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
        self.layout_edits();
        send(&self.proxy, json!({"t": "focusPage"}));
        self.invalidate();
    }

    /// Close the tab search: filter box hidden, focus back on the page.
    fn close_search(&mut self) {
        if !self.search_open {
            return;
        }
        self.search_open = false;
        self.search_rows.clear();
        self.search_sel = 0;
        if self.state.as_ref().is_some_and(|s| s.settings.compact || s.peek) {
            send(&self.proxy, json!({"t": "peek", "on": false}));
        }
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
        self.layout_edits();
        send(&self.proxy, json!({"t": "focusPage"}));
        self.invalidate();
    }

    /// Close the command palette: filter box hidden, focus back on the page.
    fn close_palette(&mut self) {
        if !self.pal_open {
            return;
        }
        self.pal_open = false;
        self.pal_rows.clear();
        self.pal_sel = 0;
        if self.state.as_ref().is_some_and(|s| s.settings.compact || s.peek) {
            send(&self.proxy, json!({"t": "peek", "on": false}));
        }
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
        send(&self.proxy, json!({"t": "palOpen", "open": false}));
        self.layout_edits();
        send(&self.proxy, json!({"t": "focusPage"}));
        self.invalidate();
    }

    /// Enter in the command palette: run the picked row, then close.
    fn run_palette(&mut self, n: usize) {
        let Some(row) = self.pal_rows.get(n).cloned() else { return };
        match row.run {
            PalRun::Msg(v) => send(&self.proxy, v),
            PalRun::Set(words) => {
                // The Settings page has its own search box; the words land there, so
                // the page opens on the setting the row named.
                if self.state.as_ref().is_some_and(|s| s.panel == "settings") {
                    unsafe {
                        SetWindowTextW(self.set_edit, &HSTRING::from(words)).ok();
                    }
                } else {
                    self.pal_jump = Some(words);
                    send(&self.proxy, json!({"t": "panel", "open": true}));
                }
            }
            PalRun::Switch { id, other } => {
                let msg = match other {
                    Some((ws, idx)) => json!({"t": "switchTab", "ws": ws, "idx": idx, "other": true}),
                    None => json!({"t": "switchTab", "id": id}),
                };
                send(&self.proxy, msg);
            }
            PalRun::Open(url) => send(&self.proxy, json!({"t": "openTab", "url": url})),
        }
        self.close_palette();
    }

    /// Enter in the tab search: switch to the picked row, workspace and all.
    fn switch_to(&mut self, n: usize) {
        let Some(row) = self.search_rows.get(n).cloned() else { return };
        let msg = match row.other {
            Some((ws, idx)) => json!({"t": "switchTab", "ws": ws, "idx": idx, "other": true}),
            None => json!({"t": "switchTab", "id": row.id}),
        };
        send(&self.proxy, msg);
        self.close_search();
    }

    fn begin_edit(&mut self) {
        let Some(s) = &self.state else { return };
        if s.compact() {
            send(&self.proxy, json!({"t": "peek", "on": true}));
        }
        let url = s.active_tab().filter(|t| t.status != "new").map(|t| t.url.clone()).unwrap_or_default();
        self.focus_edit = s.compact();
        self.editing = true;
        self.omni_typed = Typed { text: url.clone(), ..Typed::default() };
        unsafe {
            SetWindowTextW(self.edit, &HSTRING::from(url)).ok();
        }
        self.sugg.clear();
        self.layout_edits();
        self.invalidate();
    }

    fn end_edit(&mut self) {
        if !self.editing {
            return;
        }
        self.editing = false;
        self.focus_edit = false;
        self.renaming = None;
        self.sugg.clear();
        // Peek ends with the edit, whatever the compact setting says now: Ctrl+B while the
        // sidebar was peeking turns compact off, and a peek left on would pin the sidebar open
        // the next time compact is switched on.
        if self.state.as_ref().is_some_and(|s| s.settings.compact || s.peek) {
            send(&self.proxy, json!({"t": "peek", "on": false}));
        }
        self.layout_edits();
        self.invalidate();
    }

    /// Show the omnibox edit filled with `name` for renaming workspace `idx`.
    pub fn begin_workspace_rename(&mut self, idx: usize, name: &str) {
        self.editing = true;
        self.renaming = Some(idx);
        self.sugg.clear();
        unsafe {
            SetWindowTextW(self.edit, &HSTRING::from(name)).ok();
            let _ = SetFocus(Some(self.edit));
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
        self.layout_edits();
        self.invalidate();
    }

    fn suggest(&mut self) {
        let q = self.omni_typed.changed(self.edit).trim().to_string();
        self.sugg.clear();
        self.sel = 0;
        if q.is_empty() {
            self.invalidate();
            return;
        }
        let looks_url = !q.contains(' ') && (q.contains("://") || q.starts_with("localhost") || {
            let h = q.split(['/', '?', '#']).next().unwrap_or("");
            h.rsplit_once('.').is_some_and(|(_, t)| t.len() >= 2 && t.chars().all(|c| c.is_ascii_alphabetic()))
        });
        if looks_url {
            self.sugg.push(Sugg { kind: SuggKind::Go(q.clone()), label: q.clone(), detail: String::new() });
        }
        self.sugg.push(Sugg { kind: SuggKind::Search(q.clone()), label: q.clone(), detail: String::new() });
        send(&self.proxy, json!({"t": "suggest", "q": q}));
        // Open tabs as "Switch to tab" rows, this workspace's first, then the workspaces that
        // are put away. Ranked as the quick switcher ranks them, best match first.
        if let Some(s) = &self.state {
            let mut all: Vec<(SuggKind, String, String, String)> = s
                .tabs
                .iter()
                .filter(|t| t.id != s.active && !t.url.is_empty())
                .map(|t| (SuggKind::Tab(t.id), t.title.clone(), t.url.clone(), String::new()))
                .collect();
            for o in &s.other_tabs {
                all.push((SuggKind::TabOther(o.ws, o.idx), o.title.clone(), o.url.clone(), o.ws_name.clone()));
            }
            let order = {
                let entries: Vec<crate::tabsearch::Entry> = all.iter().map(|(_, t, u, _)| crate::tabsearch::Entry { title: t, url: u }).collect();
                crate::tabsearch::rank(&q, &entries)
            };
            for n in order.into_iter().take(6) {
                let (kind, title, url, tag) = all[n].clone();
                let label = if title.is_empty() { host(&url) } else { title };
                self.sugg.push(Sugg { kind, label, detail: tag });
            }
        }
        self.invalidate();
    }

    fn pick(&mut self, i: usize) {
        let Some(sg) = self.sugg.get(i).cloned() else { return };
        match sg.kind {
            SuggKind::Tab(id) => send(&self.proxy, json!({"t": "activate", "id": id})),
            SuggKind::TabOther(ws, idx) => send(&self.proxy, json!({"t": "switchTab", "ws": ws, "idx": idx, "other": true})),
            SuggKind::Go(q) | SuggKind::Search(q) | SuggKind::Url(q, _) => send(&self.proxy, json!({"t": "nav", "url": q})),
        }
        self.end_edit();
    }

    // -------------------------------------------------------------- draw

    fn paint(&mut self) {
        let Some(s) = self.state.clone() else { return };
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let th = self.theme;
        let t0 = Instant::now();
        if !self.gfx.begin(rc.right as u32, rc.bottom as u32, th.bg) {
            return;
        }
        let t1 = Instant::now();
        self.draw_all(&s);
        let t2 = Instant::now();
        self.gfx.end();
        // LUMEN_PROFILE_PAINT=1: each frame's three phases on stderr. Read once.
        static PROFILE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *PROFILE.get_or_init(|| std::env::var_os("LUMEN_PROFILE_PAINT").is_some()) {
            eprintln!("PAINT begin {:.1} draw {:.1} end {:.1}", (t1 - t0).as_secs_f64() * 1e3, (t2 - t1).as_secs_f64() * 1e3, t2.elapsed().as_secs_f64() * 1e3);
        }
        self.sync_float_layer(&s);
        // The frame timer runs only while something moves, so an idle chrome
        // costs nothing: no timer, no repaint, no wake.
        if self.motion.running() != self.animating {
            self.animating = self.motion.running();
            unsafe {
                if self.animating {
                    SetTimer(Some(self.hwnd), TIMER_ANIM, 16, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_ANIM);
                }
            }
        }
        #[cfg(debug_assertions)]
        shot::after_paint(self);
    }

    /// One whole frame, onto whatever target the gfx has begun. Shared by the window and the debug shot.
    fn draw_all(&mut self, s: &UiState) {
        let lo = self.look.clone();
        let th = lo.theme;
        self.hits.clear();
        self.floats.clear();
        if lo.style == theme::Style::Glass {
            // Glass's wallpaper: two soft blooms on its ground, under everything else.
            let (w, h) = self.size_dip();
            self.gfx.bloom(w * 0.08, -h * 0.1, w.max(h) * 0.72, theme::Color::from_hex("#5a43e6").unwrap_or(th.accent).alpha(0.95));
            self.gfx.bloom(w * 1.02, h * 1.08, w.max(h) * 0.62, theme::Color::from_hex("#d2457e").unwrap_or(th.accent).alpha(0.85));
        }
        self.motion.set_config(lo.motion);
        self.motion.begin();
        // The workspace changed: the tab list cross-fades in its place.
        if self.last_ws != s.active_workspace {
            self.last_ws = s.active_workspace;
            self.motion.restart(Id::Workspace(s.active_workspace), Prop::Opacity, 0.0, 1.0, 150, Ease::OutCubic);
        }
        // A closed tab's ghost is done once it has collapsed, or after a moment
        // whatever the motion setting says.
        self.closing.retain(|c| c.at.elapsed().as_secs_f32() < 0.5);
        // The glance card's dim wash over the page fades with the card.
        if let Some(dim) = self.glance_dim {
            let a = if s.glance {
                self.motion.enter(Id::Glance, Prop::Opacity, 0.0, 1.0, 170, Ease::OutCubic)
            } else {
                self.motion.tween(Id::Glance, Prop::Opacity, 0.0, 170, Ease::OutCubic)
            };
            unsafe {
                if a <= 0.01 {
                    let _ = ShowWindow(dim, SW_HIDE);
                } else {
                    let _ = SetLayeredWindowAttributes(dim, COLORREF(0), (75.0 * a.min(1.0)) as u8, LWA_ALPHA);
                }
            }
        }
        if s.full {
            let (x, y, w, h) = s.stage;
            let st = R::new(x, y, w, h);
            // A page is the look's ground; its blocks are cards on it.
            self.gfx.round(st, lo.page_radius, page_ground(&lo));
            if s.panel != "none" {
                // Opening: a fade and a slight rise, over the surface underneath.
                let a = self.motion.enter(Id::Panel(s.panel), Prop::Opacity, 0.0, 1.0, 170, Ease::OutCubic);
                let rise = self.motion.enter(Id::Panel(s.panel), Prop::Y, 8.0, 0.0, 170, Ease::OutCubic);
                let st = R::new(st.x, st.y + rise, st.w, st.h);
                pages::draw(self, st, s);
                if a < 1.0 {
                    self.gfx.round(st, lo.page_radius, page_ground(&lo).alpha(1.0 - a));
                }
            } else if self.ntp_visible() && !s.settings.first_run_done {
                pages::draw_first_run(self, st, s);
            } else if self.ntp_visible() {
                self.draw_ntp(st, s);
                self.draw_update_note(st, s);
            }
        }
        match lo.kind {
            Kind::Vertical => self.draw_sidebar(s),
            Kind::Horizontal => self.draw_top(s),
            Kind::Hybrid => {
                self.draw_toolbar(s);
                self.draw_side_body(s);
                self.draw_suggestions(s);
            }
        }
        // The page's own outline, when it was asked for. The page is a window
        // over the chrome, so this is the one place a line around it can be drawn.
        if lo.page_border && !s.full {
            let (x, y, w, h) = s.stage;
            let st = R::new(x - 1.0, y - 1.0, w + 2.0, h + 2.0);
            self.gfx.round_stroke(st, lo.page_radius + 1.0, th.line, 1.0);
        }
        // Where the hovered link goes: a pill at the page's bottom left, under every card.
        self.draw_link(s);
        // The cards over everything else. Their order is their z-order.
        color::draw(self, s);
        form::draw(self, s);
        files::draw(self, s);
        confirm::draw(self, s);
        menu::draw(self, s);
        self.motion.end();
    }

    /// One card at a time: opening one puts the others away. A form, a question, a
    /// menu and a file card are dropped (their actions never run); a colour keeps
    /// what it shows.
    /// The card for the active page's question: opened when a new one comes, put away
    /// unanswered when the person moves to another tab (it waits there for them).
    fn sync_page_ask(&mut self) {
        let want = self.state.as_ref().and_then(|s| s.page_ask.clone());
        if want.as_ref().map(|a| a.aid) == self.page_ask_shown {
            return;
        }
        if self.page_ask_shown.take().is_some() {
            confirm::put_away(self);
            form::put_away(self);
        }
        let Some(a) = want else { return };
        self.page_ask_shown = Some(a.aid);
        use crate::engine::Ask;
        match &a.ask {
            Ask::Prompt(text, default) => {
                let label: String = text.chars().take(120).collect();
                form::open_fields(self, &format!("{} asks", a.host), &[(&label, default, false)], form::FormAct::PagePrompt { aid: a.aid });
            }
            Ask::SignIn(realm) => {
                let title = if realm.is_empty() { format!("Sign in to {}", a.host) } else { format!("Sign in to {} \u{b7} {realm}", a.host) };
                form::open_fields(self, &title, &[("User name", "", false), ("Password", "", true)], form::FormAct::SignIn { aid: a.aid });
            }
            _ => confirm::open(self, confirm::ConfirmKind::Page { aid: a.aid, host: a.host.clone(), ask: a.ask.clone(), many: a.many }),
        }
    }

    fn close_cards(&mut self) {
        if self.form.is_some() {
            form::cancel(self);
        }
        if self.confirm.is_some() {
            confirm::cancel(self);
        }
        if self.menu.is_some() {
            menu::cancel(self);
        }
        if self.files.is_some() {
            files::cancel(self);
        }
        if self.color.is_some() {
            color::done(self);
        }
    }

    fn hit(&mut self, r: R, h: Hit) {
        self.hits.push((r, h));
    }

    fn is_hover(&self, h: &Hit) -> bool {
        self.hover.as_ref() == Some(h)
    }

    /// The motion id of a button, so its hover plate is animated per button.
    fn button_id(h: &Hit) -> Id {
        Id::Button(match h {
            Hit::Back => "back",
            Hit::Fwd => "fwd",
            Hit::Reload => "reload",
            Hit::Min => "min",
            Hit::Max => "max",
            Hit::Close => "close",
            Hit::Sidebar => "sidebar",
            Hit::Gear => "gear",
            Hit::Mem => "mem",
            Hit::MediaPlay => "mediaplay",
            Hit::MediaNext => "medianext",
            Hit::MediaTitle(_) => "mediatitle",
            Hit::DlFooter => "dl",
            Hit::DashFooter => "dash",
            Hit::SiteInfo => "siteinfo",
            Hit::ZoomChip => "zoom",
            Hit::SiteClear => "siteclear",
            Hit::SiteSettings => "sitesettings",
            Hit::Star => "star",
            Hit::NewTab => "newtab",
            Hit::Menu => "menu",
            Hit::WorkspaceAdd => "wsadd",
            Hit::WsChip => "wschip",
            Hit::GlanceOpen => "glance",
            Hit::AgentStop => "agentstop",
            Hit::Omni => "omni",
            _ => "other",
        })
    }

    /// A section label: the look's label face, in capitals, spaced out.
    fn label(&self, text: &str, r: R, c: theme::Color) {
        let lo = &self.look;
        self.gfx.text_tracked(&text.to_uppercase(), r, lo.label, c, lo.label_track);
    }

    fn button(&mut self, r: R, icon: &str, h: Hit, enabled: bool) {
        let th = self.theme;
        let hov = enabled && self.is_hover(&h);
        // Hover glides in, and a press is one step further than hovering, so a
        // click feels received. Nothing runs at rest.
        let target = if !enabled { 0.0 } else if self.press.as_ref() == Some(&h) { 1.25 } else if hov { 1.0 } else { 0.0 };
        let a = self.motion.hover(Self::button_id(&h), target).min(1.0);
        if a > 0.01 {
            let pressed = self.press.as_ref() == Some(&h);
            let bg = if h == Hit::Close { th.danger } else if pressed { th.press } else { th.hover };
            self.gfx.round(r, self.look.r_sm, bg.alpha(a));
        }
        let c = if !enabled {
            th.faint.alpha(0.5)
        } else if hov && h == Hit::Close {
            theme::Color(0.98, 0.98, 0.98, 1.0)
        } else if hov {
            th.text
        } else {
            th.dim
        };
        self.gfx.icon(icon, r.x + (r.w - 16.0) / 2.0, r.y + (r.h - 16.0) / 2.0, 16.0, c);
        if enabled {
            self.hit(r, h);
        }
    }

    /// The navigation arrows and the reload button, which turns while the page
    /// loads. `x`, `y` is the first button's top-left. Returns the width taken.
    fn nav_buttons(&mut self, x: f32, y: f32, s: &UiState, compact: bool) -> f32 {
        let act = s.active_tab().cloned();
        let loading = act.as_ref().is_some_and(|t| t.loading);
        let size = if compact { 28.0 } else { 30.0 };
        let step = size + 1.0;
        let navs = [
            ("back", Hit::Back, act.as_ref().is_some_and(|t| t.back)),
            ("fwd", Hit::Fwd, act.as_ref().is_some_and(|t| t.fwd)),
            (if loading { "stop" } else { "reload" }, Hit::Reload, true),
        ];
        for (n, (icon, h, en)) in navs.into_iter().enumerate() {
            let r = R::new(x + n as f32 * step, y, size, size);
            if h == Hit::Reload && loading {
                // The reload icon spins while the page loads.
                self.spin_icon(r, s, icon, en);
            } else {
                self.button(r, icon, h, en);
            }
        }
        3.0 * step
    }

    /// One button whose icon turns while a page is loading (the reload arrow).
    fn spin_icon(&mut self, r: R, s: &UiState, icon: &str, enabled: bool) {
        let th = self.theme;
        let hov = enabled && self.is_hover(&Hit::Reload);
        let a = self.motion.hover(Id::Button("reload"), if hov { 1.0 } else { 0.0 }).min(1.0);
        if a > 0.01 {
            self.gfx.round(r, 7.0, th.hover.alpha(a));
        }
        let phase = (self.born.elapsed().as_secs_f32() / 0.9).fract();
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let c = if self.look.motion.enabled {
            // A turning arc, with the arrow still in the middle.
            self.gfx.arc(cx, cy, 7.0, phase * 360.0, 110.0, th.accent, 1.8);
            th.dim
        } else {
            th.dim
        };
        self.gfx.icon(icon, r.x + (r.w - 16.0) / 2.0, r.y + (r.h - 16.0) / 2.0, 16.0, c);
        if enabled {
            self.hit(r, Hit::Reload);
        }
        let _ = s;
    }

    /// The window buttons: minimize, maximize/restore, close. `right` is their
    /// right edge; they sit leftward from it. Returns the whole group's rectangle.
    fn window_buttons(&mut self, right: f32, y: f32, size: f32, s: &UiState) -> R {
        let step = size + 1.0;
        let wins = [("min", Hit::Min), (if s.maximized { "restore" } else { "max" }, Hit::Max), ("close", Hit::Close)];
        for (n, (icon, h)) in wins.into_iter().enumerate() {
            let r = R::new(right - (3 - n) as f32 * step, y, size, size);
            self.button(r, icon, h, true);
        }
        R::new(right - 3.0 * step, y, 3.0 * step, size)
    }

    // -------------------------------------------------------------- sidebar
    //
    // The vertical layout: navigation, the address bar, workspaces, the tab list
    // and the footer down one side. The hybrid layout draws the same body under a
    // top toolbar; the horizontal layout has its own strip and toolbar below.

    /// The workspace row's height, honouring the setting.
    fn ws_h(&self, s: &UiState) -> f32 {
        if self.look.ws_row { s.ws_h() } else { 0.0 }
    }

    fn draw_sidebar(&mut self, s: &UiState) {
        let Some(sb) = self.sidebar() else { return };
        let lo = self.look.clone();
        let th = lo.theme;
        // The sidebar's own colour; the page gaps behind the window keep `bg`. Glass draws a
        // frosted pane over its wallpaper instead.
        if lo.style == theme::Style::Glass {
            let pane = sb.inset(3.0, 3.0);
            self.gfx.round(pane, 16.0, theme::Color(1.0, 1.0, 1.0, 0.07));
            self.gfx.round_stroke(pane, 16.0, theme::Color(1.0, 1.0, 1.0, 0.12), 1.0);
        } else {
            self.gfx.fill(sb, th.side);
        }
        if lo.style != theme::Style::Glass {
            let x = if s.settings.sidebar_side == "right" { sb.x + 0.5 } else { sb.right() - 0.5 };
            self.gfx.line(x, sb.y, x, sb.bottom(), th.line, 1.0);
        }
        let compact = s.compact();
        let top = sb.y;
        // Top bar: navigation and window buttons. The empty rest drags the window.
        self.hit(R::new(sb.x, top, sb.w, 40.0), Hit::Drag);
        if compact {
            let x = sb.x + (sb.w - 28.0) / 2.0;
            let y = |v: f32| top + v;
            self.button(R::new(x, y(8.0), 28.0, 28.0), "close", Hit::Close, true);
            self.button(R::new(x, y(38.0), 28.0, 28.0), if s.maximized { "restore" } else { "max" }, Hit::Max, true);
            self.button(R::new(x, y(68.0), 28.0, 28.0), "min", Hit::Min, true);
            self.button(R::new(x, y(104.0), 28.0, 28.0), "sidebar", Hit::Sidebar, true);
        } else {
            // Navigation on the left, window buttons at the right edge, on either side.
            let nav_x = sb.x + 8.0;
            self.nav_buttons(nav_x, top + 9.0, s, false);
            // After the three arrows, set apart: it changes the sidebar, not the page.
            let tog = R::new(nav_x + 3.0 * 29.0 + 6.0, top + 9.0, 28.0, 28.0);
            let wins = self.window_buttons(sb.right() - 8.0, top + 9.0, 28.0, s);
            if tog.right() + 6.0 <= wins.x {
                self.button(tog, "sidebar", Hit::Sidebar, true);
            }
        }
        self.draw_ws_row(s, sb, compact);
        self.draw_omni_bar(s, compact);
        self.draw_chips(s, sb, compact);
        self.draw_tab_list(s, sb, compact);
        self.draw_footer(s, sb, compact);
        self.draw_cards(s, sb, compact);
        self.draw_suggestions(s);
    }

    /// The hybrid layout's sidebar: the tab list and the footer under the toolbar.
    fn draw_side_body(&mut self, s: &UiState) {
        let Some(sb) = self.sidebar() else { return };
        let lo = self.look.clone();
        self.gfx.fill(sb, lo.theme.side);
        let compact = s.compact();
        // A slim drag strip at the very top of the sidebar.
        self.hit(R::new(sb.x, sb.y, sb.w, 6.0), Hit::Drag);
        self.draw_ws_row(s, sb, compact);
        self.draw_chips(s, sb, compact);
        self.draw_tab_list(s, sb, compact);
        self.draw_footer(s, sb, compact);
        self.draw_cards(s, sb, compact);
    }

    /// Workspace switcher: pill tabs at the top of the sidebar (non-compact).
    fn draw_ws_row(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        if compact || self.ws_h(s) <= 0.0 {
            return;
        }
        // Under a top toolbar the sidebar starts higher: the toolbar has the
        // navigation and the address bar.
        let ws_y = sb.y + if lo.kind.top_toolbar() { 8.0 } else { 42.0 };
        let ws_h = 26.0;
        let n = s.workspaces.len();
        let glance_w = if s.glance { self.gfx.measure("Open as tab", lo.font(12.0, 500)) + 20.0 } else { 0.0 };
        // The row holds the pills, "+" and, while a card is open, "Open as tab".
        // Share the sidebar between them, so a long name cannot push a button out.
        let gaps = lo.s2 * 0.75 * (n as f32 + if s.glance { 1.0 } else { 0.0 });
        let room = (sb.w - 16.0 - 28.0 - glance_w - gaps).max(44.0 * n as f32);
        let cap = room / n.max(1) as f32;
        let mut wx = sb.x + 8.0;
        for (i, name) in s.workspaces.iter().enumerate() {
            let active = i == s.active_workspace;
            let tw = (self.gfx.measure(name, lo.font(12.0, 500)) + 20.0).min(cap).max(44.0);
            let r = R::new(wx, ws_y, tw, ws_h);
            let hit = Hit::Workspace(i);
            let hov = self.is_hover(&hit);
            let a = self.motion.hover(Id::Workspace(i), if hov { 1.0 } else { 0.0 });
            let label = R::new(r.x + 8.0, r.y, r.w - 16.0, r.h);
            if active {
                self.gfx.round(r, lo.r_sm + 1.0, th.raise);
                self.gfx.text(name, label, lo.font(12.0, 600).align(Align::Center), th.text);
            } else {
                if a > 0.01 {
                    self.gfx.round(r, lo.r_sm + 1.0, th.hover.alpha(a));
                }
                self.gfx.text(name, label, lo.font(12.0, 500).align(Align::Center), if hov { th.text } else { th.dim });
            }
            self.hit(r, hit);
            wx += tw + 6.0;
        }
        // "+" to add a workspace.
        let add_r = R::new(wx, ws_y, 26.0, ws_h);
        let ah = self.is_hover(&Hit::WorkspaceAdd);
        if ah {
            self.gfx.round(add_r, lo.r_sm + 1.0, th.hover);
        }
        self.gfx.icon("plus", add_r.x + 6.0, add_r.y + 6.0, 14.0, if ah { th.dim } else { th.faint });
        self.hit(add_r, Hit::WorkspaceAdd);
        wx += 34.0;
        // "Open as tab" when a glance is open.
        if s.glance && glance_w >= 60.0 && wx + 60.0 < sb.right() {
            let open_r = R::new(wx, ws_y, glance_w.min(sb.right() - wx - 8.0), ws_h);
            let oh = self.is_hover(&Hit::GlanceOpen);
            self.gfx.round(open_r, lo.r_md, if oh { th.accent } else { th.accent.alpha(0.85) });
            self.gfx.text("Open as tab", open_r, lo.font(11.5, 600).align(Align::Center), th.bg);
            self.hit(open_r, Hit::GlanceOpen);
        }
    }

    /// The address bar: lock or search at the left, the host, and the star.
    /// Vertical: in the sidebar. The top layouts: in the toolbar.
    fn draw_omni_bar(&mut self, s: &UiState, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        let act = s.active_tab().cloned();
        let in_toolbar = lo.kind.top_toolbar();
        let o = self.omni_rect();
        let hov = self.is_hover(&Hit::Omni);
        let focused = self.editing;
        let orad = lo.round(o.h);
        self.gfx.round(o, orad, if hov && !focused { th.raise.mix(th.hover, 0.5) } else { th.raise });
        if focused {
            self.gfx.round_stroke(o, orad, th.text.alpha(0.85), 1.5);
        } else {
            self.gfx.round_stroke(o, orad, th.line, 1.0);
        }
        let secure = act.as_ref().is_some_and(|t| t.url.starts_with("https://"));
        let site = act.as_ref().is_some_and(|t| t.url.starts_with("http"));
        // A compact sidebar folds the bar to its icon; a top toolbar never is
        // (compact there only tightens the rows).
        if compact && !in_toolbar {
            self.gfx.icon("search", o.x + (o.w - 16.0) / 2.0, o.y + 9.0, 16.0, th.dim);
        } else {
            // The icon is the site's button: a lock for https, a globe for plain http.
            let icon = if secure && !focused { "lock" } else if site && !focused { "globe" } else { "search" };
            let plate = R::new(o.x + 4.0, o.y + 5.0, 28.0, 24.0);
            let hov = self.is_hover(&Hit::SiteInfo) || self.site_open;
            if site && !focused && hov {
                self.gfx.round(plate, 7.0, th.hover);
            }
            self.gfx.icon(icon, o.x + 10.0, o.y + 9.0, 16.0, if hov && site { th.text } else if secure { th.dim } else { th.faint });
            if !focused {
                let text = act.as_ref().filter(|t| t.status != "new").map(|t| {
                    let h = host(&t.url);
                    if h.is_empty() { t.url.clone() } else { h }
                });
                let f = lo.font(13.0, 400);
                match text {
                    Some(t) => self.gfx.text(&t, R::new(o.x + 32.0, o.y, o.w - 42.0, o.h), f, th.text),
                    None => self.gfx.text("Search or enter address", R::new(o.x + 32.0, o.y, o.w - 42.0, o.h), f, th.faint),
                }
            }
        }
        self.hit(o, Hit::Omni);
        // The lock, the zoom chip and the star only exist in the full bar: a
        // compact sidebar has room for its icon and nothing else.
        let full_bar = !compact || in_toolbar;
        // The lock sits over the omnibox's own hit, so it is registered after it.
        if site && full_bar && !focused {
            self.hit(R::new(o.x + 4.0, o.y + 5.0, 28.0, 24.0), Hit::SiteInfo);
        }
        // The zoom chip, when the page is not at 100%: one click takes it back.
        if full_bar && !focused && s.zoom != 100 && act.as_ref().is_some_and(|t| !t.url.is_empty()) {
            let label = format!("{}%", s.zoom);
            let f = lo.font(11.5, 500);
            let w = self.gfx.measure(&label, f) + 16.0;
            let r = R::new(o.right() - 36.0 - w, o.y + 5.0, w, 24.0);
            let h = Hit::ZoomChip;
            let hov = self.is_hover(&h);
            self.gfx.round(r, r.h / 2.0, if hov { th.press } else { th.hover });
            self.gfx.text(&label, r, f.align(Align::Center), if hov { th.text } else { th.dim });
            self.hit(r, h);
        }
        if full_bar && !focused && act.as_ref().is_some_and(|t| !t.url.is_empty() && !t.agent) {
            let r = R::new(o.right() - 30.0, o.y + 5.0, 24.0, 24.0);
            let h = Hit::Star;
            let a = self.motion.hover(Id::Button("star"), if self.is_hover(&h) { 1.0 } else { 0.0 });
            if a > 0.02 {
                self.gfx.round(r, lo.r_sm, th.hover.alpha(a.min(1.0)));
            }
            self.gfx.icon(if s.bookmarked { "star-on" } else { "star" }, r.x + 4.0, r.y + 4.0, 16.0, if s.bookmarked { th.text } else { th.faint });
            self.hit(r, h);
        }
    }

    /// Offer chips under the address bar: read aloud in reader view, and the
    /// KeePass fills for this site. The tab list steps down for the row.
    fn draw_chips(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        if compact || (s.listen.is_empty() && s.kdbx.is_empty()) {
            return;
        }
        // Under a top toolbar the row sits right under the toolbar; in the
        // sidebar it follows the address bar and the workspace row.
        let y = if lo.kind.top_toolbar() { sb.y + 8.0 + self.ws_h(s) } else { sb.y + 84.0 + self.ws_h(s) };
        let mut cx = sb.x + 8.0;
        let right = sb.right() - 8.0;
        // The row itself fades in, so a chip appearing is not a blink.
        let a = self.motion.enter(Id::Chip("row"), Prop::Opacity, 0.0, 1.0, 150, Ease::OutCubic);
        if !s.listen.is_empty() {
            let label = match s.listen {
                "playing" => "Pause",
                "paused" => "Resume",
                _ => "Listen",
            };
            cx = self.chip(cx, y, right, label, Some(if s.listen == "playing" { "pause" } else { "play" }), Hit::Listen, true, a);
            if s.listen != "ready" {
                cx = self.chip(cx, y, right, "Stop", None, Hit::ListenStop, false, a);
            }
        }
        if !s.kdbx.is_empty() {
            self.chip(cx, y, right, "Fill from KeePass", Some("lock"), Hit::KdbxFill, false, a);
        }
        let _ = th;
    }

    /// The tab list down the sidebar: pins in a grid, then one row per tab, then
    /// "New tab". Rows glide to their places; a closed tab's row collapses.
    fn draw_tab_list(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        let row_h = lo.tab_row_h;
        let step = row_h + 1.0;
        let pad = if compact { 6.0 } else { 8.0 };
        let offer = !compact && (!s.listen.is_empty() || !s.kdbx.is_empty());
        // Rows start under whatever the top of the sidebar holds. Under a top
        // toolbar that is the workspace row and the offer chips and nothing else;
        // in the sidebar the navigation, the address bar and those follow.
        let base = if lo.kind.top_toolbar() {
            8.0 + if offer { 32.0 } else { 0.0 }
        } else if compact {
            184.0
        } else if offer {
            122.0
        } else {
            90.0
        };
        let list_top = sb.y + base + self.ws_h(s);
        let list = R::new(sb.x, list_top, sb.w, (sb.h - base - self.ws_h(s) - self.footer_h(compact)).max(1.0));
        self.gfx.clip(list);
        // A workspace switch cross-fades the list.
        let wf = self.motion.enter(Id::Workspace(s.active_workspace), Prop::Opacity, 1.0, 1.0, 150, Ease::OutCubic).min(1.0);
        let mut y = list.y - self.scroll;
        // Agent tabs are grouped per agent: their own section, under a header with the
        // agent's name, its colour and a Stop that ends that agent alone.
        let groups = agent_groups(s);
        let pins: Vec<UiTab> = s.tabs.iter().filter(|t| t.pinned && !t.agent).cloned().collect();
        let rests: Vec<UiTab> = s.tabs.iter().filter(|t| !t.pinned && !t.agent).cloned().collect();
        if !pins.is_empty() {
            let cols = if compact { 1 } else { (((sb.w - 2.0 * pad) + 4.0) / 44.0).floor().max(1.0) as usize };
            let tw = ((sb.w - 2.0 * pad) - 4.0 * (cols as f32 - 1.0)) / cols as f32;
            for (i, t) in pins.iter().enumerate() {
                let r = R::new(sb.x + pad + (i % cols) as f32 * (tw + 4.0), y + (i / cols) as f32 * 38.0, tw, 34.0);
                self.draw_tab(t, r, s, true, wf);
            }
            y += ((pins.len() + cols - 1) / cols) as f32 * 38.0 + 4.0;
            self.gfx.line(sb.x + pad, y, sb.right() - pad, y, th.line.alpha(wf), 1.0);
            y += 9.0;
        }
        // Every row of the list: the person's tabs first, then one block per agent
        // under its header. Header rows carry their own height, so the highlight and
        // the drops land right. Where each row belongs, in the list's own space. Rows
        // are drawn at their animated place: an added, closed or reordered tab makes
        // the ones around it glide instead of jump.
        if !compact {
            let name = s.workspaces.get(s.active_workspace).filter(|_| s.workspaces.len() > 1).cloned().unwrap_or_else(|| "Tabs".into());
            let lr = R::new(sb.x + pad + 8.0, y + 2.0, sb.w - 2.0 * pad - 44.0, 20.0);
            self.label(&name, lr, th.faint.alpha(wf));
            let plus = R::new(sb.right() - pad - 26.0, y, 24.0, 24.0);
            let hp = self.is_hover(&Hit::NewTab);
            if hp {
                self.gfx.round(plus, lo.r_sm, th.hover);
            }
            self.gfx.icon("plus", plus.x + 5.0, plus.y + 5.0, 14.0, if hp { th.text } else { th.faint });
            self.hit(plus, Hit::NewTab);
            y += 28.0;
        }
        let mut rows: Vec<(Row<'_>, f32)> = rests.iter().map(|t| (Row::Tab(t), step)).collect();
        for (i, g) in groups.iter().enumerate() {
            if i == 0 || !compact {
                rows.push((Row::Header(g.head), if compact { 6.0 } else { 26.0 }));
            }
            for t in &g.tabs {
                rows.push((Row::Tab(t), step));
            }
        }
        let mut want = Vec::with_capacity(rows.len());
        let mut at = y - list.y + self.scroll;
        for (_, h) in &rows {
            want.push(at);
            at += h;
        }
        let rows_h = at - (y - list.y + self.scroll);
        // The highlight slides between tabs, in list space, following its row.
        if let Some(n) = rows.iter().position(|(r, _)| matches!(r, Row::Tab(t) if t.id == s.active))
            && !self.drag.as_ref().is_some_and(|d| d.moved && d.id == s.active)
        {
            let slid = self.motion.spring(Id::Named("active-y"), Prop::Y, want[n], 220);
            let hl = R::new(sb.x + pad, list.y - self.scroll + slid, sb.w - 2.0 * pad, row_h);
            self.gfx.round(hl, lo.round(row_h), th.pill.alpha(wf));
            if lo.style == theme::Style::Precise {
                // Precise marks the row with a violet edge, not a fill alone.
                self.gfx.round(R::new(hl.x, hl.y + 4.0, 2.0, hl.h - 8.0), 1.0, th.accent.alpha(wf));
            }
        }
        for ((row, h), want_y) in rows.iter().zip(want.iter().copied()) {
            match row {
                Row::Tab(t) => {
                    let oy = self.motion.spring(Id::Tab(t.id), Prop::Y, want_y, 220);
                    let r = R::new(sb.x + pad, list.y - self.scroll + oy, sb.w - 2.0 * pad, row_h);
                    let a = self.motion.enter(Id::Tab(t.id), Prop::Opacity, 0.0, 1.0, 160, Ease::OutCubic) * wf;
                    if r.bottom() >= list.y && r.y <= list.bottom() {
                        self.draw_tab(t, r, s, false, a);
                    }
                    self.seen_tabs.insert(t.id);
                }
                Row::Header(head) => {
                    let oy = self.motion.spring(Id::Group(head.id), Prop::Y, want_y, 220);
                    let r = R::new(sb.x + pad, list.y - self.scroll + oy, sb.w - 2.0 * pad, *h - 2.0);
                    if *h > 8.0 && r.bottom() >= list.y && r.y <= list.bottom() {
                        let n = groups.iter().find(|g| g.head.id == head.id).map_or(0, |g| g.tabs.len());
                        let note = s.agent_notes.iter().find(|(id, _)| *id == head.owner).map(|(_, n)| n.clone());
                        self.draw_agent_group(head, r, n, wf, note);
                    }
                }
            }
        }
        // The ghosts of just-closed tabs: their rows collapse and fade in place.
        let ghosts: Vec<(UiTab, f32)> = self.closing.iter().map(|c| (c.tab.clone(), c.y)).collect();
        for (tab, gy) in &ghosts {
            let o = self.motion.enter(Id::Ghost(tab.id), Prop::H, 1.0, 0.0, 190, Ease::OutCubic).clamp(0.0, 1.0);
            if o <= 0.03 {
                continue;
            }
            let r = R::new(sb.x + pad, *gy, sb.w - 2.0 * pad, row_h * o);
            if r.bottom() >= list.y && r.y <= list.bottom() {
                self.draw_tab_body(tab, r, s, false, false, o * wf);
            }
        }
        let nr = R::new(sb.x + pad, y + rows_h + 3.0, sb.w - 2.0 * pad, row_h);
        let hov = self.is_hover(&Hit::NewTab);
        let a = self.motion.hover(Id::Button("newtab"), if hov { 1.0 } else { 0.0 });
        if a > 0.01 {
            self.gfx.round(nr, lo.r_md, th.hover.alpha(a.min(1.0)));
        }
        let c = if hov { th.text } else { th.faint };
        if compact {
            self.gfx.icon("plus", nr.x + (nr.w - 15.0) / 2.0, nr.y + (nr.h - 15.0) / 2.0, 15.0, c);
        } else {
            self.gfx.icon("plus", nr.x + 9.0, nr.y + (nr.h - 15.0) / 2.0, 15.0, c);
            self.gfx.text("New tab", R::new(nr.x + 34.0, nr.y, nr.w - 40.0, nr.h), lo.font(13.0, 400), c);
        }
        if nr.y < list.bottom() {
            self.hit(nr, Hit::NewTab);
        }
        self.list_max = (y + rows_h + 3.0 + row_h + self.scroll - list.y - list.h).max(0.0);
        // Drag ghost on top.
        if let Some(d) = &self.drag
            && d.moved
            && let Some(t) = s.tabs.iter().find(|t| t.id == d.id)
        {
            let (dx, dy, pinned) = (d.dx, d.dy, d.pinned);
            let base = self.hits.iter().find(|(_, h)| *h == Hit::Tab(t.id)).map(|(r, _)| *r);
            if let Some(b) = base {
                let r = R::new(b.x + if pinned { dx } else { 0.0 }, b.y + dy, b.w, b.h);
                self.gfx.round(r, lo.r_md, th.press);
                self.draw_tab_body(t, r, s, pinned, false, 1.0);
            }
        }
        self.gfx.unclip();
    }

    /// The footer's height: the row of buttons, and above it, in the compact
    /// column, the memory number.
    fn footer_h(&self, compact: bool) -> f32 {
        if compact && self.look.show_memory { 54.0 } else { 40.0 }
    }

    /// The memory number in the compact column: above the buttons, centred, and
    /// short. It warms and glows as the browser's own memory grows.
    fn draw_compact_mem(&mut self, s: &UiState, sb: R, fy: f32) {
        if !self.look.show_memory {
            return;
        }
        let Some((bytes, _)) = s.mem else { return };
        let lo = self.look.clone();
        let th = lo.theme;
        let t = mem_short(bytes);
        // The figures are in the monospace face, so they do not jump about as they change.
        let f = lo.mono_at(10.5, 500);
        let tw = self.gfx.measure(&t, f);
        let r = R::new(sb.x + (sb.w - tw) / 2.0, fy - 17.0, tw, 15.0);
        // Under a gigabyte: plain faint text. Over two: red, with a glow that
        // breathes. In between: the colour warms and the glow grows.
        let hot = bytes >= MEM_HOT;
        let warm = bytes >= MEM_WARN;
        let t01 = if warm { (bytes - MEM_WARN) as f32 / (MEM_HOT - MEM_WARN) as f32 } else { 0.0 };
        let c = if hot {
            th.danger
        } else if warm {
            th.faint.mix(th.danger, t01.clamp(0.0, 1.0))
        } else {
            th.faint
        };
        // The glow behind the number. Over the hard line it breathes, a few
        // seconds a breath; with reduced motion it is simply there.
        let mut glow = if hot { 0.30 } else { 0.24 * t01.clamp(0.0, 1.0) };
        if hot && lo.motion.enabled {
            let phase = self.born.elapsed().as_secs_f32() / MEM_BREATH * std::f32::consts::TAU;
            glow *= 0.72 + 0.28 * phase.sin();
        }
        if glow > 0.01 {
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            self.gfx.bloom(cx, cy, (r.w.max(22.0) * 0.7).max(18.0), th.danger.alpha(glow));
        }
        let h = Hit::Mem;
        let hov = self.is_hover(&h);
        let a = self.motion.hover(Self::button_id(&h), if hov { 1.0 } else { 0.0 });
        if a > 0.01 {
            self.gfx.round(R::new(r.x - 5.0, r.y - 2.0, r.w + 10.0, r.h + 4.0), lo.r_sm, th.hover.alpha(a.min(1.0)));
        }
        self.gfx.text(&t, r, f.align(Align::Center), if hov { th.text } else { c });
        self.hit(r, h);
    }

    /// The footer: the memory readout and the agent chip (left), then the
    /// dashboard, Drive, downloads and settings buttons (right). The compact sidebar keeps the two that fit; the agent chip is a
    /// status, and shows wherever it fits.
    fn draw_footer(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        let fy = sb.bottom() - 38.0;
        let size = 28.0;
        let agent = s.agent_busy && !compact;
        // A hairline over the footer, across the sidebar's own width.
        if !compact {
            let inset = if lo.style == theme::Style::Glass { 12.0 } else { 8.0 };
            self.gfx.line(sb.x + inset, fy - 2.0, sb.right() - inset, fy - 2.0, th.line, 1.0);
        }
        // The buttons from the left: the dashboard, Drive, downloads, settings. What is
        // live on the right: the agent chip while one works, else the memory readout.
        let mut right_edge = sb.x + 8.0;
        if lo.footer_btns {
            if compact {
                let gear = R::new(sb.x + 28.0, fy + 3.0, 26.0, 26.0);
                self.button(gear, "sliders", Hit::Gear, true);
                let dl = R::new(sb.x + 2.0, fy + 3.0, 26.0, 26.0);
                self.draw_dl_button(dl, s);
                right_edge = gear.right();
            } else {
                let x0 = sb.x + 8.0;
                let db = R::new(x0, fy + 2.0, size, size);
                self.footer_page_button(db, s, "dashboard", "dashboard", Hit::DashFooter);
                let dr = R::new(db.right() + 2.0, db.y, size, size);
                self.footer_page_button(dr, s, "send", "drive", Hit::DriveFooter);
                let dl = R::new(dr.right() + 2.0, db.y, size, size);
                self.draw_dl_button(dl, s);
                let gear = R::new(dl.right() + 2.0, db.y, size, size);
                self.button(gear, "sliders", Hit::Gear, true);
                right_edge = gear.right();
            }
        }
        let room = (sb.right() - 8.0 - right_edge - 6.0).max(0.0);
        // The compact column has no room for the long figure beside the
        // buttons, but it has room for the short one above them.
        if compact {
            self.draw_compact_mem(s, sb, fy);
        }
        // What lives at the right end of the footer steps leftward: the agent chip or
        // the memory number first, then the now-playing card before them.
        let mut right = sb.right() - 8.0;
        if agent {
            // The long label when it fits, then just the verb, then only the dot: the
            // chip is the way to stop every agent, so it never leaves the footer.
            let f = lo.font(11.5, 500);
            let t = ["Agent working · Stop", "Stop"].into_iter().find(|t| self.gfx.measure(t, f) + 28.0 <= room).unwrap_or("");
            let tw = if t.is_empty() { 20.0 } else { self.gfx.measure(t, f) + 28.0 };
            let r = R::new(right - tw, fy + 5.0, tw, 24.0);
            let h = Hit::AgentStop;
            let a = self.motion.hover(Self::button_id(&h), if self.is_hover(&h) { 1.0 } else { 0.0 });
            self.gfx.round(r, 12.0, th.accent.alpha(0.14 + 0.16 * a.min(1.0)));
            self.gfx.circle(r.x + 10.0, r.y + 12.0, 3.0, th.accent);
            self.gfx.text(t, R::new(r.x + 18.0, r.y, r.w - 18.0, r.h), f, th.text);
            self.hit(r, h);
            right = r.x - 6.0;
        } else if !compact && lo.show_memory
            && let Some((bytes, _)) = s.mem
        {
            let mb = bytes as f64 / 1048576.0;
            let t = if mb >= 1024.0 { format!("{:.2} GB", mb / 1024.0) } else { format!("{} MB", mb.round()) };
            // The figures are in the monospace face, so they do not jump about as they change.
            let f = lo.mono_at(11.0, 500);
            let tw = self.gfx.measure(&t, f) + 10.0;
            // A number cut short reads wrong, so it shows whole or not at all. The
            // dashboard shows it too.
            if tw <= room {
                let r = R::new(right - tw, fy + 6.0, tw, 22.0);
                let h = Hit::Mem;
                let hov = self.is_hover(&h);
                let a = self.motion.hover(Self::button_id(&h), if hov { 1.0 } else { 0.0 });
                if a > 0.01 {
                    self.gfx.round(r, lo.r_sm, th.hover.alpha(a.min(1.0)));
                }
                self.gfx.text(&t, R::new(r.x + 5.0, r.y, r.w - 5.0, r.h), f, if hov { th.text } else { th.faint });
                self.hit(r, h);
                right = r.x - 6.0;
            }
        }
        // Now playing: the tab that has sound, with pause and next on the card.
        if !compact
            && let Some(id) = s.media
            && let Some(t) = s.tabs.iter().find(|t| t.id == id)
        {
            right = self.draw_now_playing(t, right_edge + 6.0, right, fy);
        }
        // Battery saver in force: a leaf, so it is plain why tabs sleep sooner. The card
        // above is worth more; the leaf takes what room is left.
        if s.battery && right - 20.0 > right_edge + 6.0 {
            self.gfx.icon("leaf", right - 17.0, fy + 10.0, 14.0, th.faint);
        }
        self.draw_footer_overlays(s);
    }

    /// The footer's now-playing card: the tab's title (a click switches to it), pause,
    /// next. Right-aligned ending at `right`, never past `left` (the footer's buttons end
    /// there); left out when not even a short title fits. Returns where the next thing may go.
    fn draw_now_playing(&mut self, t: &UiTab, left: f32, right: f32, fy: f32) -> f32 {
        let lo = self.look.clone();
        let th = lo.theme;
        let f = lo.font(11.5, 500);
        let title = if t.title.is_empty() { host(&t.url) } else { t.title.clone() };
        // The buttons take their room first; the title takes what is left, to a cap
        // so one long title never shoves the card off the footer.
        let btn = 20.0;
        let fixed = 6.0 + btn + 6.0 + 6.0 + btn + 6.0;
        let tw = self.gfx.measure(&title, f).min(170.0).min(right - left - fixed);
        if tw < 28.0 {
            return right;
        }
        let w = fixed + tw;
        let r = R::new(right - w, fy + 5.0, w, 24.0);
        self.gfx.round(r, 12.0, th.bg);
        self.gfx.round_stroke(r, 12.0, th.line, 1.0);
        // The card is only here while the tab has sound, so what it offers is pause.
        let pb = R::new(r.x + 4.0, r.y + 2.0, btn, btn);
        let ph = Hit::MediaPlay;
        if self.is_hover(&ph) {
            self.gfx.round(pb, 10.0, th.hover);
        }
        self.gfx.icon("pause", pb.x + 3.0, pb.y + 3.0, 14.0, th.dim);
        self.hit(pb, ph);
        let tr = R::new(pb.right() + 6.0, r.y, tw, r.h);
        let thh = Hit::MediaTitle(t.id);
        self.gfx.text(&title, tr, f, if self.is_hover(&thh) { th.text } else { th.dim });
        self.hit(tr, thh);
        let nb = R::new(tr.right() + 6.0, r.y + 2.0, btn, btn);
        let nh = Hit::MediaNext;
        if self.is_hover(&nh) {
            self.gfx.round(nb, 10.0, th.hover);
        }
        self.gfx.icon("next", nb.x + 3.0, nb.y + 3.0, 14.0, th.dim);
        self.hit(nb, nh);
        r.x - 6.0
    }

    /// The quick switcher sits over everything else in the sidebar; so does the lock's card.
    fn draw_footer_overlays(&mut self, s: &UiState) {
        if self.say.is_some() {
            self.draw_say();
        } else if self.search_open {
            self.draw_search(s);
        } else if self.site_open {
            self.draw_site_menu(s);
        } else if self.pal_open {
            self.draw_palette(s);
        }
    }

    /// The site popover under the lock: what the site may do, what it stores, and the ways out.
    fn draw_site_menu(&mut self, s: &UiState) {
        let th = self.theme;
        let Some(sb) = self.float_area() else { return };
        let Some(tab) = s.active_tab() else { return };
        let origin = crate::engine::origin_of(&tab.url);
        if !origin.starts_with("http") {
            self.site_open = false;
            return;
        }
        // The common kinds always; any other kind this site was answered about, after them.
        let answered = |kind: &str| s.rules.iter().find(|(o, k, _)| o == &origin && k == kind).map(|(_, _, a)| *a);
        let mut kinds: Vec<String> = crate::sites::KINDS.iter().take(crate::sites::COMMON).map(|k| k.key.to_string()).collect();
        for (o, k, _) in &s.rules {
            if o == &origin && !kinds.iter().any(|x| x == k) {
                kinds.push(k.clone());
            }
        }
        let lo = self.look.clone();
        let h = 196.0 + kinds.len() as f32 * 56.0;
        let card = if self.look.kind.top_toolbar() {
            let o = self.omni_rect();
            R::new(o.x, o.bottom() + 6.0, o.w.min(320.0), h)
        } else {
            let (x, w) = float_col(sb, self.cards_right());
            R::new(x, 86.0 + self.ws_h(s), w, h)
        };
        self.float_card(card, 1.0);
        // Who: the site's icon and name, and what it keeps.
        let site = host(&tab.url);
        pages::site_icon(self, R::new(card.x + 14.0, card.y + 16.0, 18.0, 18.0), &site);
        self.gfx.text(&site, R::new(card.x + 42.0, card.y + 12.0, card.w - 56.0, 20.0), lo.font(13.0, 600), th.text);
        let note = match s.site_cookies {
            Some(1) => "1 cookie stored".to_string(),
            Some(n) => format!("{n} cookies stored"),
            None => "Checking what it stores…".to_string(),
        };
        self.gfx.text(&note, R::new(card.x + 42.0, card.y + 32.0, card.w - 56.0, 18.0), lo.font(11.5, 400), th.faint);
        let mut y = card.y + 60.0;
        self.gfx.line(card.x + 1.0, y, card.right() - 1.0, y, th.line, 1.0);
        y += 12.0;
        // The site's own switch: out of the list means ads and trackers are blocked here.
        // The words are long for a card this wide, so they wrap beside the switch.
        let row = R::new(card.x + 12.0, y, card.w - 24.0, 40.0);
        self.gfx.text_wrap(
            "Block ads and trackers on this site",
            R::new(row.x + 4.0, row.y + 3.0, row.w - 54.0, 34.0),
            lo.font(12.5, 450),
            th.faint,
        );
        let on = !crate::blocker::site_off(&s.settings.adblock_off_sites, &site);
        self.switch(R::new(row.right() - 38.0, row.y + (row.h - 22.0) / 2.0, 38.0, 22.0), on, "siteadblock", 1.0, Some(Hit::SiteAdblock));
        y += 46.0;
        for kind in &kinds {
            self.label(&crate::sites::label(kind), R::new(card.x + 16.0, y, card.w - 32.0, 16.0), th.faint);
            y += 20.0;
            let cur = answered(kind);
            let cells: Vec<(&str, Hit)> = [("Allow", Some(true)), ("Block", Some(false)), ("Ask", None)].iter().map(|(l, _)| (*l, Hit::SitePerm(kind.to_string(), l))).collect();
            let at = [Some(true), Some(false), None].iter().position(|v| *v == cur);
            self.seg_cells(R::new(card.x + 12.0, y, card.w - 24.0, 30.0), &cells, at);
            y += 36.0;
        }
        // The two ways out of the permissions grid, stacked: the card is sidebar-wide.
        y += 4.0;
        for (hit, label) in [(Hit::SiteClear, "Clear data for this site"), (Hit::SiteSettings, "Site settings…")] {
            self.pill_button(R::new(card.x + 12.0, y, card.w - 24.0, 30.0), label, hit, false, 1.0);
            y += 36.0;
        }
    }

    /// The origin (scheme://host) of the active page, when it is a site at all.
    fn active_origin(&self) -> Option<String> {
        let s = self.state.as_ref()?;
        let t = s.active_tab()?;
        t.url.starts_with("http").then(|| crate::engine::origin_of(&t.url))
    }

    /// The message card: who it is for, the text, the kind (steer now, queue, new task), what
    /// that kind means, and Send.
    fn draw_say(&mut self) {
        let lo = self.look.clone();
        let th = lo.theme;
        let Some(sb) = self.float_area() else { return };
        let Some((session, name, mode)) = self.say.as_ref().map(|s| (s.session.clone(), s.name.clone(), s.mode)) else { return };
        let card = say_card(sb, self.cards_right());
        self.float_card(card, 1.0);
        self.hit(card, Hit::SayCard);
        // Who it is for.
        self.gfx.circle(card.x + 18.0, card.y + 22.0, 4.0, agent_color(&session));
        self.gfx.text(&format!("Message to {name}"), R::new(card.x + 30.0, card.y + 10.0, card.w - 66.0, 24.0), lo.font(12.5, 600), th.text);
        let x = R::new(card.right() - 34.0, card.y + 8.0, 26.0, 26.0);
        if self.is_hover(&Hit::SayClose) {
            self.gfx.round(x, lo.round(x.h), th.hover);
        }
        self.gfx.icon("close", x.x + 7.0, x.y + 7.0, 12.0, th.dim);
        self.hit(x, Hit::SayClose);
        // The text (a real edit window sits over this), in a field that says it has the keys.
        let field = R::new(card.x + 12.0, card.y + 42.0, card.w - 24.0, 38.0);
        self.gfx.round(field, lo.r_md + 2.0, th.bg);
        self.gfx.round_stroke(field, lo.r_md + 2.0, th.text.alpha(0.85), 1.5);
        if wide_text(self.say_edit).is_empty() {
            self.gfx.text("Tell it what to do", R::new(field.x + 10.0, field.y, field.w - 20.0, field.h), lo.font(12.5, 400), th.faint);
        }
        // The kind, as one segmented control.
        let cells: Vec<(&str, Hit)> = SAY_MODES.iter().enumerate().map(|(n, (label, _, _))| (*label, Hit::SayMode(n))).collect();
        self.seg_cells(R::new(card.x + 12.0, card.y + 88.0, card.w - 24.0, 30.0), &cells, Some(mode));
        // What that kind does, and Send.
        let send_w = 64.0;
        let hint = R::new(card.x + 14.0, card.y + 126.0, card.w - 28.0 - send_w - 8.0, 32.0);
        self.gfx.text_wrap(SAY_MODES[mode.min(2)].2, hint, lo.font(11.0, 400), th.faint);
        self.pill_button(R::new(card.right() - 12.0 - send_w, card.y + 128.0, send_w, 28.0), "Send", Hit::SaySend, true, 1.0);
    }

    /// The tab search card: a filter box on top, then the matching tabs of every workspace.
    fn draw_search(&mut self, s: &UiState) {
        let lo = self.look.clone();
        let th = lo.theme;
        let Some(sb) = self.float_area() else { return };
        let q = wide_text(self.search_edit).trim().to_lowercase();
        self.search_rows = switch_rows(s, &q);
        let shown = self.search_rows.len().min(SEARCH_ROWS);
        let card = search_card(sb, self.cards_right(), shown, self.search_rows.len() > shown);
        self.search_sel = self.search_sel.min(self.search_rows.len().saturating_sub(1));
        self.float_card(card, 1.0);
        // The card is not "outside itself"; rows registered after win over it.
        self.hit(card, Hit::SearchCard);
        // The filter field (a real edit window is placed over this).
        let field = R::new(card.x + 8.0, card.y + 8.0, card.w - 16.0, 32.0);
        self.gfx.round(field, lo.round(field.h), th.bg);
        self.gfx.round_stroke(field, lo.round(field.h), th.line, 1.0);
        self.gfx.icon("search", field.x + 11.0, field.y + 9.0, 14.0, th.faint);
        if q.is_empty() {
            self.gfx.text("Search open tabs", R::new(field.x + 30.0, field.y, field.w - 40.0, field.h), lo.font(12.5, 400), th.faint);
        }
        // Hits.
        let mut y = card.y + 48.0;
        if self.search_rows.is_empty() {
            self.gfx.text("No open tab matches", R::new(card.x, y + 8.0, card.w, 26.0), lo.font(12.5, 400).align(Align::Center), th.faint);
        }
        for n in 0..shown {
            // Right: where the tab lives, or that it is private. Left: its icon and what it says.
            let (label, tag, url) = {
                let row = &self.search_rows[n];
                let tag = if row.private { "Private".to_string() } else if row.other.is_some() { row.ws_name.clone() } else { String::new() };
                (row.label(), tag, row.url.clone())
            };
            let r = R::new(card.x + 6.0, y, card.w - 12.0, 32.0);
            let h = Hit::TabSearch(n);
            if n == self.search_sel {
                self.gfx.round(r, lo.round(r.h), th.hover);
            } else if self.is_hover(&h) {
                self.gfx.round(r, lo.round(r.h), th.hover.alpha(0.6));
            }
            let site = host(&url);
            pages::site_icon(self, R::new(r.x + 10.0, r.y + 8.0, 16.0, 16.0), &site);
            let tx = if tag.is_empty() { r.right() - 8.0 } else { self.tag(&tag, r.right() - 8.0, r.y + r.h / 2.0, th.faint) - 6.0 };
            self.gfx.text(&label, R::new(r.x + 34.0, r.y, tx - r.x - 34.0, r.h), lo.font(12.5, 450), th.text);
            self.hit(r, h);
            y += 32.0;
        }
        if self.search_rows.len() > shown {
            self.gfx.text(
                &format!("{} more — type to narrow", self.search_rows.len() - shown),
                R::new(card.x, y, card.w, 18.0),
                lo.font(11.0, 400).align(Align::Center),
                th.faint,
            );
        }
    }

    /// The command palette over the sidebar: one field to type into, the rows in groups
    /// under it. The field itself is a real edit window, placed over the drawn box.
    fn draw_palette(&mut self, s: &UiState) {
        let lo = self.look.clone();
        let th = lo.theme;
        let Some(sb) = self.float_area() else { return };
        let q = wide_text(self.palette_edit).trim().to_lowercase();
        let (rows, groups, total) = pal_view(s, &q);
        // The rows the frame lists, so a click knows what it lands on.
        self.pal_rows = rows;
        let shown = self.pal_rows.len();
        let card = pal_card(sb, self.cards_right(), shown, groups, total > shown);
        self.pal_sel = self.pal_sel.min(shown.saturating_sub(1));
        self.float_card(card, 1.0);
        // The card is not "outside itself"; rows registered after win over it.
        self.hit(card, Hit::PaletteCard);
        // The filter field (a real edit window is placed over this).
        let field = R::new(card.x + 8.0, card.y + 8.0, card.w - 16.0, 32.0);
        self.gfx.round(field, lo.round(field.h), th.bg);
        self.gfx.round_stroke(field, lo.round(field.h), th.line, 1.0);
        self.gfx.icon("palette", field.x + 11.0, field.y + 9.0, 14.0, th.faint);
        if q.is_empty() {
            self.gfx.text("Type a command", R::new(field.x + 30.0, field.y, field.w - 40.0, field.h), lo.font(12.5, 400), th.faint);
        }
        // Hits.
        let mut y = card.y + 48.0;
        if shown == 0 {
            self.gfx.text("Nothing matches", R::new(card.x, y + 8.0, card.w, 26.0), lo.font(12.5, 400).align(Align::Center), th.faint);
        }
        for n in 0..shown {
            // Right: the group it belongs to. Left: its icon and what it says.
            let (group, label, detail, is_set, tab_url) = {
                let row = &self.pal_rows[n];
                let is_set = matches!(row.run, PalRun::Set(_));
                let tab_url = match &row.run {
                    PalRun::Switch { id, other } if other.is_none() => s.tabs.iter().find(|t| t.id == *id).map_or(String::new(), |t| t.url.clone()),
                    _ => String::new(),
                };
                (row.group, row.label.clone(), row.detail.clone(), is_set, tab_url)
            };
            if let Some(g) = group {
                self.label(g, R::new(card.x + 12.0, y, card.w - 24.0, 16.0), th.faint);
                y += 18.0;
            }
            let r = R::new(card.x + 6.0, y, card.w - 12.0, 30.0);
            let h = Hit::Pal(n);
            if n == self.pal_sel {
                self.gfx.round(r, lo.round(r.h), th.hover);
            } else if self.is_hover(&h) {
                self.gfx.round(r, lo.round(r.h), th.hover.alpha(0.6));
            }
            let tx = if detail.is_empty() { r.right() - 8.0 } else { self.tag(&detail, r.right() - 8.0, r.y + r.h / 2.0, th.faint) - 6.0 };
            if is_set {
                // A setting row: its section's icon, the one the Settings menu shows.
                self.gfx.icon(pages::section_icon(&detail), r.x + 11.0, r.y + 8.0, 14.0, th.faint);
            } else if !tab_url.is_empty() {
                // A tab row: the same site icon the tab search gives its rows.
                pages::site_icon(self, R::new(r.x + 10.0, r.y + 7.0, 16.0, 16.0), &host(&tab_url));
            }
            self.gfx.text(&label, R::new(r.x + 34.0, r.y, tx - r.x - 34.0, r.h), lo.font(12.5, 450), th.text);
            self.hit(r, h);
            y += 30.0;
        }
        if total > shown {
            self.gfx.text(
                &format!("{} more — type to narrow", total - shown),
                R::new(card.x, y, card.w, 18.0),
                lo.font(11.0, 400).align(Align::Center),
                th.faint,
            );
        }
    }

    /// The header of one agent's block: its colour dot, its name, how many tabs it has,
    /// and Stop, which ends that agent (its tabs close and its profile data is deleted).
    fn draw_agent_group(&mut self, head: &UiTab, r: R, tabs: usize, alpha: f32, note: Option<String>) {
        let lo = self.look.clone();
        let th = lo.theme;
        let dot = agent_color(&head.owner);
        self.gfx.circle(r.x + 11.0, r.y + r.h / 2.0, 3.5, dot.alpha(alpha));
        let name = if head.owner_name.trim().is_empty() { "agent" } else { head.owner_name.trim() };
        // What became of the person's last message to it, else how many tabs it has.
        let label = match note {
            Some(n) => format!("{name} · {n}"),
            None if tabs > 1 => format!("{name} · {tabs} tabs"),
            None => name.to_string(),
        };
        let stop = R::new(r.right() - 26.0, r.y + (r.h - 22.0) / 2.0, 22.0, 22.0);
        // Talk to it: steer, queue, or a new task.
        let say = R::new(stop.x - 24.0, stop.y, 22.0, 22.0);
        let say_hit = Hit::AgentMessage(head.owner.clone(), name.to_string());
        let sa = self.motion.hover(Self::button_id(&say_hit), if self.is_hover(&say_hit) { 1.0 } else { 0.0 });
        if sa > 0.01 {
            self.gfx.round(say, lo.r_sm, th.hover.alpha(sa.min(1.0) * alpha));
        }
        self.gfx.icon("send", say.x + (say.w - 13.0) / 2.0, say.y + (say.h - 13.0) / 2.0, 13.0, (if sa > 0.5 { th.text } else { th.dim }).alpha(alpha));
        self.hit(say, say_hit);
        let stop_x = say.x;
        let hit = Hit::AgentStopSession(head.owner.clone());
        let a = self.motion.hover(Self::button_id(&hit), if self.is_hover(&hit) { 1.0 } else { 0.0 });
        if a > 0.01 {
            self.gfx.round(stop, lo.r_sm, th.accent.alpha(0.16 * a.min(1.0) * alpha));
        }
        self.label(&label, R::new(r.x + 22.0, r.y, stop_x - r.x - 26.0, r.h), th.faint.alpha(alpha));
        // Stop: a square in the live colour, the one thing here that means "now".
        self.gfx.round_stroke(R::new(stop.x + 6.0, stop.y + 6.0, 10.0, 10.0), 2.5, th.accent.alpha(alpha), 1.6);
        self.hit(stop, hit);
    }

    /// One of the footer's page buttons: a square that opens a panel, lit while
    /// that panel is the open one. Bookmarks, the privacy report and the
    /// dashboard share it.
    fn footer_page_button(&mut self, r: R, s: &UiState, icon: &str, panel: &str, hit: Hit) {
        let lo = self.look.clone();
        let th = lo.theme;
        // Drive is also lit while its run is going, wherever the person is.
        let on = s.panel == panel || (panel == "drive" && s.drive.as_ref().is_some_and(|d| d.running));
        let hov = self.is_hover(&hit);
        let a = self.motion.hover(Self::button_id(&hit), if hov || on { 1.0 } else { 0.0 });
        if a > 0.01 {
            self.gfx.round(r, lo.r_sm + 1.0, if on { th.press } else { th.hover }.alpha(a.min(1.0)));
        }
        // Drive's run going is live: its icon takes the accent while it runs.
        let live = panel == "drive" && s.drive.as_ref().is_some_and(|d| d.running);
        self.gfx.icon(icon, r.x + 6.0, r.y + 6.0, 16.0, if live { th.accent } else if on || hov { th.text } else { th.dim });
        self.hit(r, hit);
    }

    /// The downloads button: an arrow with a progress ring while something runs,
    /// and a count badge or an unseen dot. Shared by the sidebar footer and the
    /// top toolbar.
    fn draw_dl_button(&mut self, dl: R, s: &UiState) {
        let th = self.theme;
        let h = Hit::DlFooter;
        let a = self.motion.hover(Self::button_id(&h), if self.is_hover(&h) { 1.0 } else { 0.0 });
        if a > 0.01 {
            self.gfx.round(dl, 7.0, th.hover.alpha(a.min(1.0)));
        }
        let live: Vec<&UiDl> = s.downloads.iter().filter(|d| d.state == "progress").collect();
        if !live.is_empty() {
            let (got, total): (i64, i64) = live.iter().fold((0, 0), |a, d| (a.0 + d.received, a.1 + d.total.max(0)));
            let p = if total > 0 { got as f32 / total as f32 } else { 0.0 };
            self.gfx.ring(dl.x + 14.0, dl.y + 14.0, 10.0, 10.0, th.line, 1.5);
            if p > 0.0 {
                // The ring eases toward the real progress, so a step is a glide.
                let p = self.motion.tween(Id::Named("dl-ring"), Prop::Progress, p.min(1.0), 220, Ease::OutCubic);
                self.gfx.arc(dl.x + 14.0, dl.y + 14.0, 10.0, -90.0, 359.0 * p, th.accent, 2.0);
            }
        }
        self.gfx.icon("download", dl.x + (dl.w - 12.0) / 2.0, dl.y + (dl.h - 12.0) / 2.0, 12.0, th.dim);
        let busy_n = s.downloads.iter().filter(|d| matches!(d.state, "progress" | "paused" | "seeding")).count();
        if busy_n > 0 {
            let t = if busy_n > 9 { "9+".to_string() } else { busy_n.to_string() };
            let d = 13.0;
            let bx = dl.x + 14.0 + 9.0 - d / 2.0;
            let by = dl.y + 14.0 - 9.0 - d / 2.0;
            self.gfx.circle(bx + d / 2.0, by + d / 2.0, d / 2.0, th.accent);
            self.gfx.text(&t, R::new(bx, by + 0.5, d, d), self.look.font(9.0, 700).align(Align::Center), th.bg);
        } else if s.dl_unseen {
            self.gfx.circle(dl.right() - 4.0, dl.y - 1.0, 3.0, th.accent);
        }
        self.hit(dl, h);
    }

    /// The handoff and permission cards above the footer.
    fn draw_cards(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        // Above the footer in a sidebar; with none (horizontal), above the window's edge.
        let _ = compact;
        let foot = if self.sidebar().is_some() { 44.0 } else { 12.0 };
        // An agent asking the person to take over: the live dot, what it needs, one button.
        if let Some((who, msg)) = s.handoff.as_ref().filter(|_| s.prompt.is_none()) {
            // As tall as what it says, up to five lines.
            let (x, w) = float_col(sb, self.cards_right());
            let lines = pages::wrap_lines(self, msg, lo.font(12.5, 400), w - 28.0).len().clamp(1, 5) as f32;
            let ch = 92.0 + 18.0 * lines;
            let card = R::new(x, sb.bottom() - foot - ch, w, ch);
            let o = self.motion.enter(Id::Chip("handoff"), Prop::Opacity, 0.0, 1.0, 160, Ease::OutCubic);
            let card = R::new(card.x, card.y + (1.0 - o) * 9.0, card.w, card.h);
            self.float_card(card, o);
            self.gfx.circle(card.x + 18.0, card.y + 21.0, 3.5, th.accent.alpha(o));
            self.gfx.text(&format!("{who} needs you"), R::new(card.x + 30.0, card.y + 10.0, card.w - 42.0, 22.0), lo.font(12.5, 600), th.text.alpha(o));
            self.gfx.text_wrap(msg, R::new(card.x + 14.0, card.y + 36.0, card.w - 28.0, 18.0 * lines + 4.0), lo.font(12.5, 400), th.dim.alpha(o));
            let done = R::new(card.x + 12.0, card.bottom() - 42.0, card.w - 24.0, 30.0);
            self.pill_button(done, "Done, hand it back", Hit::HandoffDone, true, o);
        }
        // A site asking for something: a card above the footer.
        if let Some((pid, origin, kind)) = &s.prompt {
            let (x, w) = float_col(sb, self.cards_right());
            let card = R::new(x, sb.bottom() - foot - 112.0, w, 112.0);
            let o = self.motion.enter(Id::Chip("prompt"), Prop::Opacity, 0.0, 1.0, 160, Ease::OutCubic);
            let card = R::new(card.x, card.y + (1.0 - o) * 9.0, card.w, card.h);
            self.float_card(card, o);
            let site = host(origin);
            pages::site_icon(self, R::new(card.x + 14.0, card.y + 14.0, 18.0, 18.0), &site);
            self.gfx.text(&site, R::new(card.x + 40.0, card.y + 12.0, card.w - 52.0, 22.0), lo.font(12.5, 600), th.text.alpha(o));
            self.gfx.text(&ask_text(kind), R::new(card.x + 14.0, card.y + 38.0, card.w - 28.0, 20.0), lo.font(12.5, 400), th.dim.alpha(o));
            let bw = (card.w - 32.0) / 2.0;
            let block = R::new(card.x + 12.0, card.bottom() - 42.0, bw, 30.0);
            let allow = R::new(block.right() + 8.0, block.y, bw, 30.0);
            self.pill_button(block, "Block", Hit::PermBlock(*pid), false, o);
            self.pill_button(allow, "Allow", Hit::PermAllow(*pid), true, o);
        }
    }

    /// Address-bar suggestions, floating over the tab list.
    fn draw_suggestions(&mut self, s: &UiState) {
        let lo = self.look.clone();
        let th = lo.theme;
        if !(self.editing && !self.sugg.is_empty()) {
            // Reset the entrance so the next dropdown animates again.
            self.motion.tween(Id::Named("sugg"), Prop::Opacity, 0.0, 40, Ease::OutCubic);
            return;
        }
        let o = self.omni_rect();
        let a = self.motion.enter(Id::Named("sugg"), Prop::Opacity, 0.0, 1.0, 150, Ease::OutCubic);
        let rise = self.motion.enter(Id::Named("sugg"), Prop::Y, -6.0, 0.0, 150, Ease::OutCubic);
        let row_h = |sg: &Sugg| if sg.detail.is_empty() { 34.0 } else { 50.0 };
        let h = self.sugg.iter().map(row_h).sum::<f32>() + 12.0;
        let box_ = R::new(o.x, o.bottom() + 6.0 - rise, o.w, h);
        self.float_card(box_, a);
        let mut y = box_.y + 6.0;
        for i in 0..self.sugg.len() {
            let rh = row_h(&self.sugg[i]);
            let r = R::new(box_.x + 6.0, y, box_.w - 12.0, rh);
            y += rh;
            if i == self.sel {
                self.gfx.round(r, lo.round(34.0).min(r.h / 2.0), th.hover.alpha(a));
            }
            let sg = self.sugg[i].clone();
            let (icon, hint) = match sg.kind {
                SuggKind::Go(_) => ("globe", "Open"),
                SuggKind::Search(_) => ("search", "Search"),
                SuggKind::Tab(_) | SuggKind::TabOther(..) => ("tab", "Tab"),
                SuggKind::Url(_, true) => ("star", "Bookmark"),
                SuggKind::Url(_, false) if !sg.detail.is_empty() => ("clock", "In page"),
                SuggKind::Url(_, false) => ("clock", "History"),
            };
            self.gfx.icon(icon, r.x + 10.0, r.y + 9.0, 16.0, th.faint.alpha(a));
            // What kind of hit, in the quiet spaced capitals every label uses.
            let ht = hint.to_uppercase();
            let hw = self.gfx.measure_tracked(&ht, lo.label, lo.label_track) + 12.0;
            self.gfx.text(&sg.label, R::new(r.x + 36.0, r.y, r.w - 36.0 - hw - 8.0, 34.0), lo.font(13.0, 400), th.text.alpha(a));
            self.gfx.text_tracked(&ht, R::new(r.right() - hw, r.y + 10.0, hw - 6.0, 16.0), lo.label, th.faint.alpha(a), lo.label_track);
            if !sg.detail.is_empty() {
                self.gfx.text(&sg.detail, R::new(r.x + 36.0, r.y + 27.0, r.w - 44.0, 18.0), lo.font(11.5, 400), th.dim.alpha(a));
            }
            self.hit(r, Hit::Sugg(i));
        }
        let _ = s;
    }

    // ------------------------------------------------------------- one tab

    fn draw_tab(&mut self, t: &UiTab, r: R, s: &UiState, pinned: bool, alpha: f32) {
        let lo = self.look.clone();
        let th = lo.theme;
        if self.drag.as_ref().is_some_and(|d| d.moved && d.id == t.id) {
            self.hit(r, Hit::Tab(t.id));
            return;
        }
        let active = t.id == s.active;
        let h_ = Hit::Tab(t.id);
        let hov = self.is_hover(&h_) || self.is_hover(&Hit::TabX(t.id)) || self.is_hover(&Hit::TabSnd(t.id));
        let h = self.motion.hover(Id::Tab(t.id), if hov { 1.0 } else { 0.0 }).min(1.0);
        let pressed = self.press.as_ref() == Some(&h_);
        let rad = lo.round(r.h);
        if pinned {
            let lit = th.raise.mix(th.hover, h * 0.6);
            // A pin with no page loaded stays in the row, but quieter: it is a place, not a page.
            let c = if t.status == "off" && !active { lit.alpha(0.55 + 0.45 * h) } else { lit };
            self.gfx.round(r, lo.r_md, c.alpha(alpha));
            if active {
                self.gfx.round_stroke(r, lo.r_md, th.text.alpha(alpha), 1.5);
            } else {
                self.gfx.round_stroke(r, lo.r_md, th.line.alpha(alpha), 1.0);
            }
        } else if !active && h > 0.01 {
            // Held down: one step further than hovering, so a click feels received.
            let c = if pressed { th.press } else { th.hover };
            self.gfx.round(r, rad, c.alpha(h * alpha));
        } else if active && alpha < 1.0 {
            // The row the person just left behind, fading with the last frame.
            self.gfx.round(r, rad, th.pill.alpha(alpha));
        }
        self.hit(r, Hit::Tab(t.id));
        let show_x = !pinned && match lo.close_btn {
            style::CloseBtn::Always => true,
            style::CloseBtn::Hover => hov || active,
            style::CloseBtn::Off => false,
        };
        self.draw_tab_body(t, r, s, pinned, show_x, alpha);
    }

    fn draw_tab_body(&mut self, t: &UiTab, r: R, s: &UiState, pinned: bool, show_x: bool, alpha: f32) {
        let lo = self.look.clone();
        let th = lo.theme;
        let compact = s.compact();
        let active = t.id == s.active;
        // Asleep, unloaded or dead: drawn dim. None of them are live pages.
        let asleep = matches!(t.status, "asleep" | "off" | "crashed");
        let dim = if asleep { 0.5 } else { 1.0 };
        let (fx, fy) = if pinned || compact { (r.x + (r.w - 16.0) / 2.0, r.y + (r.h - 16.0) / 2.0) } else { (r.x + 9.0, r.y + (r.h - 16.0) / 2.0) };
        let fav = R::new(fx, fy, 16.0, 16.0);
        if t.loading {
            let phase = (self.born.elapsed().as_secs_f32() / 0.8).fract();
            self.gfx.spinner(fx + 8.0, fy + 8.0, 6.5, phase, th.line, th.accent.alpha(alpha));
        } else {
            let png = lo.show_favicons.then(|| self.favs.get(&t.id).cloned()).flatten();
            let bmp = png.filter(|p| !p.is_empty()).and_then(|p| self.gfx.bitmap(t.id, &p));
            match bmp {
                Some(b) => self.gfx.draw_bitmap(&b, fav, alpha * dim),
                None => {
                    let on_pill = active && !pinned;
                    if t.status == "new" {
                        let ink = if on_pill { th.pill_ink } else { th.dim };
                        self.gfx.round_stroke(fav, 5.0, ink.alpha(0.5 * alpha), 1.0);
                        self.gfx.icon("plus", fav.x + 3.0, fav.y + 3.0, 10.0, ink.alpha(alpha));
                    } else {
                        let letter = host(&t.url).chars().next().unwrap_or('·').to_uppercase().to_string();
                        self.gfx.round(fav, 5.0, th.raise.alpha(alpha * dim));
                        self.gfx.round_stroke(fav, 5.0, th.line.alpha(alpha * dim), 1.0);
                        self.gfx.text(&letter, fav, lo.font(9.5, 650).align(Align::Center), th.dim.alpha(alpha * dim));
                    }
                }
            }
        }
        if t.agent && (pinned || compact) {
            self.gfx.circle(r.right() - 5.0, r.y + 5.0, 3.0, th.accent.alpha(alpha));
        }
        if t.agent_ok && (pinned || compact) {
            // The person's own tab, lent to agents: the AI dot, fainter.
            self.gfx.circle(r.right() - 5.0, r.y + 5.0, 3.0, th.accent.alpha(0.5 * alpha));
        }
        if t.private && (pinned || compact) {
            // A dot, like the AI one, in a colour of its own.
            self.gfx.circle(r.right() - 5.0, r.y + 5.0, 3.0, style::private_mark(th.dark).alpha(alpha));
        }
        if (t.audio || t.muted || t.call) && pinned {
            let b = R::new(r.right() - 15.0, r.y + 1.0, 14.0, 14.0);
            self.gfx.round(b, 4.0, th.bg);
            self.gfx.icon(sound_icon(t), b.x + 2.0, b.y + 2.0, 10.0, th.dim);
            self.hit(b, Hit::TabSnd(t.id));
        }
        if pinned || compact {
            return;
        }
        // Right to left, each item takes its own room: close, sound, AI badge, then the title.
        let mut right = r.right() - 6.0;
        if show_x {
            let x = R::new(right - 22.0, r.y + (r.h - 22.0) / 2.0, 22.0, 22.0);
            if self.is_hover(&Hit::TabX(t.id)) {
                self.gfx.round(x, lo.r_sm, if active { th.pill_ink.alpha(0.14 * alpha) } else { th.press.alpha(alpha) });
            }
            let ink = if active { th.pill_ink } else if self.is_hover(&Hit::TabX(t.id)) { th.text } else { th.dim };
            self.gfx.icon("close", x.x + 4.0, x.y + 4.0, 14.0, ink.alpha(alpha * if active { 0.75 } else { 1.0 }));
            self.hit(x, Hit::TabX(t.id));
            right = x.x - 2.0;
        }
        if t.audio || t.muted || t.call {
            let b = R::new(right - 22.0, r.y + (r.h - 22.0) / 2.0, 22.0, 22.0);
            if self.is_hover(&Hit::TabSnd(t.id)) {
                self.gfx.round(b, lo.r_sm, if active { th.pill_ink.alpha(0.14 * alpha) } else { th.press.alpha(alpha) });
            }
            let ink = if active { th.pill_ink.alpha(0.75) } else { th.faint };
            self.gfx.icon(sound_icon(t), b.x + 4.0, b.y + 4.0, 14.0, ink.alpha(alpha));
            self.hit(b, Hit::TabSnd(t.id));
            right = b.x - 2.0;
        }
        if t.audio && !t.muted {
            // A tab playing video gets its own way out: the biggest video, in a
            // picture-in-picture window, asked for the same way the page would.
            let b = R::new(right - 22.0, r.y + (r.h - 22.0) / 2.0, 22.0, 22.0);
            if self.is_hover(&Hit::TabPip(t.id)) {
                self.gfx.round(b, lo.r_sm, if active { th.pill_ink.alpha(0.14 * alpha) } else { th.press.alpha(alpha) });
            }
            let ink = if active { th.pill_ink.alpha(0.75) } else { th.faint };
            self.gfx.icon("pip", b.x + 4.0, b.y + 4.0, 14.0, ink.alpha(alpha));
            self.hit(b, Hit::TabPip(t.id));
            right = b.x - 2.0;
        }
        let badge_y = r.y + (r.h - 16.0) / 2.0;
        // A tag: small spaced capitals in a hairline pill, quiet on any row.
        let tag_ink = if active { th.pill_ink.alpha(0.7) } else { th.faint };
        let tag_line = if active { th.pill_ink.alpha(0.3) } else { th.line };
        let tag = |me: &mut Self, text: &str, right: f32| -> f32 {
            let f = lo.mono_at(9.0, 600);
            let w = me.gfx.measure_tracked(text, f, 0.9) + 12.0;
            let b = R::new(right - w, badge_y, w, 16.0);
            me.gfx.round_stroke(b, 8.0, tag_line.alpha(alpha), 1.0);
            me.gfx.text_tracked(text, R::new(b.x + 6.0, b.y, b.w, b.h), f, tag_ink.alpha(alpha), 0.9);
            b.x - 4.0
        };
        if t.agent {
            right = tag(self, "AI", right);
        }
        if t.agent_ok {
            // Lent to agents: the same tag, the person's tab open to them.
            right = tag(self, "AI OK", right);
        }
        if t.private {
            right = tag(self, "PRIVATE", right);
        }
        if t.split {
            // Both halves wear it, so the pair reads as one thing in the list.
            let b = R::new(right - 18.0, badge_y, 16.0, 16.0);
            self.gfx.icon("split", b.x, b.y, 16.0, if active { th.dim.alpha(alpha) } else { th.faint.alpha(alpha) });
            right = b.x - 4.0;
        }
        if t.translated || t.translating {
            // In another language, or on its way there: a globe, quiet until it is on.
            let b = R::new(right - 18.0, badge_y, 16.0, 16.0);
            self.gfx.icon("globe", b.x, b.y, 16.0, if t.translating { th.faint.alpha(alpha) } else { th.accent.alpha(alpha) });
            right = b.x - 4.0;
        }
        if t.servo {
            // A quiet mark: this page is the other engine's.
            let b = R::new(right - 22.0, badge_y, 20.0, 14.0);
            self.gfx.round(b, 4.0, th.press.alpha(alpha));
            self.gfx.text("SV", b, lo.font(9.0, 650).align(Align::Center), th.dim.alpha(alpha));
            right = b.x - 4.0;
        }
        let title = if !t.title.is_empty() {
            t.title.clone()
        } else if t.status == "new" {
            "New tab".into()
        } else {
            let h = host(&t.url);
            if h.is_empty() { "Untitled".into() } else { h }
        };
        let c = if active { th.pill_ink } else if self.is_hover(&Hit::Tab(t.id)) { th.text } else { th.dim };
        let f = if active { lo.font(13.0, 500) } else { lo.font(13.0, 400) };
        self.gfx.text(&title, R::new(r.x + 34.0, r.y, (right - r.x - 34.0).max(0.0), r.h), f, c.alpha(alpha * dim));
    }

    // ------------------------------------------------------------ top rows

    /// The horizontal layout: the tab strip, the toolbar, the bookmarks bar.
    fn draw_top(&mut self, s: &UiState) {
        let Some((strip, tool, bm)) = self.top_rows() else { return };
        let lo = self.look.clone();
        let th = lo.theme;
        self.gfx.fill(R::new(0.0, 0.0, tool.w, tool.bottom() + bm.map_or(0.0, |b| b.h)), th.side);
        if let Some(strip) = strip {
            self.draw_strip(s, strip);
        }
        self.draw_toolbar(s);
        if let Some(bm) = bm {
            self.draw_bm_bar(s, bm);
        }
        if let Some(area) = self.float_area() {
            self.draw_cards(s, area, false);
        }
        self.draw_footer_overlays(s);
        self.draw_suggestions(s);
    }

    /// The tab strip: pinned tabs as small icon cells at the left, then the tabs,
    /// shrinking as they multiply, then "+", the workspaces chip and the window
    /// buttons. Tabs glide to their places when one is added or closed.
    fn draw_strip(&mut self, s: &UiState, area: R) {
        let lo = self.look.clone();
        let th = lo.theme;
        let compact = s.compact();
        let pins: Vec<UiTab> = s.tabs.iter().filter(|t| t.pinned).cloned().collect();
        let tabs: Vec<UiTab> = s.tabs.iter().filter(|t| !t.pinned).cloned().collect();
        let ws = s.workspaces.len() >= 2;
        let geo = layout::strip(area, pins.len(), tabs.len(), compact, ws);
        // The empty strip drags the window.
        self.hit(area, Hit::Drag);
        for (n, t) in pins.iter().enumerate() {
            if let Some(r) = geo.pinned.get(n) {
                self.draw_strip_tab(t, *r, s, true);
            }
        }
        for (n, t) in tabs.iter().enumerate() {
            let Some(target) = geo.tabs.get(n) else { break };
            // Where it is and how wide it is: both ease toward the layout's.
            let x = self.motion.spring(Id::Tab(t.id), Prop::X, target.x, 200);
            let room = (geo.plus.x - x - 2.0).max(20.0);
            let w = self.motion.tween(Id::Tab(t.id), Prop::W, target.w, 190, Ease::OutCubic).min(room);
            let r = R::new(x, target.y, w, target.h);
            if r.right() > geo.plus.x + 1.0 {
                continue;
            }
            self.draw_strip_tab(t, r, s, false);
        }
        // "+".
        self.button(geo.plus, "plus", Hit::NewTab, true);
        // The workspaces chip: the active workspace, a small switcher.
        if let Some(r) = geo.ws {
            let h = Hit::WsChip;
            let hov = self.is_hover(&h);
            let a = self.motion.hover(Id::Button("wschip"), if hov { 1.0 } else { 0.0 });
            if a > 0.01 {
                self.gfx.round(r, lo.r_sm, th.hover.alpha(a.min(1.0)));
            }
            let name = s.workspaces.get(s.active_workspace).map(String::as_str).unwrap_or("");
            let mut short: String = name.chars().take(if compact { 2 } else { 10 }).collect();
            if name.chars().count() > (if compact { 2 } else { 10 }) {
                short.push('…');
            }
            self.gfx.text(&short, r, lo.font(11.5, 500).align(Align::Center), if hov { th.text } else { th.dim });
            self.hit(r, h);
        }
        // The window buttons at the far right.
        let size = if compact { 26.0 } else { 30.0 };
        self.window_buttons(geo.wins.right(), geo.wins.y + (geo.wins.h - size) / 2.0, size, s);
    }

    /// One tab in the horizontal strip: icon, title, sound, close.
    fn draw_strip_tab(&mut self, t: &UiTab, r: R, s: &UiState, pinned: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        let active = t.id == s.active;
        let h_ = Hit::Tab(t.id);
        let hov = self.is_hover(&h_) || self.is_hover(&Hit::TabX(t.id)) || self.is_hover(&Hit::TabSnd(t.id));
        let h = self.motion.hover(Id::Tab(t.id), if hov { 1.0 } else { 0.0 }).min(1.0);
        let pressed = self.press.as_ref() == Some(&h_);
        // As in the sidebar: pins are small cards, the active tab is the ink pill, the
        // others light up under the mouse.
        let rad = lo.round(r.h);
        let on_pill = active && !pinned;
        if pinned {
            let lit = th.raise.mix(th.hover, h * 0.6);
            let c = if t.status == "off" && !active { lit.alpha(0.55 + 0.45 * h) } else { lit };
            self.gfx.round(r, lo.r_md, c);
            self.gfx.round_stroke(r, lo.r_md, if active { th.text } else { th.line }, if active { 1.5 } else { 1.0 });
        } else if active {
            self.gfx.round(r, rad, th.pill);
            if lo.style == theme::Style::Precise {
                // Precise marks the current tab with its one accent, as its sidebar does.
                self.gfx.line(r.x + rad.max(6.0), r.bottom() - 1.0, r.right() - rad.max(6.0), r.bottom() - 1.0, th.accent, 2.0);
            }
        } else if h > 0.01 || pressed {
            self.gfx.round(r, rad, if pressed { th.press } else { th.hover.alpha(h) });
        }
        // What is written on the pill is in the pill's ink.
        let ink = |c: theme::Color| if on_pill { th.pill_ink } else { c };
        self.hit(r, Hit::Tab(t.id));
        // The site's icon, or its letter; a spinner while it loads.
        let (fx, fy) = if pinned { (r.x + (r.w - 16.0) / 2.0, r.y + (r.h - 16.0) / 2.0) } else { (r.x + 10.0, r.y + (r.h - 16.0) / 2.0) };
        let fav = R::new(fx, fy, 16.0, 16.0);
        let asleep = matches!(t.status, "asleep" | "off" | "crashed");
        let dim = if asleep { 0.5 } else { 1.0 };
        if t.loading {
            let phase = (self.born.elapsed().as_secs_f32() / 0.8).fract();
            self.gfx.spinner(fx + 8.0, fy + 8.0, 6.5, phase, th.line, th.accent);
        } else {
            let png = lo.show_favicons.then(|| self.favs.get(&t.id).cloned()).flatten();
            let bmp = png.filter(|p| !p.is_empty()).and_then(|p| self.gfx.bitmap(t.id, &p));
            match bmp {
                Some(b) => self.gfx.draw_bitmap(&b, fav, dim),
                None if t.status == "new" => {
                    self.gfx.round_stroke(fav, 5.0, ink(th.dim).alpha(0.5), 1.0);
                    self.gfx.icon("plus", fav.x + 3.0, fav.y + 3.0, 10.0, ink(th.dim));
                }
                None => {
                    // No icon: the site's letter on a quiet tile, never a colour of its own.
                    let letter = host(&t.url).chars().next().unwrap_or('·').to_uppercase().to_string();
                    self.gfx.round(fav, 5.0, th.raise.alpha(dim));
                    self.gfx.round_stroke(fav, 5.0, th.line.alpha(dim), 1.0);
                    self.gfx.text(&letter, fav, lo.font(9.5, 650).align(Align::Center), th.dim.alpha(dim));
                }
            }
        }
        // The AI and private dots, as in the sidebar's pins.
        if t.agent {
            self.gfx.circle(r.right() - 5.0, r.y + 5.0, 3.0, th.accent);
        }
        if t.private {
            self.gfx.circle(r.right() - 5.0, r.y + 5.0, 3.0, style::private_mark(th.dark));
        }
        if pinned {
            if t.audio || t.muted || t.call {
                let b = R::new(r.right() - 13.0, r.y + 1.0, 12.0, 12.0);
                self.gfx.round(b, 4.0, th.side);
                self.gfx.icon(sound_icon(t), b.x + 1.0, b.y + 1.0, 10.0, th.dim);
                self.hit(b, Hit::TabSnd(t.id));
            }
            return;
        }
        // Right to left: close, sound, split, translate.
        let mut right = r.right() - 5.0;
        let show_x = match lo.close_btn {
            style::CloseBtn::Always => true,
            style::CloseBtn::Hover => hov || active,
            style::CloseBtn::Off => false,
        };
        if show_x {
            let x = R::new(right - 20.0, r.y + (r.h - 20.0) / 2.0, 20.0, 20.0);
            let xh = self.is_hover(&Hit::TabX(t.id));
            if xh {
                self.gfx.round(x, lo.round(20.0), if on_pill { th.pill_ink.alpha(0.18) } else { th.press });
            }
            self.gfx.icon("close", x.x + 3.0, x.y + 3.0, 14.0, ink(if xh { th.text } else { th.dim }));
            self.hit(x, Hit::TabX(t.id));
            right = x.x - 2.0;
        }
        if t.audio || t.muted || t.call {
            let b = R::new(right - 20.0, r.y + (r.h - 20.0) / 2.0, 20.0, 20.0);
            if self.is_hover(&Hit::TabSnd(t.id)) {
                self.gfx.round(b, lo.round(20.0), if on_pill { th.pill_ink.alpha(0.18) } else { th.press });
            }
            self.gfx.icon(sound_icon(t), b.x + 3.0, b.y + 3.0, 14.0, ink(th.dim));
            self.hit(b, Hit::TabSnd(t.id));
            right = b.x - 2.0;
        }
        if t.audio && !t.muted {
            let b = R::new(right - 20.0, r.y + (r.h - 20.0) / 2.0, 20.0, 20.0);
            if self.is_hover(&Hit::TabPip(t.id)) {
                self.gfx.round(b, lo.round(20.0), if on_pill { th.pill_ink.alpha(0.18) } else { th.press });
            }
            self.gfx.icon("pip", b.x + 3.0, b.y + 3.0, 14.0, ink(th.dim));
            self.hit(b, Hit::TabPip(t.id));
            right = b.x - 2.0;
        }
        if t.split {
            let b = R::new(right - 16.0, r.y + (r.h - 16.0) / 2.0, 16.0, 16.0);
            self.gfx.icon("split", b.x, b.y, 16.0, ink(th.faint));
            right = b.x - 3.0;
        }
        if t.translated || t.translating {
            let b = R::new(right - 16.0, r.y + (r.h - 16.0) / 2.0, 16.0, 16.0);
            self.gfx.icon("globe", b.x, b.y, 16.0, if t.translating { th.faint } else { th.accent });
            right = b.x - 3.0;
        }
        let title = if !t.title.is_empty() {
            t.title.clone()
        } else if t.status == "new" {
            "New tab".into()
        } else {
            let h = host(&t.url);
            if h.is_empty() { "Untitled".into() } else { h }
        };
        let c = ink(if hov { th.text } else { th.dim });
        let c = if asleep { c.alpha(0.5) } else { c };
        self.gfx.text(&title, R::new(fx + 24.0, r.y, (right - fx - 24.0).max(0.0), r.h), lo.font(12.5, if on_pill { 550 } else { 450 }), c);
        let _ = s;
    }

    /// The toolbar: navigation, the sidebar toggle where there is a sidebar, the
    /// address bar with its lock and star, downloads, and the menu.
    fn draw_toolbar(&mut self, s: &UiState) {
        let Some((_, tool, _)) = self.top_rows() else { return };
        let lo = self.look.clone();
        let th = lo.theme;
        self.gfx.fill(tool, th.side);
        // The empty toolbar drags the window; the buttons drawn after win the mouse.
        self.hit(tool, Hit::Drag);
        let compact = s.compact();
        let size = if compact { 28.0 } else { 30.0 };
        let y = tool.y + (tool.h - size) / 2.0;
        let nav_w = self.nav_buttons(tool.x + 8.0, y, s, compact);
        // A sidebar, where the layout has one to fold.
        if lo.kind.sidebar() {
            let r = R::new(tool.x + 8.0 + nav_w + 4.0, y, size, size);
            self.button(r, "sidebar", Hit::Sidebar, true);
        }
        // The address bar, with the lock at its left and the star at its right.
        self.draw_omni_bar(s, compact);
        // Downloads, then the menu, at the far right.
        let right = tool.right() - 8.0;
        let dl = R::new(right - size, y, size, size);
        self.draw_dl_button(dl, s);
        let menu = R::new(dl.x - size - 2.0, y, size, size);
        let h = Hit::Menu;
        let hov = self.is_hover(&h);
        let a = self.motion.hover(Self::button_id(&h), if hov { 1.0 } else { 0.0 }).min(1.0);
        if a > 0.01 {
            self.gfx.round(menu, lo.r_sm + 1.0, th.hover.alpha(a));
        }
        // Three dots, drawn rather than an icon: "more".
        let c = if hov { th.text } else { th.dim };
        for n in 0..3 {
            self.gfx.circle(menu.x + menu.w / 2.0, menu.y + menu.h / 2.0 + (n as f32 - 1.0) * 4.5, 1.6, c);
        }
        self.hit(menu, h);
    }

    /// The bookmarks bar: one chip per bookmark, in order, as many as fit.
    fn draw_bm_bar(&mut self, s: &UiState, bm: R) {
        let lo = self.look.clone();
        let th = lo.theme;
        self.gfx.fill(bm, th.side);
        self.gfx.line(bm.x, bm.y, bm.right(), bm.y, th.line, 1.0);
        let f = lo.font(12.0, 450);
        let mut x = bm.x + 8.0;
        for (n, m) in s.bookmarks.iter().enumerate() {
            let name = if m.title.is_empty() { m.url.clone() } else { m.title.clone() };
            let w = (self.gfx.measure(&name, f) + 20.0).min(220.0);
            if x + w > bm.right() - 8.0 {
                break;
            }
            let r = R::new(x, bm.y + 3.0, w, bm.h - 6.0);
            let hit = Hit::Page(pages::PageHit::Bm(m.id));
            let hov = self.is_hover(&hit);
            let a = self.motion.hover(Id::Mark(n), if hov { 1.0 } else { 0.0 });
            if a > 0.01 {
                self.gfx.round(r, lo.r_sm, th.hover.alpha(a.min(1.0)));
            }
            self.gfx.text(&name, r, f.align(Align::Center), if hov { th.text } else { th.dim });
            self.hit(r, hit);
            x += w + 2.0;
        }
    }

    /// The menu button's popup: the pages and the dashboard.
    fn menu(&mut self, x: f32, y: f32) {
        let items: Vec<(u32, String)> = [
            (1, "New tab"),
            (2, "Dashboard"),
            (0, ""),
            (3, "Settings"),
            (4, "History"),
            (5, "Bookmarks"),
            (6, "Downloads"),
            (7, "Control"),
        ]
        .into_iter()
        .map(|(id, label)| (id, label.to_string()))
        .collect();
        menu::open(self, &items, x, y, |i, cmd| {
            let which = match cmd {
                1 => {
                    send(&i.proxy, json!({"t": "newtab"}));
                    return;
                }
                2 => {
                    send(&i.proxy, json!({"t": "dashboard"}));
                    return;
                }
                3 => "settings",
                4 => "history",
                5 => "bookmarks",
                6 => "downloads",
                7 => "control",
                _ => return,
            };
            send(&i.proxy, json!({"t": "panel", "open": true, "which": which}));
        });
    }

    /// The workspaces chip's popup: switch, or make a new one.
    fn ws_menu(&mut self, x: f32, y: f32) {
        let Some(s) = self.state.clone() else { return };
        let mut items: Vec<(u32, String)> = s
            .workspaces
            .iter()
            .enumerate()
            .map(|(n, name)| (n as u32 + 1, if n == s.active_workspace { format!("· {name}") } else { name.clone() }))
            .collect();
        items.push((0, String::new()));
        items.push((100, "New workspace".into()));
        menu::open(self, &items, x, y, move |i, cmd| {
            if cmd == 100 {
                send(&i.proxy, json!({"t": "workspaceAdd"}));
            } else if cmd as usize <= s.workspaces.len() {
                send(&i.proxy, json!({"t": "workspaceSwitch", "to": cmd - 1}));
            }
        });
    }

    // --------------------------------------------------------- new tab page

    /// The wallpaper fills the stage, cropped like a cover photo. Blur is a small
    /// bitmap drawn large; the dim is a wash over it.
    fn draw_wallpaper(&mut self, st: R) {
        let lo = self.look.clone();
        if lo.ntp_bg != style::NtpBg::Wallpaper {
            return;
        }
        let Some((path, _)) = &self.wallpaper else { return };
        let key = wallpaper_key(path) ^ if lo.wallpaper_blur { 0x9e37 } else { 0 };
        let max_px = if lo.wallpaper_blur { 72 } else { 4096 };
        let bmp = match self.wallpaper.as_ref().map(|(_, b)| b) {
            Some(bytes) => self.gfx.bitmap_scaled(key, bytes, max_px),
            None => None,
        };
        if let Some(b) = bmp {
            let size = unsafe { b.GetSize() };
            if size.width > 0.0 && size.height > 0.0 {
                let k = (st.w / size.width).max(st.h / size.height);
                let (dw, dh) = (size.width * k, size.height * k);
                let dest = R::new(st.x + (st.w - dw) / 2.0, st.y + (st.h - dh) / 2.0, dw, dh);
                self.gfx.draw_bitmap(&b, dest, 1.0);
            }
        }
        if lo.wallpaper_dim > 0 {
            self.gfx.fill(st, theme::Color(0.0, 0.0, 0.0, lo.wallpaper_dim as f32 / 100.0));
        }
    }

    fn draw_ntp(&mut self, st: R, s: &UiState) {
        let lo = self.look.clone();
        let th = lo.theme;
        if lo.ntp_bg != style::NtpBg::Wallpaper && lo.style != theme::Style::Glass {
            self.gfx.fill(st, th.bg);
        }
        self.draw_wallpaper(st);
        let geo = self.ntp_geometry();
        let field = geo.field;
        let now = local_now();
        // Entrance: the clock rises first, then the numbers, then the cards.
        let t1 = self.motion.enter(Id::Named("ntp1"), Prop::Opacity, 0.0, 1.0, 200, Ease::OutCubic);
        let t2 = self.motion.enter(Id::Named("ntp2"), Prop::Opacity, 0.0, 1.0, 260, Ease::OutCubic);
        let t3 = self.motion.enter(Id::Named("ntp3"), Prop::Opacity, 0.0, 1.0, 320, Ease::OutQuint);
        let rise1 = self.motion.enter(Id::Named("ntp1"), Prop::Y, 12.0, 0.0, 220, Ease::OutCubic);
        let rise2 = self.motion.enter(Id::Named("ntp2"), Prop::Y, 10.0, 0.0, 260, Ease::OutCubic);
        let x0 = field.x;
        let top = field.y - 18.0 - if lo.clock { self.ntp_head_h() } else { 40.0 };
        if lo.clock {
            let cf = self.ntp_clock_font(st.w);
            let big = cf.size10 as f32 / 10.0;
            let hh = now.hour(lo.clock_24h);
            let mm = format!("{:02}", now.m);
            let (wh, wc, wm) = (self.gfx.measure(&hh, cf), self.gfx.measure(":", cf), self.gfx.measure(&mm, cf));
            let row = R::new(x0 - big * 0.04, top + rise1, wh + wc + wm + 20.0, big * 1.05);
            let mut cx = row.x;
            self.gfx.text(&hh, R::new(cx, row.y, wh + 6.0, row.h), cf, th.text.alpha(t1));
            cx += wh;
            // The colon is the one live thing on the page: it carries the accent.
            let colon = if lo.style == theme::Style::Glass { th.text.alpha(0.5) } else { th.accent };
            self.gfx.text(":", R::new(cx, row.y, wc + 6.0, row.h), cf, colon.alpha(t1));
            cx += wc;
            self.gfx.text(&mm, R::new(cx, row.y, wm + 8.0, row.h), cf, th.text.alpha(t1));
            cx += wm;
            if lo.clock_seconds {
                let fs = lo.mono_at(big * 0.3, 600);
                let ss = format!("{:02}", now.s);
                let ws = self.gfx.measure(&ss, fs);
                self.gfx.text(&ss, R::new(cx + 8.0, row.y + row.h * 0.18, ws + 8.0, row.h * 0.4), fs, th.faint.alpha(t1));
                cx += ws + 16.0;
            }
            if !lo.clock_24h {
                let fs = lo.mono_at(big * 0.24, 600);
                let w = self.gfx.measure(now.suffix(), fs);
                self.gfx.text(now.suffix(), R::new(cx + 6.0, row.y + row.h * 0.5, w + 8.0, row.h * 0.35), fs, th.faint.alpha(t1));
            }
            let date = now.date(lo.date_fmt).replace(", ", " · ");
            if !date.is_empty() {
                let dated = if matches!(lo.date_fmt, style::DateFmt::Long) { format!("{date} · {}", now.year) } else { date };
                self.label(&dated, R::new(x0, row.y + big * 0.92 + 10.0, field.w * 0.55, 18.0), th.faint.alpha(t1));
            }
            if !lo.greeting.is_empty() {
                self.gfx.text(&lo.greeting, R::new(x0, row.y - 28.0, field.w, 22.0), lo.font(14.0, 500), th.dim.alpha(t1));
            }
            // The two numbers Lumen is proud of, beside the clock.
            let stats_w = (field.w * 0.44).min(440.0);
            if field.w >= 640.0 {
                let ch = 96.0;
                let sy = top + big * 0.92 + 30.0 - ch + rise2;
                let cw = (stats_w - 12.0) / 2.0;
                let blocked = s.privacy_today.unwrap_or_default();
                let mem = s.mem.map(|(b, _)| b as f64 / 1048576.0);
                let open = s.tabs.iter().filter(|t| !t.agent).count();
                let asleep = s.tabs.iter().filter(|t| !t.agent && matches!(t.status, "asleep" | "off")).count();
                let cards = [
                    ("Blocked today", group_digits(blocked.blocked()), String::new(), format!("ads and trackers · {} saved", pages::fmt_bytes(blocked.bytes as i64))),
                    (
                        "Lumen",
                        mem.map(|m| format!("{}", m.round())).unwrap_or_else(|| "—".into()),
                        if mem.is_some() { "MB".into() } else { String::new() },
                        format!("{open} tabs · {asleep} asleep"),
                    ),
                ];
                for (n, (cap, num, unit, sub)) in cards.iter().enumerate() {
                    let r = R::new(field.right() - stats_w + n as f32 * (cw + 12.0), sy, cw, ch);
                    self.card(r, t2);
                    self.label(cap, R::new(r.x + 16.0, r.y + 12.0, r.w - 32.0, 16.0), th.faint.alpha(t2));
                    let nf = lo.big_num;
                    let nw = self.gfx.measure(num, nf);
                    self.gfx.text(num, R::new(r.x + 16.0, r.y + 30.0, nw + 6.0, 40.0), nf, th.text.alpha(t2));
                    if !unit.is_empty() {
                        self.gfx.text(unit, R::new(r.x + 22.0 + nw, r.y + 40.0, 40.0, 26.0), lo.mono_at(13.0, 500), th.faint.alpha(t2));
                    }
                    self.gfx.text(sub, R::new(r.x + 16.0, r.y + 70.0, r.w - 32.0, 16.0), lo.font(11.5, 400), th.faint.alpha(t2));
                }
            }
        }
        // A private new tab page says what private means, under the search bar.
        if s.active_tab().is_some_and(|t| t.private) {
            let mark = style::private_mark(th.dark);
            self.gfx.text(
                "Private tab. Nothing you do here is saved: no history, no cookies after the last private tab closes.",
                R::new(x0 + 4.0, field.y - 22.0, field.w, 18.0),
                lo.font(12.0, 450),
                mark.alpha(t2),
            );
        }
        // The search bar: the edit box sits on top of this.
        if lo.ntp_search {
            let fill = ntp_bar_fill(&lo);
            let rad = lo.round(field.h);
            let focused = unsafe { GetFocus() } == self.ntp_edit;
            self.gfx.round(field, rad, fill.alpha(t2));
            let (ink, faint) = if lo.style == theme::Style::Signal { (th.pill_ink, th.pill_ink.alpha(0.6)) } else { (th.text, th.faint) };
            if lo.style != theme::Style::Signal {
                self.gfx.round_stroke(field, rad, if focused { th.accent } else { th.line }, if focused { 1.5 } else { 1.0 });
            }
            self.gfx.icon("search", field.x + 22.0, field.y + (field.h - 18.0) / 2.0, 18.0, faint.alpha(t2));
            if wide_text(self.ntp_edit).is_empty() && !focused {
                self.gfx.text("Search the web or type an address", R::new(field.x + 52.0, field.y, field.w - 140.0, field.h), lo.font(15.0, 400), faint.alpha(t2));
            }
            // The key that gets here from anywhere.
            let k = "Ctrl L";
            let kf = lo.mono_at(10.5, 500);
            let kw = self.gfx.measure(k, kf) + 14.0;
            let kr = R::new(field.right() - 24.0 - kw, field.y + (field.h - 20.0) / 2.0, kw, 20.0);
            self.gfx.round_stroke(kr, 5.0, faint.alpha(0.6 * t2), 1.0);
            // Centred in a wider box than the chip: a box measured to the pixel wraps "L" away.
            self.gfx.text(k, R::new(kr.x - 10.0, kr.y, kr.w + 20.0, kr.h), kf.align(Align::Center), faint.alpha(t2));
            let _ = ink;
            self.hit(field, Hit::NtpSearch);
        }
        self.draw_dial(s, t3);
    }

    /// A card: the look's raised surface, a hairline, and in Glass a frosted pane.
    /// A card that floats over the chrome: a soft shadow under it, then the card.
    /// The link pill: one line, the address cut to fit, over the page's bottom-left corner.
    fn draw_link(&mut self, s: &UiState) {
        if self.link.is_empty() || s.full || s.panel != "none" {
            return;
        }
        let (sx, sy, sw, sh) = s.stage;
        let f = self.look.font(12.0, 450);
        let text = self.link.clone();
        let w = (self.gfx.measure(&text, f) + 20.0).min(sw * 0.6).max(40.0);
        let r = R::new(sx + 8.0, sy + sh - 8.0 - 26.0, w, 26.0);
        let th = self.look.theme;
        self.floats.push((r, 13.0));
        self.gfx.round(r, 13.0, th.solid(th.raise));
        self.gfx.round_stroke(r, 13.0, th.line, 1.0);
        self.gfx.text(&text, R::new(r.x + 10.0, r.y, r.w - 20.0, r.h), f, th.dim);
    }

    fn float_card(&mut self, r: R, a: f32) {
        self.floats.push((r, self.look.r_card));
        let lo = &self.look;
        let th = lo.theme;
        self.gfx.round(R::new(r.x, r.y + 6.0, r.w, r.h), lo.r_card, th.shade.alpha(0.55 * a));
        // Opaque even where cards are glass: what floats must hide what is under it.
        self.gfx.round(r, lo.r_card, th.solid(th.raise).alpha(a));
        self.gfx.round_stroke(r, lo.r_card, th.line.alpha(a), 1.0);
    }

    /// A switch: a track and a knob that glides to the side `on` says. `a` dims it when
    /// it cannot be pressed; no hit makes it dead.
    fn switch(&mut self, r: R, on: bool, key: &'static str, a: f32, hit: Option<Hit>) {
        let th = self.theme;
        let t = self.motion.tween(Id::Chip(key), Prop::Progress, if on { 1.0 } else { 0.0 }, 160, Ease::OutCubic);
        let on_fill = if self.look.style == theme::Style::Signal { th.pill } else { th.accent };
        self.gfx.round(r, 11.0, th.line.mix(on_fill, t).alpha(a));
        let kx = r.x + 3.0 + 16.0 * t;
        self.gfx.circle(kx + 8.0, r.y + 11.0, 8.0, th.raise.mix(th.pill_ink, t).alpha(a));
        if let Some(h) = hit {
            self.hit(r.inset(-4.0, -4.0), h);
        }
    }

    /// A pill button. Primary is the look's ink (Signal) or its accent; quiet sits on the card
    /// with a hairline.
    fn pill_button(&mut self, r: R, label: &str, hit: Hit, primary: bool, a: f32) {
        let lo = self.look.clone();
        let th = lo.theme;
        let hov = self.is_hover(&hit);
        let hb = self.motion.hover(Self::button_id(&hit), if hov { 1.0 } else { 0.0 }).min(1.0);
        let rad = lo.round(r.h);
        let f = lo.font(12.5, if primary { 600 } else { 500 }).align(Align::Center);
        if primary {
            let (fill, ink) = if lo.style == theme::Style::Signal { (th.pill, th.pill_ink) } else { (th.accent, th.bg) };
            self.gfx.round(r, rad, fill.alpha(a * (0.9 + 0.1 * hb)));
            self.gfx.text(label, r, f, ink.alpha(a));
        } else {
            self.gfx.round(r, rad, th.raise.mix(th.hover, hb).alpha(a));
            self.gfx.round_stroke(r, rad, th.line.alpha(a), 1.0);
            self.gfx.text(label, r, f, th.text.alpha(a));
        }
        self.hit(r, hit);
    }

    /// Cells side by side over a sunken pill, the chosen one in ink: the segmented control of
    /// the floating cards. `cur` is the chosen cell, if any.
    fn seg_cells(&mut self, r: R, cells: &[(&str, Hit)], cur: Option<usize>) {
        let lo = self.look.clone();
        let th = lo.theme;
        self.gfx.round(r, lo.round(r.h), th.bg);
        self.gfx.round_stroke(r, lo.round(r.h), th.line, 1.0);
        let w = (r.w - 4.0) / cells.len() as f32;
        for (n, (label, h)) in cells.iter().enumerate() {
            let c = R::new(r.x + 2.0 + n as f32 * w, r.y + 2.0, w, r.h - 4.0);
            let on = cur == Some(n);
            if on {
                self.gfx.round(c, lo.round(c.h), th.pill);
            } else if self.is_hover(h) {
                self.gfx.round(c, lo.round(c.h), th.hover);
            }
            self.gfx.text(label, c, lo.font(12.0, if on { 600 } else { 500 }).align(Align::Center), if on { th.pill_ink } else { th.dim });
            self.hit(c, h.clone());
        }
    }

    /// A quiet tag: spaced capitals in a hairline pill, at its right edge `right`. Returns its
    /// left edge.
    fn tag(&self, text: &str, right: f32, cy: f32, c: theme::Color) -> f32 {
        let lo = &self.look;
        let t = text.to_uppercase();
        let w = self.gfx.measure_tracked(&t, lo.label, lo.label_track) + 14.0;
        let r = R::new(right - w, cy - 9.0, w, 18.0);
        self.gfx.round_stroke(r, lo.round(r.h), lo.theme.line, 1.0);
        self.gfx.text_tracked(&t, R::new(r.x + 7.0, r.y + 1.0, w - 7.0, r.h), lo.label, c, lo.label_track);
        r.x
    }

    fn card(&self, r: R, a: f32) {
        let lo = &self.look;
        let th = lo.theme;
        self.gfx.round(r, lo.r_card, th.raise.alpha(a));
        self.gfx.round_stroke(r, lo.r_card, th.line.alpha(a), 1.0);
    }

    /// The speed dial: a grid of tiles, a favicon in a rounded square with its name under it.
    /// Kept tiles are bright; with none kept, the sites seen most are offered, dimmed, each with
    /// a small button to keep it. The last cell is "+".
    fn draw_dial(&mut self, s: &UiState, t3: f32) {
        let lo = self.look.clone();
        let th = lo.theme;
        let geo = self.ntp_geometry();
        let top = geo.top + (1.0 - t3) * 10.0;
        // A short note about the last thing that did not work (an address that was not one).
        if let Some((msg, at)) = &self.page.flash
            && at.elapsed().as_secs_f32() < 5.0
        {
            // Fades away over its last second instead of vanishing.
            let a = (5.0 - at.elapsed().as_secs_f32()).min(1.0);
            self.gfx.text(msg, R::new(geo.field.x + 2.0, geo.field.bottom() + 6.0, geo.field.w, 18.0), lo.font(12.0, 450), th.faint.alpha(a));
        }
        let lifted = self.dial_drag.as_ref().filter(|d| d.moved).map(|d| d.from);
        // Where a lifted tile would land, so the target can be marked.
        let target = lifted.and_then(|_| geo.grid.index_at(self.mouse.0, self.mouse.1 - geo.top)).filter(|n| matches!(geo.cells.get(*n), Some(Cell::Tile(_))));
        for (n, cell) in geo.cells.iter().enumerate() {
            let (ox, oy) = geo.grid.origin(n);
            let cr = R::new(ox, top + oy, geo.grid.cell_w(), geo.grid.row_h());
            let tile = R::new(cr.x, cr.y, cr.w, NTP_CARD_H);
            let (hit, name, url, dim) = match *cell {
                Cell::Tile(i) => {
                    let sc = &s.settings.shortcuts[i];
                    (Hit::Shortcut(i), speeddial::label(sc), sc.url.clone(), false)
                }
                Cell::Sugg(i) => {
                    let sc = &s.suggestions[i];
                    (Hit::DialSugg(i), speeddial::label(sc), sc.url.clone(), true)
                }
                Cell::Add => (Hit::DialAdd, "Add".to_string(), String::new(), false),
            };
            let pin = if let Cell::Sugg(i) = *cell { Some(Hit::DialPin(i)) } else { None };
            let hov = self.is_hover(&hit) || pin.as_ref().is_some_and(|p| self.is_hover(p));
            let me = lifted == Some(n) && matches!(cell, Cell::Tile(_));
            let hv = self.motion.hover(Id::Dial(n), if hov && !me { 1.0 } else { 0.0 });
            let a = t3 * if me { 0.3 } else if dim && !hov { 0.6 } else { 1.0 };
            // The card lifts a little under the pointer.
            let lift = hv * 2.0;
            let tile = R::new(tile.x, tile.y - lift, tile.w, tile.h);
            self.draw_dial_tile(tile, &name, &url, matches!(cell, Cell::Add), hv, a);
            if target == Some(n) && !me {
                self.gfx.round_stroke(tile.inset(-3.0, -3.0), lo.r_card + 3.0, th.accent, 1.5);
            }
            self.hit(tile, hit);
            if let (Some(p), true) = (pin, hov) {
                // "Keep it": a small button on the corner.
                let b = R::new(tile.right() - 30.0, tile.y + 8.0, 22.0, 22.0);
                let bh = self.is_hover(&p);
                self.gfx.circle(b.x + 11.0, b.y + 11.0, 11.0, if bh { th.accent } else { th.press });
                self.gfx.icon("plus", b.x + 4.0, b.y + 4.0, 14.0, if bh { th.bg } else { th.text });
                self.hit(b, p);
            }
        }
        if geo.cells.iter().any(|c| matches!(c, Cell::Sugg(_))) {
            let y = top + geo.grid.height() + 2.0;
            self.gfx.text(
                "Your most visited sites. Hover one to keep it here.",
                R::new(geo.field.x, y, geo.field.w, 18.0),
                lo.font(12.0, 450).align(Align::Center),
                th.faint.alpha(t3),
            );
        }
        // The lifted tile follows the pointer.
        if let Some(from) = lifted
            && let Some(sc) = s.settings.shortcuts.get(from)
        {
            let w = geo.grid.cell_w();
            let t = R::new(self.mouse.0 - w / 2.0, self.mouse.1 - NTP_CARD_H / 2.0, w, NTP_CARD_H);
            self.draw_dial_tile(t, &speeddial::label(sc), &sc.url, false, 1.0, 0.95);
        }
    }

    /// After an update, once: a quiet note at the top of the start page that this version is
    /// new, which opens the changelog. Its x puts it away unread.
    fn draw_update_note(&mut self, st: R, s: &UiState) {
        let now = env!("LUMEN_VERSION");
        // Never in a headless run, but for a debug shot of it (LUMEN_TEST_NOTE).
        let shot = cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_NOTE").is_some();
        if crate::headless() && !shot {
            return;
        }
        // After a run that never said goodbye, that comes first: a report, if the person
        // wants to send one. Then, after an update, what is new.
        let crash = s.unclean_last || (shot && std::env::var("LUMEN_TEST_NOTE").is_ok_and(|v| v == "crash"));
        if !crash && s.settings.seen_version == now {
            return;
        }
        let page = if crash { "bugreport" } else { "changes" };
        let open = pages::PageHit::Panels(page);
        let gone = if crash { pages::PageHit::CrashSeen } else { pages::PageHit::SeenVersion };
        let text = if crash { "Lumen did not close properly last time · send a report".to_string() } else { format!("Lumen {now} is here · see what's new") };
        let lo = self.look.clone();
        let th = lo.theme;
        let f = lo.font(12.5, 500);
        let w = self.gfx.measure(&text, f) + 80.0;
        let r = R::new(st.x + (st.w - w) / 2.0, st.y + 16.0, w, 34.0);
        let a = self.motion.enter(Id::Named("update-note"), Prop::Opacity, 0.0, 1.0, 220, Ease::OutCubic);
        let open = Hit::Page(open);
        let hov = self.is_hover(&open);
        self.gfx.round(r, lo.round(r.h), th.raise.mix(th.hover, if hov { 0.6 } else { 0.0 }).alpha(a));
        self.gfx.round_stroke(r, lo.round(r.h), th.line.alpha(a), 1.0);
        // A crash is something that went wrong: the live colour's dot, not the quiet info mark.
        if crash {
            self.gfx.circle(r.x + 21.0, r.y + r.h / 2.0, 3.5, th.accent.alpha(a));
        } else {
            self.gfx.icon("info", r.x + 14.0, r.y + 10.0, 14.0, th.dim.alpha(a));
        }
        self.gfx.text(&text, R::new(r.x + 36.0, r.y, r.w - 72.0, r.h), f, th.text.alpha(a));
        self.hit(r, open);
        let x = R::new(r.right() - 30.0, r.y + 5.0, 24.0, 24.0);
        let gone = Hit::Page(gone);
        if self.is_hover(&gone) {
            self.gfx.round(x, lo.round(x.h), th.press.alpha(a));
        }
        self.gfx.icon("close", x.x + 6.0, x.y + 6.0, 12.0, th.dim.alpha(a));
        self.hit(x, gone);
    }

    /// A site card: the site's icon at the top, its name and address at the bottom. "+" is an
    /// empty card that says what it does.
    fn draw_dial_tile(&mut self, r: R, name: &str, url: &str, plus: bool, hov: f32, a: f32) {
        let lo = self.look.clone();
        let th = lo.theme;
        if plus {
            self.gfx.round(r, lo.r_card, th.hover.alpha(hov * 0.6 * a));
            self.gfx.round_stroke(r, lo.r_card, th.line.alpha(a), 1.0);
            let c = if hov > 0.5 { th.text } else { th.faint };
            self.gfx.icon("plus", r.x + (r.w - 18.0) / 2.0, r.y + r.h / 2.0 - 22.0, 18.0, c.alpha(a));
            self.gfx.text("Add a site", R::new(r.x, r.y + r.h / 2.0 + 2.0, r.w, 18.0), lo.font(12.0, 500).align(Align::Center), c.alpha(a));
            return;
        }
        self.gfx.round(r, lo.r_card, th.raise.mix(th.hover, hov * 0.5).alpha(a));
        self.gfx.round_stroke(r, lo.r_card, if hov > 0.5 { th.text.alpha(0.35 * a) } else { th.line.alpha(a) }, 1.0);
        let icon = R::new(r.x + 14.0, r.y + 14.0, 26.0, 26.0);
        let host = speeddial::host_of(url);
        let key = site_key(&host);
        let png = self.fav_png(&host, true);
        match png.and_then(|p| self.gfx.bitmap(key, &p)) {
            Some(b) => self.gfx.draw_bitmap(&b, icon, a),
            None => {
                // No icon yet (it is being fetched, or the site has none): its letter, quietly.
                self.gfx.round(icon, 8.0, th.hover.alpha(a));
                let letter = name.chars().next().or_else(|| host.chars().next()).unwrap_or('?').to_uppercase().to_string();
                self.gfx.text(&letter, icon, lo.font(13.0, 700).align(Align::Center), th.dim.alpha(a));
            }
        }
        self.gfx.text(name, R::new(r.x + 14.0, r.bottom() - 46.0, r.w - 28.0, 20.0), lo.font(12.5, 600), th.text.alpha(a));
        self.gfx.text(&host, R::new(r.x + 14.0, r.bottom() - 28.0, r.w - 28.0, 16.0), lo.mono_at(10.5, 450), th.faint.alpha(a));
    }

    /// A site's icon as PNG: the one its tab sent, else the disk cache. With `fetch`, a host
    /// with neither is asked for its icon once per run, on a background thread; the tile
    /// shows it the next time it draws. Tests never fetch: a headless run stays off the net.
    fn fav_png(&mut self, host: &str, fetch: bool) -> Option<Vec<u8>> {
        if let Some(p) = self.site_favs.get(host).filter(|p| !p.is_empty()) {
            return Some(p.clone());
        }
        // A miss is read again every few seconds for the first minute (a fetch takes up to
        // its deadline to land), then left be until the next run.
        let stale = |(p, at, first): &(Option<Vec<u8>>, std::time::Instant, std::time::Instant)| p.is_none() && at.elapsed().as_secs() >= 4 && first.elapsed().as_secs() < 60;
        let first = self.disk_favs.get(host).map(|e| e.2);
        if first.is_none() || self.disk_favs.get(host).is_some_and(stale) {
            let png = crate::favicons::get(host);
            if png.is_some() {
                // A new picture under the same key: drop any bitmap made from an older one.
                self.gfx.bitmaps.remove(&site_key(host));
            }
            let now = std::time::Instant::now();
            self.disk_favs.insert(host.to_string(), (png, now, first.unwrap_or(now)));
        }
        let png = self.disk_favs.get(host).and_then(|(p, _, _)| p.clone());
        if png.is_none() && fetch && !crate::headless() && !host.is_empty() && self.fav_asked.insert(host.to_string()) {
            crate::favicons::fetch(vec![host.to_string()]);
        }
        png
    }

    // ------------------------------------------------------------- input

    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        // Last registered wins: things drawn later sit on top.
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| h.clone())
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        if let Some(d) = &mut self.drag {
            d.dx = x - d.x0;
            d.dy = y - d.y0;
            if !d.moved && (d.dx * d.dx + d.dy * d.dy).sqrt() > 5.0 {
                d.moved = true;
            }
            if d.moved {
                self.invalidate();
            }
            return;
        }
        if let Some(d) = &mut self.dial_drag {
            if !d.moved && ((x - d.x0).powi(2) + (y - d.y0).powi(2)).sqrt() > 6.0 {
                d.moved = true;
            }
            if d.moved {
                self.invalidate();
                return;
            }
        }
        if color::drag_move(self, x, y) {
            return;
        }
        if pages::drag_move(self, x, y) {
            return;
        }
        let h = self.hit_at(x, y);
        if h != self.hover {
            self.hover = h;
            self.invalidate();
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32, dbl: bool) {
        let h = self.hit_at(x, y);
        // Clicking anywhere but the suggestions ends address editing.
        if self.editing && !matches!(h, Some(Hit::Sugg(_)) | Some(Hit::Omni)) {
            unsafe {
                let _ = SetFocus(Some(self.hwnd));
            }
        }
        self.press = h.clone();
        match h {
            Some(Hit::Drag) => {
                if dbl {
                    send(&self.proxy, json!({"t": "win", "a": "max"}));
                } else {
                    send(&self.proxy, json!({"t": "drag"}));
                }
            }
            Some(Hit::Tab(id)) => {
                let pinned = self.state.as_ref().and_then(|s| s.tabs.iter().find(|t| t.id == id)).is_some_and(|t| t.pinned);
                self.drag = Some(Drag { id, x0: x, y0: y, dx: 0.0, dy: 0.0, moved: false, pinned });
                unsafe {
                    SetCapture(self.hwnd);
                }
            }
            Some(Hit::Omni) => {
                if !self.editing {
                    self.begin_edit();
                    unsafe {
                        let _ = SetFocus(Some(self.edit));
                        SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                    }
                }
            }
            Some(Hit::Shortcut(from)) => {
                // Pressed on a tile: a click opens it, a drag moves it.
                self.dial_drag = Some(DialDrag { from, x0: x, y0: y, moved: false });
                unsafe {
                    SetCapture(self.hwnd);
                }
            }
            Some(Hit::NtpSearch) => self.focus_ntp(),
            Some(Hit::ColorArea) => color::press(self, 0, x, y),
            Some(Hit::ColorHue) => color::press(self, 1, x, y),
            Some(Hit::Page(p)) => pages::press(self, p, x, y),
            _ => {}
        }
        self.invalidate();
    }

    fn mouse_up(&mut self, x: f32, y: f32) {
        if let Some(d) = self.drag.take() {
            unsafe {
                let _ = ReleaseCapture();
            }
            if d.moved {
                self.drop_tab(d, x, y);
                self.invalidate();
                return;
            }
        }
        if let Some(d) = self.dial_drag.take() {
            unsafe {
                let _ = ReleaseCapture();
            }
            if d.moved {
                // Let go over another tile: the lifted one takes its place.
                let geo = self.ntp_geometry();
                if let Some(Cell::Tile(to)) = geo.grid.index_at(x, y - geo.top).and_then(|n| geo.cells.get(n).copied())
                    && let Some(s) = &self.state
                {
                    let mut st = s.settings.clone();
                    speeddial::reorder(&mut st.shortcuts, d.from, to);
                    send(&self.proxy, json!({"t": "settings", "v": st}));
                }
                self.press = None;
                self.invalidate();
                return;
            }
        }
        if color::drag_end(self) {
            return;
        }
        if pages::drag_end(self, y) {
            return;
        }
        let h = self.hit_at(x, y);
        let pressed = self.press.take();
        if h.is_none() || h != pressed {
            self.invalidate();
            return;
        }
        // A click that lands anywhere but the tab search card closes it (the row still acts).
        if self.search_open && !matches!(h, Some(Hit::TabSearch(_)) | Some(Hit::SearchCard)) {
            self.close_search();
        }
        // The same for the message card.
        if self.say.is_some() && !matches!(h, Some(Hit::SayCard | Hit::SayMode(_) | Hit::SaySend | Hit::SayClose | Hit::AgentMessage(..))) {
            self.close_say();
        }
        // The same for the lock's card, which acts through its own hits.
        if self.site_open && !matches!(h, Some(Hit::SiteInfo | Hit::SitePerm(..) | Hit::SiteClear | Hit::SiteSettings | Hit::SiteAdblock)) {
            self.site_open = false;
        }
        // And for the colour picker: a click outside it keeps the colour it shows.
        if self.color.is_some()
            && !matches!(h, Some(Hit::ColorCard | Hit::ColorArea | Hit::ColorHue | Hit::ColorPreset(_) | Hit::ColorRecent(_) | Hit::ColorDone))
        {
            color::done(self);
        }
        // A click outside the form cancels it, as Escape does. So does one outside
        // a question, a menu, or a file card.
        form::clicked_outside(self, &h);
        confirm::clicked_outside(self, &h);
        menu::clicked_outside(self, &h);
        files::clicked_outside(self, &h);
        // The same for the command palette (the row still acts).
        if self.pal_open && !matches!(h, Some(Hit::Pal(_)) | Some(Hit::PaletteCard)) {
            self.close_palette();
        }
        let p = &self.proxy;
        match h.unwrap() {
            Hit::Back => send(p, json!({"t": "back"})),
            Hit::Fwd => send(p, json!({"t": "fwd"})),
            Hit::Reload => {
                let loading = self.state.as_ref().and_then(|s| s.active_tab()).is_some_and(|t| t.loading);
                send(p, json!({"t": if loading { "stop" } else { "reload" }}));
            }
            Hit::Min => send(p, json!({"t": "win", "a": "min"})),
            Hit::Max => send(p, json!({"t": "win", "a": "max"})),
            Hit::Close => send(p, json!({"t": "win", "a": "close"})),
            Hit::Tab(id) => {
                // Optimistic: move the highlight now.
                if let Some(s) = &mut self.state {
                    s.active = id;
                }
                send(p, json!({"t": "activate", "id": id}));
            }
            Hit::TabX(id) => send(p, json!({"t": "close", "id": id})),
            Hit::TabSnd(id) => send(p, json!({"t": "mute", "id": id})),
            Hit::TabPip(id) => send(p, json!({"t": "pip", "id": id})),
            Hit::MediaPlay => send(p, json!({"t": "media", "cmd": "toggle"})),
            Hit::MediaNext => send(p, json!({"t": "media", "cmd": "next"})),
            Hit::MediaTitle(id) => send(p, json!({"t": "activate", "id": id})),
            Hit::NewTab => send(p, json!({"t": "newtab"})),
            Hit::Sidebar => {
                // The same change Ctrl+B makes: through the settings, so it is saved.
                if let Some(s) = &self.state {
                    let mut st = s.settings.clone();
                    st.compact = !st.compact;
                    send(p, json!({"t": "settings", "v": st}));
                }
            }
            Hit::Gear => {
                let open = self.state.as_ref().is_some_and(|s| s.panel != "settings");
                send(p, json!({"t": "panel", "open": open, "which": "settings"}));
            }
            Hit::Mem => {
                let open = self.state.as_ref().is_some_and(|s| s.panel != "control");
                send(p, json!({"t": "panel", "open": open, "which": "control"}));
            }
            Hit::Workspace(i) => send(p, json!({"t": "workspaceSwitch", "to": i})),
            Hit::WorkspaceAdd => send(p, json!({"t": "workspaceAdd"})),
            Hit::Menu => self.menu(x, y),
            Hit::WsChip => self.ws_menu(x, y),
            Hit::GlanceOpen => send(p, json!({"t": "glanceOpen"})),
            Hit::Sugg(i) => self.pick(i),
            Hit::DlFooter => {
                let open = self.state.as_ref().is_some_and(|s| s.panel != "downloads");
                send(p, json!({"t": "panel", "open": open, "which": "downloads"}));
            }
            Hit::Report(c) => report::click(self, c),
            Hit::Dash(c) => dashboard::click(self, c),
            Hit::DashFooter => {
                let open = self.state.as_ref().is_some_and(|s| s.panel != "dashboard");
                send(p, json!({"t": "panel", "open": open, "which": "dashboard"}));
            }
            Hit::DriveFooter => {
                let open = self.state.as_ref().is_some_and(|s| s.panel != "drive");
                send(p, json!({"t": "panel", "open": open, "which": "drive"}));
            }
            Hit::AgentStop => send(p, json!({"t": "agentStop"})),
            Hit::AgentStopSession(session) => send(p, json!({"t": "agentStopSession", "session": session})),
            Hit::AgentMessage(session, name) => self.open_say(session, name),
            Hit::SayCard => {}
            Hit::SayMode(n) => {
                if let Some(say) = self.say.as_mut() {
                    say.mode = n.min(2);
                }
                unsafe {
                    let _ = SetFocus(Some(self.say_edit));
                }
                self.invalidate();
            }
            Hit::SaySend => self.send_say(),
            Hit::SayClose => self.close_say(),
            Hit::ColorCard => {}
            Hit::ColorPreset(n) => color::pick_preset(self, n),
            Hit::ColorRecent(n) => color::pick_recent(self, n),
            Hit::ColorDone => color::done(self),
            Hit::FormCard => {}
            Hit::FormSave => form::save(self),
            Hit::FormCancel => form::cancel(self),
            Hit::ConfirmCard => {}
            Hit::ConfirmButton(n) => confirm::pick(self, n),
            Hit::MenuCard => {}
            Hit::MenuRow(n) => menu::click(self, n),
            Hit::FileCard => {}
            Hit::FileRow(n) => files::click_row(self, n),
            Hit::FilePlace(n) => {
                if let Some(dir) = self.files.as_ref().and_then(|f| f.places.get(n)).map(|p| p.dir.clone()) {
                    files::nav_to(self, dir);
                }
            }
            Hit::FileDrive(n) => {
                if let Some((name, _)) = self.files.as_ref().and_then(|f| f.drive_rects.get(n)) {
                    files::nav_to(self, std::path::PathBuf::from(format!("{name}\\")));
                }
            }
            Hit::FileUp => {
                if let Some(dir) = self.files.as_ref().and_then(|f| f.dir.parent().map(|p| p.to_path_buf())) {
                    files::nav_to(self, dir);
                }
            }
            Hit::FileGo => files::go(self),
            Hit::FileCancel => files::cancel(self),
            Hit::FilePath => {
                let e = self.file_path_edit;
                unsafe {
                    let _ = SetFocus(Some(e));
                }
            }
            Hit::FileName => {
                let e = self.file_name_edit;
                unsafe {
                    let _ = SetFocus(Some(e));
                }
            }
            Hit::PermAllow(pid) => send(p, json!({"t": "perm", "pid": pid, "allow": true})),
            Hit::HandoffDone => send(p, json!({"t": "handoffDone"})),
            Hit::Star => {
                // Not bookmarked yet: one click keeps it, in Unsorted. Already kept: a menu.
                let Some(mark) = self.state.as_ref().and_then(|s| s.mark.clone()) else {
                    send(p, json!({"t": "bookmark"}));
                    return;
                };
                self.star_menu(mark, x, y);
            }
            Hit::Listen => send(p, json!({"t": "listen"})),
            Hit::ListenStop => send(p, json!({"t": "listenStop"})),
            Hit::KdbxFill => {
                let items: Vec<(u32, String)> = match self.state.as_ref() {
                    Some(s) => s.kdbx.iter().enumerate().map(|(n, l)| (n as u32 + 1, l.clone())).collect(),
                    None => return,
                };
                menu::open(self, &items, x, y, |i, cmd| {
                    if cmd > 0 {
                        send(&i.proxy, json!({"t": "kdbxFill", "n": cmd - 1}));
                    }
                });
            }
            Hit::PermBlock(pid) => send(p, json!({"t": "perm", "pid": pid, "allow": false})),
            Hit::Shortcut(i) => {
                if let Some(sc) = self.state.as_ref().and_then(|s| s.settings.shortcuts.get(i)) {
                    send(p, json!({"t": "nav", "url": sc.url}));
                }
            }
            Hit::DialSugg(i) => {
                if let Some(sc) = self.state.as_ref().and_then(|s| s.suggestions.get(i)) {
                    send(p, json!({"t": "nav", "url": sc.url}));
                }
            }
            Hit::DialPin(i) => {
                if let Some(s) = &self.state
                    && let Some(sc) = s.suggestions.get(i)
                {
                    let mut st = s.settings.clone();
                    if speeddial::add(&mut st.shortcuts, &sc.title, &sc.url) {
                        send(p, json!({"t": "settings", "v": st}));
                    }
                }
            }
            Hit::DialAdd => {
                // The address, and a name if you want one; blank means the site's own name.
                form::open(self, "Add to speed dial", &[("Address", ""), ("Name (optional)", "")], form::FormAct::DialAdd);
            }
            Hit::Page(ph) => {
                let picking = self.page.font_pick.is_some();
                pages::click(self, ph);
                // The font list opened or closed: the settings search box hides under it.
                if picking != self.page.font_pick.is_some() {
                    self.layout_edits();
                }
            }
            Hit::SiteInfo => {
                // The lock: open what the site may do, or put it away again.
                let opening = !self.site_open;
                self.site_open = opening;
                if opening {
                    send(p, json!({"t": "siteOpen"}));
                }
            }
            Hit::SitePerm(kind, v) => {
                // "lock": the answer came from the popover, not the Site settings page, so a
                // private tab must not leave it in the store on the way out.
                if let Some(origin) = self.active_origin() {
                    send(p, json!({"t": "permSet", "origin": origin, "kind": kind, "v": v, "lock": true}));
                }
            }
            Hit::SiteClear => {
                if let Some(origin) = self.active_origin() {
                    send(p, json!({"t": "siteClear", "origin": origin, "lock": true}));
                }
                self.site_open = false;
            }
            Hit::SiteSettings => {
                self.site_open = false;
                send(p, json!({"t": "panel", "open": true, "which": "sites"}));
            }
            Hit::SiteAdblock => {
                // The switch: take this site out of the list or put it back, then reload
                // so the page settles the way the switch now says.
                if let Some(s) = &self.state
                    && let Some(tab) = s.active_tab()
                {
                    let key = crate::blocker::site_key(&host(&tab.url));
                    let mut st = s.settings.clone();
                    if st.adblock_off_sites.contains(&key) {
                        st.adblock_off_sites.retain(|h| *h != key);
                    } else {
                        st.adblock_off_sites.push(key);
                    }
                    send(p, json!({"t": "settings", "v": st}));
                    send(p, json!({"t": "reload"}));
                }
            }
            Hit::ZoomChip => send(p, json!({"t": "zoomSet", "pct": 100})),
            Hit::TabSearch(n) => self.switch_to(n),
            Hit::Pal(n) => self.run_palette(n),
            Hit::PaletteCard => {
                // A click on the card itself keeps the typing going.
                unsafe {
                    let _ = SetFocus(Some(self.palette_edit));
                }
            }
            _ => {}
        }
        self.invalidate();
    }

    fn drop_tab(&mut self, d: Drag, x: f32, y: f32) {
        let Some(s) = &self.state else { return };
        let group: Vec<(u64, R)> = s
            .tabs
            .iter()
            .filter(|t| t.pinned == d.pinned && t.id != d.id)
            .filter_map(|t| self.hits.iter().find(|(_, h)| *h == Hit::Tab(t.id)).map(|(r, _)| (t.id, *r)))
            .collect();
        // The strip orders left to right; the sidebar list top to bottom.
        let idx = if self.look.kind.tab_strip() {
            group.iter().filter(|(_, r)| x > r.x + r.w / 2.0).count()
        } else {
            group
                .iter()
                .filter(|(_, r)| {
                    if d.pinned {
                        y > r.bottom() || (y >= r.y && x > r.x + r.w / 2.0)
                    } else {
                        y > r.y + r.h / 2.0
                    }
                })
                .count()
        };
        let offset = if d.pinned { 0 } else { s.tabs.iter().filter(|t| t.pinned).count() };
        send(&self.proxy, json!({"t": "move", "id": d.id, "to": offset + idx}));
    }

    fn context(&mut self, x: f32, y: f32) {
        let Some(h) = self.hit_at(x, y) else { return };
        let Some(s) = self.state.clone() else { return };
        match h {
            Hit::Tab(id) | Hit::TabX(id) | Hit::TabSnd(id) => {
                let Some(t) = s.tabs.iter().find(|t| t.id == id) else { return };
                let mut items: Vec<(u32, String)> = vec![(1, if t.pinned { "Unpin".into() } else { "Pin".into() })];
                if t.pin_away {
                    items.push((17, "Back to pinned page".into()));
                }
                // Never offer to copy a private URL anywhere it would be written down.
                if !t.url.is_empty() && !t.private {
                    items.push((2, "Duplicate".into()));
                    items.push((7, "Add to new tab page".into()));
                }
                // Move to another workspace (skip the active one).
                if s.workspaces.len() > 1 {
                    for (wi, name) in s.workspaces.iter().enumerate() {
                        if wi != s.active_workspace {
                            items.push((20 + wi as u32, format!("Move to {name}")));
                        }
                    }
                } else {
                    items.push((15, "New workspace".into()));
                }
                // Two pages in one stage. Needs a second tab to share it with.
                if t.split || s.tabs.len() > 1 {
                    items.push((12, if t.split { "Unsplit".into() } else { "Split with next tab".into() }));
                }
                if t.servo {
                    // The other engine did not have it: hand this host back to WebView2.
                    items.push((14, "Reopen in WebView2".into()));
                }
                if t.url.starts_with("http") && (t.status == "live" || t.status == "asleep") {
                    items.push((10, "Save video".into()));
                    items.push((8, if t.reader { "Exit reader view\tF9".into() } else { "Reader view\tF9".into() }));
                    // Offline: the model for the pair is fetched once, then it is local.
                    items.push((13, if t.translated { "Show the original text\tF8".into() } else { "Translate page\tF8".into() }));
                    let h = host(&t.url);
                    let skipped = s.settings.dark_skip.iter().any(|x| *x == h);
                    if s.dark && s.settings.dark_pages && !skipped {
                        items.push((9, "Keep this site's colors\tShift+F9".into()));
                    } else if s.dark {
                        items.push((9, "Darken this site\tShift+F9".into()));
                    }
                }
                if t.audio || t.muted || t.call {
                    items.push((3, if t.muted { "Unmute".into() } else { "Mute".into() }));
                }
                // The row's PiP button, for a pinned tile too small to carry one.
                if t.audio && !t.muted {
                    items.push((18, "Picture in picture".into()));
                }
                // Agents stay out of this tab unless the person says so, one tab at a time.
                if s.settings.agent && !t.agent && !t.private && t.url.starts_with("http") {
                    items.push((16, if t.agent_ok { "Stop letting agents use this tab".into() } else { "Let agents use this tab".into() }));
                }
                // A pin's Close is already an unload, so it gets no second item for it.
                if !t.pinned && t.id != s.active && (t.status == "live" || t.status == "asleep") {
                    items.push((4, "Unload now".into()));
                }
                items.push((11, "New private tab\tCtrl+Shift+N".into()));
                items.push((0, String::new()));
                items.push((5, "Close other tabs".into()));
                items.push((6, if t.pinned { "Unload, keep the pin\tCtrl+W".into() } else { "Close\tCtrl+W".into() }));
                // The tile's own name and address, so the menu's action owns no borrow.
                let (title, url) = (if t.title.is_empty() { host(&t.url) } else { t.title.clone() }, t.url.clone());
                menu::open(self, &items, x, y, move |i, cmd| {
                    let msg = match cmd {
                        1 => "pin",
                        2 => "duplicate",
                        3 => "mute",
                        4 => "sleep",
                        5 => "closeOthers",
                        6 => "close",
                        8 => "reader",
                        10 => "video",
                        9 => "darksite",
                        11 => "newPrivate",
                        12 => "split",
                        13 => "translate",
                        14 => "webview2",
                        16 => "agentGrant",
                        17 => "pinBack",
                        18 => "pip",
                        15 => {
                            send(&i.proxy, json!({"t": "workspaceAdd"}));
                            return;
                        }
                        7 => {
                            let mut st = s.settings.clone();
                            if speeddial::add(&mut st.shortcuts, &title, &url) {
                                send(&i.proxy, json!({"t": "settings", "v": st}));
                            }
                            return;
                        }
                        20..=23 => {
                            let wi = (cmd - 20) as usize;
                            if wi < s.workspaces.len() && wi != s.active_workspace {
                                send(&i.proxy, json!({"t": "moveTabToWorkspace", "id": id, "to": wi}));
                            }
                            return;
                        }
                        _ => return,
                    };
                    send(&i.proxy, json!({"t": msg, "id": id}));
                });
            }
            Hit::Workspace(w) => {
                let mut items: Vec<(u32, String)> = vec![(1, "Rename".into())];
                if s.workspaces.len() > 1 {
                    items.push((0, String::new()));
                    items.push((2, "Duplicate".into()));
                    items.push((3, "Close".into()));
                }
                items.push((0, String::new()));
                items.push((4, "New workspace".into()));
                menu::open(self, &items, x, y, move |i, cmd| match cmd {
                    // Rename happens right here in the omnibox edit; Enter sends
                    // the new name back, Escape drops it.
                    1 => {
                        let name = s.workspaces.get(w).map(|n| n.clone()).unwrap_or_default();
                        i.begin_workspace_rename(w, &name);
                    }
                    2 => send(&i.proxy, json!({"t": "workspaceDuplicate", "i": w})),
                    3 => send(&i.proxy, json!({"t": "workspaceClose", "i": w})),
                    4 => send(&i.proxy, json!({"t": "workspaceAdd"})),
                    _ => {}
                });
            }
            Hit::WorkspaceAdd => send(&self.proxy, json!({"t": "workspaceAdd"})),
            Hit::DialSugg(n) | Hit::DialPin(n) => {
                menu::open(self, &[(1, "Add to speed dial".into())], x, y, move |i, cmd| {
                    if cmd == 1 && let Some(sc) = s.suggestions.get(n) {
                        let mut st = s.settings.clone();
                        if speeddial::add(&mut st.shortcuts, &sc.title, &sc.url) {
                            send(&i.proxy, json!({"t": "settings", "v": st}));
                        }
                    }
                });
            }
            Hit::Page(pages::PageHit::Bm(id) | pages::PageHit::BmDel(id)) => {
                if let Some(m) = s.bookmarks.iter().find(|m| m.id == id) {
                    self.mark_menu(m, true, x, y);
                }
            }
            Hit::Page(pages::PageHit::BmFolder(name)) if !name.is_empty() => {
                menu::open(self, &[(1, "Rename…".into()), (2, "Delete folder".into())], x, y, move |i, cmd| match cmd {
                    1 => form::open(i, "Rename folder", &[("Name", &name)], form::FormAct::RenameFolder { old: name.clone() }),
                    2 => send(&i.proxy, json!({"t": "bmFolderDelete", "name": name})),
                    _ => {}
                });
            }
            Hit::Shortcut(n) => {
                menu::open(self, &[(1, "Edit…".into()), (2, "Remove".into())], x, y, move |i, cmd| match cmd {
                    1 => {
                        let Some(sc) = s.settings.shortcuts.get(n) else { return };
                        form::open(i, "Edit tile", &[("Address", &sc.url), ("Name (optional)", &sc.title)], form::FormAct::EditTile { i: n });
                    }
                    2 => {
                        let mut st = s.settings.clone();
                        if n < st.shortcuts.len() {
                            st.shortcuts.remove(n);
                            send(&i.proxy, json!({"t": "settings", "v": st}));
                        }
                    }
                    _ => {}
                });
            }
            _ => {}
        }
    }

    /// One small chip in the offer row under the address bar. Returns the x
    /// after it; draws nothing when it would not fit.
    fn chip(&mut self, x: f32, y: f32, right: f32, label: &str, icon: Option<&'static str>, hit: Hit, accent: bool, a: f32) -> f32 {
        let lo = self.look.clone();
        let th = lo.theme;
        let pad = if icon.is_some() { 44.0 } else { 20.0 };
        let f = lo.font(12.5, 600);
        let w = (self.gfx.measure(label, f) + pad).min(right - x);
        if x >= right || w < 44.0 {
            return x;
        }
        let r = R::new(x, y, w, 28.0);
        let hov = self.is_hover(&hit);
        let hv = self.motion.hover(Self::button_id(&hit), if hov { 1.0 } else { 0.0 });
        self.gfx.round(r, lo.r_md, th.raise.alpha(a).mix(th.hover.alpha(hv.min(1.0)), hv.min(1.0)));
        let mut tx = r.x;
        if let Some(ic) = icon {
            self.gfx.icon(ic, r.x + 10.0, r.y + 6.0, 16.0, if accent { th.accent.alpha(a) } else { th.dim.alpha(a) });
            tx = r.x + 32.0;
        }
        self.gfx.text(
            label,
            R::new(tx, r.y, r.right() - tx - 4.0, r.h),
            f.align(if icon.is_some() { Align::Left } else { Align::Center }),
            th.text.alpha(a),
        );
        self.hit(r, hit);
        r.right() + 6.0
    }

    /// The menu for one bookmark: edit it, move it to a folder, order it, remove it. The star
    /// and the rows of the Bookmarks page share it. `page` adds the entries that only make
    /// sense in the list (open in a new tab, move up and down).
    fn mark_menu(&mut self, mark: &UiMark, page: bool, x: f32, y: f32) {
        let Some(s) = self.state.clone() else { return };
        let mut items: Vec<(u32, String)> = vec![(1, "Edit…".into())];
        if page {
            items.push((2, "Open in new tab".into()));
        }
        items.push((0, String::new()));
        if !mark.folder.is_empty() {
            items.push((30, "Move to Unsorted".into()));
        }
        // The first forty folders; more than that is not a menu any more.
        for (n, f) in s.bm_folders.iter().enumerate().take(40) {
            if *f != mark.folder {
                items.push((31 + n as u32, format!("Move to {f}")));
            }
        }
        if page {
            items.push((0, String::new()));
            items.push((3, "Move up".into()));
            items.push((4, "Move down".into()));
        }
        items.push((0, String::new()));
        items.push((5, "Remove bookmark".into()));
        let (id, mark) = (mark.id, mark.clone());
        menu::open(self, &items, x, y, move |i, cmd| {
            let msg = match cmd {
                1 => {
                    // The form card takes it from here: its Save sends the edit.
                    form::open(i, "Edit bookmark", &[("Title", &mark.title), ("Address", &mark.url)], form::FormAct::EditBookmark { id, folder: mark.folder.clone() });
                    return;
                }
                2 => json!({"t": "bmOpen", "url": mark.url, "bg": true}),
                3 => json!({"t": "bmShift", "id": id, "d": -1}),
                4 => json!({"t": "bmShift", "id": id, "d": 1}),
                5 => json!({"t": "bmDelete", "id": id}),
                30 => json!({"t": "bmMove", "id": id, "folder": ""}),
                n @ 31..=70 => match s.bm_folders.get(n as usize - 31) {
                    Some(f) => json!({"t": "bmMove", "id": id, "folder": f}),
                    None => return,
                },
                _ => return,
            };
            send(&i.proxy, msg);
        });
    }

    fn star_menu(&mut self, mark: UiMark, x: f32, y: f32) {
        self.mark_menu(&mark, false, x, y);
    }

    fn wheel(&mut self, delta: i32) {
        if files::wheel(self, self.mouse.0, self.mouse.1, delta) {
            return;
        }
        if pages::wheel(self, delta) {
            return;
        }
        let next = (self.scroll - delta as f32 / 120.0 * 48.0).clamp(0.0, self.list_max);
        if next != self.scroll {
            self.scroll = next;
            self.invalidate();
        }
    }

    fn key(&mut self, vk: u32) -> bool {
        let down = |k: u16| unsafe { GetKeyState(k as i32) } < 0;
        // A menu: Up and Down walk it, Enter chooses, Escape puts it away.
        if menu::key(self, vk) {
            return true;
        }
        // The colour picker: Escape puts the old colour back, Enter is Done.
        if color::key(self, vk) {
            return true;
        }
        // The form: Escape cancels, Enter saves.
        if form::key(self, vk) {
            return true;
        }
        // The question: Escape cancels, Enter is its first button.
        if confirm::key(self, vk) {
            return true;
        }
        // The file card: Escape closes, Enter opens or takes, Up and Down move.
        if files::key(self, vk) {
            return true;
        }
        // The tab search and the lock's card sit over the sidebar: Escape puts them away.
        if vk == VK_ESCAPE.0 as u32 && !down(VK_SHIFT.0) && self.say.is_some() {
            self.close_say();
            return true;
        }
        if vk == VK_ESCAPE.0 as u32 && !down(VK_SHIFT.0) && (self.search_open || self.site_open || self.pal_open) {
            self.close_search();
            self.site_open = false;
            self.close_palette();
            self.invalidate();
            return true;
        }
        // When a glance is open, Escape closes it and Enter opens it as a tab.
        if let Some(s) = &self.state {
            if s.glance {
                if vk == VK_ESCAPE.0 as u32 && !down(VK_SHIFT.0) {
                    send(&self.proxy, json!({"t": "glanceClose"}));
                    return true;
                }
                if vk == VK_RETURN.0 as u32 {
                    send(&self.proxy, json!({"t": "glanceOpen"}));
                    return true;
                }
            }
        }
        if vk == VK_ESCAPE.0 as u32 && !down(VK_SHIFT.0) {
            if self.state.as_ref().is_some_and(|s| s.panel != "none") {
                send(&self.proxy, json!({"t": "panel", "open": false}));
                return true;
            }
        }
        if let Some(a) = crate::engine::map_key(vk, down(VK_CONTROL.0), down(VK_SHIFT.0), down(VK_MENU.0)) {
            let _ = self.proxy.send_event(Ev::Key(a));
            return true;
        }
        false
    }
}

/// Put text on the Windows clipboard.
pub(crate) fn copy_text(hwnd: HWND, text: &str) {
    use windows::Win32::{
        Foundation::HANDLE,
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        },
    };
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(h);
                // The clipboard owns the memory now.
                let _ = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(h.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

/// The clipboard's text, for the Add link button. Empty when there is none.
pub(crate) fn read_clipboard(hwnd: HWND) -> String {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::{
        DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard},
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
    };
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return String::new();
        }
        let mut out = String::new();
        if let Ok(h) = GetClipboardData(CF_UNICODETEXT) {
            let h = HGLOBAL(h.0);
            let size = GlobalSize(h);
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() && size >= 2 {
                // The clipboard NUL-terminates; do not read past it.
                let words = (size / 2).saturating_sub(1);
                let len = (0..words).take_while(|&i| *p.add(i) != 0).count();
                out = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
                let _ = GlobalUnlock(h);
            }
        }
        let _ = CloseClipboard();
        out.trim().to_string()
    }
}

/// The local clock, as the new tab page needs it: the parts, so the settings can
/// decide what to show and how.
struct Now {
    h: u16,
    m: u16,
    s: u16,
    dow: usize,
    day: u16,
    month: u16,
    year: u16,
}

impl Now {
    /// The hours, in 12- or 24-hour form.
    fn hour(&self, h24: bool) -> String {
        if h24 {
            format!("{:02}", self.h)
        } else {
            format!("{}", if self.h % 12 == 0 { 12 } else { self.h % 12 })
        }
    }
    fn suffix(&self) -> &'static str {
        if self.h < 12 { "am" } else { "pm" }
    }
    /// The date line, in the chosen shape.
    fn date(&self, fmt: style::DateFmt) -> String {
        const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
        const MONTHS: [&str; 12] =
            ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        let mi = (self.month as usize).saturating_sub(1) % 12;
        match fmt {
            style::DateFmt::Off => String::new(),
            style::DateFmt::Long => format!("{}, {} {}", DAYS[self.dow], self.day, MONTHS[mi]),
            style::DateFmt::Short => format!("{} {}", self.day, &MONTHS[mi][..3]),
            style::DateFmt::Iso => format!("{:04}-{:02}-{:02}", self.year, self.month, self.day),
        }
    }
}

fn local_now() -> Now {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    Now { h: t.wHour, m: t.wMinute, s: t.wSecond, dow: t.wDayOfWeek as usize % 7, day: t.wDay, month: t.wMonth, year: t.wYear }
}

/// The bitmap-cache key of a wallpaper file. Bit 62 keeps it clear of tab ids and
/// site keys (bit 63).
fn wallpaper_key(path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    (h.finish() | (1 << 62)) & !(1 << 63)
}

// ----------------------------------------------------------- window procs

fn inner_of(hwnd: HWND) -> Option<&'static RefCell<Inner>> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<Inner>;
    if p.is_null() { None } else { Some(unsafe { &*p }) }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let Some(cell) = inner_of(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
        // Re-entry (a message sent while we are busy) falls back to the default.
        let Ok(mut i) = cell.try_borrow_mut() else { return DefWindowProcW(hwnd, msg, wp, lp) };
        let k = i.k();
        let (px, py) = lparam_xy(lp);
        let (x, y) = (px as f32 / k, py as f32 / k);
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let _hdc: HDC = BeginPaint(hwnd, &mut ps);
                let drawn = Instant::now();
                i.paint();
                // The journal's `ui_slow`: one atomic add when a frame stuttered. The first
                // frame of a run builds everything and is reported on its own.
                static FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
                let ms = drawn.elapsed().as_secs_f64() * 1000.0;
                if FIRST.swap(false, std::sync::atomic::Ordering::Relaxed) {
                    crate::journal::first_frame(ms);
                    crate::startup_line("first_frame_at", crate::since_born());
                } else {
                    crate::journal::frame(ms);
                }
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                let (w, h) = ((lp.0 & 0xffff) as u32, ((lp.0 >> 16) & 0xffff) as u32);
                let dpi = GetDpiForWindow(hwnd) as f32;
                let dpi_changed = dpi != i.gfx.dpi;
                i.gfx.resize(w, h, dpi);
                if dpi_changed {
                    i.make_fonts();
                }
                i.layout_edits();
                i.invalidate();
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                i.mouse_move(x, y);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                i.mouse = (-1.0, -1.0);
                if i.hover.take().is_some() {
                    i.invalidate();
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                i.mouse_down(x, y, false);
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                i.mouse_down(x, y, true);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                i.mouse_up(x, y);
                LRESULT(0)
            }
            WM_MBUTTONUP => {
                match i.hit_at(x, y) {
                    Some(Hit::Tab(id)) => send(&i.proxy, json!({"t": "close", "id": id})),
                    // A bookmark: middle click opens it in a tab behind this one.
                    Some(Hit::Page(pages::PageHit::Bm(id))) => {
                        if let Some(m) = i.state.as_ref().and_then(|s| s.bookmarks.iter().find(|m| m.id == id)) {
                            send(&i.proxy, json!({"t": "bmOpen", "url": m.url, "bg": true}));
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                // The menu loop pumps messages; let go of the borrow first.
                let (hits, me) = (i.hits.clone(), hwnd);
                drop(i);
                let _ = (hits, me);
                if let Ok(mut i) = cell.try_borrow_mut() {
                    i.context(x, y);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xffff) as i16 as i32;
                i.wheel(delta);
                LRESULT(0)
            }
            WM_NCHITTEST => {
                // The top few pixels belong to the parent: it turns them into a resize edge.
                let mut pt = POINT { x: px, y: py };
                let _ = ScreenToClient(hwnd, &mut pt);
                let maximized = i.state.as_ref().is_some_and(|s| s.maximized);
                if pt.y < (5.0 * k) as i32 && !maximized {
                    return LRESULT(HTTRANSPARENT as isize);
                }
                LRESULT(HTCLIENT as isize)
            }
            WM_TIMER => {
                if wp.0 == TIMER_SPIN || wp.0 == TIMER_CLOCK || wp.0 == TIMER_ANIM || wp.0 == TIMER_MEM {
                    i.invalidate();
                }
                #[cfg(debug_assertions)]
                if wp.0 == TIMER_SHOT {
                    shot::capture(&mut i);
                }
                LRESULT(0)
            }
            WM_CHAR => {
                // Typing into the font list's filter, when it is open; or letters that
                // jump through a file card's list.
                if i.files.is_some() {
                    files::typed_char(&mut i, wp.0 as u32);
                } else if !pages::pick_char(&mut i, wp.0 as u32) {
                    drop(i);
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                LRESULT(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if wp.0 as u16 == VK_MENU.0 || wp.0 as u16 == VK_F10.0 {
                    return LRESULT(0);
                }
                if i.key(wp.0 as u32) {
                    return LRESULT(0);
                }
                let picking = i.page.font_pick.is_some();
                if pages::key(&mut i, wp.0 as u32) {
                    if picking != i.page.font_pick.is_some() {
                        i.layout_edits();
                    }
                    return LRESULT(0);
                }
                drop(i);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_SYSKEYUP if wp.0 as u16 == VK_MENU.0 || wp.0 as u16 == VK_F10.0 => LRESULT(0),
            WM_SYSCHAR => LRESULT(0),
            WM_COMMAND => {
                let id = wp.0 & 0xffff;
                let code = ((wp.0 >> 16) & 0xffff) as u32;
                if id == ID_EDIT {
                    match code {
                        // Typing a new workspace name is not a search.
                        EN_CHANGE => {
                            if i.renaming.is_none() {
                                i.suggest();
                            }
                        }
                        EN_KILLFOCUS => i.end_edit(),
                        _ => {}
                    }
                } else if id == ID_SAY && code == EN_CHANGE {
                    // The placeholder goes with the first letter.
                    i.invalidate();
                } else if id == ID_NTP && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    if code == EN_CHANGE {
                        // Ask history for the site to fill in, as the address bar does.
                        let edit = i.ntp_edit;
                        let q = i.ntp_typed.changed(edit).trim().to_string();
                        if i.ntp_typed.ok && !q.is_empty() {
                            send(&i.proxy, json!({"t": "complete", "q": q}));
                        }
                    }
                    i.layout_edits();
                    i.invalidate();
                } else if id == ID_BM && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    // A new search starts at the top of the results.
                    if code == EN_CHANGE {
                        i.page.scroll = 0.0;
                    }
                    i.invalidate();
                } else if id == ID_DRIVE && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    i.invalidate();
                } else if id == ID_BUG && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    // The report below shows the words as they are typed.
                    i.invalidate();
                } else if id == ID_SET && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    // What matches starts at the top.
                    if code == EN_CHANGE {
                        i.page.scroll = 0.0;
                    }
                    i.invalidate();
                } else if id == ID_SRCH && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    // Typing filters the tab list; the pick starts over at the best match.
                    if code == EN_CHANGE {
                        i.search_sel = 0;
                    }
                    i.invalidate();
                } else if id == ID_COLOR && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    // A whole colour typed into the hex box moves the square with it.
                    if code == EN_CHANGE {
                        color::typed(&mut i);
                    }
                    i.invalidate();
                } else if (ID_FORM0..=ID_FILENAME).contains(&id) && code == EN_CHANGE {
                    // The value a form or the file card draws under its edit child.
                    i.invalidate();
                } else if id == ID_PAL && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    // Typing filters the palette; the pick starts over at the top row.
                    if code == EN_CHANGE {
                        i.pal_sel = 0;
                    }
                    i.invalidate();
                }
                LRESULT(0)
            }
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wp.0 as _);
                // The Bookmarks search sits in a field drawn in the sidebar's color, like the new tab one.
                let h = HWND(lp.0 as _);
                if h == i.ntp_edit {
                    let fill = i.theme.solid(ntp_bar_fill(&i.look));
                    let ink = if i.look.style == theme::Style::Signal { i.theme.pill_ink } else { i.theme.text };
                    let _ = SetTextColor(hdc, COLORREF(ink.colorref()));
                    let _ = SetBkColor(hdc, COLORREF(fill.colorref()));
                    return LRESULT(i.bar_brush.0 as isize);
                }
                // The composer's box sits in the page colour; every other box is on a card.
                let is_ntp = h == i.say_edit;
                let _ = SetTextColor(hdc, COLORREF(i.theme.text.colorref()));
                let bg = if is_ntp { i.theme.bg } else { i.theme.solid(i.theme.raise) };
                let _ = SetBkColor(hdc, COLORREF(bg.colorref()));
                LRESULT(if is_ntp { i.ntp_brush.0 as isize } else { i.edit_brush.0 as isize })
            }
            #[cfg(debug_assertions)]
            shot::WM_HOVER => {
                i.mouse_move(x, y);
                LRESULT(0)
            }
            #[cfg(debug_assertions)]
            shot::WM_SHOT => {
                shot::requested(&i);
                LRESULT(0)
            }
            WM_KILLFOCUS => {
                // A menu closes when the keyboard leaves the chrome (Alt-Tab, another
                // app). Focus moving to one of the chrome's own edit boxes is not
                // leaving: the menu stays.
                let gain = HWND(lp.0 as _);
                if i.menu.is_some() && gain != hwnd && !IsChild(hwnd, gain).as_bool() {
                    menu::blur(&mut i);
                }
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => {
                drop(i);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
    }
}

/// The glance card's dim overlay. A plain STATIC paints the light 3D face,
/// which brightens a dark page instead of dimming it: fill it black instead.
/// The layered-window alpha set at creation turns that into a wash.
extern "system" fn dim_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let old: WNDPROC = std::mem::transmute(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
        match msg {
            WM_ERASEBKGND => {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let _ = PatBlt(HDC(wp.0 as _), 0, 0, rc.right, rc.bottom, BLACKNESS);
                LRESULT(1)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let _ = PatBlt(hdc, 0, 0, rc.right, rc.bottom, BLACKNESS);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            _ => CallWindowProcW(old, hwnd, msg, wp, lp),
        }
    }
}

/// Edit box subclass: Enter, Escape and arrows drive the address bar; browser
/// shortcuts still work while typing.
extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let old: WNDPROC = std::mem::transmute(GetWindowLongPtrW(hwnd, GWLP_USERDATA));
        let parent = GetParent(hwnd).unwrap_or_default();
        let id = GetDlgCtrlID(hwnd) as usize;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let vk = wp.0 as u16;
            if let Some(cell) = inner_of(parent)
                && let Ok(mut i) = cell.try_borrow_mut()
            {
                if id == ID_EDIT {
                    // Workspace rename mode: Enter confirms, Escape cancels.
                    if let Some(ri) = i.renaming {
                        match vk {
                            v if v == VK_RETURN.0 => {
                                let name = wide_text(hwnd).trim().to_string();
                                if !name.is_empty() {
                                    send(&i.proxy, json!({"t": "workspaceRename", "i": ri, "name": name}));
                                }
                                i.renaming = None;
                                i.end_edit();
                                return LRESULT(0);
                            }
                            v if v == VK_ESCAPE.0 => {
                                i.renaming = None;
                                i.end_edit();
                                send(&i.proxy, json!({"t": "focusPage"}));
                                return LRESULT(0);
                            }
                            _ => {}
                        }
                    }
                    // A deletion takes the filled-in part away and must not bring it back.
                    if vk == VK_BACK.0 || vk == VK_DELETE.0 {
                        i.omni_typed.no_fill = true;
                    }
                    match vk {
                        v if v == VK_RETURN.0 => {
                            let q = wide_text(hwnd).trim().to_string();
                            if !q.is_empty() {
                                if i.sugg.is_empty() {
                                    send(&i.proxy, json!({"t": "nav", "url": q}));
                                    i.end_edit();
                                } else {
                                    let s = i.sel;
                                    i.pick(s);
                                }
                            }
                            return LRESULT(0);
                        }
                        v if v == VK_ESCAPE.0 => {
                            i.end_edit();
                            send(&i.proxy, json!({"t": "focusPage"}));
                            return LRESULT(0);
                        }
                        v if v == VK_DOWN.0 || v == VK_UP.0 => {
                            if !i.sugg.is_empty() {
                                let n = i.sugg.len();
                                i.sel = if v == VK_DOWN.0 { (i.sel + 1) % n } else { (i.sel + n - 1) % n };
                                i.invalidate();
                            }
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                } else if id == ID_FILEPATH || id == ID_FILENAME {
                    // The file card's boxes: Enter is Open or Save, Escape closes, Tab
                    // walks between the two.
                    match vk {
                        v if v == VK_RETURN.0 => {
                            files::go(&mut i);
                            return LRESULT(0);
                        }
                        v if v == VK_ESCAPE.0 => {
                            files::cancel(&mut i);
                            return LRESULT(0);
                        }
                        v if v == VK_TAB.0 => {
                            let target = if id == ID_FILEPATH { i.file_name_edit } else { i.file_path_edit };
                            let _ = SetFocus(Some(target));
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                } else if (ID_FORM0..=ID_FORM2).contains(&id) || id == ID_FORMPW {
                    // A form's box: Enter saves, Escape cancels, Tab walks the fields.
                    match vk {
                        v if v == VK_RETURN.0 => {
                            form::save(&mut i);
                            return LRESULT(0);
                        }
                        v if v == VK_ESCAPE.0 => {
                            form::cancel(&mut i);
                            return LRESULT(0);
                        }
                        v if v == VK_TAB.0 => {
                            let back = GetKeyState(VK_SHIFT.0 as i32) < 0;
                            form::tab(&mut i, back);
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                } else if id == ID_COLOR {
                    // The picker's hex box: Enter is Done, Escape puts the old colour back.
                    match vk {
                        v if v == VK_RETURN.0 => {
                            color::done(&mut i);
                            return LRESULT(0);
                        }
                        v if v == VK_ESCAPE.0 => {
                            color::cancel(&mut i);
                            return LRESULT(0);
                        }
                        v if v == VK_TAB.0 => {
                            // Tab leaves the box rather than cycling the hidden children.
                            let _ = SetFocus(Some(parent));
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                } else if (id == ID_BM || id == ID_SET || id == ID_BUG) && vk == VK_ESCAPE.0 {
                    // Escape clears the search first; with nothing typed it leaves the page.
                    if wide_text(hwnd).is_empty() {
                        send(&i.proxy, json!({"t": "panel", "open": false}));
                    } else {
                        SetWindowTextW(hwnd, &HSTRING::new()).ok();
                    }
                    return LRESULT(0);
                } else if id == ID_SAY {
                    match vk {
                        v if v == VK_RETURN.0 => {
                            i.send_say();
                            return LRESULT(0);
                        }
                        v if v == VK_ESCAPE.0 => {
                            i.close_say();
                            return LRESULT(0);
                        }
                        v if v == VK_TAB.0 => {
                            let back = GetKeyState(VK_SHIFT.0 as i32) < 0;
                            if let Some(say) = i.say.as_mut() {
                                say.mode = if back { (say.mode + 2) % 3 } else { (say.mode + 1) % 3 };
                            }
                            i.invalidate();
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                } else if id == ID_DRIVE {
                    // Enter in the task box starts the run; Escape leaves the page.
                    if vk == VK_RETURN.0 {
                        if !wide_text(hwnd).trim().is_empty() {
                            send(&i.proxy, json!({"t": "driveStart"}));
                        }
                        return LRESULT(0);
                    }
                    if vk == VK_ESCAPE.0 {
                        if wide_text(hwnd).is_empty() {
                            send(&i.proxy, json!({"t": "panel", "open": false}));
                        } else {
                            SetWindowTextW(hwnd, &HSTRING::new()).ok();
                        }
                        return LRESULT(0);
                    }
                } else if id == ID_SRCH {
                    match vk {
                        v if v == VK_RETURN.0 => {
                            let sel = i.search_sel;
                            i.switch_to(sel);
                            return LRESULT(0);
                        }
                        v if v == VK_ESCAPE.0 => {
                            i.close_search();
                            return LRESULT(0);
                        }
                        v if v == VK_DOWN.0 || v == VK_UP.0 => {
                            let n = i.search_rows.len();
                            if n > 0 {
                                i.search_sel = if v == VK_DOWN.0 { (i.search_sel + 1) % n } else { (i.search_sel + n - 1) % n };
                                i.invalidate();
                            }
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                } else if id == ID_NTP && (vk == VK_BACK.0 || vk == VK_DELETE.0) {
                    i.ntp_typed.no_fill = true;
                } else if id == ID_PAL {
                    match vk {
                        v if v == VK_RETURN.0 => {
                            let sel = i.pal_sel;
                            i.run_palette(sel);
                            return LRESULT(0);
                        }
                        v if v == VK_ESCAPE.0 => {
                            i.close_palette();
                            return LRESULT(0);
                        }
                        v if v == VK_DOWN.0 || v == VK_UP.0 => {
                            let n = i.pal_rows.len();
                            if n > 0 {
                                i.pal_sel = if v == VK_DOWN.0 { (i.pal_sel + 1) % n } else { (i.pal_sel + n - 1) % n };
                                i.invalidate();
                            }
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                } else if id == ID_NTP && vk == VK_RETURN.0 {
                    let q = wide_text(hwnd).trim().to_string();
                    if !q.is_empty() {
                        send(&i.proxy, json!({"t": "nav", "url": q}));
                    }
                    return LRESULT(0);
                }
                if vk == VK_MENU.0 || vk == VK_F10.0 {
                    return LRESULT(0);
                }
                // Browser shortcuts win over the edit box, except the edit box's own.
                let down = |k: u16| GetKeyState(k as i32) < 0;
                if let Some(a) = crate::engine::map_key(vk as u32, down(VK_CONTROL.0), down(VK_SHIFT.0), down(VK_MENU.0)) {
                    let _ = i.proxy.send_event(Ev::Key(a));
                    return LRESULT(0);
                }
            }
        }
        // Swallow the beep from Enter and Escape.
        if msg == WM_CHAR && (wp.0 == 13 || wp.0 == 27) {
            return LRESULT(0);
        }
        if (msg == WM_SYSKEYUP || msg == WM_SYSCHAR) && (wp.0 as u16 == VK_MENU.0 || wp.0 as u16 == VK_F10.0) {
            return LRESULT(0);
        }
        CallWindowProcW(old, hwnd, msg, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(kind: SuggKind, label: &str) -> Sugg {
        Sugg { kind, label: label.to_string(), detail: String::new() }
    }

    fn hit(url: &str, title: &str, bm: bool) -> (String, String, bool, String) {
        (url.to_string(), title.to_string(), bm, String::new())
    }

    fn tab(id: u64, title: &str, url: &str, private: bool) -> UiTab {
        UiTab {
            id,
            title: title.into(),
            url: url.into(),
            loading: false,
            back: false,
            fwd: false,
            pinned: false,
            pin_away: false,
            audio: false,
            muted: false,
            call: false,
            status: "live",
            agent: false,
            agent_ok: false,
            reader: false,
            private,
            split: false,
            translated: false,
            translating: false,
            servo: false,
            owner: String::new(),
            owner_name: String::new(),
        }
    }

    /// The rows the address bar shows: "Switch to tab" sits over history, and no address is
    /// offered twice.
    #[test]
    fn switch_to_tab_rows_stay_above_history() {
        let mut list = vec![
            s(SuggKind::Go("doc.rust-lang.org".into()), "doc.rust-lang.org"),
            s(SuggKind::Search("rust".into()), "rust"),
            s(SuggKind::Tab(7), "The Rust Book"),
        ];
        merge_hits(&mut list, vec![hit("https://news.ycombinator.com/", "Hacker News", false)]);
        assert!(matches!(list[2].kind, SuggKind::Tab(7)));
        assert!(matches!(list[3].kind, SuggKind::Url(_, false)));
        assert_eq!(list[3].label, "Hacker News");
        // A second answer (the page-text worker) lands under the first.
        merge_hits(&mut list, vec![hit("https://doc.rust-lang.org/book/", "The Book", false)]);
        assert!(matches!(list[4].kind, SuggKind::Url(_, false)));
        // A hit for an address the list already offers as "Open" is not repeated.
        merge_hits(&mut list, vec![hit("doc.rust-lang.org", "already there", false)]);
        assert_eq!(list.len(), 5);
        // A workspace tab counts as a switch row too: history stays under it.
        let mut list = vec![s(SuggKind::TabOther(1, 2), "Mail (Work)")];
        merge_hits(&mut list, vec![hit("https://mail.example/", "Mail", true)]);
        assert!(matches!(list[0].kind, SuggKind::TabOther(1, 2)));
        assert!(matches!(list[1].kind, SuggKind::Url(_, true)));
    }

    /// The tab search covers every workspace, the sidebar's order when nothing is typed, and
    /// marks private tabs.
    #[test]
    fn the_tab_search_covers_every_workspace() {
        let tabs = [tab(1, "Docs", "https://docs.test/", false), tab(2, "Inbox", "https://mail.test/", true)];
        let other = [UiOther { ws: 2, idx: 0, ws_name: "Work".into(), title: "Standup notes".into(), url: "https://notes.test/".into() }];
        let all = switch_candidates(&tabs, &other);
        assert_eq!(all.len(), 3);
        assert_eq!(all[2].other, Some((2, 0)));
        assert!(all[1].private);
        // Nothing typed: every tab, in the order the sidebar has them.
        let rows = rank_rows(&all, "");
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2, 0]);
        assert_eq!(rows[2].label(), "Standup notes");
        // Typing keeps only what fits.
        let rows = rank_rows(&all, "standup");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ws_name, "Work");
        assert!(rank_rows(&all, "docs").iter().any(|r| r.id == 1));
        assert!(rank_rows(&all, "docs zebra").is_empty());
    }
}
