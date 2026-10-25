//! Native browser chrome.

mod gfx;
pub mod layout;
mod motion;
mod pages;
mod report;
mod dashboard;
mod doctor;
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

pub use layout::{Kind, Metrics};
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

const MEM_WARN: u64 = 1 << 30;
const MEM_HOT: u64 = 2 << 30;
const MEM_BREATH: f32 = 4.0;

fn mem_short(bytes: u64) -> String {
    let mb = bytes as f64 / 1048576.0;
    if mb >= 1024.0 {
        format!("{:.1}G", mb / 1024.0)
    } else {
        format!("{}M", mb.round() as u64)
    }
}

#[derive(Clone, Debug)]
pub struct UiTab {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub loading: bool,
    pub back: bool,
    pub fwd: bool,
    pub pinned: bool,
    pub pin_away: bool,
    pub essential: bool,
    pub audio: bool,
    pub muted: bool,
    pub call: bool,
    pub status: &'static str,
    pub agent: bool,
    pub agent_ok: bool,
    pub locked: bool,
    pub veiled: bool,
    pub reader: bool,
    pub private: bool,
    pub split: bool,
    pub translated: bool,
    pub translating: bool,
    pub servo: bool,
    pub owner: String,
    pub owner_name: String,
}

#[derive(Clone, Debug)]
pub struct UiEssential {
    pub url: String,
    pub title: String,
    pub away: bool,
    pub tab: u64,
}

#[derive(Clone, Debug)]
pub struct UiArchived {
    pub url: String,
    pub title: String,
    pub when: String,
}

#[derive(Clone, Debug)]
pub struct UiWorkspace {
    pub name: String,
    pub colour: String,
}

pub const WORKSPACE_COLOURS: [(&str, f32); 8] = [
    ("Red", 25.0),
    ("Orange", 60.0),
    ("Green", 145.0),
    ("Teal", 185.0),
    ("Blue", 250.0),
    ("Violet", 300.0),
    ("Pink", 350.0),
    ("Brown", 75.0),
];

pub const HEAVY_TAB: u64 = 500 * 1024 * 1024;

fn mem_label(b: u64) -> String {
    let mb = b as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{} MB", mb.round() as u64) }
}

pub fn workspace_colour(dark: bool, colour: &str) -> Option<theme::Color> {
    let n: usize = colour.parse().ok()?;
    if n >= WORKSPACE_COLOURS.len() {
        return None;
    }
    let (_, h) = WORKSPACE_COLOURS[n];
    Some(theme::oklch(if dark { 0.72 } else { 0.55 }, 0.13, h))
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
    pub kind: &'static str,
    pub up: u64,
    pub peers: u64,
    pub seeds: u64,
    pub ratio: f64,
}

#[derive(Clone, Debug)]
pub struct UiSecret {
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct UiLock {
    pub app: bool,
    pub hello: Option<bool>,
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Popup,
    Redirect,
}

#[derive(Clone, Debug)]
pub struct UiPopup {
    pub tab: u64,
    pub kind: NoteKind,
    pub target: String,
    pub site: String,
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
    pub full: bool,
    pub stage: (f32, f32, f32, f32),
    pub mem: Option<(u64, u32)>,
    pub media: Option<u64>,
    pub blocked: u64,
    pub lock: Option<UiLock>,
    pub cpu_cap: bool,
    pub battery: bool,
    pub page_ask: Option<UiAsk>,
    pub popup_note: Option<UiPopup>,
    pub downloads: Vec<UiDl>,
    pub secret_open: bool,
    pub secret: Vec<UiSecret>,
    pub agent_busy: bool,
    pub agent_notes: Vec<(String, String)>,
    pub agent_cmd: String,
    pub prompt: Option<(u64, String, &'static str)>,
    pub handoff: Option<(String, String)>,
    pub rules: Vec<(String, String, bool)>,
    pub workspaces: Vec<UiWorkspace>,
    pub essentials: Vec<UiEssential>,
    pub archived: Vec<UiArchived>,
    pub active_workspace: usize,
    pub glance: bool,
    pub bookmarked: bool,
    pub suggestions: Vec<Shortcut>,
    pub mark: Option<UiMark>,
    pub bm_folders: Vec<String>,
    pub listen: &'static str,
    pub kdbx: Vec<String>,
    pub vault: usize,
    pub history: Vec<UiVisit>,
    pub bookmarks: Vec<UiMark>,
    pub report: Option<crate::privacy_stats::Snapshot>,
    pub privacy_today: Option<crate::privacy_stats::Counts>,
    pub dl_unseen: bool,
    pub drive: Option<DriveView>,
    pub doctor: Option<DoctorView>,
    pub bug_report: Option<String>,
    pub bug_sent: bool,
    pub bug_kind: &'static str,
    pub unclean_last: bool,
    pub upnp_port_open: bool,
    pub zoom: u32,
    pub zooms: Vec<(String, u32)>,
    pub site_cookies: Option<usize>,
    pub other_tabs: Vec<UiOther>,
}

#[derive(Clone, Debug)]
pub struct UiOther {
    pub ws: usize,
    pub idx: usize,
    pub ws_name: String,
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug)]
pub struct DoctorView {
    pub report: Option<crate::doctor::Report>,
}

#[derive(Clone, Debug, Default)]
pub struct DriveView {
    pub name: String,
    pub running: bool,
    pub paused: bool,
    pub task: String,
    pub elapsed: String,
    pub steps: Vec<(String, Option<u64>)>,
    pub log: Vec<String>,
    pub exit: Option<i32>,
}

struct AgentGroup<'a> {
    head: &'a UiTab,
    tabs: Vec<&'a UiTab>,
}

enum Row<'a> {
    Tab(&'a UiTab),
    Header(&'a UiTab),
}

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
    fn ws_h(&self) -> f32 {
        if self.compact() || (self.workspaces.len() < 2 && !self.glance) { 0.0 } else { 32.0 }
    }
    pub fn sidebar_w(&self) -> f32 {
        if self.compact() { 56.0 } else { self.settings.sidebar_width as f32 }
    }
}

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
    TabPip(u64),
    MediaPlay,
    MediaNext,
    MediaTitle(u64),
    NewTab,
    Gear,
    Sidebar,
    Mem,
    DlFooter,
    DriveFooter,
    AgentStop,
    AgentStopSession(String),
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
    Essential(usize),
    PinHome(u64),
    DialSugg(usize),
    DialPin(usize),
    DialAdd,
    NtpSearch,
    Page(pages::PageHit),
    Report(report::Click),
    Dash(dashboard::Click),
    DashFooter,
    Workspace(usize),
    GlanceOpen,
    WorkspaceAdd,
    Menu,
    WsChip,
    SiteInfo,
    SitePerm(String, &'static str),
    SiteClear,
    SiteSettings,
    SiteAdblock,
    Unlock(bool),
    ZoomChip,
    TabSearch(usize),
    SearchCard,
    Pal(usize),
    PaletteCard,
    SayCard,
    SayMode(usize),
    SaySend,
    SayClose,
    ColorCard,
    ColorArea,
    ColorHue,
    ColorPreset(usize),
    ColorRecent(usize),
    ColorDone,
    FormCard,
    FormSave,
    FormCancel,
    ConfirmCard,
    ConfirmButton(usize),
    MenuCard,
    MenuRow(usize),
    FileCard,
    FileRow(usize),
    FilePlace(usize),
    FileDrive(usize),
    FileUp,
    FileGo,
    FileCancel,
    FilePath,
    FileName,
    PopupAllow(u64),
    PopupDismiss(u64),
    RedirectGo(u64),
}

#[derive(Clone, Debug)]
struct Sugg {
    kind: SuggKind,
    label: String,
    detail: String,
}

#[derive(Clone, Debug, PartialEq)]
enum SuggKind {
    Go(String),
    Search(String),
    Tab(u64),
    TabOther(usize, usize),
    Url(String, bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cell {
    Tile(usize),
    Sugg(usize),
    Add,
}

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

fn page_ground(lo: &Look) -> theme::Color {
    if lo.style == theme::Style::Glass { theme::Color(0.05, 0.04, 0.09, 0.55) } else { lo.theme.bg }
}

const NTP_CARD_H: f32 = 112.0;

fn ntp_bar_fill(lo: &Look) -> theme::Color {
    match lo.style {
        theme::Style::Signal => lo.theme.pill,
        theme::Style::Glass => theme::Color(1.0, 1.0, 1.0, 0.12),
        theme::Style::Precise => lo.theme.raise,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DialGrid {
    Classic(speeddial::Grid),
    Sized { cols: usize, cell_w: f32, n: usize, x0: f32, row_h: f32 },
}

impl DialGrid {
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
    top: f32,
    cells: Vec<Cell>,
}

fn dial_cells(s: &UiState) -> Vec<Cell> {
    if !s.settings.speed_dial {
        return Vec::new();
    }
    let mine = s.settings.shortcuts.len().min(speeddial::MAX_TILES);
    let mut cells: Vec<Cell> = if mine > 0 { (0..mine).map(Cell::Tile).collect() } else { (0..s.suggestions.len().min(5)).map(Cell::Sugg).collect() };
    if mine < speeddial::MAX_TILES {
        cells.push(Cell::Add);
    }
    cells
}

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
    pub(crate) look: Look,
    pub(crate) state: Option<UiState>,
    hits: Vec<(R, Hit)>,
    hover: Option<Hit>,
    press: Option<Hit>,
    scroll: f32,
    list_max: f32,
    drag: Option<Drag>,
    favs: HashMap<u64, Vec<u8>>,
    site_favs: HashMap<String, Vec<u8>>,
    floats: Vec<(R, f32)>,
    float_layer: Vec<[i32; 5]>,
    disk_favs: HashMap<String, (Option<Vec<u8>>, std::time::Instant, std::time::Instant)>,
    fav_asked: std::collections::HashSet<String>,
    dial_drag: Option<DialDrag>,
    edit: HWND,
    ntp_edit: HWND,
    bm_edit: HWND,
    drive_edit: HWND,
    drive_focus: bool,
    edit_font: HFONT,
    edit_face: String,
    ntp_font: HFONT,
    edit_brush: HBRUSH,
    ntp_brush: HBRUSH,
    bar_brush: HBRUSH,
    editing: bool,
    focus_edit: bool,
    sugg: Vec<Sugg>,
    sel: usize,
    omni_typed: Typed,
    ntp_typed: Typed,
    page_ask_shown: Option<u64>,
    link: String,
    tab_mem: HashMap<u64, u64>,
    born: Instant,
    spinning: bool,
    mem_pulse: bool,
    side_shown: bool,
    side_at_edge: bool,
    side_over: bool,
    side_forced: bool,
    side_slide: f32,
    motion: Motion,
    animating: bool,
    pub(crate) page: pages::Page,
    theme_key: String,
    mouse: (f32, f32),
    glance_dim: Option<HWND>,
    renaming: Option<usize>,
    closing: Vec<Closing>,
    seen_tabs: std::collections::HashSet<u64>,
    last_ws: usize,
    wallpaper: Option<(String, Vec<u8>)>,
    fonts: Rc<Vec<String>>,
    clock_period: u32,
    site_open: bool,
    search_open: bool,
    search_edit: HWND,
    search_sel: usize,
    search_rows: Vec<SwitchRow>,
    say: Option<Say>,
    say_edit: HWND,
    pal_open: bool,
    palette_edit: HWND,
    pal_sel: usize,
    pal_rows: Vec<PalRow>,
    pal_jump: Option<String>,
    set_edit: HWND,
    bug_edit: HWND,
    color_edit: HWND,
    color: Option<color::ColorCard>,
    form_edits: [HWND; 3],
    form_secret_edit: HWND,
    form: Option<form::FormCard>,
    confirm: Option<confirm::ConfirmCard>,
    menu: Option<menu::MenuCard>,
    file_path_edit: HWND,
    file_name_edit: HWND,
    files: Option<files::FileCard>,
}

struct Say {
    session: String,
    name: String,
    mode: usize,
}

const SAY_MODES: [(&str, &str, &str); 3] = [
    ("Steer now", "now", "It reads this first, on its next call."),
    ("Queue", "later", "For after its current task."),
    ("New task", "new", "Starts a new Drive run."),
];

const FLOAT_MIN_W: f32 = 248.0;

fn float_col(area: R, right: bool) -> (f32, f32) {
    let w = (area.w - 16.0).max(FLOAT_MIN_W);
    (if right { area.right() - 8.0 - w } else { area.x + 8.0 }, w)
}

fn say_card(area: R, right: bool) -> R {
    let (x, w) = float_col(area, right);
    R::new(x, area.y + 8.0, w, 168.0)
}

struct Closing {
    tab: UiTab,
    y: f32,
    at: Instant,
}

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
            (true, h) if h.is_empty() => "Start".into(),
            (true, h) => h,
        }
    }
}

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

