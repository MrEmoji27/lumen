//! Watching an agent's tab. When the person clicks an agent tab, its view moves into the
//! window (see `stage.rs`) and three small windows go over the page:
//!
//! - the **shield**: the size of the page, all but invisible (alpha 1 of 255), taking the
//!   person's mouse and wheel so a stray click cannot knock the agent off;
//! - the **cursor**: the agent's pointer, an arrow in its colour with its name beside it,
//!   gliding to each point the agent clicks, hovers or types at, with a ring on a click;
//! - the **pill**: at the bottom of the page, the agent's name and last step, **Message**,
//!   and **Take over**, which hands the tab to the person until they press **Give back**.
//!
//! The cursor and the pill are drawn with Direct2D into windows with per-pixel alpha
//! (UpdateLayeredWindow): smooth edges, soft shadows, real text. They are popups owned by
//! the main window, not children: they sit above it and the page, never take focus, and
//! Lumen moves them when the window moves. Nothing is made until the first watch, and
//! nothing draws when no agent tab is watched.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use tao::event_loop::EventLoopProxy;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Interface, PCWSTR, w};
use windows_numerics::Vector2;

use crate::engine::Ev;

const GLIDE: Duration = Duration::from_millis(160);
const RING: Duration = Duration::from_millis(420);
const TIMER: usize = 7;
/// Not in the windows crate's list: the pointer left a window that asked to be told.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// The cursor window, in DIPs, and where the tip sits in it: room for the ring around it.
const CUR_W: f32 = 240.0;
const CUR_H: f32 = 64.0;
const TIP: f32 = 18.0;
/// The pill: its height, and the room around it for its shadow.
const PILL_H: f32 = 40.0;
const PAD: f32 = 10.0;
const FONT: &str = "Segoe UI Variable Text";

#[derive(Clone, Copy, PartialEq)]
struct Rgb(f32, f32, f32);

impl Rgb {
    fn d2d(self, a: f32) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.0, g: self.1, b: self.2, a }
    }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

/// One Direct2D factory, one DirectWrite factory and one DC render target for every
/// overlay: they draw on the UI thread, one at a time.
struct Kit {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    /// What everything draws on: the DC target below, or (in the sample test) a bitmap.
    rt: ID2D1RenderTarget,
    dc: Option<ID2D1DCRenderTarget>,
}

thread_local! {
    static KIT: RefCell<Option<Rc<Kit>>> = const { RefCell::new(None) };
    static CURSOR: RefCell<Option<Rc<RefCell<Cursor>>>> = const { RefCell::new(None) };
    static PILL: RefCell<Option<Rc<RefCell<Pill>>>> = const { RefCell::new(None) };
}

fn kit() -> Option<Rc<Kit>> {
    KIT.with(|k| {
        if k.borrow().is_none() {
            let made = unsafe {
                let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
                let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    usage: D2D1_RENDER_TARGET_USAGE_NONE,
                    minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
                };
                let dc = d2d.CreateDCRenderTarget(&props).ok()?;
                // On a see-through surface ClearType's coloured fringes show: grey-scale text.
                dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                let rt: ID2D1RenderTarget = dc.cast().ok()?;
                Some(Rc::new(Kit { d2d, dw, rt, dc: Some(dc) }))
            };
            *k.borrow_mut() = made;
        }
        k.borrow().clone()
    })
}

