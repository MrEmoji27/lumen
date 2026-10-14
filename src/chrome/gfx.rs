//! Drawing. Direct2D for shapes and bitmaps, DirectWrite for text.
//! Everything is in DIPs; the render target carries the window's DPI.

use std::{cell::RefCell, collections::HashMap};

use windows::{
    Win32::{
        Foundation::{GENERIC_READ, HWND},
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Imaging::*,
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    },
    core::{HSTRING, Interface, PCWSTR, w},
};
use windows_numerics::Vector2;

use super::theme::Color;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub fn inset(&self, dx: f32, dy: f32) -> R {
        R::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Face {
    Ui,
    Serif,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Font {
    pub face: Face,
    /// Size in tenths of a DIP, so it can be a hash key.
    pub size10: u32,
    pub weight: u32,
    pub italic: bool,
    pub align: Align,
}

impl Font {
    pub const fn ui(size: f32, weight: u32) -> Font {
        Font { face: Face::Ui, size10: (size * 10.0) as u32, weight, italic: false, align: Align::Left }
    }
    pub const fn serif(size: f32) -> Font {
        Font { face: Face::Serif, size10: (size * 10.0) as u32, weight: 400, italic: false, align: Align::Left }
    }
    pub const fn italic(mut self) -> Font {
        self.italic = true;
        self
    }
    pub const fn align(mut self, a: Align) -> Font {
        self.align = a;
        self
    }
}

pub struct Gfx {
    hwnd: HWND,
    d2d: ID2D1Factory,
    dw: IDWriteFactory5,
    wic: Option<IWICImagingFactory>,
    serif: Option<IDWriteFontCollection1>,
    target: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    round: Option<ID2D1StrokeStyle>,
    formats: RefCell<HashMap<Font, IDWriteTextFormat>>,
    ellipsis: RefCell<HashMap<Font, IDWriteInlineObject>>,
    /// Decoded favicons, per tab id. Dropped when the target is lost.
    pub bitmaps: HashMap<u64, ID2D1Bitmap>,
    pub dpi: f32,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl Gfx {
    pub fn new(hwnd: HWND, dpi: f32) -> windows::core::Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory5 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
            let round = d2d
                .CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_ROUND,
                        endCap: D2D1_CAP_STYLE_ROUND,
                        dashCap: D2D1_CAP_STYLE_ROUND,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        miterLimit: 10.0,
                        dashStyle: D2D1_DASH_STYLE_SOLID,
                        dashOffset: 0.0,
                    },
                    None,
                )
                .ok();
            let serif = load_woff2(&dw, include_bytes!("../../ui/fonts/serif.woff2"), include_bytes!("../../ui/fonts/serif-italic.woff2"));
            Ok(Gfx {
                hwnd,
                d2d,
                dw,
                wic,
                serif,
                target: None,
                brush: None,
                round,
                formats: RefCell::new(HashMap::new()),
                ellipsis: RefCell::new(HashMap::new()),
                bitmaps: HashMap::new(),
                dpi,
            })
        }
    }

    fn ensure_target(&mut self, w: u32, h: u32) -> bool {
        if self.target.is_none() {
            unsafe {
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                    dpiX: 0.0,
                    dpiY: 0.0,
                    usage: D2D1_RENDER_TARGET_USAGE_NONE,
                    minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
                };
                let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd: self.hwnd,
                    pixelSize: D2D_SIZE_U { width: w.max(1), height: h.max(1) },
                    presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                };
                let Ok(t) = self.d2d.CreateHwndRenderTarget(&props, &hp) else { return false };
                t.SetDpi(self.dpi, self.dpi);
                t.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                self.brush = t.CreateSolidColorBrush(&Color(0.0, 0.0, 0.0, 1.0).d2d(), None).ok();
                self.target = Some(t);
                self.bitmaps.clear();
            }
        }
        true
    }

    pub fn resize(&mut self, w: u32, h: u32, dpi: f32) {
        self.dpi = dpi;
        if let Some(t) = &self.target {
            unsafe {
                let _ = t.Resize(&D2D_SIZE_U { width: w.max(1), height: h.max(1) });
                t.SetDpi(dpi, dpi);
            }
        }
    }

    /// Start a frame. False means nothing can be drawn right now.
    pub fn begin(&mut self, w: u32, h: u32, clear: Color) -> bool {
        if !self.ensure_target(w, h) {
            return false;
        }
        let t = self.target.as_ref().unwrap();
        unsafe {
            t.BeginDraw();
            t.Clear(Some(&clear.d2d()));
        }
        true
    }

    pub fn end(&mut self) {
        if let Some(t) = &self.target {
            let r = unsafe { t.EndDraw(None, None) };
            // Device lost (driver reset, sleep). Rebuild on the next frame.
            if r.is_err() {
                self.target = None;
                self.brush = None;
                self.bitmaps.clear();
            }
        }
    }

    fn t(&self) -> &ID2D1HwndRenderTarget {
        self.target.as_ref().unwrap()
    }

    fn b(&self, c: Color) -> &ID2D1SolidColorBrush {
        let b = self.brush.as_ref().unwrap();
        unsafe { b.SetColor(&c.d2d()) };
        b
    }

    // ----------------------------------------------------------- shapes

    #[allow(dead_code)]
    pub fn fill(&self, r: R, c: Color) {
        unsafe { self.t().FillRectangle(&r.d2d(), self.b(c)) };
    }

    pub fn round(&self, r: R, radius: f32, c: Color) {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.t().FillRoundedRectangle(&rr, self.b(c)) };
    }

    pub fn round_stroke(&self, r: R, radius: f32, c: Color, width: f32) {
        let half = width / 2.0;
        let rr = D2D1_ROUNDED_RECT { rect: r.inset(half, half).d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.t().DrawRoundedRectangle(&rr, self.b(c), width, None) };
    }

    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, c: Color, w: f32) {
        unsafe {
            self.t().DrawLine(Vector2 { X: x1, Y: y1 }, Vector2 { X: x2, Y: y2 }, self.b(c), w, self.round.as_ref())
        };
    }

    pub fn circle(&self, cx: f32, cy: f32, r: f32, c: Color) {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
        unsafe { self.t().FillEllipse(&e, self.b(c)) };
    }

    pub fn ring(&self, cx: f32, cy: f32, rx: f32, ry: f32, c: Color, w: f32) {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: rx, radiusY: ry };
        unsafe { self.t().DrawEllipse(&e, self.b(c), w, self.round.as_ref()) };
    }

    /// Open polyline or arcs, stroked. Points in DIPs.
    fn path(&self, build: impl FnOnce(&ID2D1GeometrySink), c: Color, w: f32, closed: bool) {
        unsafe {
            let Ok(g) = self.d2d.CreatePathGeometry() else { return };
            let Ok(sink) = g.Open() else { return };
            build(&sink);
            let _ = closed;
            let _ = sink.Close();
            self.t().DrawGeometry(&g, self.b(c), w, self.round.as_ref());
        }
    }

    pub fn polyline(&self, pts: &[(f32, f32)], c: Color, w: f32, closed: bool) {
        if pts.len() < 2 {
            return;
        }
        self.path(
            |s| unsafe {
                s.BeginFigure(Vector2 { X: pts[0].0, Y: pts[0].1 }, D2D1_FIGURE_BEGIN_HOLLOW);
                for p in &pts[1..] {
                    s.AddLine(Vector2 { X: p.0, Y: p.1 });
                }
                s.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            },
            c,
            w,
            closed,
        );
    }

    /// Arc around (cx, cy) from angle a0 sweeping `sweep` degrees clockwise.
    pub fn arc(&self, cx: f32, cy: f32, r: f32, a0: f32, sweep: f32, c: Color, w: f32) {
        let pt = |a: f32| {
            let a = a.to_radians();
            Vector2 { X: cx + r * a.cos(), Y: cy + r * a.sin() }
        };
        self.path(
            |s| unsafe {
                s.BeginFigure(pt(a0), D2D1_FIGURE_BEGIN_HOLLOW);
                s.AddArc(&D2D1_ARC_SEGMENT {
                    point: pt(a0 + sweep),
                    size: D2D_SIZE_F { width: r, height: r },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: if sweep > 180.0 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                });
                s.EndFigure(D2D1_FIGURE_END_OPEN);
            },
            c,
            w,
            false,
        );
    }

    pub fn clip(&self, r: R) {
        unsafe { self.t().PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    pub fn unclip(&self) {
        unsafe { self.t().PopAxisAlignedClip() };
    }

    // ------------------------------------------------------------- text

    fn format(&self, f: Font) -> Option<IDWriteTextFormat> {
        if let Some(x) = self.formats.borrow().get(&f) {
            return Some(x.clone());
        }
        unsafe {
            let (family, coll): (PCWSTR, Option<IDWriteFontCollection>) = match f.face {
                Face::Ui => (w!("Segoe UI Variable Text"), None),
                Face::Serif => match &self.serif {
                    Some(c) => (w!("Instrument Serif"), c.cast().ok()),
                    None => (w!("Georgia"), None),
                },
            };
            let fmt = self
                .dw
                .CreateTextFormat(
                    family,
                    coll.as_ref(),
                    DWRITE_FONT_WEIGHT(f.weight as i32),
                    if f.italic { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL },
                    DWRITE_FONT_STRETCH_NORMAL,
                    f.size10 as f32 / 10.0,
                    w!("en-us"),
                )
                .ok()?;
            let _ = fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            let _ = fmt.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = fmt.SetTextAlignment(match f.align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            if let Ok(sign) = self.dw.CreateEllipsisTrimmingSign(&fmt) {
                let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                let _ = fmt.SetTrimming(&trim, &sign);
                self.ellipsis.borrow_mut().insert(f, sign);
            }
            self.formats.borrow_mut().insert(f, fmt.clone());
            Some(fmt)
        }
    }

    /// One line of text, vertically centered in r, ellipsized if it does not fit.
    pub fn text(&self, s: &str, r: R, f: Font, c: Color) {
        let Some(fmt) = self.format(f) else { return };
        let w = wide(s);
        unsafe {
            self.t().DrawText(
                &w,
                &fmt,
                &r.d2d(),
                self.b(c),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }

    pub fn measure(&self, s: &str, f: Font) -> f32 {
        let Some(fmt) = self.format(f) else { return 0.0 };
        let w = wide(s);
        unsafe {
            let Ok(layout) = self.dw.CreateTextLayout(&w, &fmt, 10_000.0, 1_000.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() { m.widthIncludingTrailingWhitespace } else { 0.0 }
        }
    }

    // ----------------------------------------------------------- images

    /// PNG bytes to a GPU bitmap, cached under `key`.
    pub fn bitmap(&mut self, key: u64, png: &[u8]) -> Option<ID2D1Bitmap> {
        if let Some(b) = self.bitmaps.get(&key) {
            return Some(b.clone());
        }
        let wic = self.wic.as_ref()?;
        let t = self.target.as_ref()?;
        unsafe {
            let stream = wic.CreateStream().ok()?;
            stream.InitializeFromMemory(png).ok()?;
            let dec = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
            let frame = dec.GetFrame(0).ok()?;
            let conv = wic.CreateFormatConverter().ok()?;
            conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)
                .ok()?;
            let bmp = t.CreateBitmapFromWicBitmap(&conv, None).ok()?;
            self.bitmaps.insert(key, bmp.clone());
            Some(bmp)
        }
    }

    pub fn draw_bitmap(&self, b: &ID2D1Bitmap, r: R, opacity: f32) {
        unsafe {
            self.t().DrawBitmap(b, Some(&r.d2d()), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None)
        };
    }

    // ------------------------------------------------------------ icons

    /// Line icons on a 16-unit grid, drawn at (x, y) with size `s` DIPs.
    pub fn icon(&self, name: &str, x: f32, y: f32, s: f32, c: Color) {
        let k = s / 16.0;
        let p = |px: f32, py: f32| (x + px * k, y + py * k);
        let w = 1.5 * k.max(0.9);
        let l = |a: (f32, f32), b: (f32, f32)| self.line(a.0, a.1, b.0, b.1, c, w);
        match name {
            "back" => self.polyline(&[p(10.0, 3.5), p(5.5, 8.0), p(10.0, 12.5)], c, w, false),
            "fwd" => self.polyline(&[p(6.0, 3.5), p(10.5, 8.0), p(6.0, 12.5)], c, w, false),
            "reload" => {
                let (cx, cy) = p(8.0, 8.0);
                self.arc(cx, cy, 4.8 * k, -50.0, 300.0, c, w);
                self.polyline(&[p(12.7, 2.9), p(12.7, 5.6), p(10.0, 5.6)], c, w, false);
            }
            "stop" | "close" => {
                l(p(4.5, 4.5), p(11.5, 11.5));
                l(p(11.5, 4.5), p(4.5, 11.5));
            }
            "min" => l(p(4.0, 8.5), p(12.0, 8.5)),
            "max" => {
                let (a, b) = (p(4.0, 4.0), p(12.0, 12.0));
                self.round_stroke(R::new(a.0, a.1, b.0 - a.0, b.1 - a.1), 1.6 * k, c, w);
            }
            "restore" => {
                let (a, b) = (p(3.5, 5.5), p(10.5, 12.5));
                self.round_stroke(R::new(a.0, a.1, b.0 - a.0, b.1 - a.1), 1.4 * k, c, w);
                self.polyline(&[p(6.0, 5.5), p(6.0, 3.5), p(12.5, 3.5), p(12.5, 10.0), p(10.5, 10.0)], c, w, false);
            }
            "plus" => {
                l(p(8.0, 3.5), p(8.0, 12.5));
                l(p(3.5, 8.0), p(12.5, 8.0));
            }
            "search" => {
                let (cx, cy) = p(7.2, 7.2);
                self.ring(cx, cy, 4.2 * k, 4.2 * k, c, w);
                l(p(10.4, 10.4), p(13.2, 13.2));
            }
            "lock" => {
                let (a, b) = (p(4.0, 7.0), p(12.0, 13.0));
                self.round_stroke(R::new(a.0, a.1, b.0 - a.0, b.1 - a.1), 1.6 * k, c, w);
                let (cx, cy) = p(8.0, 5.6);
                self.arc(cx, cy, 2.2 * k, 180.0, 180.0, c, w);
                l(p(5.8, 5.6), p(5.8, 7.0));
                l(p(10.2, 5.6), p(10.2, 7.0));
            }
            "globe" => {
                let (cx, cy) = p(8.0, 8.0);
                self.ring(cx, cy, 5.2 * k, 5.2 * k, c, w);
                self.ring(cx, cy, 2.2 * k, 5.2 * k, c, w);
                l(p(2.8, 8.0), p(13.2, 8.0));
            }
            "tab" => {
                let (a, b) = (p(2.8, 3.5), p(13.2, 12.5));
                self.round_stroke(R::new(a.0, a.1, b.0 - a.0, b.1 - a.1), 2.0 * k, c, w);
                l(p(2.8, 6.5), p(13.2, 6.5));
            }
            "sliders" => {
                l(p(3.0, 5.0), p(9.5, 5.0));
                l(p(12.5, 5.0), p(13.0, 5.0));
                l(p(3.0, 11.0), p(3.5, 11.0));
                l(p(6.5, 11.0), p(13.0, 11.0));
                let (a, b) = (p(11.0, 5.0), p(5.0, 11.0));
                self.ring(a.0, a.1, 1.5 * k, 1.5 * k, c, w);
                self.ring(b.0, b.1, 1.5 * k, 1.5 * k, c, w);
            }
            "sound" | "muted" => {
                self.polyline(&[p(3.0, 6.4), p(5.1, 6.4), p(8.0, 4.0), p(8.0, 12.0), p(5.1, 9.6), p(3.0, 9.6)], c, w, true);
                if name == "sound" {
                    let (cx, cy) = p(9.2, 8.0);
                    self.arc(cx, cy, 2.4 * k, -50.0, 100.0, c, w);
                } else {
                    l(p(10.5, 6.5), p(13.5, 9.5));
                    l(p(13.5, 6.5), p(10.5, 9.5));
                }
            }
            "pin" => {
                l(p(8.0, 10.0), p(8.0, 14.0));
                self.polyline(&[p(5.0, 10.0), p(11.0, 10.0), p(10.0, 7.0), p(10.0, 3.0), p(6.0, 3.0), p(6.0, 7.0)], c, w, true);
            }
            _ => {}
        }
    }

    /// Loading ring: a faint track and a turning arc.
    pub fn spinner(&self, cx: f32, cy: f32, r: f32, phase: f32, track: Color, c: Color) {
        self.ring(cx, cy, r, r, track, 1.5);
        self.arc(cx, cy, r, phase * 360.0, 90.0, c, 1.5);
    }
}

/// Unpack WOFF2 fonts into an in-memory collection DirectWrite can use.
fn load_woff2(dw: &IDWriteFactory5, regular: &[u8], italic: &[u8]) -> Option<IDWriteFontCollection1> {
    unsafe {
        let loader = dw.CreateInMemoryFontFileLoader().ok()?;
        dw.RegisterFontFileLoader(&loader).ok()?;
        let builder: IDWriteFontSetBuilder1 = dw.CreateFontSetBuilder().ok()?.cast().ok()?;
        for data in [regular, italic] {
            let Ok(stream) = dw.UnpackFontFile(DWRITE_CONTAINER_TYPE_WOFF2, data.as_ptr() as _, data.len() as u32) else { continue };
            let Ok(size) = stream.GetFileSize() else { continue };
            let mut ptr = std::ptr::null_mut();
            let mut ctx = std::ptr::null_mut();
            if stream.ReadFileFragment(&mut ptr, 0, size, &mut ctx).is_err() {
                continue;
            }
            let bytes = std::slice::from_raw_parts(ptr as *const u8, size as usize).to_vec();
            stream.ReleaseFileFragment(ctx);
            // No owner: DirectWrite copies the bytes.
            let Ok(file) = loader.CreateInMemoryFontFileReference(dw, bytes.as_ptr() as _, bytes.len() as u32, None) else { continue };
            let _ = builder.AddFontFile(&file);
        }
        let set = builder.CreateFontSet().ok()?;
        dw.CreateFontCollectionFromFontSet(&set).ok()
    }
}

/// Keep the compiler quiet about helpers only some builds use.
#[allow(dead_code)]
fn _unused(_: HSTRING) {
    let _ = GENERIC_READ;
}
