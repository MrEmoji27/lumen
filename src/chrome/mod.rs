//! Native browser chrome. One child window, drawn with Direct2D.
//!
//! Rust state comes in through `set_state` (at most once a frame). User actions go
//! out as the same small JSON messages the old HTML UI sent, so main.rs handles
//! them unchanged. The address bar and new-tab search are real Win32 edit boxes;
//! everything else is drawn.

mod anim;
mod gfx;
mod pages;
pub mod theme;

use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};

use serde_json::{Value, json};
use tao::event_loop::EventLoopProxy;
use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CLEARTYPE_QUALITY, ClientToScreen, ScreenToClient, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateFontW,
            CreateSolidBrush, DEFAULT_CHARSET, DeleteDC, DeleteObject, EndPaint, FF_DONTCARE, FW_NORMAL, GetTextFaceW, HBRUSH, HDC,
            HFONT, InvalidateRect, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SelectObject, SetBkColor, SetTextColor,
        },
        System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW},
        UI::{
            Controls::{EM_SETMARGINS, EM_SETSEL, WM_MOUSELEAVE},
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{
                GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, VK_CONTROL,
                VK_DOWN, VK_ESCAPE, VK_F10, VK_MENU, VK_RETURN, VK_SHIFT, VK_UP,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCSTR, PCWSTR, w},
};

use crate::{engine::Ev, store::Settings};
use gfx::{Align, Font, Gfx, R};
use theme::Theme;

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
    pub audio: bool,
    pub muted: bool,
    pub status: &'static str,
    pub agent: bool,
    pub reader: bool,
    /// InPrivate tab. Marked in the sidebar, never saved anywhere.
    pub private: bool,
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
    pub blocked: u64,
    pub cpu_cap: bool,
    pub downloads: Vec<UiDl>,
    pub agent_busy: bool,
    pub agent_cmd: String,
    /// A site waiting for a permission answer: (request id, origin, what).
    pub prompt: Option<(u64, String, &'static str)>,
    /// An agent waiting for the person: (agent name, message).
    pub handoff: Option<(String, String)>,
    /// Remembered answers: (origin, what, allowed).
    pub rules: Vec<(String, String, bool)>,
    /// The active page is bookmarked.
    pub bookmarked: bool,
    /// Filled only while the History page is open.
    pub history: Vec<UiVisit>,
    /// Filled only while the Bookmarks page is open: (url, title).
    pub bookmarks: Vec<(String, String)>,
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
    NewTab,
    Gear,
    Mem,
    DlFooter,
    AgentStop,
    PermAllow(u64),
    HandoffDone,
    PermBlock(u64),
    Star,
    Sugg(usize),
    Shortcut(usize),
    NtpSearch,
    /// Page widgets (settings and Control live in pages.rs).
    Page(pages::PageHit),
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
    /// From history (false) or bookmarks (true).
    Url(String, bool),
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
    pub(crate) state: Option<UiState>,
    hits: Vec<(R, Hit)>,
    hover: Option<Hit>,
    press: Option<Hit>,
    scroll: f32,
    list_max: f32,
    drag: Option<Drag>,
    favs: HashMap<u64, Vec<u8>>,
    edit: HWND,
    ntp_edit: HWND,
    edit_font: HFONT,
    /// The family the edit boxes were made with, to remake them when it changes.
    edit_face: String,
    ntp_font: HFONT,
    edit_brush: HBRUSH,
    ntp_brush: HBRUSH,
    editing: bool,
    sugg: Vec<Sugg>,
    sel: usize,
    born: Instant,
    spinning: bool,
    anim: anim::Anims,
    animating: bool,
    pub(crate) page: pages::Page,
    theme_key: String,
    mouse: (f32, f32),
}

/// The native chrome. Cheap to clone the handle; state lives behind the RefCell.
pub struct Chrome {
    pub hwnd: HWND,
    inner: Rc<RefCell<Inner>>,
}

const CLASS: PCWSTR = w!("LumenChrome");
const ID_EDIT: usize = 101;
const ID_NTP: usize = 102;
const TIMER_SPIN: usize = 1;
const TIMER_CLOCK: usize = 2;
const TIMER_ANIM: usize = 3;