/// Draw into a window with per-pixel alpha: `draw` gets the target in DIPs (the scale is
/// the target's DPI), the window goes to (x, y) on screen with the given size in pixels.
fn present(hwnd: HWND, x: i32, y: i32, w: i32, h: i32, scale: f32, draw: impl FnOnce(&Kit)) {
    let Some(k) = kit() else { return };
    let Some(dc) = k.dc.clone() else { return };
    if w < 1 || h < 1 {
        return;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        if let Ok(dib) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            let old = SelectObject(mem, dib.into());
            let rc = RECT { left: 0, top: 0, right: w, bottom: h };
            if dc.BindDC(mem, &rc).is_ok() {
                k.rt.SetDpi(96.0 * scale, 96.0 * scale);
                k.rt.BeginDraw();
                k.rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
                draw(&k);
                let _ = k.rt.EndDraw(None, None);
                let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
                let _ = UpdateLayeredWindow(
                    hwnd,
                    Some(screen),
                    Some(&POINT { x, y }),
                    Some(&SIZE { cx: w, cy: h }),
                    Some(mem),
                    Some(&POINT { x: 0, y: 0 }),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
            }
            SelectObject(mem, old);
            let _ = DeleteObject(dib.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
    }
}

fn brush(k: &Kit, c: D2D1_COLOR_F) -> Option<ID2D1SolidColorBrush> {
    unsafe { k.rt.CreateSolidColorBrush(&c, None).ok() }
}

fn rr(x: f32, y: f32, w: f32, h: f32, r: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, radiusX: r, radiusY: r }
}

/// A text format: one line, centred vertically, ellipsis when it does not fit.
fn format(k: &Kit, size: f32, weight: i32) -> Option<IDWriteTextFormat> {
    unsafe {
        let f = k
            .dw
            .CreateTextFormat(&HSTRING::from(FONT), None, DWRITE_FONT_WEIGHT(weight), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))
            .ok()?;
        let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
        if let Ok(sign) = k.dw.CreateEllipsisTrimmingSign(&f) {
            let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            let _ = f.SetTrimming(&trim, &sign);
        }
        Some(f)
    }
}

/// How wide a line of text is, in DIPs.
fn measure(k: &Kit, s: &str, size: f32, weight: i32) -> f32 {
    let Some(f) = format(k, size, weight) else { return 0.0 };
    let w: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        let Ok(l) = k.dw.CreateTextLayout(&w, &f, 10_000.0, 100.0) else { return 0.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        if l.GetMetrics(&mut m).is_ok() { m.widthIncludingTrailingWhitespace.ceil() } else { 0.0 }
    }
}

fn text(k: &Kit, s: &str, x: f32, y: f32, w: f32, h: f32, size: f32, weight: i32, c: D2D1_COLOR_F, center: bool) {
    let Some(f) = format(k, size, weight) else { return };
    if center {
        unsafe {
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
        }
    }
    let Some(b) = brush(k, c) else { return };
    let s: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        k.rt.DrawText(&s, &f, &D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, &b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    }
}

// ------------------------------------------------------------------- the cursor

struct Cursor {
    hwnd: HWND,
    color: Rgb,
    name: String,
    /// Where the tip is, and where it is gliding to, in screen pixels.
    at: (f32, f32),
    from: (f32, f32),
    to: (f32, f32),
    since: Instant,
    ring_at: Option<Instant>,
    caret_at: Option<Instant>,
    scale: f32,
}

/// The arrow, tip at (0, 0), in DIPs: a little larger than the system's, so it reads as the
/// agent's and not the person's.
const ARROW: [(f32, f32); 7] = [(0.0, 0.0), (0.0, 21.0), (5.3, 16.1), (9.0, 24.0), (12.5, 22.5), (9.0, 14.7), (15.7, 14.7)];

fn arrow(k: &Kit, dx: f32, dy: f32) -> Option<ID2D1PathGeometry> {
    unsafe {
        let g = k.d2d.CreatePathGeometry().ok()?;
        let s = g.Open().ok()?;
        s.BeginFigure(Vector2 { X: ARROW[0].0 + dx, Y: ARROW[0].1 + dy }, D2D1_FIGURE_BEGIN_FILLED);
        for p in &ARROW[1..] {
            s.AddLine(Vector2 { X: p.0 + dx, Y: p.1 + dy });
        }
        s.EndFigure(D2D1_FIGURE_END_CLOSED);
        s.Close().ok()?;
        Some(g)
    }
}

fn draw_cursor(c: &Cursor) {
    let k = c.scale;
    let tip = (TIP * k).round() as i32;
    let (w, h) = ((CUR_W * k) as i32, (CUR_H * k) as i32);
    let (x, y) = (c.at.0.round() as i32 - tip, c.at.1.round() as i32 - tip);
    let now = Instant::now();
    let ring = c.ring_at.map(|t| now.duration_since(t)).filter(|d| *d < RING).map(|d| d.as_secs_f32() / RING.as_secs_f32());
    let caret = c.caret_at.map(|t| now.duration_since(t)).filter(|d| *d < RING).map(|d| d.as_secs_f32() / RING.as_secs_f32());
    present(c.hwnd, x, y, w, h, k, |kit| paint_cursor(kit, c, ring, caret));
}

