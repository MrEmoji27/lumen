//! Watching an agent's tab. When the person clicks an agent tab, its view moves into the
//! window (see `stage.rs`) and three small windows go over the page:
//!
//! - the **shield**: the size of the page, all but invisible (alpha 1 of 255), taking the
//!   person's mouse and wheel so a stray click cannot knock the agent off;
//! - the **cursor**: the agent's pointer, an arrow in its colour with its name beside it,
//!   gliding to each point the agent clicks, hovers or types at, with a ring on a click;
//! - the **pill**: at the bottom of the page, the agent's name and last step, and
//!   **Take over**, which hands the tab to the person until they press **Give back**.
//!
//! They are popups owned by the main window, not children: they sit above it and the page,
//! never take focus, and Lumen moves them when the window moves. Nothing is made until the
//! first watch, and nothing draws when no agent tab is watched.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use tao::event_loop::EventLoopProxy;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use crate::engine::Ev;

/// The colour that is not there: drawn pixels of this colour are see-through.
const KEY: COLORREF = COLORREF(0x00FF_00FF);
const GLIDE: Duration = Duration::from_millis(150);
const RING: Duration = Duration::from_millis(350);
const TIMER: usize = 7;

/// What the cursor window draws and where it is going.
struct Cursor {
    hwnd: HWND,
    color: COLORREF,
    name: String,
    /// Where the tip is, and where it is gliding to, in screen pixels.
    at: (f32, f32),
    from: (f32, f32),
    to: (f32, f32),
    since: Instant,
    ring_until: Option<Instant>,
    caret_until: Option<Instant>,
    scale: f32,
}

struct Pill {
    hwnd: HWND,
    color: COLORREF,
    text: String,
    taken: bool,
    proxy: EventLoopProxy<Ev>,
    scale: f32,
}

thread_local! {
    static CURSOR: RefCell<Option<Rc<RefCell<Cursor>>>> = const { RefCell::new(None) };
    static PILL: RefCell<Option<Rc<RefCell<Pill>>>> = const { RefCell::new(None) };
}

pub struct Watch {
    owner: HWND,
    shield: HWND,
    cursor: Rc<RefCell<Cursor>>,
    pill: Rc<RefCell<Pill>>,
    /// The page area on screen, physical pixels: where the shield is.
    page: Cell<RECT>,
    shown: Cell<bool>,
}

/// Where the tip sits inside the cursor window: room around it for the click ring.
const TIP: f32 = 16.0;

