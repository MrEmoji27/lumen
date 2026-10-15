//! Split view: two pages side by side, one divider between them.
//!
//! The divider is a child window of its own. It sits in the gap the two page
//! rectangles leave open, so it never covers a page, and it can be grabbed,
//! dragged and double-clicked like any other piece of window chrome.
//! The main loop owns the split itself (which two tabs, where the divider is);
//! this file owns the strip and passes mouse positions back as events.

use std::cell::RefCell;
use std::rc::Rc;

use tao::event_loop::EventLoopProxy;
use windows::Win32::{
    Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetStockObject, HDC, InvalidateRect, NULL_PEN,
        PAINTSTRUCT, RoundRect, ScreenToClient, SelectObject,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::WM_MOUSELEAVE,
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent},
        WindowsAndMessaging::*,
    },
};
use windows::core::{PCWSTR, w};

use crate::engine::Ev;

/// The divider strip, in DIP. Wide enough to hit without aiming.
pub const GAP: f64 = 7.0;
/// How narrow a pane may get before the divider refuses to move further.
pub const MIN_PANE: f64 = 180.0;

/// Where the divider sits for a stage of `stage_w` and a split fraction `at`:
/// the left pane's width. Each pane keeps a workable width; a stage too small for
/// two of those shares what there is, half each, rather than stacking panes.
pub fn left_width(stage_w: f64, at: f64, gap: f64) -> f64 {
    let min = MIN_PANE.min(((stage_w - gap) / 2.0).max(1.0));
    let max = (stage_w - gap - min).max(min);
    (stage_w * at).clamp(min, max)
}

/// The two pane rectangles and the divider strip, from a stage rectangle.
/// (left, right, divider). The stage is (x, y, w, h).
pub fn panes(stage: (f64, f64, f64, f64), at: f64) -> ((f64, f64, f64, f64), (f64, f64, f64, f64), (f64, f64, f64, f64)) {
    let (x, y, w, h) = stage;
    let lw = left_width(w, at, GAP);
    let left = (x, y, lw.max(1.0), h);
    let divider = (x + lw, y, GAP, h);
    let right = (x + lw + GAP, y, (w - lw - GAP).max(1.0), h);
    (left, divider, right)
}

/// The fraction of the stage the divider is at, from a pointer x in stage space.
/// 0.5 when the stage is too small to tell.
pub fn fraction(stage_x: f64, stage_w: f64, x: f64) -> f64 {
    if stage_w <= 0.0 {
        return 0.5;
    }
    ((x - stage_x) / stage_w).clamp(0.02, 0.98)
}

struct Inner {
    proxy: EventLoopProxy<Ev>,
    /// Left button down on the strip.
    dragging: bool,
    /// Pointer is over the strip.
    hover: bool,
    bg: COLORREF,
    grip: COLORREF,
    grip_hot: COLORREF,
}

/// The strip between the panes. Cheap to hold; the window lives as long as the app.
pub struct Divider {
    hwnd: HWND,
    inner: Rc<RefCell<Inner>>,
}

const CLASS: PCWSTR = w!("LumenSplit");

impl Divider {
    pub fn new(parent: HWND, proxy: EventLoopProxy<Ev>) -> Divider {
        unsafe {
            let hinst: HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_DBLCLKS,
                lpfnWndProc: Some(wndproc),
                hInstance: hinst,
                hCursor: LoadCursorW(None, IDC_SIZEWE).unwrap_or_default(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS,
                w!(""),
                WS_CHILD,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                Some(hinst),
                None,
            )
            .expect("divider window");
            let inner = Rc::new(RefCell::new(Inner {
                proxy,
                dragging: false,
                hover: false,
                bg: COLORREF(0x1a18_1818),
                grip: COLORREF(0x0055_5555),
                grip_hot: COLORREF(0x0080_8080),
            }));
            // The window proc finds its state through this pointer, like the chrome window.
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(&inner) as isize);
            Divider { hwnd, inner }
        }
    }

    /// Place the strip, in logical coordinates relative to the parent.
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

    pub fn set_visible(&self, on: bool) {
        unsafe {
            // Showing it puts it at the top of the child order: it must not be
            // covered by a page that was made later.
            let _ = ShowWindow(self.hwnd, if on { SW_SHOWNA } else { SW_HIDE });
            if on {
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    /// The strip's colors: the same as the gap around the pages, plus the grip.
    pub fn set_colors(&self, bg: (u8, u8, u8), grip: (u8, u8, u8)) {
        let Ok(mut i) = self.inner.try_borrow_mut() else { return };
        let c = |(r, g, b): (u8, u8, u8)| COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16));
        i.bg = c(bg);
        i.grip = c(grip);
        // Under the pointer the grip steps toward the page color, so it reads as live.
        let lift = |v: u8| if v > 128 { v.saturating_sub(48) } else { v.saturating_add(48) };
        i.grip_hot = c((lift(grip.0), lift(grip.1), lift(grip.2)));
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
}

impl Inner {
    fn paint(&self, hwnd: HWND) {
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let dc: HDC = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let bg = CreateSolidBrush(self.bg);
            FillRect(dc, &rc, bg);
            let _ = DeleteObject(bg.into());
            // A short rounded grip in the middle: the only thing that says "drag me".
            let hot = self.dragging || self.hover;
            let brush = CreateSolidBrush(if hot { self.grip_hot } else { self.grip });
            let old_b = SelectObject(dc, brush.into());
            let old_p = SelectObject(dc, GetStockObject(NULL_PEN));
            let (w, h) = (rc.right as f32, rc.bottom as f32);
            let gw = (w * 0.5).clamp(2.0, 3.0);
            let gh = (h - 80.0).clamp(24.0, 44.0);
            let _ = RoundRect(dc, ((w - gw) / 2.0) as i32, ((h - gh) / 2.0) as i32, ((w + gw) / 2.0) as i32, ((h + gh) / 2.0) as i32, gw as i32, gw as i32);
            SelectObject(dc, old_p);
            SelectObject(dc, old_b);
            let _ = DeleteObject(brush.into());
            let _ = EndPaint(hwnd, &ps);
        }
    }

    /// The pointer's x in the parent's client area, in DIP.
    fn parent_x(&self, hwnd: HWND) -> Option<f64> {
        unsafe {
            let mut p = POINT::default();
            if GetCursorPos(&mut p).is_err() {
                return None;
            }
            let parent = GetParent(hwnd).ok()?;
            let _ = ScreenToClient(parent, &mut p);
            let k = GetDpiForWindow(parent) as f64 / 96.0;
            Some(p.x as f64 / k.max(0.001))
        }
    }
}