fn send(proxy: &EventLoopProxy<Ev>, v: Value) {
    let _ = proxy.send_event(Ev::Ui(v.to_string()));
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

fn host(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or("");
    let h = rest.split(['/', '?', '#']).next().unwrap_or("");
    let h = h.rsplit('@').next().unwrap_or(h);
    h.strip_prefix("www.").unwrap_or(h).to_string()
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
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
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
            for e in [edit, ntp_edit] {
                let old = SetWindowLongPtrW(e, GWLP_WNDPROC, edit_proc as *const () as isize);
                SetWindowLongPtrW(e, GWLP_USERDATA, old);
            }
            let dpi = GetDpiForWindow(hwnd) as f32;
            let gfx = Gfx::new(hwnd, dpi).expect("direct2d");
            let inner = Rc::new(RefCell::new(Inner {
                hwnd,
                proxy,
                gfx,
                theme: Theme::new("graphite", true, ""),
                state: None,
                hits: Vec::new(),
                hover: None,
                press: None,
                scroll: 0.0,
                list_max: 0.0,
                drag: None,
                favs: HashMap::new(),
                edit,
                ntp_edit,
                edit_font: HFONT::default(),
                edit_face: String::new(),
                ntp_font: HFONT::default(),
                edit_brush: HBRUSH::default(),
                ntp_brush: HBRUSH::default(),
                editing: false,
                sugg: Vec::new(),
                sel: 0,
                born: Instant::now(),
                spinning: false,
                anim: anim::Anims::default(),
                animating: false,
                page: pages::Page::default(),
                theme_key: String::new(),
                mouse: (-1.0, -1.0),
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

    /// Sidebar color, for the window behind the page gaps.
    pub fn bg(&self) -> (u8, u8, u8) {
        self.inner.try_borrow().map(|i| i.theme.bg.rgb8()).unwrap_or((24, 24, 26))
    }

    pub fn set_visible(&self, on: bool) {
        unsafe {
            let _ = ShowWindow(self.hwnd, if on { SW_SHOWNA } else { SW_HIDE });
        }
    }

    pub fn set_state(&self, s: UiState) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        i.apply(s);
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

    /// History and bookmark matches for `q`, from the main loop. Merged if still current.
    /// Hits are (url, title, bookmarked, detail). Detail, when not empty, is a second line.
    pub fn set_suggest(&self, q: &str, hits: Vec<(String, String, bool, String)>) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        if !i.editing || wide_text(i.edit).trim() != q {
            return;
        }
        let at = i.sugg.iter().position(|s| matches!(s.kind, SuggKind::Tab(_))).unwrap_or(i.sugg.len());
        let mut add = Vec::new();
        for (url, title, bm, detail) in hits {
            if i.sugg.iter().any(|s| matches!(&s.kind, SuggKind::Go(u) | SuggKind::Url(u, _) if *u == url)) {
                continue;
            }
            let label = if title.is_empty() { url.clone() } else { title };
            add.push(Sugg { kind: SuggKind::Url(url, bm), label, detail });
        }
        for (n, s) in add.into_iter().enumerate() {
            i.sugg.insert(at + n, s);
        }
        i.sugg.truncate(9);
        i.invalidate();
    }

    pub fn usage(&self, v: Value) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        i.page.usage = Some(v);
        if i.state.as_ref().is_some_and(|s| s.panel == "control") {
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
            i.begin_edit();
            i.edit
        };
        unsafe {
            let _ = SetFocus(Some(edit));
            SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        }
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
            for e in [self.edit, self.ntp_edit] {
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
            self.edit_brush = CreateSolidBrush(COLORREF(self.theme.raise.colorref()));
            self.ntp_brush = CreateSolidBrush(COLORREF(self.theme.bg.colorref()));
        }
    }

    fn apply(&mut self, s: UiState) {
        self.gfx.set_families(&s.settings.ui_font, &s.settings.display_font);
        if self.gfx.ui_family() != self.edit_face && !self.edit.is_invalid() {
            self.make_fonts();
        }
        let key = format!("{}|{}|{}", s.settings.palette, s.dark, s.settings.accent);
        if key != self.theme_key {
            self.theme_key = key;
            self.theme = Theme::new(&s.settings.palette, s.dark, &s.settings.accent);
            self.make_brushes();
            dark_menus(s.dark);
        }
        let was_ntp = self.ntp_visible();
        self.state = Some(s);
        if !self.editing {
            self.sugg.clear();
        }
        let now_ntp = self.ntp_visible();
        if now_ntp != was_ntp {
            unsafe {
                if now_ntp {
                    SetWindowTextW(self.ntp_edit, &HSTRING::new()).ok();
                    SetTimer(Some(self.hwnd), TIMER_CLOCK, 10_000, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_CLOCK);
                }
            }
        }
        self.layout_edits();
        self.spin();
        self.invalidate();
    }

    fn ntp_visible(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| s.full && s.panel == "none" && s.active_tab().is_some_and(|t| t.status == "new"))
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

    // ------------------------------------------------------------ layout

    fn sidebar(&self) -> R {
        let s = self.state.as_ref().unwrap();
        let (w, h) = self.size_dip();
        let sw = s.sidebar_w();
        let x = if s.full && s.settings.sidebar_side == "right" { w - sw } else { 0.0 };
        R::new(x, 0.0, sw, h)
    }

    fn size_dip(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        (rc.right as f32 / self.k(), rc.bottom as f32 / self.k())
    }

    fn omni_rect(&self) -> R {
        let sb = self.sidebar();
        let s = self.state.as_ref().unwrap();
        if s.compact() {
            R::new(sb.x + (sb.w - 36.0) / 2.0, 104.0, 36.0, 34.0)
        } else {
            R::new(sb.x + 8.0, 46.0, sb.w - 16.0, 34.0)
        }
    }

    fn ntp_field(&self) -> R {
        let s = self.state.as_ref().unwrap();
        let (x, y, w, h) = s.stage;
        let fw = (w * 0.82).min(560.0);
        R::new(x + (w - fw) / 2.0, y + h * 0.5 - 23.0, fw, 46.0)
    }

    /// Edit boxes are real windows; keep them on top of what is drawn under them.
    fn layout_edits(&self) {
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
        place(self.edit, R::new(o.x + 32.0, o.y + (o.h - text_h) / 2.0, o.w - 42.0, text_h), self.editing && !s.compact());
        let f = self.ntp_field();
        // The box hides when idle and empty, so the drawn placeholder shows through.
        let ntp_live = unsafe { GetFocus() } == self.ntp_edit || !wide_text(self.ntp_edit).is_empty();
        place(self.ntp_edit, R::new(f.x + 42.0, f.y + (f.h - 22.0) / 2.0, f.w - 58.0, 22.0), self.ntp_visible() && ntp_live);
    }

    fn focus_ntp(&mut self) {
        unsafe {
            let _ = ShowWindow(self.ntp_edit, SW_SHOWNA);
            let _ = SetFocus(Some(self.ntp_edit));
        }
        self.layout_edits();
        self.invalidate();
    }

    fn begin_edit(&mut self) {
        let Some(s) = &self.state else { return };
        if s.compact() {
            send(&self.proxy, json!({"t": "peek", "on": true}));
        }
        let url = s.active_tab().filter(|t| t.status != "new").map(|t| t.url.clone()).unwrap_or_default();
        self.editing = true;
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
        self.sugg.clear();
        if self.state.as_ref().is_some_and(|s| s.settings.compact) {
            send(&self.proxy, json!({"t": "peek", "on": false}));
        }
        self.layout_edits();
        self.invalidate();
    }

    fn suggest(&mut self) {
        let q = wide_text(self.edit).trim().to_string();
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
        let lq = q.to_lowercase();
        if let Some(s) = &self.state {
            for t in s.tabs.iter().filter(|t| t.id != s.active && !t.url.is_empty()) {
                if self.sugg.len() >= 6 {
                    break;
                }
                if t.title.to_lowercase().contains(&lq) || t.url.to_lowercase().contains(&lq) {
                    let label = if t.title.is_empty() { host(&t.url) } else { t.title.clone() };
                    self.sugg.push(Sugg { kind: SuggKind::Tab(t.id), label, detail: String::new() });
                }
            }
        }
        self.invalidate();
    }

    fn pick(&mut self, i: usize) {
        let Some(sg) = self.sugg.get(i).cloned() else { return };
        match sg.kind {
            SuggKind::Tab(id) => send(&self.proxy, json!({"t": "activate", "id": id})),
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
        if !self.gfx.begin(rc.right as u32, rc.bottom as u32, th.bg) {
            return;
        }
        self.hits.clear();
        self.anim.enabled = s.settings.animations && anim::system_allows();
        self.anim.begin();
        if s.full {
            let (x, y, w, h) = s.stage;
            let st = R::new(x, y, w, h);
            self.gfx.round(st, s.settings.radius as f32, th.raise);
            if s.panel != "none" {
                pages::draw(self, st, &s);
                // Panels fade in over a few frames instead of popping.
                let a = self.anim.from(&format!("panel:{}", s.panel), 0.0, 1.0, 0.06);
                if a < 1.0 {
                    self.gfx.round(st, s.settings.radius as f32, th.raise.alpha(1.0 - a));
                }
            } else if self.ntp_visible() {
                self.draw_ntp(st, &s);
            }
        }
        self.draw_sidebar(&s);
        self.gfx.end();
        self.anim.end();
        // The frame timer runs only while something moves.
        if self.anim.moving != self.animating {
            self.animating = self.anim.moving;
            unsafe {
                if self.animating {
                    SetTimer(Some(self.hwnd), TIMER_ANIM, 16, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TIMER_ANIM);
                }
            }
        }
    }

    fn hit(&mut self, r: R, h: Hit) {
        self.hits.push((r, h));
    }

    fn is_hover(&self, h: &Hit) -> bool {
        self.hover.as_ref() == Some(h)
    }

    fn button(&mut self, r: R, icon: &str, h: Hit, enabled: bool) {
        let th = self.theme;
        let hov = enabled && self.is_hover(&h);
        let a = self.anim.get(&format!("btn:{h:?}"), if hov { 1.0 } else { 0.0 }, 0.05);
        if a > 0.01 {
            let bg = if h == Hit::Close { th.danger } else if self.press.as_ref() == Some(&h) { th.press } else { th.hover };
            self.gfx.round(r, 7.0, bg.alpha(a));
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

    fn draw_sidebar(&mut self, s: &UiState) {
        let th = self.theme;
        let sb = self.sidebar();
        let compact = s.compact();
        let act = s.active_tab().cloned();
        // Top bar: navigation and window buttons. The empty rest drags the window.
        self.hit(R::new(sb.x, 0.0, sb.w, 40.0), Hit::Drag);
        if compact {
            let x = sb.x + (sb.w - 28.0) / 2.0;
            self.button(R::new(x, 8.0, 28.0, 28.0), "close", Hit::Close, true);
            self.button(R::new(x, 38.0, 28.0, 28.0), if s.maximized { "restore" } else { "max" }, Hit::Max, true);
            self.button(R::new(x, 68.0, 28.0, 28.0), "min", Hit::Min, true);
        } else {
            // Navigation on the left, window buttons on the right edge, on either side.
            let (nav_x, win_x) = (sb.x + 8.0, sb.right() - 8.0 - 86.0);
            let loading = act.as_ref().is_some_and(|t| t.loading);
            let navs = [
                ("back", Hit::Back, act.as_ref().is_some_and(|t| t.back)),
                ("fwd", Hit::Fwd, act.as_ref().is_some_and(|t| t.fwd)),
                (if loading { "stop" } else { "reload" }, Hit::Reload, true),
            ];
            for (i, (icon, h, en)) in navs.into_iter().enumerate() {
                self.button(R::new(nav_x + i as f32 * 29.0, 9.0, 28.0, 28.0), icon, h, en);
            }
            let wins = [("min", Hit::Min), (if s.maximized { "restore" } else { "max" }, Hit::Max), ("close", Hit::Close)];
            for (i, (icon, h)) in wins.into_iter().enumerate() {
                self.button(R::new(win_x + i as f32 * 29.0, 9.0, 28.0, 28.0), icon, h, true);
            }
        }

        // Address bar.
        let o = self.omni_rect();
        let hov = self.is_hover(&Hit::Omni);
        let focused = self.editing;
        self.gfx.round(o, 9.0, if hov && !focused { th.hover } else { th.raise });
        if focused {
            self.gfx.round_stroke(o, 9.0, th.acc_soft, 1.5);
        }
        let secure = act.as_ref().is_some_and(|t| t.url.starts_with("https://"));
        if compact {
            self.gfx.icon("search", o.x + (o.w - 16.0) / 2.0, o.y + 9.0, 16.0, th.dim);
        } else {
            self.gfx.icon(if secure && !focused { "lock" } else { "search" }, o.x + 10.0, o.y + 9.0, 16.0, if secure { th.dim } else { th.faint });
            if !focused {
                let text = act.as_ref().filter(|t| t.status != "new").map(|t| {
                    let h = host(&t.url);
                    if h.is_empty() { t.url.clone() } else { h }
                });
                match text {
                    Some(t) => self.gfx.text(&t, R::new(o.x + 32.0, o.y, o.w - 42.0, o.h), Font::ui(13.0, 400), th.text),
                    None => self.gfx.text("Search or enter address", R::new(o.x + 32.0, o.y, o.w - 42.0, o.h), Font::ui(13.0, 400), th.faint),
                }
            }
        }
        self.hit(o, Hit::Omni);
        if !compact && !focused && act.as_ref().is_some_and(|t| !t.url.is_empty() && !t.agent) {
            let r = R::new(o.right() - 30.0, o.y + 5.0, 24.0, 24.0);
            if self.is_hover(&Hit::Star) {
                self.gfx.round(r, 6.0, th.hover);
            }
            self.gfx.icon(if s.bookmarked { "star-on" } else { "star" }, r.x + 4.0, r.y + 4.0, 16.0, if s.bookmarked { th.accent } else { th.faint });
            self.hit(r, Hit::Star);
        }

        // Tabs.
        let list_top = if compact { 148.0 } else { 90.0 };
        let list = R::new(sb.x, list_top, sb.w, sb.h - list_top - 40.0);
        self.gfx.clip(list);
        let mut y = list.y - self.scroll;
        let pad = if compact { 6.0 } else { 8.0 };
        let pins: Vec<&UiTab> = s.tabs.iter().filter(|t| t.pinned).collect();
        if !pins.is_empty() {
            let cols = if compact { 1 } else { (((sb.w - 2.0 * pad) + 4.0) / 44.0).floor().max(1.0) as usize };
            let tw = ((sb.w - 2.0 * pad) - 4.0 * (cols as f32 - 1.0)) / cols as f32;
            for (i, t) in pins.iter().enumerate() {
                let r = R::new(sb.x + pad + (i % cols) as f32 * (tw + 4.0), y + (i / cols) as f32 * 38.0, tw, 34.0);
                self.draw_tab(t, r, s, true);
            }
            y += ((pins.len() + cols - 1) / cols) as f32 * 38.0 + 4.0;
            self.gfx.line(sb.x + pad, y, sb.right() - pad, y, th.line, 1.0);
            y += 9.0;
        }
        // The active highlight slides between tabs. Animated in list space, so scrolling
        // moves it with the list instead of making it chase.
        let mut ay = y;
        let mut found = false;
        for t in s.tabs.iter().filter(|t| !t.pinned) {
            if t.id == s.active {
                found = true;
                break;
            }
            ay += 33.0;
        }
        if found && !self.drag.as_ref().is_some_and(|d| d.moved && d.id == s.active) {
            let target = ay + self.scroll;
            let slid = self.anim.get("active-y", target, 0.07) - self.scroll;
            self.gfx.round(R::new(sb.x + pad, slid, sb.w - 2.0 * pad, 32.0), 8.0, th.raise);
        }
        for t in s.tabs.iter().filter(|t| !t.pinned) {
            let r = R::new(sb.x + pad, y, sb.w - 2.0 * pad, 32.0);
            self.draw_tab(t, r, s, false);
            y += 33.0;
        }
        let nr = R::new(sb.x + pad, y + 3.0, sb.w - 2.0 * pad, 32.0);
        let hov = self.is_hover(&Hit::NewTab);
        if hov {
            self.gfx.round(nr, 8.0, th.hover);
        }
        let c = if hov { th.text } else { th.faint };
        if compact {
            self.gfx.icon("plus", nr.x + (nr.w - 15.0) / 2.0, nr.y + 8.5, 15.0, c);
        } else {
            self.gfx.icon("plus", nr.x + 9.0, nr.y + 8.5, 15.0, c);
            self.gfx.text("New tab", R::new(nr.x + 34.0, nr.y, nr.w - 40.0, nr.h), Font::ui(13.0, 400), c);
        }
        if nr.y < list.bottom() {
            self.hit(nr, Hit::NewTab);
        }
        self.list_max = (y + 3.0 + 32.0 + self.scroll - list.y - list.h).max(0.0);
        // Drag ghost on top.
        if let Some(d) = &self.drag
            && d.moved
            && let Some(t) = s.tabs.iter().find(|t| t.id == d.id)
        {
            let (dx, dy, pinned) = (d.dx, d.dy, d.pinned);
            let base = self.hits.iter().find(|(_, h)| *h == Hit::Tab(t.id)).map(|(r, _)| *r);
            if let Some(b) = base {
                let r = R::new(b.x + if pinned { dx } else { 0.0 }, b.y + dy, b.w, b.h);
                self.gfx.round(r, 8.0, th.press);
                self.draw_tab_body(t, r, s, pinned, true);
            }
        }
        self.gfx.unclip();

        // Suggestions float over the list.
        if !(self.editing && !self.sugg.is_empty()) {
            // Reset the entrance so the next dropdown animates again.
            self.anim.get("sugg", 0.0, 0.001);
        }
        if self.editing && !self.sugg.is_empty() {
            let t = self.anim.from("sugg", 0.0, 1.0, 0.05);
            let row_h = |s: &Sugg| if s.detail.is_empty() { 31.0 } else { 47.0 };
            let h = self.sugg.iter().map(row_h).sum::<f32>() + 8.0;
            let box_ = R::new(o.x, o.bottom() + 6.0 - (1.0 - t) * 6.0, o.w, h);
            self.gfx.round(R::new(box_.x, box_.y + 4.0, box_.w, box_.h), 10.0, th.shade.alpha(0.6));
            self.gfx.round(box_, 10.0, th.raise);
            self.gfx.round_stroke(box_, 10.0, th.line, 1.0);
            let mut y = box_.y + 4.0;
            for i in 0..self.sugg.len() {
                let rh = row_h(&self.sugg[i]);
                let r = R::new(box_.x + 4.0, y, box_.w - 8.0, rh - 1.0);
                y += rh;
                if i == self.sel {
                    self.gfx.round(r, 7.0, th.hover);
                }
                let sg = self.sugg[i].clone();
                let (icon, hint) = match sg.kind {
                    SuggKind::Go(_) => ("globe", "Open"),
                    SuggKind::Search(_) => ("search", "Search"),
                    SuggKind::Tab(_) => ("tab", "Switch to tab"),
                    SuggKind::Url(_, true) => ("star", "Bookmark"),
                    SuggKind::Url(_, false) if !sg.detail.is_empty() => ("clock", "In page"),
                    SuggKind::Url(_, false) => ("clock", "History"),
                };
                self.gfx.icon(icon, r.x + 8.0, r.y + 7.0, 16.0, th.faint);
                let hw = self.gfx.measure(hint, Font::ui(11.5, 400)) + 10.0;
                self.gfx.text(&sg.label, R::new(r.x + 33.0, r.y, r.w - 33.0 - hw, 30.0), Font::ui(13.0, 400), th.text);
                self.gfx.text(hint, R::new(r.right() - hw, r.y, hw - 8.0, 30.0), Font::ui(11.5, 400).align(Align::Right), th.faint);
                if !sg.detail.is_empty() {
                    self.gfx.text(&sg.detail, R::new(r.x + 33.0, r.y + 24.0, r.w - 41.0, 18.0), Font::ui(11.5, 400), th.dim);
                }
                self.hit(r, Hit::Sugg(i));
            }
        }

        // An agent asking the person to take over: same place, accent card, one button.
        if let Some((who, msg)) = s.handoff.as_ref().filter(|_| s.prompt.is_none()) {
            let card = if compact { R::new(sb.x + 4.0, sb.h - 166.0, sb.w - 8.0, 122.0) } else { R::new(sb.x + 8.0, sb.h - 166.0, sb.w - 16.0, 122.0) };
            self.gfx.round(R::new(card.x, card.y + 3.0, card.w, card.h), 10.0, th.shade.alpha(0.5));
            self.gfx.round(card, 10.0, th.raise);
            self.gfx.round_stroke(card, 10.0, th.accent, 1.0);
            self.gfx.text(&format!("{who} needs you"), R::new(card.x + 12.0, card.y + 8.0, card.w - 24.0, 20.0), Font::ui(12.5, 600), th.accent);
            self.gfx.text_wrap(msg, R::new(card.x + 12.0, card.y + 30.0, card.w - 24.0, 46.0), Font::ui(12.5, 400), th.text);
            let done = R::new(card.x + 10.0, card.bottom() - 40.0, card.w - 20.0, 30.0);
            let h = self.is_hover(&Hit::HandoffDone);
            self.gfx.round(done, 8.0, if h { th.accent } else { th.accent.alpha(0.85) });
            self.gfx.text("Done, hand it back", done, Font::ui(12.5, 600).align(Align::Center), th.bg);
            self.hit(done, Hit::HandoffDone);
        }

        // A site asking for something: a card above the footer. The page covers
        // everything else, so the sidebar is where the question lives.
        if let Some((pid, origin, kind)) = &s.prompt {
            let card = if compact { R::new(sb.x + 4.0, sb.h - 150.0, sb.w - 8.0, 106.0) } else { R::new(sb.x + 8.0, sb.h - 150.0, sb.w - 16.0, 106.0) };
            self.gfx.round(R::new(card.x, card.y + 3.0, card.w, card.h), 10.0, th.shade.alpha(0.5));
            self.gfx.round(card, 10.0, th.raise);
            self.gfx.round_stroke(card, 10.0, th.accent.alpha(0.5), 1.0);
            let site = host(origin);
            self.gfx.text(&site, R::new(card.x + 12.0, card.y + 8.0, card.w - 24.0, 20.0), Font::ui(12.5, 600), th.text);
            self.gfx.text(&ask_text(kind), R::new(card.x + 12.0, card.y + 28.0, card.w - 24.0, 20.0), Font::ui(12.5, 400), th.dim);
            let bw = (card.w - 30.0) / 2.0;
            let block = R::new(card.x + 10.0, card.bottom() - 40.0, bw, 30.0);
            let allow = R::new(block.right() + 10.0, block.y, bw, 30.0);
            let hb = self.is_hover(&Hit::PermBlock(*pid));
            let ha = self.is_hover(&Hit::PermAllow(*pid));
            self.gfx.round(block, 8.0, if hb { th.press } else { th.bg });
            self.gfx.text("Block", block, Font::ui(12.5, 500).align(Align::Center), th.text);
            self.gfx.round(allow, 8.0, if ha { th.accent } else { th.accent.alpha(0.85) });
            self.gfx.text("Allow", allow, Font::ui(12.5, 600).align(Align::Center), th.bg);
            self.hit(block, Hit::PermBlock(*pid));
            self.hit(allow, Hit::PermAllow(*pid));
        }

        // Footer: memory readout and settings.
        let fy = sb.h - 38.0;
        let gear = if compact { R::new(sb.x + (sb.w - 28.0) / 2.0, fy + 2.0, 28.0, 28.0) } else { R::new(sb.right() - 8.0 - 28.0, fy + 2.0, 28.0, 28.0) };
        self.button(gear, "sliders", Hit::Gear, true);
        // Downloads in flight: an arrow with a progress ring, left of the gear.
        let live: Vec<&UiDl> = s.downloads.iter().filter(|d| d.state == "progress").collect();
        let mut mem_room = sb.w - 56.0;
        if !live.is_empty() && !compact {
            let r = R::new(gear.x - 30.0, gear.y, 28.0, 28.0);
            let (got, total): (i64, i64) = live.iter().fold((0, 0), |a, d| (a.0 + d.received, a.1 + d.total.max(0)));
            let p = if total > 0 { got as f32 / total as f32 } else { 0.0 };
            if self.is_hover(&Hit::DlFooter) {
                self.gfx.round(r, 7.0, th.hover);
            }
            self.gfx.ring(r.x + 14.0, r.y + 14.0, 10.0, 10.0, th.line, 1.5);
            if p > 0.0 {
                let p = self.anim.get("dl-ring", p.min(1.0), 0.15);
                self.gfx.arc(r.x + 14.0, r.y + 14.0, 10.0, -90.0, 359.0 * p, th.accent, 2.0);
            }
            self.gfx.icon("download", r.x + 8.0, r.y + 8.0, 12.0, th.dim);
            self.hit(r, Hit::DlFooter);
            mem_room -= 30.0;
        }
        if s.agent_busy && !compact {
            let t = "Agent working · Stop";
            let tw = self.gfx.measure(t, Font::ui(11.5, 500)) + 28.0;
            let r = R::new(sb.x + 8.0, fy + 5.0, tw.min(mem_room), 24.0);
            let hov = self.is_hover(&Hit::AgentStop);
            self.gfx.round(r, 12.0, if hov { th.accent.alpha(0.35) } else { th.accent.alpha(0.18) });
            self.gfx.circle(r.x + 10.0, r.y + 12.0, 3.0, th.accent);
            self.gfx.text(t, R::new(r.x + 18.0, r.y, r.w - 18.0, r.h), Font::ui(11.5, 500), th.text);
            self.hit(r, Hit::AgentStop);
            return;
        }
        if !compact && s.settings.show_memory
            && let Some((bytes, _)) = s.mem
        {
            let mb = bytes as f64 / 1048576.0;
            let mut t = if mb >= 1024.0 { format!("{:.2} GB", mb / 1024.0) } else { format!("{} MB", mb.round()) };
            if s.settings.adblock && s.blocked > 0 {
                t.push_str(&format!(" · {} blocked", s.blocked));
            }
            let tw = self.gfx.measure(&t, Font::ui(11.5, 400)) + 10.0;
            let r = R::new(sb.x + 8.0, fy + 6.0, tw.min(mem_room), 22.0);
            let hov = self.is_hover(&Hit::Mem);
            if hov {
                self.gfx.round(r, 6.0, th.hover);
            }
            self.gfx.text(&t, R::new(r.x + 5.0, r.y, r.w - 5.0, r.h), Font::ui(11.5, 400), if hov { th.text } else { th.faint });
            self.hit(r, Hit::Mem);
        }
    }

    fn draw_tab(&mut self, t: &UiTab, r: R, s: &UiState, pinned: bool) {
        let th = self.theme;
        if self.drag.as_ref().is_some_and(|d| d.moved && d.id == t.id) {
            self.hit(r, Hit::Tab(t.id));
            return;
        }
        let active = t.id == s.active;
        let hov = self.is_hover(&Hit::Tab(t.id)) || self.is_hover(&Hit::TabX(t.id)) || self.is_hover(&Hit::TabSnd(t.id));
        let h = self.anim.get(&format!("tab:{}", t.id), if hov { 1.0 } else { 0.0 }, 0.05);
        if pinned {
            let base = if active { th.press } else { th.raise };
            self.gfx.round(r, 8.0, if active { base } else { base.mix(th.hover, h) });
        } else if !active && h > 0.01 {
            self.gfx.round(r, 8.0, th.hover.alpha(h));
        }
        self.hit(r, Hit::Tab(t.id));
        self.draw_tab_body(t, r, s, pinned, hov || active);
    }

    fn draw_tab_body(&mut self, t: &UiTab, r: R, s: &UiState, pinned: bool, show_x: bool) {
        let th = self.theme;
        let compact = s.compact();
        let active = t.id == s.active;
        // Asleep, unloaded or dead: drawn dim. None of them are live pages.
        let asleep = matches!(t.status, "asleep" | "off" | "crashed");
        let (fx, fy) = if pinned || compact { (r.x + (r.w - 16.0) / 2.0, r.y + (r.h - 16.0) / 2.0) } else { (r.x + 9.0, r.y + 8.0) };
        let fav = R::new(fx, fy, 16.0, 16.0);
        if t.loading {
            let phase = (self.born.elapsed().as_secs_f32() / 0.8).fract();
            self.gfx.spinner(fx + 8.0, fy + 8.0, 6.5, phase, th.line, th.accent);
        } else {
            let png = self.favs.get(&t.id).cloned();
            let bmp = png.filter(|p| !p.is_empty()).and_then(|p| self.gfx.bitmap(t.id, &p));
            match bmp {
                Some(b) => self.gfx.draw_bitmap(&b, fav, if asleep { 0.5 } else { 1.0 }),
                None => {
                    let letter = if t.status == "new" { "+".to_string() } else { host(&t.url).chars().next().unwrap_or('·').to_uppercase().to_string() };
                    let bg = th.press.mix(th.accent, 0.3);
                    self.gfx.round(fav, 5.0, if asleep { bg.alpha(0.5) } else { bg });
                    self.gfx.text(&letter, fav, Font::ui(9.5, 650).align(Align::Center), th.text.alpha(if asleep { 0.5 } else { 1.0 }));
                }
            }
        }
        if t.agent && (pinned || compact) {
            self.gfx.circle(r.right() - 5.0, r.y + 5.0, 3.0, th.accent);
        }
        if t.private && (pinned || compact) {
            // A dot, like the AI one, in a colour of its own.
            self.gfx.circle(r.right() - 5.0, r.y + 5.0, 3.0, private_mark(self.theme.dark));
        }
        if (t.audio || t.muted) && pinned {
            let b = R::new(r.right() - 15.0, r.y + 1.0, 14.0, 14.0);
            self.gfx.round(b, 4.0, th.bg);
            self.gfx.icon(if t.muted { "muted" } else { "sound" }, b.x + 2.0, b.y + 2.0, 10.0, th.dim);
            self.hit(b, Hit::TabSnd(t.id));
        }
        if pinned || compact {
            return;
        }
        // Right to left, each item takes its own room: close, sound, AI badge, then the title.
        let mut right = r.right() - 6.0;
        if show_x {
            let x = R::new(right - 22.0, r.y + 5.0, 22.0, 22.0);
            if self.is_hover(&Hit::TabX(t.id)) {
                self.gfx.round(x, 6.0, th.press);
            }
            self.gfx.icon("close", x.x + 4.0, x.y + 4.0, 14.0, if self.is_hover(&Hit::TabX(t.id)) { th.text } else { th.dim });
            self.hit(x, Hit::TabX(t.id));
            right = x.x - 2.0;
        }
        if t.audio || t.muted {
            let b = R::new(right - 22.0, r.y + 5.0, 22.0, 22.0);
            if self.is_hover(&Hit::TabSnd(t.id)) {
                self.gfx.round(b, 6.0, th.press);
            }
            self.gfx.icon(if t.muted { "muted" } else { "sound" }, b.x + 4.0, b.y + 4.0, 14.0, th.dim);
            self.hit(b, Hit::TabSnd(t.id));
            right = b.x - 2.0;
        }
        if t.agent {
            let b = R::new(right - 24.0, r.y + 9.0, 22.0, 14.0);
            self.gfx.round(b, 4.0, th.accent.alpha(0.22));
            self.gfx.text("AI", b, Font::ui(9.0, 650).align(Align::Center), th.accent);
            right = b.x - 4.0;
        }
        if t.private {
            // Same shape as the AI badge, muted violet so it never reads as accent.
            let mark = private_mark(self.theme.dark);
            let b = R::new(right - 46.0, r.y + 9.0, 44.0, 14.0);
            self.gfx.round(b, 4.0, mark.alpha(0.22));
            self.gfx.text("Private", b, Font::ui(9.0, 650).align(Align::Center), mark);
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
        let c = if active || self.is_hover(&Hit::Tab(t.id)) { th.text } else { th.dim };
        let c = if asleep { c.alpha(0.5) } else { c };
        self.gfx.text(&title, R::new(r.x + 34.0, r.y, right - r.x - 34.0, r.h), Font::ui(13.0, 400), c);
    }

    fn draw_ntp(&mut self, st: R, s: &UiState) {
        let th = self.theme;
        let field = self.ntp_field();
        // Clock: hours plain, minutes italic, a quiet colon. Local time.
        let (hh, mm, date) = local_time();
        let big = (st.w * 0.09).clamp(64.0, 104.0);
        // Entrance: the clock rises first, then the date, then the shortcuts.
        let t1 = self.anim.from("ntp-1", 0.0, 1.0, 0.09);
        let t2 = self.anim.from("ntp-2", 0.0, 1.0, 0.13);
        let t3 = self.anim.from("ntp-3", 0.0, 1.0, 0.18);
        let cy = field.y - 30.0 - 40.0 - big * 0.5 + (1.0 - t1) * 14.0;
        let fh = Font::serif(big);
        let wh = self.gfx.measure(&hh, fh);
        let wc = self.gfx.measure(":", fh);
        let wm = self.gfx.measure(&mm, fh.italic());
        let x0 = field.x;
        let row = R::new(x0, cy - big * 0.6, wh + wc + wm + 20.0, big * 1.2);
        self.gfx.text(&hh, R::new(row.x, row.y, wh + 4.0, row.h), fh, th.text.alpha(t1));
        self.gfx.text(":", R::new(row.x + wh, row.y, wc + 4.0, row.h), fh, th.faint.alpha(t1));
        self.gfx.text(&mm, R::new(row.x + wh + wc, row.y, wm + 8.0, row.h), fh.italic(), th.text.alpha(t1));
        self.gfx.text(&date.to_uppercase(), R::new(x0 + 2.0, row.bottom() + 2.0 + (1.0 - t2) * 8.0, field.w, 20.0), Font::ui(12.0, 450), th.faint.alpha(t2));
        // A private new tab page says what private means, right under the clock.
        if s.active_tab().is_some_and(|t| t.private) {
            let mark = private_mark(th.dark);
            self.gfx.text(
                "Private tab. Nothing you do here is saved: no history, no cookies after the last private tab closes.",
                R::new(x0 + 2.0, row.bottom() + 26.0 + (1.0 - t2) * 8.0, field.w, 20.0),
                Font::ui(12.0, 450),
                mark.alpha(t2),
            );
        }
        // Search field: the edit box sits on top of this.
        self.gfx.round(field, 12.0, th.bg);
        let focused = unsafe { GetFocus() } == self.ntp_edit;
        self.gfx.round_stroke(field, 12.0, if focused { th.acc_soft } else { th.line }, if focused { 1.5 } else { 1.0 });
        self.gfx.icon("search", field.x + 16.0, field.y + 15.0, 16.0, th.faint);
        if wide_text(self.ntp_edit).is_empty() && !focused {
            self.gfx.text("Search or enter address", R::new(field.x + 42.0, field.y, field.w - 58.0, field.h), Font::ui(15.0, 400), th.faint);
        }
        self.hit(field, Hit::NtpSearch);
        // Shortcuts: a quiet row of names.
        let mut x = field.x;
        let mut y = field.bottom() + 18.0 + (1.0 - t3) * 10.0;
        for (i, sc) in s.settings.shortcuts.iter().enumerate() {
            let name = if sc.title.is_empty() { host(&sc.url) } else { sc.title.clone() };
            let tw = self.gfx.measure(&name, Font::ui(13.0, 450));
            let w = tw + 40.0;
            if x + w > field.right() + 20.0 {
                x = field.x;
                y += 38.0;
            }
            let r = R::new(x, y, w, 32.0);
            let hov = self.is_hover(&Hit::Shortcut(i));
            if hov {
                self.gfx.round(r, 9.0, th.hover);
            }
            let m = R::new(r.x + 8.0, r.y + 8.0, 16.0, 16.0);
            self.gfx.round(m, 5.0, th.press.mix(th.accent, 0.3));
            let letter = name.chars().next().unwrap_or('?').to_uppercase().to_string();
            self.gfx.text(&letter, m, Font::ui(9.5, 650).align(Align::Center), th.text);
            self.gfx.text(&name, R::new(r.x + 32.0, r.y, tw + 4.0, r.h), Font::ui(13.0, 450), if hov { th.text } else { th.dim.alpha(t3) });
            self.hit(r, Hit::Shortcut(i));
            x += w + 6.0;
        }
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
            Some(Hit::NtpSearch) => self.focus_ntp(),
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
        if pages::drag_end(self) {
            return;
        }
        let h = self.hit_at(x, y);
        let pressed = self.press.take();
        if h.is_none() || h != pressed {
            self.invalidate();
            return;
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
            Hit::NewTab => send(p, json!({"t": "newtab"})),
            Hit::Gear => {
                let open = self.state.as_ref().is_some_and(|s| s.panel != "settings");
                send(p, json!({"t": "panel", "open": open, "which": "settings"}));
            }
            Hit::Mem => {
                let open = self.state.as_ref().is_some_and(|s| s.panel != "control");
                send(p, json!({"t": "panel", "open": open, "which": "control"}));
            }
            Hit::Sugg(i) => self.pick(i),
            Hit::DlFooter => {
                let open = self.state.as_ref().is_some_and(|s| s.panel != "downloads");
                send(p, json!({"t": "panel", "open": open, "which": "downloads"}));
            }
            Hit::AgentStop => send(p, json!({"t": "agentStop"})),
            Hit::PermAllow(pid) => send(p, json!({"t": "perm", "pid": pid, "allow": true})),
            Hit::HandoffDone => send(p, json!({"t": "handoffDone"})),
            Hit::Star => send(p, json!({"t": "bookmark"})),
            Hit::PermBlock(pid) => send(p, json!({"t": "perm", "pid": pid, "allow": false})),
            Hit::Shortcut(i) => {
                if let Some(sc) = self.state.as_ref().and_then(|s| s.settings.shortcuts.get(i)) {
                    send(p, json!({"t": "nav", "url": sc.url}));
                }
            }
            Hit::Page(ph) => pages::click(self, ph),
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
        let idx = group
            .iter()
            .filter(|(_, r)| {
                if d.pinned {
                    y > r.bottom() || (y >= r.y && x > r.x + r.w / 2.0)
                } else {
                    y > r.y + r.h / 2.0
                }
            })
            .count();
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
                // Never offer to copy a private URL anywhere it would be written down.
                if !t.url.is_empty() && !t.private {
                    items.push((2, "Duplicate".into()));
                    items.push((7, "Add to new tab page".into()));
                }
                if t.url.starts_with("http") && (t.status == "live" || t.status == "asleep") {
                    items.push((10, "Save video".into()));
                    items.push((8, if t.reader { "Exit reader view\tF9".into() } else { "Reader view\tF9".into() }));
                    let h = host(&t.url);
                    let skipped = s.settings.dark_skip.iter().any(|x| *x == h);
                    if s.dark && s.settings.dark_pages && !skipped {
                        items.push((9, "Keep this site's colors\tShift+F9".into()));
                    } else if s.dark {
                        items.push((9, "Darken this site\tShift+F9".into()));
                    }
                }
                if t.audio || t.muted {
                    items.push((3, if t.muted { "Unmute".into() } else { "Mute".into() }));
                }
                if t.id != s.active && (t.status == "live" || t.status == "asleep") {
                    items.push((4, "Unload now".into()));
                }
                items.push((11, "New private tab\tCtrl+Shift+N".into()));
                items.push((0, String::new()));
                items.push((5, "Close other tabs".into()));
                items.push((6, "Close\tCtrl+W".into()));
                let cmd = self.popup(&items, x, y);
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
                    7 => {
                        let mut st = s.settings.clone();
                        let title = if t.title.is_empty() { host(&t.url) } else { t.title.clone() };
                        st.shortcuts.push(crate::store::Shortcut { title, url: t.url.clone() });
                        send(&self.proxy, json!({"t": "settings", "v": st}));
                        return;
                    }
                    _ => return,
                };
                send(&self.proxy, json!({"t": msg, "id": id}));
            }
            Hit::Shortcut(i) => {
                if self.popup(&[(1, "Remove from new tab page".into())], x, y) == 1 {
                    let mut st = s.settings.clone();
                    if i < st.shortcuts.len() {
                        st.shortcuts.remove(i);
                        send(&self.proxy, json!({"t": "settings", "v": st}));
                    }
                }
            }
            _ => {}
        }
    }

    /// A native popup menu. Returns the chosen id, 0 for none.
    fn popup(&self, items: &[(u32, String)], x: f32, y: f32) -> u32 {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return 0 };
            for (id, label) in items {
                if *id == 0 {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                } else {
                    let _ = AppendMenuW(menu, MF_STRING, *id as usize, &HSTRING::from(label.as_str()));
                }
            }
            let mut pt = POINT { x: (x * self.k()) as i32, y: (y * self.k()) as i32 };
            let _ = ClientToScreen(self.hwnd, &mut pt);
            let cmd = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, self.hwnd, None);
            let _ = DestroyMenu(menu);
            cmd.0 as u32
        }
    }

    fn wheel(&mut self, delta: i32) {
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

fn local_time() -> (String, String, String) {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    (
        format!("{:02}", t.wHour),
        format!("{:02}", t.wMinute),
        format!("{}, {} {}", DAYS[t.wDayOfWeek as usize % 7], t.wDay, MONTHS[(t.wMonth as usize).saturating_sub(1) % 12]),
    )
}

/// The private-tab mark. A muted violet, never the accent: private is not a brand.
fn private_mark(dark: bool) -> theme::Color {
    theme::oklch(if dark { 0.74 } else { 0.52 }, 0.14, 300.0)
}

/// Win32 menus follow the app theme only through two undocumented uxtheme calls.
/// Every Windows 10 1903+ app that has dark menus uses them.
fn dark_menus(dark: bool) {
    unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        // 135: SetPreferredAppMode(mode). 2 = force dark, 3 = force light.
        if let Some(f) = GetProcAddress(ux, PCSTR(135 as *const u8)) {
            let f: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            f(if dark { 2 } else { 3 });
        }
        // 136: FlushMenuThemes().
        if let Some(f) = GetProcAddress(ux, PCSTR(136 as *const u8)) {
            let f: extern "system" fn() = std::mem::transmute(f);
            f();
        }
    }
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
                i.paint();
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
                if let Some(Hit::Tab(id)) = i.hit_at(x, y) {
                    send(&i.proxy, json!({"t": "close", "id": id}));
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
                if wp.0 == TIMER_SPIN || wp.0 == TIMER_CLOCK || wp.0 == TIMER_ANIM {
                    i.invalidate();
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
                if pages::key(&mut i, wp.0 as u32) {
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
                        EN_CHANGE => i.suggest(),
                        EN_KILLFOCUS => i.end_edit(),
                        _ => {}
                    }
                } else if id == ID_NTP && (code == EN_SETFOCUS || code == EN_KILLFOCUS || code == EN_CHANGE) {
                    i.layout_edits();
                    i.invalidate();
                }
                LRESULT(0)
            }
            WM_CTLCOLOREDIT => {
                let hdc = HDC(wp.0 as _);
                let is_ntp = HWND(lp.0 as _) == i.ntp_edit;
                let _ = SetTextColor(hdc, COLORREF(i.theme.text.colorref()));
                let bg = if is_ntp { i.theme.bg } else { i.theme.raise };
                let _ = SetBkColor(hdc, COLORREF(bg.colorref()));
                LRESULT(if is_ntp { i.ntp_brush.0 as isize } else { i.edit_brush.0 as isize })
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            _ => {
                drop(i);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
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