/// The cursor's picture, the window's top-left at (0, 0), in DIPs.
fn paint_cursor(kit: &Kit, c: &Cursor, ring: Option<f32>, caret: Option<f32>) {
    unsafe {
        let rt = &kit.rt;
        // The ring of a click: grows and fades.
        if let Some(t) = ring {
            let e = 1.0 - (1.0 - t).powi(2);
            if let Some(b) = brush(kit, c.color.d2d(0.75 * (1.0 - t))) {
                let el = D2D1_ELLIPSE { point: Vector2 { X: TIP, Y: TIP }, radiusX: 5.0 + 11.0 * e, radiusY: 5.0 + 11.0 * e };
                rt.DrawEllipse(&el, &b, 2.2, None);
            }
        }
        // A soft shadow: the arrow twice, offset and faint.
        for (o, a) in [(2.4, 0.10), (1.4, 0.18)] {
            if let (Some(g), Some(b)) = (arrow(kit, TIP + o * 0.4, TIP + o), brush(kit, rgba(0.0, 0.0, 0.0, a))) {
                rt.FillGeometry(&g, &b, None);
            }
        }
        if let Some(g) = arrow(kit, TIP, TIP) {
            if let Some(b) = brush(kit, c.color.d2d(1.0)) {
                rt.FillGeometry(&g, &b, None);
            }
            if let Some(b) = brush(kit, rgba(1.0, 1.0, 1.0, 1.0)) {
                rt.DrawGeometry(&g, &b, 1.5, None);
            }
        }
        // Typing: a caret blink beside the tip.
        if let Some(t) = caret
            && let Some(b) = brush(kit, c.color.d2d(1.0 - t))
        {
            rt.FillRoundedRectangle(&rr(TIP + 20.0, TIP - 2.0, 2.0, 18.0, 1.0), &b);
        }
        // The name on a tag of its colour, dark text: the agent colours are light.
        if !c.name.is_empty() {
            let tw = measure(kit, &c.name, 11.5, 600).min(CUR_W - TIP - 30.0);
            let (tx, ty) = (TIP + 14.0, TIP + 21.0);
            if let Some(b) = brush(kit, rgba(0.0, 0.0, 0.0, 0.16)) {
                rt.FillRoundedRectangle(&rr(tx, ty + 1.5, tw + 14.0, 20.0, 10.0), &b);
            }
            if let Some(b) = brush(kit, c.color.d2d(1.0)) {
                rt.FillRoundedRectangle(&rr(tx, ty, tw + 14.0, 20.0, 10.0), &b);
            }
            text(kit, &c.name, tx + 7.0, ty, tw + 1.0, 20.0, 11.5, 600, rgba(0.08, 0.08, 0.09, 1.0), false);
        }
    }
}

// ------------------------------------------------------------------- the pill

struct Pill {
    hwnd: HWND,
    color: Rgb,
    name: String,
    step: String,
    taken: bool,
    proxy: EventLoopProxy<Ev>,
    scale: f32,
    /// Where it is on screen (pixels), and where its two buttons are in it (DIPs).
    at: (i32, i32, i32, i32),
    message: (f32, f32),
    take: (f32, f32),
    hover: Option<u8>,
}

/// Lay the pill out for this page width (pixels): its window rectangle on screen, and the
/// spans of its buttons.
fn layout_pill(p: &mut Pill, page: RECT) {
    let Some(k) = kit() else { return };
    let s = p.scale;
    let take_label = if p.taken { "Give back" } else { "Take over" };
    let (mw, tw) = (measure(&k, "Message", 12.5, 600) + 26.0, measure(&k, take_label, 12.5, 600) + 26.0);
    let name_w = measure(&k, &p.name, 13.0, 600);
    let step_w = measure(&k, &pill_says(p), 13.0, 400).min(260.0);
    let max_w = ((page.right - page.left) as f32 / s - 24.0).max(200.0);
    let w = (34.0 + name_w + 10.0 + step_w + 16.0 + mw + 6.0 + tw + 6.0).min(max_w);
    p.take = (w - 6.0 - tw, tw);
    p.message = (p.take.0 - 6.0 - mw, mw);
    let (ww, wh) = (((w + 2.0 * PAD) * s) as i32, ((PILL_H + 2.0 * PAD) * s) as i32);
    let x = page.left + (page.right - page.left - ww) / 2;
    let y = page.bottom - wh - (6.0 * s) as i32;
    p.at = (x, y, ww, wh);
}

