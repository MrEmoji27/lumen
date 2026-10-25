//! Colors.

use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub f32, pub f32, pub f32, pub f32);

impl Color {
    pub fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.0, g: self.1, b: self.2, a: self.3 }
    }
    pub fn alpha(self, a: f32) -> Color {
        Color(self.0, self.1, self.2, self.3 * a)
    }
    pub fn mix(self, o: Color, t: f32) -> Color {
        let m = |a: f32, b: f32| a + (b - a) * t;
        Color(m(self.0, o.0), m(self.1, o.1), m(self.2, o.2), m(self.3, o.3))
    }
    pub fn colorref(self) -> u32 {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        c(self.0) | (c(self.1) << 8) | (c(self.2) << 16)
    }
    pub fn rgb8(self) -> (u8, u8, u8) {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        (c(self.0), c(self.1), c(self.2))
    }
    pub fn hex(self) -> String {
        let (r, g, b) = self.rgb8();
        format!("#{r:02x}{g:02x}{b:02x}")
    }
    pub fn from_hex(s: &str) -> Option<Color> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        let f = |x: u32| (x & 0xff) as f32 / 255.0;
        Some(Color(f(v >> 16), f(v >> 8), f(v), 1.0))
    }
}

pub fn oklch(l: f32, c: f32, h: f32) -> Color {
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let bl = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    let gamma = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
    };
    Color(gamma(r), gamma(g), gamma(bl), 1.0)
}