impl Watch {
    pub fn new(owner: isize, proxy: EventLoopProxy<Ev>) -> Watch {
        let owner = HWND(owner as _);
        let hinst: HINSTANCE = unsafe { GetModuleHandleW(None) }.map(|h| h.into()).unwrap_or_default();
        let class = |name: PCWSTR, proc: WNDPROC, cursor: PCWSTR| unsafe {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: proc,
                hInstance: hinst,
                hCursor: LoadCursorW(None, cursor).unwrap_or_default(),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: name,
                ..Default::default()
            };
            RegisterClassExW(&wc);
        };
        class(w!("LumenShield"), Some(shield_proc), IDC_NO);
        class(w!("LumenAgentCursor"), Some(cursor_proc), IDC_ARROW);
        class(w!("LumenWatchPill"), Some(pill_proc), IDC_HAND);
        let make = |name: PCWSTR, ex: WINDOW_EX_STYLE| unsafe {
            CreateWindowExW(
                ex | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                name,
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                Some(owner),
                None,
                Some(hinst),
                None,
            )
            .unwrap_or_default()
        };
        let shield = make(w!("LumenShield"), WINDOW_EX_STYLE::default());
        let pill_h = make(w!("LumenWatchPill"), WINDOW_EX_STYLE::default());
        // The cursor never takes a click: it is a picture, the shield is under it.
        let cursor_h = make(w!("LumenAgentCursor"), WS_EX_TRANSPARENT);
        unsafe {
            let _ = SetLayeredWindowAttributes(shield, COLORREF(0), 1, LWA_ALPHA);
            let _ = SetLayeredWindowAttributes(pill_h, KEY, 0, LWA_COLORKEY);
            let _ = SetLayeredWindowAttributes(cursor_h, KEY, 0, LWA_COLORKEY);
        }
        let scale = unsafe { GetDpiForWindow(owner) }.max(96) as f32 / 96.0;
        let cursor = Rc::new(RefCell::new(Cursor {
            hwnd: cursor_h,
            color: COLORREF(0),
            name: String::new(),
            at: (0.0, 0.0),
            from: (0.0, 0.0),
            to: (0.0, 0.0),
            since: Instant::now(),
            ring_until: None,
            caret_until: None,
            scale,
        }));
        let pill = Rc::new(RefCell::new(Pill { hwnd: pill_h, color: COLORREF(0), text: String::new(), taken: false, proxy, scale }));
        CURSOR.with(|c| *c.borrow_mut() = Some(cursor.clone()));
        PILL.with(|p| *p.borrow_mut() = Some(pill.clone()));
        Watch { owner, shield, cursor, pill, page: Cell::new(RECT::default()), shown: Cell::new(false) }
    }

    /// Over this page area (client coordinates of the main window, logical pixels), for this
    /// agent: its name, its colour (r, g, b), its last step, and whether the person has it.
    pub fn show(&self, page: (f64, f64, f64, f64), name: &str, rgb: (u8, u8, u8), step: &str, taken: bool) {
        let scale = unsafe { GetDpiForWindow(self.owner) }.max(96) as f32 / 96.0;
        let mut o = POINT::default();
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(self.owner, &mut o);
        }
        let r = RECT {
            left: o.x + (page.0 as f32 * scale) as i32,
            top: o.y + (page.1 as f32 * scale) as i32,
            right: o.x + ((page.0 + page.2) as f32 * scale) as i32,
            bottom: o.y + ((page.1 + page.3) as f32 * scale) as i32,
        };
        let color = COLORREF(rgb.0 as u32 | (rgb.1 as u32) << 8 | (rgb.2 as u32) << 16);
        let moved = !self.shown.get() || self.page.get() != r;
        self.page.set(r);
        {
            let mut p = self.pill.borrow_mut();
            let text = format!("{name} · {}", if step.is_empty() { "working" } else { step });
            let changed = p.text != text || p.taken != taken || p.color != color;
            p.text = text;
            p.color = color;
            p.taken = taken;
            p.scale = scale;
            let (pw, ph) = ((420.0 * scale) as i32, (34.0 * scale) as i32);
            let pw = pw.min(r.right - r.left - (24.0 * scale) as i32).max(1);
            unsafe {
                let _ = SetWindowPos(
                    p.hwnd,
                    Some(HWND_TOP),
                    r.left + (r.right - r.left - pw) / 2,
                    r.bottom - ph - (14.0 * scale) as i32,
                    pw,
                    ph,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                if changed {
                    let _ = InvalidateRect(Some(p.hwnd), None, true);
                }
            }
        }
        {
            let mut c = self.cursor.borrow_mut();
            if c.name != name || c.color != color || c.scale != scale {
                c.name = name.to_string();
                c.color = color;
                c.scale = scale;
                unsafe {
                    let _ = InvalidateRect(Some(c.hwnd), None, true);
                }
            }
            if moved && !self.shown.get() {
                // The first look: the cursor waits in the middle until the agent moves.
                let mid = ((r.left + r.right) as f32 / 2.0, (r.top + r.bottom) as f32 / 2.0);
                c.at = mid;
                c.from = mid;
                c.to = mid;
            }
        }
        unsafe {
            if taken {
                let _ = ShowWindow(self.shield, SW_HIDE);
            } else if moved || !IsWindowVisible(self.shield).as_bool() {
                let _ = SetWindowPos(self.shield, Some(HWND_TOP), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            // Order: shield, pill, cursor on top.
            let _ = SetWindowPos(self.pill.borrow().hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
        }
        place_cursor(&self.cursor.borrow(), true);
        self.shown.set(true);
    }

    pub fn hide(&self) {
        if !self.shown.get() {
            return;
        }
        self.shown.set(false);
        unsafe {
            let _ = ShowWindow(self.shield, SW_HIDE);
            let _ = ShowWindow(self.pill.borrow().hwnd, SW_HIDE);
            let c = self.cursor.borrow();
            let _ = KillTimer(Some(c.hwnd), TIMER);
            let _ = ShowWindow(c.hwnd, SW_HIDE);
        }
    }

    pub fn shown(&self) -> bool {
        self.shown.get()
    }

    /// The agent's pointer went to (x, y), CSS pixels in the page, at this zoom (percent).
    /// `kind`: "move", "press" (a click: the ring) or "type" (a caret flash).
    pub fn point(&self, x: f64, y: f64, zoom: u32, kind: &str) {
        if !self.shown.get() {
            return;
        }
        let r = self.page.get();
        let k = self.cursor.borrow().scale * zoom.max(10) as f32 / 100.0;
        let to = ((r.left as f32 + x as f32 * k).clamp(r.left as f32, r.right as f32), (r.top as f32 + y as f32 * k).clamp(r.top as f32, r.bottom as f32));
        let mut c = self.cursor.borrow_mut();
        if (c.to.0 - to.0).abs() > 0.5 || (c.to.1 - to.1).abs() > 0.5 {
            c.from = c.at;
            c.to = to;
            c.since = Instant::now();
        }
        match kind {
            "press" => c.ring_until = Some(Instant::now() + RING),
            "type" => c.caret_until = Some(Instant::now() + RING),
            _ => {}
        }
        unsafe {
            let _ = SetTimer(Some(c.hwnd), TIMER, 15, None);
            let _ = InvalidateRect(Some(c.hwnd), None, true);
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.shield);
            let _ = DestroyWindow(self.pill.borrow().hwnd);
            let _ = DestroyWindow(self.cursor.borrow().hwnd);
        }
    }
}

/// Put the cursor window where its tip is now.
fn place_cursor(c: &Cursor, show: bool) {
    let (w, h) = ((220.0 * c.scale) as i32, (56.0 * c.scale) as i32);
    let tip = (TIP * c.scale) as i32;
    unsafe {
        let _ = SetWindowPos(
            c.hwnd,
            Some(HWND_TOP),
            c.at.0 as i32 - tip,
            c.at.1 as i32 - tip,
            w,
            h,
            SWP_NOACTIVATE | if show { SWP_SHOWWINDOW } else { SET_WINDOW_POS_FLAGS(0) },
        );
    }
}

unsafe extern "system" fn shield_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match m {
            // A click on the shield never takes focus from the window.
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            // Every click, wheel and key stops here: the agent's page is not to be touched.
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => LRESULT(0),
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

unsafe extern "system" fn cursor_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match m {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_TIMER => {
                CURSOR.with(|c| {
                    let Some(c) = c.borrow().clone() else { return };
                    let mut c = c.borrow_mut();
                    let t = (c.since.elapsed().as_secs_f32() / GLIDE.as_secs_f32()).min(1.0);
                    // Ease out: quick to leave, gentle to arrive.
                    let e = 1.0 - (1.0 - t).powi(3);
                    c.at = (c.from.0 + (c.to.0 - c.from.0) * e, c.from.1 + (c.to.1 - c.from.1) * e);
                    place_cursor(&c, false);
                    let now = Instant::now();
                    let marks = c.ring_until.is_some_and(|u| u > now) || c.caret_until.is_some_and(|u| u > now);
                    if c.ring_until.is_some_and(|u| u <= now) || c.caret_until.is_some_and(|u| u <= now) {
                        c.ring_until = c.ring_until.filter(|u| *u > now);
                        c.caret_until = c.caret_until.filter(|u| *u > now);
                        let _ = InvalidateRect(Some(h), None, true);
                    }
                    if t >= 1.0 && !marks {
                        let _ = KillTimer(Some(h), TIMER);
                    }
                });
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                CURSOR.with(|c| {
                    if let Some(c) = c.borrow().clone() {
                        paint_cursor(dc, &c.borrow());
                    }
                });
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

fn paint_cursor(dc: HDC, c: &Cursor) {
    let k = c.scale;
    let px = |v: f32| (v * k).round() as i32;
    let tip = TIP;
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(c.hwnd, &mut rc);
        let key = CreateSolidBrush(KEY);
        FillRect(dc, &rc, key);
        let _ = DeleteObject(key.into());
        let now = Instant::now();
        // The ring of a click, around the tip.
        if c.ring_until.is_some_and(|u| u > now) {
            let pen = CreatePen(PS_SOLID, px(2.0), c.color);
            let old = SelectObject(dc, pen.into());
            let oldb = SelectObject(dc, GetStockObject(NULL_BRUSH));
            let _ = Ellipse(dc, px(tip - 11.0), px(tip - 11.0), px(tip + 11.0), px(tip + 11.0));
            SelectObject(dc, oldb);
            SelectObject(dc, old);
            let _ = DeleteObject(pen.into());
        }
        // The arrow: the accent fill, a white edge so it shows on any page.
        let shape = [(0.0, 0.0), (0.0, 17.0), (4.2, 13.2), (7.2, 19.8), (10.0, 18.6), (7.1, 12.1), (12.4, 12.1)];
        let pts: Vec<POINT> = shape.iter().map(|(x, y)| POINT { x: px(tip + x), y: px(tip + y) }).collect();
        let brush = CreateSolidBrush(c.color);
        let pen = CreatePen(PS_SOLID, px(1.4).max(1), COLORREF(0x00FF_FFFF));
        let oldp = SelectObject(dc, pen.into());
        let oldb = SelectObject(dc, brush.into());
        let _ = Polygon(dc, &pts);
        SelectObject(dc, oldb);
        SelectObject(dc, oldp);
        let _ = DeleteObject(pen.into());
        let _ = DeleteObject(brush.into());
        // Typing: a caret flash just right of the tip.
        if c.caret_until.is_some_and(|u| u > now) {
            let b = CreateSolidBrush(c.color);
            let r = RECT { left: px(tip - 1.0), top: px(tip - 9.0), right: px(tip + 1.0), bottom: px(tip + 9.0) };
            FillRect(dc, &r, b);
            let _ = DeleteObject(b.into());
        }
        // The name, on a tag of its colour.
        if !c.name.is_empty() {
            let font = ui_font(k, 12.0, 600);
            let oldf = SelectObject(dc, font.into());
            let text: Vec<u16> = c.name.encode_utf16().collect();
            let mut sz = windows::Win32::Foundation::SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &text, &mut sz);
            let r = RECT { left: px(tip + 14.0), top: px(tip + 16.0), right: px(tip + 14.0) + sz.cx + px(12.0), bottom: px(tip + 16.0) + sz.cy + px(4.0) };
            let b = CreateSolidBrush(c.color);
            let oldb = SelectObject(dc, b.into());
            let oldp = SelectObject(dc, GetStockObject(NULL_PEN));
            let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, px(8.0), px(8.0));
            SelectObject(dc, oldp);
            SelectObject(dc, oldb);
            let _ = DeleteObject(b.into());
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let _ = TextOutW(dc, r.left + px(6.0), r.top + px(2.0), &text);
            SelectObject(dc, oldf);
            let _ = DeleteObject(font.into());
        }
    }
}

unsafe extern "system" fn pill_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match m {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_LBUTTONUP => {
                PILL.with(|p| {
                    if let Some(p) = p.borrow().clone() {
                        let p = p.borrow();
                        let x = (l.0 & 0xFFFF) as i16 as i32;
                        let mut rc = RECT::default();
                        let _ = GetClientRect(h, &mut rc);
                        if x >= rc.right - (110.0 * p.scale) as i32 {
                            let t = if p.taken { "watchGiveBack" } else { "watchTakeOver" };
                            let _ = p.proxy.send_event(Ev::Ui(format!("{{\"t\":\"{t}\"}}")));
                        }
                    }
                });
                LRESULT(0)
            }
            WM_MOUSEWHEEL | WM_RBUTTONUP | WM_LBUTTONDOWN => LRESULT(0),
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                PILL.with(|p| {
                    if let Some(p) = p.borrow().clone() {
                        paint_pill(dc, &p.borrow());
                    }
                });
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

fn paint_pill(dc: HDC, p: &Pill) {
    let k = p.scale;
    let px = |v: f32| (v * k).round() as i32;
    unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(p.hwnd, &mut rc);
        let key = CreateSolidBrush(KEY);
        FillRect(dc, &rc, key);
        let _ = DeleteObject(key.into());
        // A dark pill whatever the page: it must read on white and on black.
        let bg = CreateSolidBrush(COLORREF(0x0024_2220));
        let oldb = SelectObject(dc, bg.into());
        let oldp = SelectObject(dc, GetStockObject(NULL_PEN));
        let _ = RoundRect(dc, 0, 0, rc.right, rc.bottom, rc.bottom, rc.bottom);
        SelectObject(dc, oldb);
        let _ = DeleteObject(bg.into());
        // The agent's dot.
        let dot = CreateSolidBrush(p.color);
        let oldd = SelectObject(dc, dot.into());
        let cy = rc.bottom / 2;
        let _ = Ellipse(dc, px(14.0), cy - px(4.0), px(22.0), cy + px(4.0));
        SelectObject(dc, oldd);
        let _ = DeleteObject(dot.into());
        // The button: Take over, or Give back while the person has the tab.
        let bw = px(100.0);
        let btn = RECT { left: rc.right - bw - px(4.0), top: px(4.0), right: rc.right - px(4.0), bottom: rc.bottom - px(4.0) };
        let b = CreateSolidBrush(if p.taken { p.color } else { COLORREF(0x0040_3D3A) });
        let oldb = SelectObject(dc, b.into());
        let _ = RoundRect(dc, btn.left, btn.top, btn.right, btn.bottom, btn.bottom - btn.top, btn.bottom - btn.top);
        SelectObject(dc, oldb);
        SelectObject(dc, oldp);
        let _ = DeleteObject(b.into());
        SetBkMode(dc, TRANSPARENT);
        let font = ui_font(k, 12.0, 500);
        let oldf = SelectObject(dc, font.into());
        SetTextColor(dc, COLORREF(0x00F0_EEEA));
        let mut text: Vec<u16> = if p.taken { format!("{} · the tab is yours", p.text.split(" · ").next().unwrap_or("")) } else { p.text.clone() }.encode_utf16().collect();
        let mut tr = RECT { left: px(30.0), top: 0, right: btn.left - px(8.0), bottom: rc.bottom };
        let _ = DrawTextW(dc, &mut text, &mut tr, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
        let mut label: Vec<u16> = (if p.taken { "Give back" } else { "Take over" }).encode_utf16().collect();
        let mut br = btn;
        SetTextColor(dc, COLORREF(0x00FF_FFFF));
        let _ = DrawTextW(dc, &mut label, &mut br, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
        SelectObject(dc, oldf);
        let _ = DeleteObject(font.into());
    }
}

/// Segoe UI at `pt` logical points: grey-scale smoothing, which keeps a clean edge on the
/// colour key that ClearType's coloured fringes would not.
fn ui_font(k: f32, size: f32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -(size * k).round() as i32,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            FF_DONTCARE.0 as u32,
            &HSTRING::from("Segoe UI"),
        )
    }
}