fn draw_pill(p: &Pill) {
    let (x, y, w, h) = p.at;
    let width = w as f32 / p.scale - 2.0 * PAD;
    present(p.hwnd, x, y, w, h, p.scale, |kit| paint_pill(kit, p, width));
}

/// The pill's picture, the window's top-left at (0, 0), in DIPs; `width` is the pill's own.
fn paint_pill(kit: &Kit, p: &Pill, width: f32) {
    unsafe {
        let rt = &kit.rt;
        // Shadow, then the pill: dark glass that reads on any page.
        for g in [6.0f32, 4.5, 3.0, 1.5] {
            if let Some(b) = brush(kit, rgba(0.0, 0.0, 0.0, 0.035)) {
                rt.FillRoundedRectangle(&rr(PAD - g, PAD - g + 2.0, width + 2.0 * g, PILL_H + 2.0 * g, PILL_H / 2.0 + g), &b);
            }
        }
        if let Some(b) = brush(kit, rgba(0.105, 0.102, 0.098, 0.94)) {
            rt.FillRoundedRectangle(&rr(PAD, PAD, width, PILL_H, PILL_H / 2.0), &b);
        }
        if let Some(b) = brush(kit, rgba(1.0, 1.0, 1.0, 0.09)) {
            rt.DrawRoundedRectangle(&rr(PAD + 0.5, PAD + 0.5, width - 1.0, PILL_H - 1.0, PILL_H / 2.0), &b, 1.0, None);
        }
        // The agent's dot, its name, what it did last.
        if let Some(b) = brush(kit, p.color.d2d(1.0)) {
            rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: PAD + 19.0, Y: PAD + PILL_H / 2.0 }, radiusX: 4.5, radiusY: 4.5 }, &b);
        }
        let left = PAD + 32.0;
        let room = p.message.0 - 32.0 - 12.0;
        let name_w = measure(kit, &p.name, 13.0, 600).min(room.max(0.0));
        text(kit, &p.name, left, PAD, name_w + 1.0, PILL_H, 13.0, 600, rgba(0.96, 0.95, 0.93, 1.0), false);
        let says = pill_says(p);
        text(kit, &says, left + name_w + 10.0, PAD, (room - name_w - 10.0).max(0.0), PILL_H, 13.0, 400, rgba(0.70, 0.69, 0.66, 1.0), false);
        // Message: quiet. Take over: the agent's colour. Give back: white.
        let chip = |(cx, cw): (f32, f32), label: &str, fill: D2D1_COLOR_F, ink: D2D1_COLOR_F, hot: bool| {
            if let Some(b) = brush(kit, fill) {
                rt.FillRoundedRectangle(&rr(PAD + cx, PAD + 6.0, cw, PILL_H - 12.0, (PILL_H - 12.0) / 2.0), &b);
            }
            if hot && let Some(b) = brush(kit, rgba(1.0, 1.0, 1.0, 0.10)) {
                rt.FillRoundedRectangle(&rr(PAD + cx, PAD + 6.0, cw, PILL_H - 12.0, (PILL_H - 12.0) / 2.0), &b);
            }
            text(kit, label, PAD + cx, PAD, cw, PILL_H, 12.5, 600, ink, true);
        };
        chip(p.message, "Message", rgba(1.0, 1.0, 1.0, 0.10), rgba(0.96, 0.95, 0.93, 1.0), p.hover == Some(0));
        if p.taken {
            chip(p.take, "Give back", rgba(0.96, 0.95, 0.93, 1.0), rgba(0.08, 0.08, 0.09, 1.0), p.hover == Some(1));
        } else {
            chip(p.take, "Take over", p.color.d2d(1.0), rgba(0.08, 0.08, 0.09, 1.0), p.hover == Some(1));
        }
    }
}