fn palette(name: &str) -> (f32, f32, f32, f32, f32) {
    match name {
        "sand" => (70.0, 0.012, 0.72, 0.13, 48.0),
        "slate" => (245.0, 0.016, 0.75, 0.10, 250.0),
        "moss" => (150.0, 0.012, 0.76, 0.10, 145.0),
        "rose" => (10.0, 0.012, 0.73, 0.12, 15.0),
        _ => (260.0, 0.005, 0.78, 0.12, 68.0),
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Custom {
    pub bg: Option<Color>,
    pub side: Option<Color>,
    pub surface: Option<Color>,
    pub text: Option<Color>,
    pub dim: Option<Color>,
}

impl Custom {
    pub fn is_empty(&self) -> bool {
        self.bg.is_none() && self.side.is_none() && self.surface.is_none() && self.text.is_none() && self.dim.is_none()
    }
    pub fn from_hex(bg: &str, side: &str, surface: &str, text: &str, dim: &str) -> Custom {
        let c = |s: &str| Color::from_hex(s);
        Custom { bg: c(bg), side: c(side), surface: c(surface), text: c(text), dim: c(dim) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Style {
    #[default]
    Signal,
    Glass,
    Precise,
}

impl Style {
    pub fn of(name: &str) -> Style {
        match name {
            "glass" => Style::Glass,
            "precise" => Style::Precise,
            _ => Style::Signal,
        }
    }
    pub fn dark(self, wanted: bool) -> bool {
        self != Style::Signal || wanted
    }
}

fn hx(s: &str) -> Color {
    Color::from_hex(s).unwrap_or(Color(1.0, 0.0, 1.0, 1.0))
}

fn wh(a: f32) -> Color {
    Color(1.0, 1.0, 1.0, a)
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub dark: bool,
    pub pill: Color,
    pub pill_ink: Color,
    pub side: Color,
    pub bg: Color,
    pub raise: Color,
    pub hover: Color,
    pub press: Color,
    pub line: Color,
    pub text: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    pub acc_soft: Color,
    pub shade: Color,
    pub danger: Color,
}

impl Theme {
    pub fn new(palette_name: &str, dark: bool, accent_hex: &str) -> Theme {
        Theme::custom(palette_name, dark, accent_hex, &Custom::default())
    }

    pub fn styled(style: Style, dark: bool, accent_hex: &str, c: &Custom) -> Theme {
        let dark = style.dark(dark);
        let t = match (style, dark) {
            (Style::Signal, false) => (hx("#f1efe9"), hx("#ffffff"), hx("#e7e4dc"), hx("#dedad0"), hx("#dedad0"), hx("#141414"), hx("#3a3936"), hx("#8b877d"), hx("#e8322b"), hx("#141414"), hx("#ffffff")),
            (Style::Signal, true) => (hx("#121211"), hx("#1c1c1a"), hx("#23221f"), hx("#2b2a27"), hx("#2b2a27"), hx("#f2efe8"), hx("#cfcbc2"), hx("#8e8a80"), hx("#ff4a3d"), hx("#f2efe8"), hx("#121211")),
            (Style::Glass, _) => (hx("#15121f"), wh(0.08), wh(0.09), wh(0.14), wh(0.12), hx("#f4f2ff"), wh(0.82), wh(0.56), hx("#ff7ab8"), wh(0.18), hx("#ffffff")),
            (Style::Precise, _) => (hx("#0b0b0d"), hx("#111114"), hx("#16161a"), hx("#1f1f24"), hx("#1f1f24"), hx("#ececef"), hx("#b9b9c0"), hx("#6d6d75"), hx("#8b7bff"), hx("#17171b"), hx("#ffffff")),
        };
        let (bg, raise, hover, press, line, text, dim, faint, accent, pill, pill_ink) = t;
        let accent = Color::from_hex(accent_hex).unwrap_or(accent);
        let mut th = Theme {
            dark,
            pill,
            pill_ink,
            side: bg,
            bg,
            raise,
            hover,
            press,
            line,
            text,
            dim,
            faint,
            accent,
            acc_soft: bg.mix(accent, 0.22),
            shade: Color(0.0, 0.0, 0.0, if dark { 0.5 } else { 0.10 }),
            danger: if dark { hx("#ff6b5e") } else { hx("#c4241c") },
        };
        if c.is_empty() {
            return th;
        }
        th.bg = c.bg.unwrap_or(th.bg);
        th.side = c.side.or(c.bg).unwrap_or(th.bg);
        let toward = if dark { Color(1.0, 1.0, 1.0, 1.0) } else { Color(0.0, 0.0, 0.0, 1.0) };
        if let Some(s) = c.surface {
            th.raise = s;
            th.hover = s.mix(toward, 0.05);
            th.press = s.mix(toward, 0.11);
            th.line = s.mix(th.text, 0.18);
        }
        if let Some(t) = c.text {
            th.text = t;
        }
        if c.bg.is_some() || c.text.is_some() {
            th.dim = th.text.mix(th.bg, 0.31);
            th.faint = th.text.mix(th.bg, 0.55);
        }
        th.dim = c.dim.unwrap_or(th.dim);
        th
    }

    pub fn solid(&self, c: Color) -> Color {
        let o = self.bg.mix(c, c.3);
        Color(o.0, o.1, o.2, 1.0)
    }

    pub fn custom(palette_name: &str, dark: bool, accent_hex: &str, c: &Custom) -> Theme {
        let (h, chroma, al, ac, ah) = palette(palette_name);
        let k = |l: f32, cm: f32| oklch(l, chroma * cm, h);
        let accent = Color::from_hex(accent_hex).unwrap_or_else(|| oklch(al, ac, ah));
        let (bg, raise, hover, press, line, text, dim, faint) = if dark {
            (k(0.185, 1.0), k(0.225, 1.2), k(0.245, 1.2), k(0.28, 1.3), k(0.27, 1.2), k(0.94, 0.5), k(0.72, 0.8), k(0.52, 1.0))
        } else {
            (k(0.955, 1.0), k(0.988, 0.5), k(0.918, 1.0), k(0.89, 1.0), k(0.9, 1.0), k(0.22, 1.0), k(0.46, 1.0), k(0.62, 1.0))
        };
        if c.is_empty() {
            return Theme {
                dark,
                pill: text,
                pill_ink: bg,
                side: bg,
                bg,
                raise,
                hover,
                press,
                line,
                text,
                dim,
                faint,
                accent,
                acc_soft: bg.mix(accent, 0.35),
                shade: Color(0.0, 0.0, 0.0, if dark { 0.45 } else { 0.14 }),
                danger: oklch(0.58, 0.2, 27.0),
            };
        }
        let bg = c.bg.unwrap_or(bg);
        let side = c.side.or(c.bg).unwrap_or(bg);
        let raise = c.surface.unwrap_or(raise);
        let text = c.text.unwrap_or(text);
        let toward = if dark { Color(1.0, 1.0, 1.0, 1.0) } else { Color(0.0, 0.0, 0.0, 1.0) };
        let (hover, press, line) = if c.surface.is_some() {
            (raise.mix(toward, 0.05), raise.mix(toward, 0.11), raise.mix(text, 0.18))
        } else {
            (hover, press, line)
        };
        let (dim, faint) = if c.bg.is_some() || c.text.is_some() {
            (text.mix(bg, 0.31), text.mix(bg, 0.55))
        } else {
            (dim, faint)
        };
        let dim = c.dim.unwrap_or(dim);
        Theme {
            dark,
            pill: text,
            pill_ink: bg,
            side,
            bg,
            raise,
            hover,
            press,
            line,
            text,
            dim,
            faint,
            accent,
            acc_soft: bg.mix(accent, 0.35),
            shade: Color(0.0, 0.0, 0.0, if dark { 0.45 } else { 0.14 }),
            danger: oklch(0.58, 0.2, 27.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_look_is_its_reference_colours() {
        let s = Theme::styled(Style::Signal, false, "", &Custom::default());
        assert_eq!((s.bg.hex(), s.raise.hex(), s.text.hex(), s.accent.hex()), ("#f1efe9".into(), "#ffffff".into(), "#141414".into(), "#e8322b".into()));
        assert_eq!(s.pill.hex(), "#141414");
        let d = Theme::styled(Style::Signal, true, "", &Custom::default());
        assert_eq!((d.bg.hex(), d.accent.hex(), d.pill.hex()), ("#121211".into(), "#ff4a3d".into(), "#f2efe8".into()));
        assert!(Theme::styled(Style::Glass, false, "", &Custom::default()).dark);
        assert_eq!(Theme::styled(Style::Precise, false, "", &Custom::default()).accent.hex(), "#8b7bff");
        assert_eq!(Theme::styled(Style::Signal, false, "#2f6bff", &Custom::default()).accent.hex(), "#2f6bff");
        assert_eq!(Style::of("nonsense"), Style::Signal);
    }

    #[test]
    fn no_overrides_is_exactly_the_palette() {
        for dark in [true, false] {
            let a = Theme::new("slate", dark, "#3366ff");
            let b = Theme::custom("slate", dark, "#3366ff", &Custom::default());
            assert_eq!(a.side, b.side);
            assert_eq!(a.bg, b.bg);
            assert_eq!(a.hover, b.hover);
            assert_eq!(a.dim, b.dim);
        }
    }

    #[test]
    fn overrides_reach_the_theme_and_its_steps() {
        let c = Custom::from_hex("#102030", "#203040", "#304050", "#f0f0f0", "");
        let t = Theme::custom("graphite", true, "", &c);
        assert_eq!(t.bg, Color::from_hex("#102030").unwrap());
        assert_eq!(t.side, Color::from_hex("#203040").unwrap());
        assert_eq!(t.raise, Color::from_hex("#304050").unwrap());
        assert_eq!(t.text, Color::from_hex("#f0f0f0").unwrap());
        assert_ne!(t.hover, Theme::new("graphite", true, "").hover);
        let c = Custom::from_hex("", "#203040", "", "", "");
        let t = Theme::custom("graphite", true, "", &c);
        assert_eq!(t.side, Color::from_hex("#203040").unwrap());
        assert_eq!(t.bg, Theme::new("graphite", true, "").bg);
        let c = Custom::from_hex("#101418", "", "", "", "");
        let t = Theme::custom("graphite", true, "", &c);
        assert_eq!(t.side, t.bg);
        assert_ne!(t.faint, Theme::new("graphite", true, "").faint);
        let c = Custom::from_hex("#101418", "", "", "", "#888888");
        let t = Theme::custom("graphite", true, "", &c);
        assert_eq!(t.dim, Color::from_hex("#888888").unwrap());
    }
}