static MEDIA_KEYS: Mutex<Option<(usize, EventLoopProxy<Ev>)>> = Mutex::new(None);

unsafe extern "system" fn media_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_APPCOMMAND {
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

const SEARCH_ROWS: usize = 9;

fn switch_rows(s: &UiState, q: &str) -> Vec<SwitchRow> {
    let mut all = switch_candidates(&s.tabs, &s.other_tabs);
    let ws_name = s.workspaces.get(s.active_workspace).map(|w| w.name.clone()).unwrap_or_default();
    for row in &mut all {
        if row.other.is_none() {
            row.ws_name = ws_name.clone();
        }
    }
    rank_rows(&all, q)
}

fn switch_candidates(tabs: &[UiTab], other: &[UiOther]) -> Vec<SwitchRow> {
    let mut all: Vec<SwitchRow> = tabs
        .iter()
        .map(|t| {
            let (title, url) = if t.veiled { ("Locked tab".to_string(), String::new()) } else { (t.title.clone(), t.url.clone()) };
            SwitchRow { id: t.id, other: None, ws_name: String::new(), title, url, private: t.private }
        })
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

fn rank_rows(all: &[SwitchRow], q: &str) -> Vec<SwitchRow> {
    let order = {
        let entries: Vec<crate::tabsearch::Entry> = all.iter().map(|r| crate::tabsearch::Entry { title: &r.title, url: &r.url }).collect();
        crate::tabsearch::rank(q, &entries)
    };
    order.into_iter().map(|i| all[i].clone()).collect()
}

fn search_card(area: R, right: bool, rows: usize, more: bool) -> R {
    let h = 52.0 + rows as f32 * 32.0 + if rows == 0 { 34.0 } else { 8.0 } + if more { 20.0 } else { 0.0 };
    let (x, w) = float_col(area, right);
    R::new(x, area.y + 8.0, w, h.min(area.h - 16.0))
}

const PAL_ROWS: usize = 12;

#[derive(Clone)]
struct PalRow {
    group: Option<&'static str>,
    label: String,
    detail: String,
    run: PalRun,
}

#[derive(Clone)]
enum PalRun {
    Msg(Value),
    Set(String),
    Switch { id: u64, other: Option<(usize, usize)> },
    Open(String),
}

fn pal_words(row: &PalRow, q: &str) -> bool {
    let hay = format!("{} {}", row.label, row.detail).to_lowercase();
    q.split_whitespace().all(|w| hay.contains(w))
}

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

fn pal_view(s: &UiState, q: &str) -> (Vec<PalRow>, usize, usize) {
    let all = pal_rows(s, q);
    let total = all.len();
    let mut rows = all;
    rows.truncate(PAL_ROWS);
    let groups = rows.iter().filter(|r| r.group.is_some()).count();
    (rows, groups, total)
}

fn pal_card(area: R, right: bool, rows: usize, groups: usize, more: bool) -> R {
    let h = 56.0 + groups as f32 * 18.0 + rows as f32 * 30.0 + if rows == 0 { 34.0 } else { 0.0 } + if more { 20.0 } else { 0.0 };
    let (x, w) = float_col(area, right);
    R::new(x, area.y + 8.0, w, h.min(area.h - 16.0))
}

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

#[derive(Clone, Debug)]
pub struct UiAsk {
    pub aid: u64,
    pub host: String,
    pub ask: crate::engine::Ask,
    pub many: bool,
}

#[derive(Default)]
struct Typed {
    text: String,
    no_fill: bool,
    ok: bool,
}

impl Typed {
    fn changed(&mut self, edit: HWND) -> String {
        let raw = wide_text(edit);
        let (start, end) = edit_sel(edit);
        let at_end = start == end && end == raw.encode_utf16().count();
        self.ok = !self.no_fill && at_end && raw.chars().count() > self.text.chars().count();
        self.no_fill = false;
        self.text = raw.clone();
        raw
    }

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

fn edit_sel(hwnd: HWND) -> (usize, usize) {
    let (mut start, mut end) = (0u32, 0u32);
    unsafe {
        SendMessageW(hwnd, EM_GETSEL, Some(WPARAM(&mut start as *mut u32 as usize)), Some(LPARAM(&mut end as *mut u32 as isize)));
    }
    (start as usize, end as usize)
}

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
            let cue = HSTRING::from("Find the cheapest X and put it in the cart, stop before paying");
            let _ = SendMessageW(drive_edit, EM_SETCUEBANNER, Some(WPARAM(1)), Some(LPARAM(cue.as_ptr() as isize)));
            let dpi = GetDpiForWindow(hwnd) as f32;
            let gfx = Gfx::new(hwnd, dpi).expect("direct2d");
            let fonts = Rc::new(gfx.installed_families().to_vec());
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
                tab_mem: HashMap::new(),
                born: Instant::now(),
                spinning: false,
                mem_pulse: false,
                side_shown: false,
                side_at_edge: false,
                side_over: false,
                side_forced: false,
                side_slide: 0.0,
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
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(&inner) as isize);
            inner.borrow_mut().make_fonts();
            Chrome { hwnd, inner }
        }
    }

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

    pub fn bounds_for(_kind: Kind, w: f64, h: f64) -> (f64, f64, f64, f64) {
        (0.0, 0.0, w, h)
    }

    pub fn bg(&self) -> (u8, u8, u8) {
        self.inner.try_borrow().map(|i| i.look.theme.bg.rgb8()).unwrap_or((24, 24, 26))
    }

    pub fn look(&self) -> Look {
        self.inner.try_borrow().map(|i| i.look.clone()).unwrap_or_default()
    }

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

    pub fn set_glance(&self, on: bool) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if let Some(s) = i.state.as_mut() {
            s.glance = on;
        }
        if on {
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

    pub fn drive_task(&self) -> String {
        self.inner.try_borrow().map(|i| wide_text(i.drive_edit)).unwrap_or_default()
    }

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
            i.sugg.retain(|s| !matches!(&s.kind, SuggKind::Go(_)));
            i.sugg.insert(0, Sugg { kind: SuggKind::Go(f.clone()), label: f, detail: String::new() });
            i.sel = 0;
        }
        i.invalidate();
    }

    pub fn set_ntp_fill(&self, q: &str, fill: Option<String>) {
        let Ok(i) = self.inner.try_borrow_mut() else { return };
        if let Some(f) = fill
            && i.ntp_typed.text.trim() == q
        {
            i.ntp_typed.fill(i.ntp_edit, q, &f);
        }
    }

    pub fn set_tab_mem(&self, m: HashMap<u64, u64>) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        let heavy = |m: &HashMap<u64, u64>| {
            let mut v: Vec<u64> = m.iter().filter(|e| *e.1 >= HEAVY_TAB).map(|e| *e.0).collect();
            v.sort_unstable();
            v
        };
        let changed = heavy(&i.tab_mem) != heavy(&m);
        i.tab_mem = m;
        if changed {
            i.invalidate();
        }
    }

    pub fn usage(&self, v: Value) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        i.page.usage = Some(v);
        if i.state.as_ref().is_some_and(|s| s.panel == "control" || s.panel == "dashboard") {
            i.invalidate();
        }
    }

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

    pub fn open_message(&self, session: &str, name: &str) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        i.open_say(session.to_string(), name.to_string());
    }

    pub fn file_list(&self, m: &serde_json::Value) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        files::list_landed(&mut i, m);
    }

    pub fn ask_external(&self, origin: &str, scheme: &str, remember: bool, uri: &str) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        confirm::open(&mut i, confirm::ConfirmKind::External {
            origin: origin.to_string(),
            uri: uri.to_string(),
            remember,
            scheme: scheme.to_string(),
        });
    }

    pub fn open_tab_search(&self) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if i.search_open {
            i.close_search();
            return;
        }
        i.close_palette();
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
        i.invalidate();
    }

    pub fn open_palette(&self) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if i.pal_open {
            i.close_palette();
            return;
        }
        i.close_search();
        i.close_say();
        i.site_open = false;
        if i.state.as_ref().is_some_and(|s| s.settings.compact) {
            send(&i.proxy, json!({"t": "peek", "on": true}));
        }
        i.pal_open = true;
        i.pal_sel = 0;
        unsafe {
            SetWindowTextW(i.palette_edit, &HSTRING::from("")).ok();
        }
        send(&i.proxy, json!({"t": "palOpen", "open": true}));
        i.layout_edits();
        unsafe {
            let _ = SetFocus(Some(i.palette_edit));
        }
        i.invalidate();
    }

    pub fn show_sidebar(&self) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        i.side_forced = true;
        i.side_shown = true;
        i.invalidate();
    }
}

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
        #[cfg(debug_assertions)]
        self.tab_mem.extend(shot::fake_mem());
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
        let want_wp = s.settings.ntp_bg == "wallpaper" && !s.settings.wallpaper.is_empty();
        let held = self.wallpaper.as_ref().map(|(p, _)| p.clone());
        if !want_wp {
            self.wallpaper = None;
        } else if held.as_deref() != Some(s.settings.wallpaper.as_str()) {
            let path = s.settings.wallpaper.clone();
            self.gfx.bitmaps.remove(&wallpaper_key(&path));
            self.wallpaper = std::fs::read(&path).ok().filter(|b| !b.is_empty()).map(|bytes| (path, bytes));
        }
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
        let page_changed = self.state.as_ref().map_or("none", |o| o.panel) != s.panel;
        if page_changed {
            self.page.scroll = 0.0;
            unsafe {
                SetWindowTextW(self.bm_edit, &HSTRING::new()).ok();
                SetWindowTextW(self.set_edit, &HSTRING::new()).ok();
                SetWindowTextW(self.bug_edit, &HSTRING::new()).ok();
            }
            self.page.bm_collapsed.clear();
            if s.panel == "drive" {
                self.drive_focus = true;
            }
            self.page.font_pick = None;
            self.page.theme_name.clear();
        }
        if s.panel == "settings" {
            if let Some(words) = self.pal_jump.take() {
                unsafe {
                    SetWindowTextW(self.set_edit, &HSTRING::from(words)).ok();
                }
            }
        } else if page_changed {
            self.pal_jump = None;
        }
        if let Some(d) = s.drive.as_ref().filter(|d| !d.task.is_empty())
            && wide_text(self.drive_edit).is_empty()
            && unsafe { GetFocus() } != self.drive_edit
        {
            unsafe {
                SetWindowTextW(self.drive_edit, &HSTRING::from(d.task.as_str())).ok();
            }
        }
        #[cfg(debug_assertions)]
        if page_changed
            && std::env::var("LUMEN_TEST_PANEL").map(|p| p == s.panel).unwrap_or(false)
            && let Some(px) = std::env::var("LUMEN_TEST_SCROLL").ok().and_then(|v| v.parse::<f32>().ok())
            && px > 0.0
        {
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
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_CLOCK);
            }
            self.clock_timer();
        }
        self.layout_edits();
        if self.focus_edit && self.editing && self.state.as_ref().is_some_and(|s| !s.compact()) {
            self.focus_edit = false;
            unsafe {
                let _ = SetFocus(Some(self.edit));
                SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            }
        }
        self.spin();
        self.mem_timer();
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
        #[cfg(debug_assertions)]
        if self.color.is_none() && self.form.is_none() && self.confirm.is_none() && self.files.is_none() && self.menu.is_none() {
            match std::env::var("LUMEN_TEST_CARD").as_deref() {
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

    fn metrics(&self) -> layout::Metrics {
        self.look.metrics()
    }

    fn float_area(&self) -> Option<R> {
        if let Some(sb) = self.sidebar() {
            return Some(sb);
        }
        self.state.as_ref()?;
        let (w, h) = self.size_dip();
        let top = self.metrics().top();
        Some(R::new(0.0, top, (FLOAT_MIN_W + 16.0).min(w), (h - top).max(1.0)))
    }

    fn cards_right(&self) -> bool {
        self.look.kind.sidebar() && self.state.as_ref().is_some_and(|s| s.settings.sidebar_side == "right")
    }

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
            SetWindowRgn(self.hwnd, Some(rgn), true);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags);
        }
    }

    fn sidebar(&self) -> Option<R> {
        let s = self.state.as_ref()?;
        if !self.look.kind.sidebar() {
            return None;
        }
        let (w, h) = self.size_dip();
        let sw = s.sidebar_w();
        let right = s.settings.sidebar_side == "right";
        let mut x = if right { (w - sw).max(0.0) } else { 0.0 };
        if self.side_slide < 1.0 {
            let away = if right { w } else { -sw };
            x = away + (x - away) * self.side_slide;
        }
        match self.look.kind {
            Kind::Hybrid => {
                let top = self.metrics().top();
                Some(R::new(x, top, sw, (h - top).max(1.0)))
            }
            _ => Some(R::new(x, 0.0, sw, h)),
        }
    }

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

    fn ntp_geometry(&self) -> NtpGeo {
        let s = self.state.as_ref().unwrap();
        let lo = &self.look;
        let (x, y, w, h) = s.stage;
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

    fn ntp_head_h(&self) -> f32 {
        let s = self.state.as_ref().unwrap();
        let big = self.ntp_clock_font(s.stage.2).size10 as f32 / 10.0;
        big * 0.92 + 40.0
    }

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
        let edit_on = self.editing && (!s.compact() || self.look.kind.top_toolbar());
        place(self.edit, R::new(o.x + 32.0, o.y + (o.h - text_h) / 2.0, o.w - 42.0, text_h), edit_on);
        let f = self.ntp_field();
        let ntp_live = unsafe { GetFocus() } == self.ntp_edit || !wide_text(self.ntp_edit).is_empty();
        let box_on = self.ntp_visible() && self.look.ntp_search;
        place(self.ntp_edit, R::new(f.x + 52.0, f.y + (f.h - 22.0) / 2.0, f.w - 130.0, 22.0), box_on && ntp_live);
        let b = pages::bm_search_rect(R::new(s.stage.0, s.stage.1, s.stage.2, s.stage.3), self.look.s5);
        place(self.bm_edit, R::new(b.x + 34.0, b.y + (b.h - 20.0) / 2.0, b.w - 46.0, 20.0), s.full && s.panel == "bookmarks");
        let d = pages::drive_task_rect(R::new(s.stage.0, s.stage.1, s.stage.2, s.stage.3), self.look.s5);
        let show = s.full && s.panel == "drive";
        place(self.drive_edit, R::new(d.x + 8.0, d.y + (d.h - 22.0) / 2.0, d.w - 16.0, 22.0), show);
        let q = pages::set_search_rect(R::new(s.stage.0, s.stage.1, s.stage.2, s.stage.3));
        place(self.set_edit, R::new(q.x + 34.0, q.y + (q.h - 20.0) / 2.0, q.w - 46.0, 20.0), s.full && s.panel == "settings" && self.page.font_pick.is_none());
        let b = pages::bug_story_rect(R::new(s.stage.0, s.stage.1, s.stage.2, s.stage.3), self.look.s5);
        place(self.bug_edit, R::new(b.x + 16.0, b.y + (b.h - 22.0) / 2.0, b.w - 32.0, 22.0), s.full && s.panel == "bugreport");
        match &self.color {
            Some(c) => place(self.color_edit, R::new(c.hex_box().x + 8.0, c.hex_box().y + 4.0, c.hex_box().w - 16.0, 18.0), true),
            None => place(self.color_edit, R::default(), false),
        }
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
        if show && std::mem::take(&mut self.drive_focus) {
            unsafe {
                let _ = SetFocus(Some(self.drive_edit));
            }
        }
        if let Some(sb) = self.float_area() {
            let right = self.cards_right();
            let q = wide_text(self.search_edit).trim().to_lowercase();
            let n = switch_rows(s, &q).len();
            let card = search_card(sb, right, n.min(SEARCH_ROWS), n > SEARCH_ROWS);
            place(self.search_edit, R::new(card.x + 38.0, card.y + 13.0, card.w - 52.0, 22.0), self.search_open);
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

    fn run_palette(&mut self, n: usize) {
        let Some(row) = self.pal_rows.get(n).cloned() else { return };
        match row.run {
            PalRun::Msg(v) => send(&self.proxy, v),
            PalRun::Set(words) => {
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
        if self.state.as_ref().is_some_and(|s| s.settings.compact || s.peek) {
            send(&self.proxy, json!({"t": "peek", "on": false}));
        }
        self.layout_edits();
        self.invalidate();
    }

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
        static PROFILE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *PROFILE.get_or_init(|| std::env::var_os("LUMEN_PROFILE_PAINT").is_some()) {
            eprintln!("PAINT begin {:.1} draw {:.1} end {:.1}", (t1 - t0).as_secs_f64() * 1e3, (t2 - t1).as_secs_f64() * 1e3, t2.elapsed().as_secs_f64() * 1e3);
        }
        self.sync_float_layer(&s);
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

    fn draw_all(&mut self, s: &UiState) {
        let lo = self.look.clone();
        let th = lo.theme;
        self.hits.clear();
        self.floats.clear();
        if lo.style == theme::Style::Glass {
            let (w, h) = self.size_dip();
            self.gfx.bloom(w * 0.08, -h * 0.1, w.max(h) * 0.72, theme::Color::from_hex("#5a43e6").unwrap_or(th.accent).alpha(0.95));
            self.gfx.bloom(w * 1.02, h * 1.08, w.max(h) * 0.62, theme::Color::from_hex("#d2457e").unwrap_or(th.accent).alpha(0.85));
        }
        self.motion.set_config(lo.motion);
        self.motion.begin();
        if let Some(l) = s.lock.as_ref().filter(|l| l.app) {
            let (w, h) = self.size_dip();
            let all = R::new(0.0, 0.0, w, h);
            if lo.style != theme::Style::Glass {
                self.gfx.fill(all, page_ground(&lo));
            }
            self.draw_lock(all, l);
            self.hit(R::new(0.0, 0.0, w, 40.0), Hit::Drag);
            self.window_buttons(w - 8.0, 6.0, 30.0, s);
            self.motion.end();
            return;
        }
        self.slide_sidebar(s);
        if self.last_ws != s.active_workspace {
            self.last_ws = s.active_workspace;
            self.motion.restart(Id::Workspace(s.active_workspace), Prop::Opacity, 0.0, 1.0, 150, Ease::OutCubic);
        }
        self.closing.retain(|c| c.at.elapsed().as_secs_f32() < 0.5);
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
            self.gfx.round(st, lo.page_radius, page_ground(&lo));
            if s.panel != "none" {
                let a = self.motion.enter(Id::Panel(s.panel), Prop::Opacity, 0.0, 1.0, 170, Ease::OutCubic);
                let rise = self.motion.enter(Id::Panel(s.panel), Prop::Y, 8.0, 0.0, 170, Ease::OutCubic);
                let st = R::new(st.x, st.y + rise, st.w, st.h);
                pages::draw(self, st, s);
                if a < 1.0 {
                    self.gfx.round(st, lo.page_radius, page_ground(&lo).alpha(1.0 - a));
                }
            } else if let Some(l) = &s.lock {
                self.draw_lock(st, l);
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
        if lo.page_border && !s.full {
            let (x, y, w, h) = s.stage;
            let st = R::new(x - 1.0, y - 1.0, w + 2.0, h + 2.0);
            self.gfx.round_stroke(st, lo.page_radius + 1.0, th.line, 1.0);
        }
        self.draw_link(s);
        self.draw_popup_note(s);
        if self.color.is_some() {
            let f = self.card_in("color");
            color::draw(self, s);
            self.card_out(f);
        }
        if self.form.is_some() {
            let f = self.card_in("form");
            form::draw(self, s);
            self.card_out(f);
        }
        if self.files.is_some() {
            let f = self.card_in("files");
            files::draw(self, s);
            self.card_out(f);
        }
        if self.confirm.is_some() {
            let f = self.card_in("confirm");
            confirm::draw(self, s);
            self.card_out(f);
        }
        if self.menu.is_some() {
            let f = self.card_in("menu");
            menu::draw(self, s);
            self.card_out(f);
        }
        self.motion.end();
    }

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

    fn label(&self, text: &str, r: R, c: theme::Color) {
        let lo = &self.look;
        self.gfx.text_tracked(&text.to_uppercase(), r, lo.label, c, lo.label_track);
    }

    fn button(&mut self, r: R, icon: &str, h: Hit, enabled: bool) {
        let th = self.theme;
        let hov = enabled && self.is_hover(&h);
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
                self.spin_icon(r, s, icon, en);
            } else {
                self.button(r, icon, h, en);
            }
        }
        3.0 * step
    }

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

    fn window_buttons(&mut self, right: f32, y: f32, size: f32, s: &UiState) -> R {
        let step = size + 1.0;
        let wins = [("min", Hit::Min), (if s.maximized { "restore" } else { "max" }, Hit::Max), ("close", Hit::Close)];
        for (n, (icon, h)) in wins.into_iter().enumerate() {
            let r = R::new(right - (3 - n) as f32 * step, y, size, size);
            self.button(r, icon, h, true);
        }
        R::new(right - 3.0 * step, y, 3.0 * step, size)
    }

    fn ws_h(&self, s: &UiState) -> f32 {
        if self.look.ws_row { s.ws_h() } else { 0.0 }
    }

    fn essentials_h(&self, s: &UiState, compact: bool) -> f32 {
        let n = s.essentials.len();
        if n == 0 {
            return 0.0;
        }
        let per = if compact { 1 } else { 4 };
        let rows = n.div_ceil(per);
        rows as f32 * 32.0 + 8.0
    }

    fn draw_pin_home(&mut self, t: &UiTab, r: R) {
        if !t.pin_away {
            return;
        }
        let th = self.look.theme;
        let b = R::new(r.right() - 16.0, r.bottom() - 16.0, 13.0, 13.0);
        let mh = Hit::PinHome(t.id);
        self.hit(b, mh.clone());
        let hov = self.is_hover(&Hit::Tab(t.id)) || self.is_hover(&mh);
        if !hov {
            return;
        }
        self.gfx.round(b, 4.0, th.side);
        self.gfx.icon("home", b.x + 1.0, b.y + 1.0, 11.0, if self.is_hover(&mh) { th.text } else { th.dim });
    }

    fn draw_essentials(&mut self, s: &UiState, sb: R, compact: bool, y: f32) {
        if s.essentials.is_empty() {
            return;
        }
        let lo = self.look.clone();
        let th = lo.theme;
        let per = if compact { 1 } else { 4 };
        let pad = if compact { 6.0 } else { 8.0 };
        let gap = if compact { 0.0 } else { 6.0 };
        let cell = if compact { 30.0 } else { ((sb.w - 2.0 * pad - 3.0 * gap) / 4.0).max(28.0) };
        let active = s.active_tab().map(|t| t.url.clone()).unwrap_or_default();
        for (n, e) in s.essentials.iter().enumerate() {
            let col = n % per;
            let row = n / per;
            let r = R::new(sb.x + pad + col as f32 * (cell + gap), y + row as f32 * 32.0, cell, 28.0);
            let h = Hit::Essential(n);
            let hov = self.is_hover(&h);
            let a = self.motion.hover(Id::Essential(n), if hov { 1.0 } else { 0.0 }).min(1.0);
            if a > 0.01 {
                self.gfx.round(r, lo.r_sm, th.hover.alpha(a));
            }
            let host = host(&e.url);
            let icon = R::new(r.x + (r.w - 20.0) / 2.0, r.y + 4.0, 20.0, 20.0);
            let png = self.fav_png(&host, true);
            match png.as_ref().and_then(|p| self.gfx.bitmap(site_key(&host), p)) {
                Some(b) => self.gfx.draw_bitmap(&b, icon, 1.0),
                None => {
                    let letter = e.title.chars().next().or_else(|| host.chars().next()).unwrap_or('·').to_uppercase().to_string();
                    self.gfx.round(icon, 6.0, th.raise);
                    self.gfx.round_stroke(icon, 6.0, th.line, 1.0);
                    self.gfx.text(&letter, icon, lo.font(11.0, 700).align(Align::Center), th.dim);
                }
            }
            if e.url == active {
                self.gfx.round_stroke(icon, 6.0, th.accent, 2.0);
            }
            if e.away {
                let b = R::new(r.right() - 12.0, r.y + 1.0, 11.0, 11.0);
                let mh = Hit::PinHome(e.tab);
                self.hit(b, mh.clone());
                if hov || self.is_hover(&mh) {
                    self.gfx.round(b, 3.0, th.side);
                    self.gfx.icon("home", b.x + 1.0, b.y + 1.0, 9.0, if self.is_hover(&mh) { th.text } else { th.dim });
                }
            }
            self.hit(r, h);
        }
    }

    fn hidden_sidebar(&self, s: &UiState) -> bool {
        s.settings.hide_sidebar && self.look.kind.sidebar()
    }

    fn slide_sidebar(&mut self, s: &UiState) {
        if !self.hidden_sidebar(s) {
            self.side_slide = 1.0;
            self.side_shown = true;
            return;
        }
        let dwell = self.motion.enter(Id::Named("side-dwell"), Prop::Progress, 0.0, if self.side_at_edge { 1.0 } else { 0.0 }, 250, Ease::OutCubic);
        let hold = self.side_over || self.side_forced || self.side_busy();
        let want = if dwell >= 1.0 || hold { 1.0 } else { 0.0 };
        self.side_shown = want > 0.5;
        self.side_slide = self.motion.enter(Id::Named("sidebar"), Prop::X, 0.0, want, 200, Ease::OutCubic);
    }

    fn side_mouse(&mut self, s: &UiState, x: f32, y: f32) {
        if !self.hidden_sidebar(s) {
            return;
        }
        let (w, h) = self.size_dip();
        let right = s.settings.sidebar_side == "right";
        let band = if right { w - x } else { x };
        let at_edge = band <= layout::EDGE_BAND && y >= 0.0 && y <= h;
        let over = self.sidebar().is_some_and(|sb| x >= sb.x && x <= sb.right() && y >= sb.y && y <= sb.bottom());
        if self.side_at_edge != at_edge || self.side_over != over {
            self.side_at_edge = at_edge;
            self.side_over = over;
            self.invalidate();
        }
        if !at_edge && !over {
            self.side_forced = false;
        }
    }

    fn side_leave(&mut self) {
        self.side_at_edge = false;
        self.side_over = false;
        self.side_forced = false;
        self.invalidate();
    }

    fn side_busy(&self) -> bool {
        self.menu.is_some() || self.editing || self.drag.is_some()
    }

    fn side_over_page(&mut self, s: &UiState, sb: R) {
        let th = self.look.theme;
        let right = s.settings.sidebar_side == "right";
        let sh = if right { -8.0 } else { 8.0 };
        self.gfx.round(R::new(sb.x + sh, sb.y, sb.w, sb.h), 0.0, th.shade.alpha(0.4));
        let area = if right { R::new(sb.x - 8.0, sb.y, sb.w + 8.0, sb.h) } else { R::new(sb.x, sb.y, sb.w + 8.0, sb.h) };
        self.floats.push((area, 0.0));
    }

    fn draw_sidebar(&mut self, s: &UiState) {
        let Some(sb) = self.sidebar() else { return };
        let lo = self.look.clone();
        let th = lo.theme;
        if self.hidden_sidebar(s) && self.side_slide > 0.01 {
            self.side_over_page(s, sb);
        }
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
        if let Some(c) = s.workspaces.get(s.active_workspace).filter(|_| s.workspaces.len() > 1).and_then(|w| workspace_colour(th.dark, &w.colour)) {
            self.gfx.round(R::new(sb.x, sb.y, sb.w, 2.0), 0.0, c);
        }
        let compact = s.compact();
        let top = sb.y;
        self.hit(R::new(sb.x, top, sb.w, 40.0), Hit::Drag);
        if compact {
            let x = sb.x + (sb.w - 28.0) / 2.0;
            let y = |v: f32| top + v;
            self.button(R::new(x, y(8.0), 28.0, 28.0), "close", Hit::Close, true);
            self.button(R::new(x, y(38.0), 28.0, 28.0), if s.maximized { "restore" } else { "max" }, Hit::Max, true);
            self.button(R::new(x, y(68.0), 28.0, 28.0), "min", Hit::Min, true);
            self.button(R::new(x, y(104.0), 28.0, 28.0), "sidebar", Hit::Sidebar, true);
        } else {
            let nav_x = sb.x + 8.0;
            self.nav_buttons(nav_x, top + 9.0, s, false);
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

    fn draw_side_body(&mut self, s: &UiState) {
        let Some(sb) = self.sidebar() else { return };
        let lo = self.look.clone();
        if self.hidden_sidebar(s) && self.side_slide > 0.01 {
            self.side_over_page(s, sb);
        }
        self.gfx.fill(sb, lo.theme.side);
        let compact = s.compact();
        self.hit(R::new(sb.x, sb.y, sb.w, 6.0), Hit::Drag);
        self.draw_ws_row(s, sb, compact);
        self.draw_chips(s, sb, compact);
        self.draw_tab_list(s, sb, compact);
        self.draw_footer(s, sb, compact);
        self.draw_cards(s, sb, compact);
    }

    fn draw_ws_row(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        if compact || self.ws_h(s) <= 0.0 {
            return;
        }
        let ws_y = sb.y + if lo.kind.top_toolbar() { 8.0 } else { 42.0 };
        let ws_h = 26.0;
        let n = s.workspaces.len();
        let glance_w = if s.glance { self.gfx.measure("Open as tab", lo.font(12.0, 500)) + 20.0 } else { 0.0 };
        let gaps = lo.s2 * 0.75 * (n as f32 + if s.glance { 1.0 } else { 0.0 });
        let room = (sb.w - 16.0 - 28.0 - glance_w - gaps).max(44.0 * n as f32);
        let cap = room / n.max(1) as f32;
        let mut wx = sb.x + 8.0;
        for (i, w) in s.workspaces.iter().enumerate() {
            let name = &w.name;
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
            if let Some(c) = workspace_colour(th.dark, &w.colour) {
                self.gfx.circle(r.x + 7.0, r.y + r.h / 2.0, 3.0, c);
            }
            self.hit(r, hit);
            wx += tw + 6.0;
        }
        let add_r = R::new(wx, ws_y, 26.0, ws_h);
        let ah = self.is_hover(&Hit::WorkspaceAdd);
        if ah {
            self.gfx.round(add_r, lo.r_sm + 1.0, th.hover);
        }
        self.gfx.icon("plus", add_r.x + 6.0, add_r.y + 6.0, 14.0, if ah { th.dim } else { th.faint });
        self.hit(add_r, Hit::WorkspaceAdd);
        wx += 34.0;
        if s.glance && glance_w >= 60.0 && wx + 60.0 < sb.right() {
            let open_r = R::new(wx, ws_y, glance_w.min(sb.right() - wx - 8.0), ws_h);
            let oh = self.is_hover(&Hit::GlanceOpen);
            self.gfx.round(open_r, lo.r_md, if oh { th.accent } else { th.accent.alpha(0.85) });
            self.gfx.text("Open as tab", open_r, lo.font(11.5, 600).align(Align::Center), th.bg);
            self.hit(open_r, Hit::GlanceOpen);
        }
    }

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
        if compact && !in_toolbar {
            self.gfx.icon("search", o.x + (o.w - 16.0) / 2.0, o.y + 9.0, 16.0, th.dim);
        } else {
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
        let full_bar = !compact || in_toolbar;
        if site && full_bar && !focused {
            self.hit(R::new(o.x + 4.0, o.y + 5.0, 28.0, 24.0), Hit::SiteInfo);
        }
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

    fn draw_chips(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        if compact || (s.listen.is_empty() && s.kdbx.is_empty()) {
            return;
        }
        let y = if lo.kind.top_toolbar() { sb.y + 8.0 + self.ws_h(s) } else { sb.y + 84.0 + self.ws_h(s) };
        let mut cx = sb.x + 8.0;
        let right = sb.right() - 8.0;
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

    fn draw_tab_list(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        let row_h = lo.tab_row_h;
        let step = row_h + 1.0;
        let pad = if compact { 6.0 } else { 8.0 };
        let offer = !compact && (!s.listen.is_empty() || !s.kdbx.is_empty());
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
        let ess_y = list_top;
        let list_top = list_top + self.essentials_h(s, compact);
        let list = R::new(sb.x, list_top, sb.w, (sb.h - base - self.ws_h(s) - self.footer_h(compact)).max(1.0));
        self.draw_essentials(s, sb, compact, ess_y);
        self.gfx.clip(list);
        let wf = self.motion.enter(Id::Workspace(s.active_workspace), Prop::Opacity, 1.0, 1.0, 150, Ease::OutCubic).min(1.0);
        let mut y = list.y - self.scroll;
        let groups = agent_groups(s);
        let pins: Vec<UiTab> = s.tabs.iter().filter(|t| t.pinned && !t.agent && !t.essential).cloned().collect();
        let rests: Vec<UiTab> = s.tabs.iter().filter(|t| !t.pinned && !t.agent && !t.essential).cloned().collect();
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
        if !compact {
            let name = s.workspaces.get(s.active_workspace).filter(|_| s.workspaces.len() > 1).map(|w| w.name.clone()).unwrap_or_else(|| "Tabs".into());
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
        if let Some(n) = rows.iter().position(|(r, _)| matches!(r, Row::Tab(t) if t.id == s.active))
            && !self.drag.as_ref().is_some_and(|d| d.moved && d.id == s.active)
        {
            let slid = self.motion.spring(Id::Named("active-y"), Prop::Y, want[n], 220);
            let hl = R::new(sb.x + pad, list.y - self.scroll + slid, sb.w - 2.0 * pad, row_h);
            self.gfx.round(hl, lo.round(row_h), th.pill.alpha(wf));
            if lo.style == theme::Style::Precise {
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

    fn footer_h(&self, compact: bool) -> f32 {
        if compact && self.look.show_memory { 54.0 } else { 40.0 }
    }

    fn draw_compact_mem(&mut self, s: &UiState, sb: R, fy: f32) {
        if !self.look.show_memory {
            return;
        }
        let Some((bytes, _)) = s.mem else { return };
        let lo = self.look.clone();
        let th = lo.theme;
        let t = mem_short(bytes);
        let f = lo.mono_at(10.5, 500);
        let tw = self.gfx.measure(&t, f);
        let r = R::new(sb.x + (sb.w - tw) / 2.0, fy - 17.0, tw, 15.0);
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

    fn draw_footer(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        let fy = sb.bottom() - 38.0;
        let size = 28.0;
        let agent = s.agent_busy && !compact;
        if !compact {
            let inset = if lo.style == theme::Style::Glass { 12.0 } else { 8.0 };
            self.gfx.line(sb.x + inset, fy - 2.0, sb.right() - inset, fy - 2.0, th.line, 1.0);
        }
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
        if compact {
            self.draw_compact_mem(s, sb, fy);
        }
        let mut right = sb.right() - 8.0;
        if agent {
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
            let f = lo.mono_at(11.0, 500);
            let tw = self.gfx.measure(&t, f) + 10.0;
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
        if !compact
            && let Some(id) = s.media
            && let Some(t) = s.tabs.iter().find(|t| t.id == id)
        {
            right = self.draw_now_playing(t, right_edge + 6.0, right, fy);
        }
        if s.battery && right - 20.0 > right_edge + 6.0 {
            self.gfx.icon("leaf", right - 17.0, fy + 10.0, 14.0, th.faint);
        }
        self.draw_footer_overlays(s);
    }

    fn draw_now_playing(&mut self, t: &UiTab, left: f32, right: f32, fy: f32) -> f32 {
        let lo = self.look.clone();
        let th = lo.theme;
        let f = lo.font(11.5, 500);
        let title = if t.title.is_empty() { host(&t.url) } else { t.title.clone() };
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

    fn draw_footer_overlays(&mut self, s: &UiState) {
        if self.say.is_some() {
            let f = self.card_in("say");
            self.draw_say();
            self.card_out(f);
        } else if self.search_open {
            let f = self.card_in("search");
            self.draw_search(s);
            self.card_out(f);
        } else if self.site_open {
            let f = self.card_in("site");
            self.draw_site_menu(s);
            self.card_out(f);
        } else if self.pal_open {
            let f = self.card_in("palette");
            self.draw_palette(s);
            self.card_out(f);
        }
    }

    fn card_in(&mut self, name: &'static str) -> bool {
        let a = self.motion.enter(Id::Named(name), Prop::Opacity, 0.0, 1.0, 160, Ease::OutCubic);
        let moving = a < 0.999;
        if moving {
            self.gfx.begin_fade(a, (1.0 - a) * 8.0);
        }
        moving
    }

    fn card_out(&self, faded: bool) {
        if faded {
            self.gfx.end_fade();
        }
    }

    fn draw_site_menu(&mut self, s: &UiState) {
        let th = self.theme;
        let Some(sb) = self.float_area() else { return };
        let Some(tab) = s.active_tab() else { return };
        let origin = crate::engine::origin_of(&tab.url);
        if !origin.starts_with("http") {
            self.site_open = false;
            return;
        }
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
        let row = R::new(card.x + 12.0, y, card.w - 24.0, 40.0);
        let on = !crate::blocker::site_off(&s.settings.adblock_off_sites, &site);
        let words = match s.blocked {
            n if on && n > 0 => format!("Block ads and trackers on this site · {} blocked on this page", group_digits(n)),
            _ => "Block ads and trackers on this site".into(),
        };
        self.gfx.text_wrap(&words, R::new(row.x + 4.0, row.y + 3.0, row.w - 54.0, 34.0), lo.font(12.5, 450), th.faint);
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
        y += 4.0;
        for (hit, label) in [(Hit::SiteClear, "Clear data for this site"), (Hit::SiteSettings, "Site settings…")] {
            self.pill_button(R::new(card.x + 12.0, y, card.w - 24.0, 30.0), label, hit, false, 1.0);
            y += 36.0;
        }
    }

    fn active_origin(&self) -> Option<String> {
        let s = self.state.as_ref()?;
        let t = s.active_tab()?;
        t.url.starts_with("http").then(|| crate::engine::origin_of(&t.url))
    }

    fn draw_say(&mut self) {
        let lo = self.look.clone();
        let th = lo.theme;
        let Some(sb) = self.float_area() else { return };
        let Some((session, name, mode)) = self.say.as_ref().map(|s| (s.session.clone(), s.name.clone(), s.mode)) else { return };
        let card = say_card(sb, self.cards_right());
        self.float_card(card, 1.0);
        self.hit(card, Hit::SayCard);
        self.gfx.circle(card.x + 18.0, card.y + 22.0, 4.0, agent_color(&session));
        self.gfx.text(&format!("Message to {name}"), R::new(card.x + 30.0, card.y + 10.0, card.w - 66.0, 24.0), lo.font(12.5, 600), th.text);
        let x = R::new(card.right() - 34.0, card.y + 8.0, 26.0, 26.0);
        if self.is_hover(&Hit::SayClose) {
            self.gfx.round(x, lo.round(x.h), th.hover);
        }
        self.gfx.icon("close", x.x + 7.0, x.y + 7.0, 12.0, th.dim);
        self.hit(x, Hit::SayClose);
        let field = R::new(card.x + 12.0, card.y + 42.0, card.w - 24.0, 38.0);
        self.gfx.round(field, lo.r_md + 2.0, th.bg);
        self.gfx.round_stroke(field, lo.r_md + 2.0, th.text.alpha(0.85), 1.5);
        if wide_text(self.say_edit).is_empty() {
            self.gfx.text("Tell it what to do", R::new(field.x + 10.0, field.y, field.w - 20.0, field.h), lo.font(12.5, 400), th.faint);
        }
        let cells: Vec<(&str, Hit)> = SAY_MODES.iter().enumerate().map(|(n, (label, _, _))| (*label, Hit::SayMode(n))).collect();
        self.seg_cells(R::new(card.x + 12.0, card.y + 88.0, card.w - 24.0, 30.0), &cells, Some(mode));
        let send_w = 64.0;
        let hint = R::new(card.x + 14.0, card.y + 126.0, card.w - 28.0 - send_w - 8.0, 32.0);
        self.gfx.text_wrap(SAY_MODES[mode.min(2)].2, hint, lo.font(11.0, 400), th.faint);
        self.pill_button(R::new(card.right() - 12.0 - send_w, card.y + 128.0, send_w, 28.0), "Send", Hit::SaySend, true, 1.0);
    }

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
        self.hit(card, Hit::SearchCard);
        let field = R::new(card.x + 8.0, card.y + 8.0, card.w - 16.0, 32.0);
        self.gfx.round(field, lo.round(field.h), th.bg);
        self.gfx.round_stroke(field, lo.round(field.h), th.line, 1.0);
        self.gfx.icon("search", field.x + 11.0, field.y + 9.0, 14.0, th.faint);
        if q.is_empty() {
            self.gfx.text("Search open tabs", R::new(field.x + 30.0, field.y, field.w - 40.0, field.h), lo.font(12.5, 400), th.faint);
        }
        let mut y = card.y + 48.0;
        if self.search_rows.is_empty() {
            self.gfx.text("No open tab matches", R::new(card.x, y + 8.0, card.w, 26.0), lo.font(12.5, 400).align(Align::Center), th.faint);
        }
        for n in 0..shown {
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

    fn draw_palette(&mut self, s: &UiState) {
        let lo = self.look.clone();
        let th = lo.theme;
        let Some(sb) = self.float_area() else { return };
        let q = wide_text(self.palette_edit).trim().to_lowercase();
        let (rows, groups, total) = pal_view(s, &q);
        self.pal_rows = rows;
        let shown = self.pal_rows.len();
        let card = pal_card(sb, self.cards_right(), shown, groups, total > shown);
        self.pal_sel = self.pal_sel.min(shown.saturating_sub(1));
        self.float_card(card, 1.0);
        self.hit(card, Hit::PaletteCard);
        let field = R::new(card.x + 8.0, card.y + 8.0, card.w - 16.0, 32.0);
        self.gfx.round(field, lo.round(field.h), th.bg);
        self.gfx.round_stroke(field, lo.round(field.h), th.line, 1.0);
        self.gfx.icon("palette", field.x + 11.0, field.y + 9.0, 14.0, th.faint);
        if q.is_empty() {
            self.gfx.text("Type a command", R::new(field.x + 30.0, field.y, field.w - 40.0, field.h), lo.font(12.5, 400), th.faint);
        }
        let mut y = card.y + 48.0;
        if shown == 0 {
            self.gfx.text("Nothing matches", R::new(card.x, y + 8.0, card.w, 26.0), lo.font(12.5, 400).align(Align::Center), th.faint);
        }
        for n in 0..shown {
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
                self.gfx.icon(pages::section_icon(&detail), r.x + 11.0, r.y + 8.0, 14.0, th.faint);
            } else if !tab_url.is_empty() {
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

    fn draw_agent_group(&mut self, head: &UiTab, r: R, tabs: usize, alpha: f32, note: Option<String>) {
        let lo = self.look.clone();
        let th = lo.theme;
        let dot = agent_color(&head.owner);
        self.gfx.circle(r.x + 11.0, r.y + r.h / 2.0, 3.5, dot.alpha(alpha));
        let name = if head.owner_name.trim().is_empty() { "agent" } else { head.owner_name.trim() };
        let label = match note {
            Some(n) => format!("{name} · {n}"),
            None if tabs > 1 => format!("{name} · {tabs} tabs"),
            None => name.to_string(),
        };
        let stop = R::new(r.right() - 26.0, r.y + (r.h - 22.0) / 2.0, 22.0, 22.0);
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
        self.gfx.round_stroke(R::new(stop.x + 6.0, stop.y + 6.0, 10.0, 10.0), 2.5, th.accent.alpha(alpha), 1.6);
        self.hit(stop, hit);
    }

    fn footer_page_button(&mut self, r: R, s: &UiState, icon: &str, panel: &str, hit: Hit) {
        let lo = self.look.clone();
        let th = lo.theme;
        let on = s.panel == panel || (panel == "drive" && s.drive.as_ref().is_some_and(|d| d.running));
        let hov = self.is_hover(&hit);
        let a = self.motion.hover(Self::button_id(&hit), if hov || on { 1.0 } else { 0.0 });
        if a > 0.01 {
            self.gfx.round(r, lo.r_sm + 1.0, if on { th.press } else { th.hover }.alpha(a.min(1.0)));
        }
        let live = panel == "drive" && s.drive.as_ref().is_some_and(|d| d.running);
        self.gfx.icon(icon, r.x + 6.0, r.y + 6.0, 16.0, if live { th.accent } else if on || hov { th.text } else { th.dim });
        self.hit(r, hit);
    }

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

    fn draw_cards(&mut self, s: &UiState, sb: R, compact: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        let _ = compact;
        let foot = if self.sidebar().is_some() { 44.0 } else { 12.0 };
        if let Some((who, msg)) = s.handoff.as_ref().filter(|_| s.prompt.is_none()) {
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

    fn draw_suggestions(&mut self, s: &UiState) {
        let lo = self.look.clone();
        let th = lo.theme;
        if !(self.editing && !self.sugg.is_empty()) {
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
            let c = if t.status == "off" && !active { lit.alpha(0.55 + 0.45 * h) } else { lit };
            self.gfx.round(r, lo.r_md, c.alpha(alpha));
            if active {
                self.gfx.round_stroke(r, lo.r_md, th.text.alpha(alpha), 1.5);
            } else {
                self.gfx.round_stroke(r, lo.r_md, th.line.alpha(alpha), 1.0);
            }
        } else if !active && h > 0.01 {
            let c = if pressed { th.press } else { th.hover };
            self.gfx.round(r, rad, c.alpha(h * alpha));
        } else if active && alpha < 1.0 {
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
        let asleep = matches!(t.status, "asleep" | "off" | "crashed");
        let dim = if asleep { 0.5 } else { 1.0 };
        let (fx, fy) = if pinned || compact { (r.x + (r.w - 16.0) / 2.0, r.y + (r.h - 16.0) / 2.0) } else { (r.x + 9.0, r.y + (r.h - 16.0) / 2.0) };
        let fav = R::new(fx, fy, 16.0, 16.0);
        if t.veiled {
            let ink = if active && !pinned { th.pill_ink } else { th.dim };
            self.gfx.icon("lock", fav.x + 1.0, fav.y + 1.0, 14.0, ink.alpha(alpha));
        } else if t.loading {
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
                        self.gfx.icon("home", fav.x + 1.0, fav.y + 1.0, 14.0, ink.alpha(alpha));
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
            self.gfx.circle(r.right() - 5.0, r.y + 5.0, 3.0, th.accent.alpha(0.5 * alpha));
        }
        if t.private && (pinned || compact) {
            self.gfx.circle(r.right() - 5.0, r.y + 5.0, 3.0, style::private_mark(th.dark).alpha(alpha));
        }
        if (t.audio || t.muted || t.call) && pinned {
            let b = R::new(r.right() - 15.0, r.y + 1.0, 14.0, 14.0);
            self.gfx.round(b, 4.0, th.bg);
            self.gfx.icon(sound_icon(t), b.x + 2.0, b.y + 2.0, 10.0, th.dim);
            self.hit(b, Hit::TabSnd(t.id));
        }
        if pinned {
            self.draw_pin_home(t, r);
        }
        if pinned || compact {
            return;
        }
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
            let b = R::new(right - 22.0, r.y + (r.h - 22.0) / 2.0, 22.0, 22.0);
            if self.is_hover(&Hit::TabPip(t.id)) {
                self.gfx.round(b, lo.r_sm, if active { th.pill_ink.alpha(0.14 * alpha) } else { th.press.alpha(alpha) });
            }
            let ink = if active { th.pill_ink.alpha(0.75) } else { th.faint };
            self.gfx.icon("pip", b.x + 4.0, b.y + 4.0, 14.0, ink.alpha(alpha));
            self.hit(b, Hit::TabPip(t.id));
            right = b.x - 2.0;
        }
        let mem = self.tab_mem.get(&t.id).copied().filter(|b| *b > 0);
        if let Some(b) = mem.filter(|b| *b >= HEAVY_TAB || self.is_hover(&Hit::Tab(t.id)) || self.is_hover(&Hit::TabX(t.id))) {
            let text = mem_label(b);
            let f = lo.font(11.0, 500);
            let w = self.gfx.measure(&text, f) + 4.0;
            let heavy = b >= HEAVY_TAB;
            let ink = match (heavy, active) {
                (true, _) => th.danger.alpha(if active { 0.9 } else { 0.75 }),
                (false, true) => th.pill_ink.alpha(0.6),
                (false, false) => th.faint,
            };
            self.gfx.text(&text, R::new(right - w, r.y, w, r.h), f.align(Align::Right), ink.alpha(alpha));
            right -= w + 4.0;
        }
        let badge_y = r.y + (r.h - 16.0) / 2.0;
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
            right = tag(self, "AI OK", right);
        }
        if t.private {
            right = tag(self, "PRIVATE", right);
        }
        if t.split {
            let b = R::new(right - 18.0, badge_y, 16.0, 16.0);
            self.gfx.icon("split", b.x, b.y, 16.0, if active { th.dim.alpha(alpha) } else { th.faint.alpha(alpha) });
            right = b.x - 4.0;
        }
        if t.translated || t.translating {
            let b = R::new(right - 18.0, badge_y, 16.0, 16.0);
            self.gfx.icon("globe", b.x, b.y, 16.0, if t.translating { th.faint.alpha(alpha) } else { th.accent.alpha(alpha) });
            right = b.x - 4.0;
        }
        if t.servo {
            let b = R::new(right - 22.0, badge_y, 20.0, 14.0);
            self.gfx.round(b, 4.0, th.press.alpha(alpha));
            self.gfx.text("SV", b, lo.font(9.0, 650).align(Align::Center), th.dim.alpha(alpha));
            right = b.x - 4.0;
        }
        let title = if t.veiled {
            "Locked tab".into()
        } else if !t.title.is_empty() {
            t.title.clone()
        } else if t.status == "new" {
            "Start".into()
        } else {
            let h = host(&t.url);
            if h.is_empty() { "Untitled".into() } else { h }
        };
        let c = if active { th.pill_ink } else if self.is_hover(&Hit::Tab(t.id)) { th.text } else { th.dim };
        let f = if active { lo.font(13.0, 500) } else { lo.font(13.0, 400) };
        self.gfx.text(&title, R::new(r.x + 34.0, r.y, (right - r.x - 34.0).max(0.0), r.h), f, c.alpha(alpha * dim));
    }

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

    fn draw_strip(&mut self, s: &UiState, area: R) {
        let lo = self.look.clone();
        let th = lo.theme;
        let compact = s.compact();
        let pins: Vec<UiTab> = s.tabs.iter().filter(|t| t.pinned).cloned().collect();
        let tabs: Vec<UiTab> = s.tabs.iter().filter(|t| !t.pinned).cloned().collect();
        let ws = s.workspaces.len() >= 2;
        let geo = layout::strip(area, pins.len(), tabs.len(), compact, ws);
        self.hit(area, Hit::Drag);
        for (n, t) in pins.iter().enumerate() {
            if let Some(r) = geo.pinned.get(n) {
                self.draw_strip_tab(t, *r, s, true);
            }
        }
        for (n, t) in tabs.iter().enumerate() {
            let Some(target) = geo.tabs.get(n) else { break };
            let x = self.motion.spring(Id::Tab(t.id), Prop::X, target.x, 200);
            let room = (geo.plus.x - x - 2.0).max(20.0);
            let w = self.motion.tween(Id::Tab(t.id), Prop::W, target.w, 190, Ease::OutCubic).min(room);
            let r = R::new(x, target.y, w, target.h);
            if r.right() > geo.plus.x + 1.0 {
                continue;
            }
            self.draw_strip_tab(t, r, s, false);
        }
        self.button(geo.plus, "plus", Hit::NewTab, true);
        if let Some(r) = geo.ws {
            let h = Hit::WsChip;
            let hov = self.is_hover(&h);
            let a = self.motion.hover(Id::Button("wschip"), if hov { 1.0 } else { 0.0 });
            if a > 0.01 {
                self.gfx.round(r, lo.r_sm, th.hover.alpha(a.min(1.0)));
            }
            let name = s.workspaces.get(s.active_workspace).map(|w| w.name.as_str()).unwrap_or("");
            let mut short: String = name.chars().take(if compact { 2 } else { 10 }).collect();
            if name.chars().count() > (if compact { 2 } else { 10 }) {
                short.push('…');
            }
            self.gfx.text(&short, r, lo.font(11.5, 500).align(Align::Center), if hov { th.text } else { th.dim });
            self.hit(r, h);
        }
        let size = if compact { 26.0 } else { 30.0 };
        self.window_buttons(geo.wins.right(), geo.wins.y + (geo.wins.h - size) / 2.0, size, s);
    }

    fn draw_strip_tab(&mut self, t: &UiTab, r: R, s: &UiState, pinned: bool) {
        let lo = self.look.clone();
        let th = lo.theme;
        let active = t.id == s.active;
        let h_ = Hit::Tab(t.id);
        let hov = self.is_hover(&h_) || self.is_hover(&Hit::TabX(t.id)) || self.is_hover(&Hit::TabSnd(t.id));
        let h = self.motion.hover(Id::Tab(t.id), if hov { 1.0 } else { 0.0 }).min(1.0);
        let pressed = self.press.as_ref() == Some(&h_);
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
                self.gfx.line(r.x + rad.max(6.0), r.bottom() - 1.0, r.right() - rad.max(6.0), r.bottom() - 1.0, th.accent, 2.0);
            }
        } else if h > 0.01 || pressed {
            self.gfx.round(r, rad, if pressed { th.press } else { th.hover.alpha(h) });
        }
        let ink = |c: theme::Color| if on_pill { th.pill_ink } else { c };
        self.hit(r, Hit::Tab(t.id));
        let (fx, fy) = if pinned { (r.x + (r.w - 16.0) / 2.0, r.y + (r.h - 16.0) / 2.0) } else { (r.x + 10.0, r.y + (r.h - 16.0) / 2.0) };
        let fav = R::new(fx, fy, 16.0, 16.0);
        let asleep = matches!(t.status, "asleep" | "off" | "crashed");
        let dim = if asleep { 0.5 } else { 1.0 };
        if t.veiled {
            self.gfx.icon("lock", fav.x + 1.0, fav.y + 1.0, 14.0, th.dim);
        } else if t.loading {
            let phase = (self.born.elapsed().as_secs_f32() / 0.8).fract();
            self.gfx.spinner(fx + 8.0, fy + 8.0, 6.5, phase, th.line, th.accent);
        } else {
            let png = lo.show_favicons.then(|| self.favs.get(&t.id).cloned()).flatten();
            let bmp = png.filter(|p| !p.is_empty()).and_then(|p| self.gfx.bitmap(t.id, &p));
            match bmp {
                Some(b) => self.gfx.draw_bitmap(&b, fav, dim),
                None if t.status == "new" => {
                    self.gfx.icon("home", fav.x + 1.0, fav.y + 1.0, 14.0, ink(th.dim));
                }
                None => {
                    let letter = host(&t.url).chars().next().unwrap_or('·').to_uppercase().to_string();
                    self.gfx.round(fav, 5.0, th.raise.alpha(dim));
                    self.gfx.round_stroke(fav, 5.0, th.line.alpha(dim), 1.0);
                    self.gfx.text(&letter, fav, lo.font(9.5, 650).align(Align::Center), th.dim.alpha(dim));
                }
            }
        }
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
            self.draw_pin_home(t, r);
            return;
        }
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
        let title = if t.veiled {
            "Locked tab".into()
        } else if !t.title.is_empty() {
            t.title.clone()
        } else if t.status == "new" {
            "Start".into()
        } else {
            let h = host(&t.url);
            if h.is_empty() { "Untitled".into() } else { h }
        };
        let c = ink(if hov { th.text } else { th.dim });
        let c = if asleep { c.alpha(0.5) } else { c };
        self.gfx.text(&title, R::new(fx + 24.0, r.y, (right - fx - 24.0).max(0.0), r.h), lo.font(12.5, if on_pill { 550 } else { 450 }), c);
        let _ = s;
    }

    fn draw_toolbar(&mut self, s: &UiState) {
        let Some((_, tool, _)) = self.top_rows() else { return };
        let lo = self.look.clone();
        let th = lo.theme;
        self.gfx.fill(tool, th.side);
        self.hit(tool, Hit::Drag);
        let compact = s.compact();
        let size = if compact { 28.0 } else { 30.0 };
        let y = tool.y + (tool.h - size) / 2.0;
        let nav_w = self.nav_buttons(tool.x + 8.0, y, s, compact);
        if lo.kind.sidebar() {
            let r = R::new(tool.x + 8.0 + nav_w + 4.0, y, size, size);
            self.button(r, "sidebar", Hit::Sidebar, true);
        }
        self.draw_omni_bar(s, compact);
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
        let c = if hov { th.text } else { th.dim };
        for n in 0..3 {
            self.gfx.circle(menu.x + menu.w / 2.0, menu.y + menu.h / 2.0 + (n as f32 - 1.0) * 4.5, 1.6, c);
        }
        self.hit(menu, h);
    }

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

    fn ws_menu(&mut self, x: f32, y: f32) {
        let Some(s) = self.state.clone() else { return };
        let mut items: Vec<(u32, String)> = s
            .workspaces
            .iter()
            .enumerate()
            .map(|(n, w)| (n as u32 + 1, if n == s.active_workspace { format!("· {}", w.name) } else { w.name.clone() }))
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
                    ("Blocked today", group_digits(blocked.blocked()), String::new(), "ads, trackers and the rest".to_string()),
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
        if s.active_tab().is_some_and(|t| t.private) {
            let mark = style::private_mark(th.dark);
            self.gfx.text(
                "Private tab. Nothing you do here is saved: no history, no cookies after the last private tab closes.",
                R::new(x0 + 4.0, field.y - 22.0, field.w, 18.0),
                lo.font(12.0, 450),
                mark.alpha(t2),
            );
        }
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
            let k = "Ctrl L";
            let kf = lo.mono_at(10.5, 500);
            let kw = self.gfx.measure(k, kf) + 14.0;
            let kr = R::new(field.right() - 24.0 - kw, field.y + (field.h - 20.0) / 2.0, kw, 20.0);
            self.gfx.round_stroke(kr, 5.0, faint.alpha(0.6 * t2), 1.0);
            self.gfx.text(k, R::new(kr.x - 10.0, kr.y, kr.w + 20.0, kr.h), kf.align(Align::Center), faint.alpha(t2));
            let _ = ink;
            self.hit(field, Hit::NtpSearch);
        }
        self.draw_dial(s, t3);
    }

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

    fn draw_popup_note(&mut self, s: &UiState) {
        let Some(note) = s.popup_note.clone() else { return };
        if s.full || s.panel != "none" {
            return;
        }
        let (sx, sy, sw, _sh) = s.stage;
        let lo = self.look.clone();
        let th = lo.theme;
        let redirect = note.kind == NoteKind::Redirect;
        let head = if redirect { "Redirect blocked" } else { "Pop-up blocked" };
        let target = if note.target.is_empty() { String::new() } else { format!(" · {}", note.target) };
        let allow = if note.site.is_empty() { "Allow on this site".to_string() } else { format!("Allow on {}", note.site) };
        let go = "Go anyway";
        let fw = lo.font(12.5, 500);
        let aw = self.gfx.measure(&allow, lo.font(12.5, 600)) + 26.0;
        let gw = if redirect { self.gfx.measure(go, lo.font(12.5, 500)) + 26.0 } else { 0.0 };
        let tw = self.gfx.measure(head, fw);
        let tt = self.gfx.measure(&target, lo.font(12.5, 450));
        let gap = if redirect { 8.0 } else { 0.0 };
        let w = (tw + tt + 46.0 + aw + gw + gap + 30.0).min(sw - 24.0).max(240.0);
        let h = 38.0;
        let r = R::new(sx + (sw - w) / 2.0, sy + 12.0, w, h);
        self.floats.push((r, lo.r_card));
        self.gfx.round(r, lo.r_card, th.solid(th.raise));
        self.gfx.round_stroke(r, lo.r_card, th.line, 1.0);
        self.gfx.circle(r.x + 20.0, r.y + h / 2.0, 3.5, th.accent);
        self.gfx.text(head, R::new(r.x + 32.0, r.y, tw + 2.0, h), fw, th.text);
        self.gfx.text(&target, R::new(r.x + 34.0 + tw, r.y, tt + 2.0, h), lo.font(12.5, 450), th.dim);
        let b = R::new(r.right() - 30.0 - aw, r.y + (h - 26.0) / 2.0, aw, 26.0);
        self.pill_button(b, &allow, Hit::PopupAllow(note.tab), true, 1.0);
        if redirect {
            let g = R::new(b.x - gap - gw, r.y + (h - 26.0) / 2.0, gw, 26.0);
            self.pill_button(g, go, Hit::RedirectGo(note.tab), false, 1.0);
        }
        let x = R::new(r.right() - 28.0, r.y + (h - 24.0) / 2.0, 24.0, 24.0);
        let gone = Hit::PopupDismiss(note.tab);
        if self.is_hover(&gone) {
            self.gfx.round(x, 12.0, th.press);
        }
        self.gfx.icon("close", x.x + 6.0, x.y + 6.0, 12.0, th.dim);
        self.hit(x, gone);
    }

    fn float_card(&mut self, r: R, a: f32) {
        self.floats.push((r, self.look.r_card));
        let lo = &self.look;
        let th = lo.theme;
        self.gfx.round(R::new(r.x, r.y + 6.0, r.w, r.h), lo.r_card, th.shade.alpha(0.55 * a));
        self.gfx.round(r, lo.r_card, th.solid(th.raise).alpha(a));
        self.gfx.round_stroke(r, lo.r_card, th.line.alpha(a), 1.0);
    }

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

    fn draw_lock(&mut self, area: R, l: &UiLock) {
        let f = self.card_in(if l.app { "lock-app" } else { "lock-tab" });
        self.draw_lock_card(area, l);
        self.card_out(f);
    }

    fn draw_lock_card(&mut self, area: R, l: &UiLock) {
        let lo = self.look.clone();
        let th = lo.theme;
        let w = 420.0_f32.min(area.w - 40.0);
        let (cx, top) = (area.x + area.w / 2.0, area.y + area.h * 0.36);
        let ring = R::new(cx - 34.0, top - 34.0, 68.0, 68.0);
        match lo.style {
            theme::Style::Glass => {
                self.gfx.bloom(cx, top, 90.0, th.accent.alpha(0.3));
                self.gfx.round(ring, 34.0, theme::Color(1.0, 1.0, 1.0, 0.1));
                self.gfx.round_stroke(ring, 34.0, theme::Color(1.0, 1.0, 1.0, 0.18), 1.0);
            }
            theme::Style::Precise => self.gfx.round_stroke(ring, 12.0, th.accent, 1.5),
            theme::Style::Signal => self.gfx.round(ring, 34.0, th.raise),
        }
        let ink = if lo.style == theme::Style::Precise { th.accent } else { th.text };
        self.gfx.icon("lock", cx - 13.0, top - 13.0, 26.0, ink);
        let title = if l.app { "Lumen is locked" } else { "This tab is locked" };
        self.gfx.text(title, R::new(cx - w / 2.0, top + 54.0, w, 30.0), lo.h2.align(Align::Center), th.text);
        let line = if l.app {
            "Windows Hello opens it: your face, your finger or your PIN."
        } else {
            "Its page stays hidden until Windows Hello says it is you."
        };
        self.gfx.text_wrap(line, R::new(cx - w / 2.0, top + 90.0, w, 40.0), lo.small.align(Align::Center), th.dim);
        let mut y = top + 136.0;
        if l.hello == Some(false) {
            let no = "Windows Hello is not set up on this PC. Set a PIN in Windows Settings, Accounts, Sign-in options.";
            self.gfx.text_wrap(no, R::new(cx - w / 2.0, y, w, 40.0), lo.small.align(Align::Center), th.faint);
            return;
        }
        let label = "Unlock with Windows Hello";
        let bw = self.gfx.measure(label, lo.body_strong) + 44.0;
        let b = R::new(cx - bw / 2.0, y, bw, 40.0);
        self.pill_button(b, label, Hit::Unlock(l.app), true, 1.0);
        y = b.bottom() + 14.0;
        if !l.note.is_empty() {
            self.gfx.text(&l.note, R::new(cx - w / 2.0, y, w, 18.0), lo.small.align(Align::Center), th.faint);
        }
    }

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

    fn draw_dial(&mut self, s: &UiState, t3: f32) {
        let lo = self.look.clone();
        let th = lo.theme;
        let geo = self.ntp_geometry();
        let top = geo.top + (1.0 - t3) * 10.0;
        if let Some((msg, at)) = &self.page.flash
            && at.elapsed().as_secs_f32() < 5.0
        {
            let a = (5.0 - at.elapsed().as_secs_f32()).min(1.0);
            self.gfx.text(msg, R::new(geo.field.x + 2.0, geo.field.bottom() + 6.0, geo.field.w, 18.0), lo.font(12.0, 450), th.faint.alpha(a));
        }
        let lifted = self.dial_drag.as_ref().filter(|d| d.moved).map(|d| d.from);
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
            let lift = hv * 2.0;
            let tile = R::new(tile.x, tile.y - lift, tile.w, tile.h);
            self.draw_dial_tile(tile, &name, &url, matches!(cell, Cell::Add), hv, a);
            if target == Some(n) && !me {
                self.gfx.round_stroke(tile.inset(-3.0, -3.0), lo.r_card + 3.0, th.accent, 1.5);
            }
            self.hit(tile, hit);
            if let (Some(p), true) = (pin, hov) {
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
        if let Some(from) = lifted
            && let Some(sc) = s.settings.shortcuts.get(from)
        {
            let w = geo.grid.cell_w();
            let t = R::new(self.mouse.0 - w / 2.0, self.mouse.1 - NTP_CARD_H / 2.0, w, NTP_CARD_H);
            self.draw_dial_tile(t, &speeddial::label(sc), &sc.url, false, 1.0, 0.95);
        }
    }

    fn draw_update_note(&mut self, st: R, s: &UiState) {
        let now = env!("LUMEN_VERSION");
        let shot = cfg!(debug_assertions) && std::env::var_os("LUMEN_TEST_NOTE").is_some();
        if crate::headless() && !shot {
            return;
        }
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
                self.gfx.round(icon, 8.0, th.hover.alpha(a));
                let letter = name.chars().next().or_else(|| host.chars().next()).unwrap_or('?').to_uppercase().to_string();
                self.gfx.text(&letter, icon, lo.font(13.0, 700).align(Align::Center), th.dim.alpha(a));
            }
        }
        self.gfx.text(name, R::new(r.x + 14.0, r.bottom() - 46.0, r.w - 28.0, 20.0), lo.font(12.5, 600), th.text.alpha(a));
        self.gfx.text(&host, R::new(r.x + 14.0, r.bottom() - 28.0, r.w - 28.0, 16.0), lo.mono_at(10.5, 450), th.faint.alpha(a));
    }

    fn fav_png(&mut self, host: &str, fetch: bool) -> Option<Vec<u8>> {
        if let Some(p) = self.site_favs.get(host).filter(|p| !p.is_empty()) {
            return Some(p.clone());
        }
        let stale = |(p, at, first): &(Option<Vec<u8>>, std::time::Instant, std::time::Instant)| p.is_none() && at.elapsed().as_secs() >= 4 && first.elapsed().as_secs() < 60;
        let first = self.disk_favs.get(host).map(|e| e.2);
        if first.is_none() || self.disk_favs.get(host).is_some_and(stale) {
            let png = crate::favicons::get(host);
            if png.is_some() {
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

    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|(_, h)| h.clone())
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        if let Some(s) = self.state.clone() {
            self.side_mouse(&s, x, y);
        }
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
        if self.search_open && !matches!(h, Some(Hit::TabSearch(_)) | Some(Hit::SearchCard)) {
            self.close_search();
        }
        if self.say.is_some() && !matches!(h, Some(Hit::SayCard | Hit::SayMode(_) | Hit::SaySend | Hit::SayClose | Hit::AgentMessage(..))) {
            self.close_say();
        }
        if self.site_open && !matches!(h, Some(Hit::SiteInfo | Hit::SitePerm(..) | Hit::SiteClear | Hit::SiteSettings | Hit::SiteAdblock)) {
            self.site_open = false;
        }
        if self.color.is_some()
            && !matches!(h, Some(Hit::ColorCard | Hit::ColorArea | Hit::ColorHue | Hit::ColorPreset(_) | Hit::ColorRecent(_) | Hit::ColorDone))
        {
            color::done(self);
        }
        form::clicked_outside(self, &h);
        confirm::clicked_outside(self, &h);
        menu::clicked_outside(self, &h);
        files::clicked_outside(self, &h);
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
                if let Some(s) = &self.state
                    && self.hidden_sidebar(s)
                {
                    self.side_forced = true;
                    self.side_shown = true;
                    self.invalidate();
                    return;
                }
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
            Hit::PopupAllow(tab) => {
                let (site, kind) = self
                    .state
                    .as_ref()
                    .and_then(|s| s.popup_note.as_ref())
                    .map(|n| (n.site.clone(), n.kind))
                    .unwrap_or((String::new(), NoteKind::Popup));
                let kind = if kind == NoteKind::Redirect { "redirect" } else { "popup" };
                send(p, json!({"t": "popupAllow", "id": tab, "site": site, "kind": kind}));
            }
            Hit::PopupDismiss(tab) => send(p, json!({"t": "popupDismiss", "id": tab})),
            Hit::RedirectGo(tab) => send(p, json!({"t": "redirectGo", "id": tab})),
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
            Hit::Essential(n) => {
                if let Some(e) = self.state.as_ref().and_then(|s| s.essentials.get(n)) {
                    let url = e.url.clone();
                    send(p, json!({"t": "essential", "url": url}));
                }
            }
            Hit::PinHome(id) => {
                send(p, json!({"t": "pinBack", "id": id}));
            }
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
                form::open(self, "Add to speed dial", &[("Address", ""), ("Name (optional)", "")], form::FormAct::DialAdd);
            }
            Hit::Page(ph) => {
                let picking = self.page.font_pick.is_some();
                pages::click(self, ph);
                if picking != self.page.font_pick.is_some() {
                    self.layout_edits();
                }
            }
            Hit::SiteInfo => {
                let opening = !self.site_open;
                self.site_open = opening;
                if opening {
                    send(p, json!({"t": "siteOpen"}));
                }
            }
            Hit::SitePerm(kind, v) => {
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
            Hit::Unlock(app) => {
                if let Some(s) = &self.state {
                    send(&self.proxy, json!({"t": "helloUnlock", "what": if app { "app" } else { "tab" }, "id": s.active}));
                }
            }
            Hit::SiteAdblock => {
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
                    items.push((24, "Pin this page instead".into()));
                }
                if !t.essential && !t.private && !t.agent && t.url.starts_with("http") && s.essentials.len() < 12 {
                    items.push((19, "Add to Essentials".into()));
                }
                if !t.url.is_empty() && !t.private {
                    items.push((2, "Duplicate".into()));
                    items.push((7, "Add to new tab page".into()));
                }
                if s.workspaces.len() > 1 {
                    for (wi, w) in s.workspaces.iter().enumerate() {
                        if wi != s.active_workspace {
                            items.push((20 + wi as u32, format!("Move to {}", w.name)));
                        }
                    }
                } else {
                    items.push((15, "New workspace".into()));
                }
                if t.split || s.tabs.len() > 1 {
                    items.push((12, if t.split { "Unsplit".into() } else { "Split with next tab".into() }));
                }
                if t.servo {
                    items.push((14, "Reopen in WebView2".into()));
                }
                if t.url.starts_with("http") && (t.status == "live" || t.status == "asleep") {
                    items.push((10, "Save video".into()));
                    items.push((8, if t.reader { "Exit reader view\tF9".into() } else { "Reader view\tF9".into() }));
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
                if t.audio && !t.muted {
                    items.push((18, "Picture in picture".into()));
                }
                if !t.agent && t.status != "new" {
                    items.push(if t.veiled {
                        (27, "Unlock with Windows Hello".into())
                    } else if t.locked {
                        (26, "Remove the lock".into())
                    } else {
                        (25, "Lock this tab".into())
                    });
                }
                if s.settings.agent && !t.agent && !t.private && !t.locked && t.url.starts_with("http") {
                    items.push((16, if t.agent_ok { "Stop letting agents use this tab".into() } else { "Let agents use this tab".into() }));
                }
                if !t.pinned && t.id != s.active && (t.status == "live" || t.status == "asleep") {
                    items.push((4, "Unload now".into()));
                }
                items.push((11, "New private tab\tCtrl+Shift+N".into()));
                items.push((0, String::new()));
                items.push((5, "Close other tabs".into()));
                items.push((6, if t.pinned { "Unload, keep the pin\tCtrl+W".into() } else { "Close\tCtrl+W".into() }));
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
                        19 => "essentialAdd",
                        24 => "pinHome",
                        25 | 26 => "lockTab",
                        27 => {
                            send(&i.proxy, json!({"t": "helloUnlock", "what": "tab", "id": id}));
                            return;
                        }
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
            Hit::Essential(n) => {
                let Some(e) = s.essentials.get(n) else { return };
                let url = e.url.clone();
                let away = e.away;
                let mut items: Vec<(u32, String)> = vec![(1, "Remove from Essentials".into())];
                if away {
                    items.push((0, String::new()));
                    items.push((2, "Back to the address it was kept at".into()));
                    items.push((3, "Keep this page instead".into()));
                }
                menu::open(self, &items, x, y, move |i, cmd| match cmd {
                    1 => send(&i.proxy, json!({"t": "essentialRemove", "url": url})),
                    2 => send(&i.proxy, json!({"t": "essentialBack", "url": url})),
                    3 => send(&i.proxy, json!({"t": "essentialHome", "url": url})),
                    _ => {}
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
                items.push((5, "Colour".into()));
                items.push((4, "New workspace".into()));
                menu::open(self, &items, x, y, move |i, cmd| match cmd {
                    1 => {
                        let name = s.workspaces.get(w).map(|n| n.name.clone()).unwrap_or_default();
                        i.begin_workspace_rename(w, &name);
                    }
                    2 => send(&i.proxy, json!({"t": "workspaceDuplicate", "i": w})),
                    3 => send(&i.proxy, json!({"t": "workspaceClose", "i": w})),
                    4 => send(&i.proxy, json!({"t": "workspaceAdd"})),
                    5 => {
                        let pal: Vec<(u32, String)> = WORKSPACE_COLOURS
                            .iter()
                            .enumerate()
                            .map(|(n, (name, _))| (100 + n as u32, name.to_string()))
                            .collect();
                        menu::open(i, &pal, x, y, move |i, cmd| {
                            if cmd >= 100 {
                                send(&i.proxy, json!({"t": "workspaceColour", "i": w, "c": (cmd - 100).to_string()}));
                            }
                        });
                    }
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
        if menu::key(self, vk) {
            return true;
        }
        if color::key(self, vk) {
            return true;
        }
        if form::key(self, vk) {
            return true;
        }
        if confirm::key(self, vk) {
            return true;
        }
        if files::key(self, vk) {
            return true;
        }
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
                let _ = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(h.0)));
            }
        }
        let _ = CloseClipboard();
    }
}

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

fn wallpaper_key(path: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    (h.finish() | (1 << 62)) & !(1 << 63)
}

fn inner_of(hwnd: HWND) -> Option<&'static RefCell<Inner>> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<Inner>;
    if p.is_null() { None } else { Some(unsafe { &*p }) }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let Some(cell) = inner_of(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
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
                i.side_leave();
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
                        EN_CHANGE => {
                            if i.renaming.is_none() {
                                i.suggest();
                            }
                        }
                        EN_KILLFOCUS => i.end_edit(),
                        _ => {}
                    }
                } else if id == ID_SAY && code == EN_CHANGE {
                    i.invalidate();
                } else if id == ID_NTP && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    if code == EN_CHANGE {
                        let edit = i.ntp_edit;
                        let q = i.ntp_typed.changed(edit).trim().to_string();
                        if i.ntp_typed.ok && !q.is_empty() {
                            send(&i.proxy, json!({"t": "complete", "q": q}));
                        }
                    }
                    i.layout_edits();
                    i.invalidate();
                } else if id == ID_BM && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    if code == EN_CHANGE {
                        i.page.scroll = 0.0;
                    }
                    i.invalidate();
                } else if id == ID_DRIVE && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    i.invalidate();
                } else if id == ID_BUG && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    i.invalidate();
                } else if id == ID_SET && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    if code == EN_CHANGE {
                        i.page.scroll = 0.0;
                    }
                    i.invalidate();
                } else if id == ID_SRCH && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    if code == EN_CHANGE {
                        i.search_sel = 0;
                    }
                    i.invalidate();
                } else if id == ID_COLOR && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    if code == EN_CHANGE {
                        color::typed(&mut i);
                    }
                    i.invalidate();
                } else if (ID_FORM0..=ID_FILENAME).contains(&id) && code == EN_CHANGE {
                    i.invalidate();
                } else if id == ID_PAL && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    if code == EN_CHANGE {
                        i.pal_sel = 0;
                    }
                    i.invalidate();
                }
                LRESULT(0)
            }
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wp.0 as _);
                let h = HWND(lp.0 as _);
                if h == i.ntp_edit {
                    let fill = i.theme.solid(ntp_bar_fill(&i.look));
                    let ink = if i.look.style == theme::Style::Signal { i.theme.pill_ink } else { i.theme.text };
                    let _ = SetTextColor(hdc, COLORREF(ink.colorref()));
                    let _ = SetBkColor(hdc, COLORREF(fill.colorref()));
                    return LRESULT(i.bar_brush.0 as isize);
                }
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
                            let _ = SetFocus(Some(parent));
                            return LRESULT(0);
                        }
                        _ => {}
                    }
                } else if (id == ID_BM || id == ID_SET || id == ID_BUG) && vk == VK_ESCAPE.0 {
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
                let down = |k: u16| GetKeyState(k as i32) < 0;
                if let Some(a) = crate::engine::map_key(vk as u32, down(VK_CONTROL.0), down(VK_SHIFT.0), down(VK_MENU.0)) {
                    let _ = i.proxy.send_event(Ev::Key(a));
                    return LRESULT(0);
                }
            }
        }
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
            essential: false,
            audio: false,
            muted: false,
            call: false,
            status: "live",
            agent: false,
            agent_ok: false,
            locked: false,
            veiled: false,
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
        merge_hits(&mut list, vec![hit("https://doc.rust-lang.org/book/", "The Book", false)]);
        assert!(matches!(list[4].kind, SuggKind::Url(_, false)));
        merge_hits(&mut list, vec![hit("doc.rust-lang.org", "already there", false)]);
        assert_eq!(list.len(), 5);
        let mut list = vec![s(SuggKind::TabOther(1, 2), "Mail (Work)")];
        merge_hits(&mut list, vec![hit("https://mail.example/", "Mail", true)]);
        assert!(matches!(list[0].kind, SuggKind::TabOther(1, 2)));
        assert!(matches!(list[1].kind, SuggKind::Url(_, true)));
    }

    #[test]
    fn a_tab_row_says_what_it_costs() {
        const MB: u64 = 1024 * 1024;
        assert_eq!(mem_label(212 * MB), "212 MB");
        assert_eq!(mem_label(1400 * MB), "1.4 GB");
        assert!(500 * MB >= HEAVY_TAB && 499 * MB < HEAVY_TAB);
    }

    #[test]
    fn the_tab_search_covers_every_workspace() {
        let tabs = [tab(1, "Docs", "https://docs.test/", false), tab(2, "Inbox", "https://mail.test/", true)];
        let other = [UiOther { ws: 2, idx: 0, ws_name: "Work".into(), title: "Standup notes".into(), url: "https://notes.test/".into() }];
        let all = switch_candidates(&tabs, &other);
        assert_eq!(all.len(), 3);
        assert_eq!(all[2].other, Some((2, 0)));
        assert!(all[1].private);
        let rows = rank_rows(&all, "");
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2, 0]);
        assert_eq!(rows[2].label(), "Standup notes");
        let rows = rank_rows(&all, "standup");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].ws_name, "Work");
        assert!(rank_rows(&all, "docs").iter().any(|r| r.id == 1));
        assert!(rank_rows(&all, "docs zebra").is_empty());
    }
}
