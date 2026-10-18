//! Drawing. Direct2D for shapes and bitmaps, DirectWrite for text.
//! Everything is in DIPs; the render target carries the window's DPI.

use std::{cell::RefCell, collections::HashMap};

use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dxgi::Common::{DXGI_FORMAT_A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM},
            Imaging::*,
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    },
    core::{HSTRING, Interface, w},
};
use windows_numerics::Vector2;

use super::theme::Color;

mod icons;

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
    Mono,
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
    /// Every bundled font, unpacked from WOFF2 into one in-memory collection.
    bundled: Option<IDWriteFontCollection1>,
    /// Every family Windows has installed, as DirectWrite names them. Read once.
    installed: Vec<String>,
    ui_family: String,
    serif_family: String,
    mono_family: String,
    /// Family names as DirectWrite sees them in the bundle (font files name themselves oddly).
    families: Vec<String>,
    /// What every draw call goes to: the window's target, or (debug shots) a WIC bitmap's.
    target: Option<ID2D1RenderTarget>,
    /// The window's own target, kept for Resize.
    hwnd_rt: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    round: Option<ID2D1StrokeStyle>,
    formats: RefCell<HashMap<Font, IDWriteTextFormat>>,
    ellipsis: RefCell<HashMap<Font, IDWriteInlineObject>>,
    /// Decoded favicons, per tab id. Dropped when the target is lost.
    pub bitmaps: HashMap<u64, ID2D1Bitmap>,
    /// Lucide icons as alpha masks for the current target, tinted when drawn.
    /// Key: (icon index, pixel size, stroke x100). Bounded; dropped with the target.
    icon_masks: RefCell<HashMap<(usize, u32, u32), ID2D1Bitmap>>,
    /// The SVG rasterizer: made when a mask is missing, dropped at the end of that frame.
    raster: RefCell<Option<icons::Rasterizer>>,
    /// Rasterizing failed once: draw the hand-made icons from now on instead of retrying every frame.
    raster_failed: std::cell::Cell<bool>,
    pub dpi: f32,
    /// Debug shots: the window's target and brush, set aside while a WIC target draws.
    #[cfg(debug_assertions)]
    parked: Option<Parked>,
}