/// The words after the agent's name.
fn pill_says(p: &Pill) -> String {
    if p.taken {
        "the tab is yours".into()
    } else if p.step.is_empty() {
        "working".into()
    } else {
        p.step.clone()
    }
}

/// Which button of the pill is at this client point (pixels): 0 Message, 1 Take over.
fn pill_hit(p: &Pill, x: i32, y: i32) -> Option<u8> {
    let (x, y) = (x as f32 / p.scale - PAD, y as f32 / p.scale - PAD);
    if !(6.0..PILL_H - 6.0).contains(&y) {
        return None;
    }
    if (p.message.0..p.message.0 + p.message.1).contains(&x) {
        Some(0)
    } else if (p.take.0..p.take.0 + p.take.1).contains(&x) {
        Some(1)
    } else {
        None
    }
}

// ------------------------------------------------------------------- the watch

pub struct Watch {
    owner: HWND,
    shield: HWND,
    cursor: Rc<RefCell<Cursor>>,
    pill: Rc<RefCell<Pill>>,
    /// The page area on screen, physical pixels: where the shield is.
    page: Cell<RECT>,
    shown: Cell<bool>,
}

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
        class(w!("LumenWatchPill"), Some(pill_proc), IDC_ARROW);
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
        }
        let scale = unsafe { GetDpiForWindow(owner) }.max(96) as f32 / 96.0;
        let cursor = Rc::new(RefCell::new(Cursor {
            hwnd: cursor_h,
            color: Rgb(0.5, 0.6, 1.0),
            name: String::new(),
            at: (0.0, 0.0),
            from: (0.0, 0.0),
            to: (0.0, 0.0),
            since: Instant::now(),
            ring_at: None,
            caret_at: None,
            scale,
        }));
        let pill = Rc::new(RefCell::new(Pill {
            hwnd: pill_h,
            color: Rgb(0.5, 0.6, 1.0),
            name: String::new(),
            step: String::new(),
            taken: false,
            proxy,
            scale,
            at: (0, 0, 1, 1),
            message: (0.0, 0.0),
            take: (0.0, 0.0),
            hover: None,
        }));
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
            let _ = ClientToScreen(self.owner, &mut o);
        }
        let r = RECT {
            left: o.x + (page.0 as f32 * scale) as i32,
            top: o.y + (page.1 as f32 * scale) as i32,
            right: o.x + ((page.0 + page.2) as f32 * scale) as i32,
            bottom: o.y + ((page.1 + page.3) as f32 * scale) as i32,
        };
        let color = Rgb(rgb.0 as f32 / 255.0, rgb.1 as f32 / 255.0, rgb.2 as f32 / 255.0);
        let first = !self.shown.get();
        let moved = first || self.page.get() != r;
        self.page.set(r);
        {
            let mut p = self.pill.borrow_mut();
            let changed = moved || p.name != name || p.step != step || p.taken != taken || p.color != color || p.scale != scale;
            if changed {
                p.name = name.to_string();
                p.step = step.to_string();
                p.taken = taken;
                p.color = color;
                p.scale = scale;
                layout_pill(&mut p, r);
                draw_pill(&p);
            }
            unsafe {
                let _ = ShowWindow(p.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        {
            let mut c = self.cursor.borrow_mut();
            let restyle = c.name != name || c.color != color || c.scale != scale;
            c.name = name.to_string();
            c.color = color;
            c.scale = scale;
            if first {
                // The first look: the cursor waits in the middle until the agent moves.
                let mid = ((r.left + r.right) as f32 / 2.0, (r.top + r.bottom) as f32 / 3.0);
                (c.at, c.from, c.to) = (mid, mid, mid);
            }
            if first || restyle || moved {
                draw_cursor(&c);
            }
            unsafe {
                let _ = ShowWindow(c.hwnd, SW_SHOWNOACTIVATE);
            }
        }
        unsafe {
            if taken {
                let _ = ShowWindow(self.shield, SW_HIDE);
            } else if moved || !IsWindowVisible(self.shield).as_bool() {
                let _ = SetWindowPos(self.shield, Some(HWND_TOP), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
            // Order: shield, pill, cursor on top.
            let flags = SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE;
            let _ = SetWindowPos(self.pill.borrow().hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags);
            let _ = SetWindowPos(self.cursor.borrow().hwnd, Some(HWND_TOP), 0, 0, 0, 0, flags);
        }
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
    /// `kind`: "move", "press" (a click: the ring) or "type" (a caret blink).
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
            "press" => c.ring_at = Some(Instant::now()),
            "type" => c.caret_at = Some(Instant::now()),
            _ => {}
        }
        unsafe {
            let _ = SetTimer(Some(c.hwnd), TIMER, 15, None);
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

unsafe extern "system" fn shield_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match m {
            // A click on the shield never takes focus from the window.
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            // Every click and wheel stops here: the agent's page is not to be touched.
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
                    draw_cursor(&c);
                    let busy = |at: Option<Instant>| at.is_some_and(|a| a.elapsed() < RING);
                    if t >= 1.0 && !busy(c.ring_at) && !busy(c.caret_at) {
                        let _ = KillTimer(Some(h), TIMER);
                    }
                });
                LRESULT(0)
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

unsafe extern "system" fn pill_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let at = || ((l.0 & 0xFFFF) as i16 as i32, ((l.0 >> 16) & 0xFFFF) as i16 as i32);
    unsafe {
        match m {
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_SETCURSOR => {
                // A hand over a button, an arrow elsewhere.
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(h, &mut pt);
                let on = PILL.with(|p| p.borrow().as_ref().and_then(|p| pill_hit(&p.borrow(), pt.x, pt.y)).is_some());
                let _ = SetCursor(LoadCursorW(None, if on { IDC_HAND } else { IDC_ARROW }).ok());
                LRESULT(1)
            }
            WM_MOUSEMOVE => {
                PILL.with(|p| {
                    if let Some(p) = p.borrow().clone() {
                        let (x, y) = at();
                        let hot = pill_hit(&p.borrow(), x, y);
                        if p.borrow().hover != hot {
                            p.borrow_mut().hover = hot;
                            draw_pill(&p.borrow());
                            // Leaving the window clears the hover.
                            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                            let _ = TrackMouseEvent(&mut tme);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                PILL.with(|p| {
                    if let Some(p) = p.borrow().clone()
                        && p.borrow().hover.is_some()
                    {
                        p.borrow_mut().hover = None;
                        draw_pill(&p.borrow());
                    }
                });
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                PILL.with(|p| {
                    if let Some(p) = p.borrow().clone() {
                        let p = p.borrow();
                        let (x, y) = at();
                        let t = match pill_hit(&p, x, y) {
                            Some(0) => "watchMessage",
                            Some(_) if p.taken => "watchGiveBack",
                            Some(_) => "watchTakeOver",
                            None => return,
                        };
                        let _ = p.proxy.send_event(Ev::Ui(format!("{{\"t\":\"{t}\"}}")));
                    }
                });
                LRESULT(0)
            }
            WM_MOUSEWHEEL | WM_RBUTTONUP | WM_LBUTTONDOWN => LRESULT(0),
            _ => DefWindowProcW(h, m, w, l),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::System::Com::StructuredStorage::IPropertyBag2;

    /// Draws the cursor (mid-click, typing) and both pills over a white and a dark page, and
    /// saves `%TEMP%\lumen-watch-sample.png`. Not a check: a picture to look at.
    #[test]
    #[ignore = "writes a picture to look at"]
    fn sample() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let (w, h, k) = (1200u32, 520u32, 1.25f32);
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).unwrap();
            let bmp = wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad).unwrap();
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0 * k,
                dpiY: 96.0 * k,
                ..Default::default()
            };
            let rt = d2d.CreateWicBitmapRenderTarget(&bmp, &props).unwrap();
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let kit = Kit { d2d, dw, rt: rt.clone(), dc: None };
            KIT.with(|x| *x.borrow_mut() = Some(Rc::new(Kit { d2d: kit.d2d.clone(), dw: kit.dw.clone(), rt: rt.clone(), dc: None })));
            rt.BeginDraw();
            let half = w as f32 / k / 2.0;
            let full_h = h as f32 / k;
            // Two pages: white, with a form; dark.
            for (n, bg, ink) in [(0.0, rgba(1.0, 1.0, 1.0, 1.0), rgba(0.1, 0.1, 0.1, 1.0)), (1.0, rgba(0.07, 0.07, 0.08, 1.0), rgba(0.9, 0.9, 0.9, 1.0))] {
                let x0 = n * half;
                rt.FillRectangle(&D2D_RECT_F { left: x0, top: 0.0, right: x0 + half, bottom: full_h }, &brush(&kit, bg).unwrap());
                text(&kit, "Customer name:", x0 + 30.0, 40.0, 200.0, 20.0, 13.0, 400, ink, false);
                rt.DrawRectangle(&D2D_RECT_F { left: x0 + 30.0, top: 64.0, right: x0 + 300.0, bottom: 92.0 }, &brush(&kit, rgba(0.5, 0.5, 0.5, 1.0)).unwrap(), 1.0, None);
                text(&kit, "Submit order", x0 + 40.0, 130.0, 120.0, 30.0, 13.0, 400, ink, false);
            }
            use tao::platform::windows::EventLoopBuilderExtWindows;
            let ev = tao::event_loop::EventLoopBuilder::<Ev>::with_user_event().with_any_thread(true).build();
            let proxy = ev.create_proxy();
            let colors = [Rgb(0.47, 0.67, 1.0), Rgb(1.0, 0.67, 0.47)];
            for (n, color) in colors.iter().enumerate() {
                let x0 = n as f32 * half;
                let c = Cursor {
                    hwnd: HWND::default(),
                    color: *color,
                    name: "claude-code".into(),
                    at: (0.0, 0.0),
                    from: (0.0, 0.0),
                    to: (0.0, 0.0),
                    since: Instant::now(),
                    ring_at: None,
                    caret_at: None,
                    scale: k,
                };
                rt.SetTransform(&windows_numerics::Matrix3x2::translation(x0 + 150.0 - TIP, 140.0 - TIP));
                paint_cursor(&kit, &c, Some(0.35), None);
                rt.SetTransform(&windows_numerics::Matrix3x2::translation(x0 + 240.0 - TIP, 70.0 - TIP));
                paint_cursor(&kit, &c, None, Some(0.2));
                for (row, taken) in [(0usize, false), (1, true)] {
                    let mut p = Pill {
                        hwnd: HWND::default(),
                        color: *color,
                        name: "claude-code".into(),
                        step: "typing \u{201C}Ada Lovelace\u{201D}".into(),
                        taken,
                        proxy: proxy.clone(),
                        scale: k,
                        at: (0, 0, 1, 1),
                        message: (0.0, 0.0),
                        take: (0.0, 0.0),
                        hover: None,
                    };
                    layout_pill(&mut p, RECT { left: 0, top: 0, right: (half * k) as i32, bottom: (full_h * k) as i32 });
                    let pw = p.at.2 as f32 / k;
                    rt.SetTransform(&windows_numerics::Matrix3x2::translation(x0 + (half - pw) / 2.0, 300.0 + row as f32 * 70.0));
                    paint_pill(&kit, &p, pw - 2.0 * PAD);
                }
            }
            rt.SetTransform(&windows_numerics::Matrix3x2::identity());
            rt.EndDraw(None, None).unwrap();
            let path = std::env::temp_dir().join("lumen-watch-sample.png");
            let stream = wic.CreateStream().unwrap();
            stream.InitializeFromFilename(&HSTRING::from(path.to_string_lossy().as_ref()), 0x4000_0000).unwrap();
            let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).unwrap();
            enc.Initialize(&stream, WICBitmapEncoderNoCache).unwrap();
            let mut frame = None;
            let mut bag: Option<IPropertyBag2> = None;
            enc.CreateNewFrame(&mut frame, &mut bag).unwrap();
            let frame = frame.unwrap();
            frame.Initialize(bag.as_ref()).unwrap();
            frame.WriteSource(&bmp, std::ptr::null()).unwrap();
            frame.Commit().unwrap();
            enc.Commit().unwrap();
            eprintln!("wrote {}", path.display());
        }
    }
}