fn inner_of(hwnd: HWND) -> Option<&'static RefCell<Inner>> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<Inner>;
    if p.is_null() { None } else { Some(unsafe { &*p }) }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let Some(cell) = inner_of(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
        let Ok(mut i) = cell.try_borrow_mut() else { return DefWindowProcW(hwnd, msg, wp, lp) };
        match msg {
            WM_PAINT => {
                i.paint(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SETCURSOR => {
                let _ = SetCursor(Some(LoadCursorW(None, IDC_SIZEWE).unwrap_or_default()));
                LRESULT(1)
            }
            WM_MOUSEMOVE => {
                let mut tme =
                    TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                if !i.hover {
                    i.hover = true;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                if i.dragging && let Some(x) = i.parent_x(hwnd) {
                    let _ = i.proxy.send_event(Ev::SplitDrag(x));
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if i.hover {
                    i.hover = false;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                i.dragging = true;
                SetCapture(hwnd);
                // No SetFocus: the pages keep their keyboard focus while the divider moves.
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if i.dragging {
                    i.dragging = false;
                    let _ = ReleaseCapture();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    let _ = i.proxy.send_event(Ev::SplitDrop);
                }
                LRESULT(0)
            }
            WM_LBUTTONDBLCLK => {
                let _ = i.proxy.send_event(Ev::SplitReset);
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                if i.dragging {
                    i.dragging = false;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    let _ = i.proxy.send_event(Ev::SplitDrop);
                }
                LRESULT(0)
            }
            WM_NCHITTEST => LRESULT(HTCLIENT as isize),
            _ => {
                drop(i);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panes_meet_without_a_seam() {
        let (l, d, r) = panes((240.0, 8.0, 1000.0, 600.0), 0.5);
        assert_eq!(d.0, l.0 + l.2);
        assert_eq!(r.0, d.0 + d.2);
        assert_eq!(r.2, 1000.0 - 500.0 - GAP);
        assert_eq!(l.1, r.1);
        assert_eq!(l.3, r.3);
    }

    #[test]
    fn a_narrow_stage_keeps_both_panes_usable() {
        let (l, _, r) = panes((0.0, 0.0, 1000.0, 600.0), 0.5);
        assert!(l.2 >= MIN_PANE && r.2 >= MIN_PANE);

        // A stage too small for two minimum panes shares what there is.
        let (l, d, r) = panes((0.0, 0.0, 300.0, 400.0), 0.5);
        assert!((l.2 - r.2).abs() < 1.0);
        assert!(r.2 > 100.0);
        assert_eq!(r.0, d.0 + d.2);
        // Dragging past either edge cannot invert the panes.
        let (l, _, r) = panes((0.0, 0.0, 900.0, 400.0), -3.0);
        assert!(l.2 >= MIN_PANE);
        assert!(r.2 >= MIN_PANE);
        let (l, _, r) = panes((0.0, 0.0, 900.0, 400.0), 4.0);
        assert!(l.2 >= MIN_PANE);
        assert!(r.2 >= MIN_PANE);
        // A stage with no room at all still yields two rectangles, never an overlap.
        let (l, d, r) = panes((0.0, 0.0, 40.0, 100.0), 0.5);
        assert!(l.2 >= 1.0 && r.2 >= 1.0);
        assert_eq!(r.0, d.0 + d.2);
    }

    #[test]
    fn a_drag_keeps_the_divider_inside_the_stage() {
        assert!((fraction(200.0, 800.0, 600.0) - 0.5).abs() < 1e-9);
        assert!(fraction(200.0, 800.0, 100.0) >= 0.02);
        assert!(fraction(200.0, 800.0, 9999.0) <= 0.98);
        // A stage with no width cannot be split: stay in the middle.
        assert_eq!(fraction(0.0, 0.0, 50.0), 0.5);
    }
}