#[cfg(debug_assertions)]
struct Parked {
    target: Option<ID2D1RenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    bitmaps: HashMap<u64, ID2D1Bitmap>,
    icon_masks: HashMap<(usize, u32, u32), ID2D1Bitmap>,
    shot: IWICBitmap,
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
            let fonts_t0 = std::time::Instant::now();
            let bundled = load_woff2(
                &dw,
                &[
                    include_bytes!("../../ui/fonts/serif.woff2"),
                    include_bytes!("../../ui/fonts/serif-italic.woff2"),
                    include_bytes!("../../ui/fonts/sans-instrument.woff2"),
                    include_bytes!("../../ui/fonts/sans-figtree.woff2"),
                    include_bytes!("../../ui/fonts/sans-atkinson.woff2"),
                    include_bytes!("../../ui/fonts/serif-fraunces.woff2"),
                    include_bytes!("../../ui/fonts/serif-fraunces-italic.woff2"),
                    include_bytes!("../../ui/fonts/serif-newsreader.woff2"),
                    include_bytes!("../../ui/fonts/serif-newsreader-italic.woff2"),
                ],
            );
            crate::startup_line("fonts", fonts_t0.elapsed().as_secs_f64() * 1000.0);
            Ok(Gfx {
                hwnd,
                d2d,
                dw: dw.clone(),
                wic,
                families: family_names(bundled.as_ref()),
                installed: system_families(&dw),
                bundled,
                ui_family: SYSTEM_UI.into(),
                serif_family: "Instrument Serif".into(),
                mono_family: MONO_FALLBACK.into(),
                target: None,
                hwnd_rt: None,
                brush: None,
                round,
                formats: RefCell::new(HashMap::new()),
                ellipsis: RefCell::new(HashMap::new()),
                bitmaps: HashMap::new(),
                icon_masks: RefCell::new(HashMap::new()),
                raster: RefCell::new(None),
                raster_failed: std::cell::Cell::new(false),
                dpi,
                #[cfg(debug_assertions)]
                parked: None,
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
                    // Present and go on. Waiting for the vertical blank held the UI thread
                    // (pages, agent calls, input) 16 to 40 ms every frame for a 1 ms draw;
                    // DWM composes the window, so nothing tears.
                    presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
                };
                let Ok(t) = self.d2d.CreateHwndRenderTarget(&props, &hp) else { return false };
                t.SetDpi(self.dpi, self.dpi);
                t.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                self.brush = t.CreateSolidColorBrush(&Color(0.0, 0.0, 0.0, 1.0).d2d(), None).ok();
                self.target = t.cast::<ID2D1RenderTarget>().ok();
                self.hwnd_rt = Some(t);
                self.bitmaps.clear();
                self.icon_masks.borrow_mut().clear();
            }
        }
        true
    }

    pub fn resize(&mut self, w: u32, h: u32, dpi: f32) {
        self.dpi = dpi;
        if let Some(t) = &self.hwnd_rt {
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
        // The SVG rasterizer only lives for the frame that needed it.
        self.raster.take();
        if let Some(t) = &self.target {
            let r = unsafe { t.EndDraw(None, None) };
            // Device lost (driver reset, sleep). Rebuild on the next frame.
            if r.is_err() {
                self.target = None;
                self.hwnd_rt = None;
                self.brush = None;
                self.bitmaps.clear();
                self.icon_masks.borrow_mut().clear();
            }
        }
    }

    fn t(&self) -> &ID2D1RenderTarget {
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

    /// A rounded rectangle filled with a straight gradient: the stops run left to
    /// right, or top to bottom with `vertical`. Each stop is a position 0..1 and a
    /// colour, alpha included, so two gradients can be laid over each other.
    pub fn grad(&self, r: R, radius: f32, stops: &[(f32, Color)], vertical: bool) {
        unsafe {
            let t = self.t();
            let stops: Vec<D2D1_GRADIENT_STOP> = stops.iter().map(|&(p, c)| D2D1_GRADIENT_STOP { position: p, color: c.d2d() }).collect();
            let Ok(coll) = t.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 { X: r.x, Y: r.y },
                endPoint: if vertical { Vector2 { X: r.x, Y: r.bottom() } } else { Vector2 { X: r.right(), Y: r.y } },
            };
            let Ok(b) = t.CreateLinearGradientBrush(&props, None, &coll) else { return };
            let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
            t.FillRoundedRectangle(&rr, &b);
        }
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

    /// A soft bloom: `c` at the centre fading to nothing at radius `r`. Glass's wallpaper is
    /// two of these on its ground; a smooth gradient needs no blur to look frosted.
    pub fn bloom(&self, cx: f32, cy: f32, r: f32, c: Color) {
        unsafe {
            let t = self.t();
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: c.d2d() },
                D2D1_GRADIENT_STOP { position: 0.55, color: c.alpha(0.45).d2d() },
                D2D1_GRADIENT_STOP { position: 1.0, color: c.alpha(0.0).d2d() },
            ];
            let Ok(coll) = t.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
            let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                center: Vector2 { X: cx, Y: cy },
                gradientOriginOffset: Vector2 { X: 0.0, Y: 0.0 },
                radiusX: r,
                radiusY: r,
            };
            let Ok(b) = t.CreateRadialGradientBrush(&props, None, &coll) else { return };
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
            t.FillEllipse(&e, &b);
        }
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

    /// Choose the interface, display and monospace families by setting. A key
    /// ("segoe", "instrument", "cascadia"...) picks a bundled font or the system
    /// default; any other name is looked up among the fonts Windows has
    /// installed. Clears cached formats on change.
    pub fn set_families(&mut self, ui: &str, display: &str, mono: &str) {
        let ui = self.resolve(ui, Face::Ui);
        let serif = self.resolve(display, Face::Serif);
        let mono = self.resolve(mono, Face::Mono);
        let (ui, serif, mono) = (ui.as_str(), serif.as_str(), mono.as_str());
        if ui != self.ui_family || serif != self.serif_family || mono != self.mono_family {
            self.ui_family = ui.into();
            self.serif_family = serif.into();
            self.mono_family = mono.into();
            self.formats.borrow_mut().clear();
            self.ellipsis.borrow_mut().clear();
        }
    }

    /// A setting value to a family DirectWrite knows: a bundled key, an installed
    /// family (by its own spelling), or the face's fallback.
    fn resolve(&self, key: &str, face: Face) -> String {
        let key = key.trim();
        if let Some(bundled_key) = keyword(key) {
            return self.bundled_named(bundled_key).unwrap_or_else(|| {
                // A bundled family that will not load: Windows' own, or the monospace fallback.
                match face {
                    Face::Ui => SYSTEM_UI.into(),
                    Face::Serif => SERIF_FALLBACK.into(),
                    Face::Mono => self.installed_named(MONO_FALLBACK).unwrap_or_else(|| SYSTEM_UI.into()),
                }
            });
        }
        if key.is_empty() {
            return self.fallback(face);
        }
        // The monospace keys name fonts that ship with Windows, not with Lumen.
        if face == Face::Mono {
            return self.installed_named(key).unwrap_or_else(|| self.installed_named(MONO_FALLBACK).unwrap_or_else(|| SYSTEM_UI.into()));
        }
        // A bundled name said directly ("Instrument Sans").
        if let Some(f) = self.bundled_named(key) {
            return f;
        }
        // An installed family, spelled the way Windows spells it.
        self.installed_named(key).unwrap_or_else(|| self.fallback(face))
    }

    fn fallback(&self, face: Face) -> String {
        match face {
            Face::Ui => SYSTEM_UI.into(),
            Face::Serif => self.bundled_named("Instrument Serif").unwrap_or_else(|| SERIF_FALLBACK.into()),
            Face::Mono => self.installed_named(MONO_FALLBACK).unwrap_or_else(|| SYSTEM_UI.into()),
        }
    }

    /// A family name in the bundle, matched by keyword (the files name themselves
    /// in their own language).
    fn bundled_named(&self, word: &str) -> Option<String> {
        self.families.iter().find(|f| f.contains(word)).cloned()
    }

    /// A family name Windows has installed, matched without case.
    fn installed_named(&self, name: &str) -> Option<String> {
        self.installed.iter().find(|f| f.eq_ignore_ascii_case(name)).cloned()
    }

    /// Every installed family name, in the order DirectWrite lists them. For the
    /// settings' searchable font picker.
    pub fn installed_families(&self) -> &[String] {
        &self.installed
    }

    /// The interface family in use ("Segoe UI Variable Text" when the system one).
    pub fn ui_family(&self) -> &str {
        &self.ui_family
    }

    fn format(&self, f: Font) -> Option<IDWriteTextFormat> {
        if let Some(x) = self.formats.borrow().get(&f) {
            return Some(x.clone());
        }
        unsafe {
            // System font from the system collection; anything else from the bundle.
            let bundled = || self.bundled.as_ref().and_then(|c| c.cast::<IDWriteFontCollection>().ok());
            let name = match f.face {
                Face::Ui => self.ui_family.as_str(),
                Face::Serif => self.serif_family.as_str(),
                Face::Mono => self.mono_family.as_str(),
            };
            // A family the bundle carries is looked up in it; every other name
            // (Windows' own fonts) comes from the system collection.
            let coll = if self.families.iter().any(|f| f == name) { bundled() } else { None };
            let (family, coll): (HSTRING, Option<IDWriteFontCollection>) = (HSTRING::from(name), coll);
            let fmt = self
                .dw
                .CreateTextFormat(
                    &family,
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

    /// Text that wraps onto as many lines as fit in r, from the top. Clipped at r's bottom.
    pub fn text_wrap(&self, s: &str, r: R, f: Font, c: Color) {
        let Some(fmt) = self.format(f) else { return };
        let w = wide(s);
        unsafe {
            let Ok(layout) = self.dw.CreateTextLayout(&w, &fmt, r.w, r.h) else { return };
            let _ = layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            let _ = layout.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
            self.t().DrawTextLayout(
                windows_numerics::Vector2 { X: r.x, Y: r.y },
                &layout,
                self.b(c),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
            );
        }
    }

    /// One line with its letters set `track` DIPs apart (the small capital labels), vertically
    /// centred in r and clipped to it.
    pub fn text_tracked(&self, s: &str, r: R, f: Font, c: Color, track: f32) {
        let Some(fmt) = self.format(f) else { return };
        let w = wide(s);
        unsafe {
            let Ok(layout) = self.dw.CreateTextLayout(&w, &fmt, r.w.max(1.0), r.h.max(1.0)) else { return };
            if let Ok(l1) = layout.cast::<IDWriteTextLayout1>() {
                let all = DWRITE_TEXT_RANGE { startPosition: 0, length: w.len() as u32 };
                let _ = l1.SetCharacterSpacing(0.0, track, 0.0, all);
            }
            self.t().DrawTextLayout(Vector2 { X: r.x, Y: r.y }, &layout, self.b(c), D2D1_DRAW_TEXT_OPTIONS_CLIP);
        }
    }

    /// How wide `text_tracked` sets a line.
    pub fn measure_tracked(&self, s: &str, f: Font, track: f32) -> f32 {
        self.measure(s, f) + track * s.chars().count() as f32
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

    /// Image file bytes (PNG, JPEG, BMP, GIF) to a bitmap cached under `key`, at
    /// no more than `max_px` on the long side. Drawing a small bitmap enlarged is
    /// the cheap, dependable blur: WIC does the work once, not every frame.
    pub fn bitmap_scaled(&mut self, key: u64, bytes: &[u8], max_px: u32) -> Option<ID2D1Bitmap> {
        if let Some(b) = self.bitmaps.get(&key) {
            return Some(b.clone());
        }
        let wic = self.wic.as_ref()?;
        let t = self.target.as_ref()?;
        unsafe {
            let stream = wic.CreateStream().ok()?;
            stream.InitializeFromMemory(bytes).ok()?;
            let dec = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
            let frame = dec.GetFrame(0).ok()?;
            let conv = wic.CreateFormatConverter().ok()?;
            conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)
                .ok()?;
            let (mut w, mut h) = (0u32, 0u32);
            conv.GetSize(&mut w, &mut h).ok()?;
            let bmp = if w > max_px || h > max_px {
                let k = max_px as f32 / w.max(h).max(1) as f32;
                let scaler = wic.CreateBitmapScaler().ok()?;
                // Fant: a soft downsample, which is what the blur wants.
                scaler
                    .Initialize(&conv, ((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1), WICBitmapInterpolationModeFant)
                    .ok()?;
                t.CreateBitmapFromWicBitmap(&scaler, None).ok()?
            } else {
                t.CreateBitmapFromWicBitmap(&conv, None).ok()?
            };
            self.bitmaps.insert(key, bmp.clone());
            Some(bmp)
        }
    }

    // ------------------------------------------------------------ icons

    /// An icon by Lumen's name, drawn at (x, y) with size `s` DIPs. Lucide where the set has
    /// it (see `icons.rs`), hand-made geometry for the rest.
    pub fn icon(&self, name: &str, x: f32, y: f32, s: f32, c: Color) {
        if self.lucide(name, x, y, s, c) {
            return;
        }
        self.geometry_icon(name, x, y, s, c);
    }

    /// The Lucide mask for (icon, pixel size, stroke): from the cache, or rasterized now.
    fn icon_mask(&self, idx: usize, px: u32, stroke100: u32) -> Option<ID2D1Bitmap> {
        let key = (idx, px, stroke100);
        if let Some(b) = self.icon_masks.borrow().get(&key) {
            return Some(b.clone());
        }
        if self.raster_failed.get() {
            return None;
        }
        let entry = &icons::TABLE[idx];
        let mut raster = self.raster.borrow_mut();
        if raster.is_none() {
            *raster = icons::Rasterizer::new(&self.d2d);
        }
        let alpha = raster.as_ref().and_then(|r| r.alpha(&entry.source(stroke100 as f32 / 100.0), px));
        let Some(alpha) = alpha else {
            self.raster_failed.set(true);
            return None;
        };
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_A8_UNORM, alphaMode: D2D1_ALPHA_MODE_STRAIGHT },
            dpiX: self.dpi,
            dpiY: self.dpi,
        };
        let bmp = unsafe {
            self.target.as_ref()?.CreateBitmap(D2D_SIZE_U { width: px, height: px }, Some(alpha.as_ptr() as _), px, &props).ok()?
        };
        let mut masks = self.icon_masks.borrow_mut();
        if masks.len() >= ICON_CACHE_MAX {
            masks.clear();
        }
        masks.insert(key, bmp.clone());
        Some(bmp)
    }

    /// Draw a Lucide icon snapped to whole device pixels, so its strokes stay crisp.
    fn lucide(&self, name: &str, x: f32, y: f32, s: f32, c: Color) -> bool {
        let Some((idx, entry)) = icons::find(name) else { return false };
        let size = s * entry.scale;
        let k = self.dpi / 96.0;
        let px = (size * k).round().clamp(8.0, 128.0) as u32;
        let stroke100 = (icons::stroke24(size) * 100.0).round() as u32;
        let Some(mask) = self.icon_mask(idx, px, stroke100) else { return false };
        let d = px as f32 / k;
        // Centred in the s-by-s box, then onto whole device pixels.
        let (left, top) = (((x + (s - d) / 2.0) * k).round() / k, ((y + (s - d) / 2.0) * k).round() / k);
        let dest = D2D_RECT_F { left, top, right: left + d, bottom: top + d };
        let src = D2D_RECT_F { left: 0.0, top: 0.0, right: d, bottom: d };
        let t = self.t();
        unsafe {
            // Opacity masks only work unantialiased; the mask is already pixel-exact.
            t.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            t.FillOpacityMask(&mask, self.b(c), D2D1_OPACITY_MASK_CONTENT_GRAPHICS, Some(&dest), Some(&src));
            t.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        true
    }

    /// Line icons on a 16-unit grid, drawn at (x, y) with size `s` DIPs. Only for names the
    /// Lucide set lacks; see `icons::GEOMETRY`.
    fn geometry_icon(&self, name: &str, x: f32, y: f32, s: f32, c: Color) {
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
            "star" | "star-on" => {
                let pts: Vec<(f32, f32)> = (0..10)
                    .map(|n| {
                        let a = (-90.0 + n as f32 * 36.0).to_radians();
                        let r = if n % 2 == 0 { 6.0 } else { 2.6 };
                        p(8.0 + r * a.cos(), 8.4 + r * a.sin())
                    })
                    .collect();
                self.polyline(&pts, c, w, true);
                if name == "star-on" {
                    let (cx, cy) = p(8.0, 8.4);
                    self.circle(cx, cy, 2.8 * k, c);
                }
            }
            "clock" => {
                let (cx, cy) = p(8.0, 8.0);
                self.ring(cx, cy, 5.5 * k, 5.5 * k, c, w);
                self.polyline(&[p(8.0, 5.0), p(8.0, 8.0), p(10.2, 9.4)], c, w, false);
            }
            "download" => {
                l(p(8.0, 2.5), p(8.0, 11.0));
                self.polyline(&[p(4.5, 7.5), p(8.0, 11.0), p(11.5, 7.5)], c, w, false);
                l(p(3.0, 13.5), p(13.0, 13.5));
            }
            "pin" => {
                l(p(8.0, 10.0), p(8.0, 14.0));
                self.polyline(&[p(5.0, 10.0), p(11.0, 10.0), p(10.0, 7.0), p(10.0, 3.0), p(6.0, 3.0), p(6.0, 7.0)], c, w, true);
            }
            "play" => self.polyline(&[p(5.5, 3.5), p(12.5, 8.0), p(5.5, 12.5)], c, w, true),
            "pause" => {
                l(p(6.0, 3.5), p(6.0, 12.5));
                l(p(10.0, 3.5), p(10.0, 12.5));
            }
            "sidebar" => {
                // A window with its left strip marked: the sidebar's own picture.
                let (a, b) = (p(2.4, 3.2), p(13.6, 12.8));
                self.round_stroke(R::new(a.0, a.1, b.0 - a.0, b.1 - a.1), 2.0 * k, c, w);
                l(p(6.4, 3.4), p(6.4, 12.6));
            }
            "split" => {
                // One window cut in two, the left half filled: two pages, one in focus. Two
                // separate outlines read as the digits "00" at this size.
                let (a, b) = (p(2.4, 3.2), p(13.6, 12.8));
                let r = R::new(a.0, a.1, b.0 - a.0, b.1 - a.1);
                let half = R::new(r.x + w * 0.5, r.y + w * 0.5, (r.w / 2.0 - w * 0.5).max(1.0), r.h - w);
                self.round(half, 1.2 * k, c.alpha(0.4));
                self.round_stroke(r, 2.0 * k, c, w);
                l(p(8.0, 3.4), p(8.0, 12.6));
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

/// Most icon masks kept at once (a few dozen are ever in use; this only bounds a bug).
const ICON_CACHE_MAX: usize = 128;

const SYSTEM_UI: &str = "Segoe UI Variable Text";
/// When the bundle will not give a serif, Windows does.
const SERIF_FALLBACK: &str = "Georgia";
/// The monospace the figures are set in when the setting says "cascadia" or
/// names a font this PC does not have.
const MONO_FALLBACK: &str = "Cascadia Mono";

/// The bundled fonts' keywords, in the settings' words.
fn keyword(key: &str) -> Option<&'static str> {
    Some(match key {
        "instrument" => "Instrument",
        "figtree" => "Figtree",
        "atkinson" => "Atkinson",
        "fraunces" => "Fraunces",
        "newsreader" => "Newsreader",
        "segoe" => return None,
        "cascadia" => MONO_FALLBACK,
        _ => return None,
    })
}

/// Every family name Windows has installed.
fn system_families(dw: &IDWriteFactory5) -> Vec<String> {
    unsafe {
        let mut c: Option<IDWriteFontCollection1> = None;
        let _ = dw.GetSystemFontCollection(true, &mut c, false);
        let names = family_names(c.as_ref());
        if cfg!(debug_assertions) {
            let _ = std::fs::write(crate::store::data_dir().join("fonts.log"), names.join("\n"));
        }
        names
    }
}

/// Every family name in a collection, in its first listed language.
fn family_names(c: Option<&IDWriteFontCollection1>) -> Vec<String> {
    let Some(c) = c else { return Vec::new() };
    let mut out = Vec::new();
    unsafe {
        for i in 0..c.GetFontFamilyCount() {
            let Ok(fam) = c.GetFontFamily(i) else { continue };
            let Ok(names) = fam.GetFamilyNames() else { continue };
            let Ok(len) = names.GetStringLength(0) else { continue };
            let mut buf = vec![0u16; len as usize + 1];
            if names.GetString(0, &mut buf).is_ok() {
                out.push(String::from_utf16_lossy(&buf[..len as usize]));
            }
        }
    }
    if cfg!(debug_assertions) {
        let _ = std::fs::write(crate::store::data_dir().join("bundle-fonts.log"), out.join("\n"));
    }
    out
}

/// Unpack WOFF2 fonts into an in-memory collection DirectWrite can use.
fn load_woff2(dw: &IDWriteFactory5, files: &[&[u8]]) -> Option<IDWriteFontCollection1> {
    unsafe {
        let loader = dw.CreateInMemoryFontFileLoader().ok()?;
        dw.RegisterFontFileLoader(&loader).ok()?;
        let builder: IDWriteFontSetBuilder1 = dw.CreateFontSetBuilder().ok()?.cast().ok()?;
        for data in files {
            let Ok(stream) = dw.UnpackFontFile(DWRITE_CONTAINER_TYPE_WOFF2, data.as_ptr() as _, data.len() as u32) else { continue };
            let Ok(size) = stream.GetFileSize() else { continue };
            let mut ptr = std::ptr::null_mut();
            let mut ctx = std::ptr::null_mut();
            if stream.ReadFileFragment(&mut ptr, 0, size, &mut ctx).is_err() {
                continue;
            }
            let bytes = std::slice::from_raw_parts(ptr as *const u8, size as usize).to_vec();
            stream.ReleaseFileFragment(ctx);
            // Also for GDI, private to this process: the address bar is a Win32 edit box.
            let mut n = 0u32;
            let _ = windows::Win32::Graphics::Gdi::AddFontMemResourceEx(bytes.as_ptr() as _, bytes.len() as u32, None, &mut n);
            // No owner: DirectWrite copies the bytes.
            let Ok(file) = loader.CreateInMemoryFontFileReference(dw, bytes.as_ptr() as _, bytes.len() as u32, None) else { continue };
            let _ = builder.AddFontFile(&file);
        }
        let set = builder.CreateFontSet().ok()?;
        dw.CreateFontCollectionFromFontSet(&set).ok()
    }
}


/// Debug-only screenshots. The window's Direct2D target cannot be read back (PrintWindow
/// returns an empty frame), so the same drawing code runs once more into a WIC bitmap.
#[cfg(debug_assertions)]
impl Gfx {
    /// Swap a WIC bitmap target in for the window's. Draw as usual, then `end_shot`.
    pub fn begin_shot(&mut self, w: u32, h: u32) -> bool {
        if self.parked.is_some() {
            return false;
        }
        let Some(wic) = self.wic.clone() else { return false };
        unsafe {
            let Ok(bmp) = wic.CreateBitmap(w.max(1), h.max(1), &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand) else {
                return false;
            };
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 0.0,
                dpiY: 0.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let Ok(t) = self.d2d.CreateWicBitmapRenderTarget(&bmp, &props) else { return false };
            t.SetDpi(self.dpi, self.dpi);
            t.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = t.CreateSolidColorBrush(&Color(0.0, 0.0, 0.0, 1.0).d2d(), None).ok();
            self.parked = Some(Parked {
                target: self.target.replace(t),
                brush: std::mem::replace(&mut self.brush, brush),
                bitmaps: std::mem::take(&mut self.bitmaps),
                icon_masks: self.icon_masks.take(),
                shot: bmp,
            });
        }
        true
    }

    /// Put the window's target back and write the shot as a PNG. `crop` is x, y, w, h in pixels.
    pub fn end_shot(&mut self, path: &std::path::Path, crop: Option<(i32, i32, i32, i32)>) -> bool {
        let Some(p) = self.parked.take() else { return false };
        let ok = self.target.as_ref().is_some_and(|t| unsafe { t.EndDraw(None, None) }.is_ok());
        self.target = p.target;
        self.brush = p.brush;
        self.bitmaps = p.bitmaps;
        self.raster.take();
        *self.icon_masks.borrow_mut() = p.icon_masks;
        ok && self.write_png(&p.shot, path, crop).is_some()
    }

    fn write_png(&self, bmp: &IWICBitmap, path: &std::path::Path, crop: Option<(i32, i32, i32, i32)>) -> Option<()> {
        let wic = self.wic.as_ref()?;
        let name = HSTRING::from(path.to_string_lossy().as_ref());
        unsafe {
            let (mut w, mut h) = (0u32, 0u32);
            bmp.GetSize(&mut w, &mut h).ok()?;
            let rect = match crop {
                Some((x, y, cw, ch)) => {
                    let x = x.clamp(0, w as i32 - 1);
                    let y = y.clamp(0, h as i32 - 1);
                    WICRect { X: x, Y: y, Width: cw.clamp(1, w as i32 - x), Height: ch.clamp(1, h as i32 - y) }
                }
                None => WICRect { X: 0, Y: 0, Width: w as i32, Height: h as i32 },
            };
            let stream = wic.CreateStream().ok()?;
            stream.InitializeFromFilename(&name, windows::Win32::Foundation::GENERIC_WRITE.0).ok()?;
            let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).ok()?;
            enc.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
            let mut frame = None;
            enc.CreateNewFrame(&mut frame, std::ptr::null_mut()).ok()?;
            let frame = frame?;
            frame.Initialize(None).ok()?;
            frame.SetSize(rect.Width as u32, rect.Height as u32).ok()?;
            let mut fmt = GUID_WICPixelFormat24bppBGR;
            frame.SetPixelFormat(&mut fmt).ok()?;
            frame.WriteSource(bmp, &rect).ok()?;
            frame.Commit().ok()?;
            enc.Commit().ok()?;
        }
        Some(())
    }
}
