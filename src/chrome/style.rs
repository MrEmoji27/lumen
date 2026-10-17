//! The style every screen draws from. One `Look`, derived from the settings,
//! carries the theme, the type scale, the spacing, the radii, the sidebar's
//! metrics, the new tab page's knobs and the motion config. Drawing code reads
//! it and never invents a colour, a font size, a radius or a spacing step
//! itself: changing a setting changes what every page draws, live.

use super::{
    gfx::{Face, Font},
    layout::{Kind, Metrics},
    motion,
    theme::{Color, Custom, Theme},
};
use crate::store::Settings;

/// The tab row's close button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CloseBtn {
    Hover,
    Always,
    Off,
}

/// The date line under the clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DateFmt {
    Off,
    Long,
    Short,
    Iso,
}

/// What the new tab page's background is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NtpBg {
    Plain,
    Wallpaper,
}

/// Everything drawing reads. Cheap to build (once per state push) and cheap to
/// copy around inside a frame.
#[derive(Clone, Debug)]
pub struct Look {
    pub theme: Theme,
    /// The layout: where the chrome lives.
    pub kind: Kind,
    /// The bookmarks bar under the toolbar (horizontal layout).
    pub bm_bar: bool,
    /// The sidebar is folded to icons (Ctrl+B).
    pub compact: bool,

    // Spacing, scaled by the density setting.
    pub s1: f32,
    pub s2: f32,
    pub s3: f32,
    pub s4: f32,
    pub s5: f32,
    // Radii: small controls, rows and fields, cards.
    pub r_sm: f32,
    pub r_md: f32,
    pub r_lg: f32,
    /// The page's own corner radius (its own setting).
    pub page_radius: f32,
    /// A hairline border around the page.
    pub page_border: bool,

    // Type, scaled by the size setting.
    pub scale: f32,
    pub title_weight: u32,
    pub title: Font,
    pub h2: Font,
    pub big_num: Font,
    pub body: Font,
    pub body_strong: Font,
    pub small: Font,
    pub small_strong: Font,
    pub caption: Font,
    /// The monospace face, for figures.
    pub mono: Font,
    pub mono_strong: Font,

    // Sidebar.
    pub tab_row_h: f32,
    pub show_favicons: bool,
    pub close_btn: CloseBtn,
    pub show_memory: bool,
    pub footer_btns: bool,
    pub ws_row: bool,

    // Clock and new tab page.
    pub clock: bool,
    pub clock_24h: bool,
    pub clock_seconds: bool,
    pub date_fmt: DateFmt,
    pub clock_display_face: bool,
    /// Clock size in px; 0 follows the window.
    pub clock_size: u32,
    pub greeting: String,
    pub ntp_bg: NtpBg,
    pub wallpaper_blur: bool,
    pub wallpaper_dim: u32,
    pub dial_cols: u32,
    pub dial_tile: f32,
    pub ntp_search: bool,

    // Motion.
    pub motion: motion::Config,
}

impl Default for Look {
    fn default() -> Self {
        Look::of(&Settings::default(), true, true)
    }
}

impl Look {
    /// The settings, read as a style. `dark` is the resolved theme (the system's
    /// own choice included), `system_allows` Windows' "Show animations".
    pub fn of(s: &Settings, dark: bool, system_allows: bool) -> Look {
        let custom = Custom::from_hex(&s.bg_color, &s.side_color, &s.surface_color, &s.text_color, &s.dim_color);
        let theme = Theme::custom(&s.palette, dark, &s.accent, &custom);
        let d = match s.density.as_str() {
            "compact" => 0.8,
            "roomy" => 1.25,
            _ => 1.0,
        };
        let sp = |v: f32| (v * d).round();
        let scale = s.font_scale as f32 / 100.0;
        let f = |size: f32, weight: u32| Font::ui(size * scale, weight);
        let sf = |size: f32| Font::serif_w(size * scale, s.title_weight);
        let m = motion::Config::of(&s.motion, s.motion_speed, s.motion_system, system_allows);
        Look {
            theme,
            kind: Kind::of(&s.layout),
            bm_bar: s.bm_bar,
            compact: s.compact,
            s1: sp(4.0),
            s2: sp(8.0),
            s3: sp(12.0),
            s4: sp(16.0),
            s5: sp(24.0),
            r_sm: 6.0,
            r_md: 8.0,
            r_lg: 10.0,
            page_radius: s.radius as f32,
            page_border: s.page_border,
            scale,
            title_weight: s.title_weight,
            title: sf(30.0),
            h2: sf(22.0),
            big_num: sf(28.0),
            body: f(13.0, 450),
            body_strong: f(13.0, 550),
            small: f(12.0, 400),
            small_strong: f(12.0, 500),
            caption: f(11.0, 600),
            mono: Font { face: Face::Mono, ..f(12.5, 450) },
            mono_strong: Font { face: Face::Mono, ..f(13.0, 550) },
            tab_row_h: s.tab_row_h as f32,
            show_favicons: s.show_favicons,
            close_btn: match s.close_btn.as_str() {
                "always" => CloseBtn::Always,
                "off" => CloseBtn::Off,
                _ => CloseBtn::Hover,
            },
            show_memory: s.show_memory,
            footer_btns: s.footer_btns,
            ws_row: s.ws_row,
            clock: s.clock,
            clock_24h: s.clock_24h,
            clock_seconds: s.clock_seconds,
            date_fmt: match s.date_format.as_str() {
                "off" => DateFmt::Off,
                "short" => DateFmt::Short,
                "iso" => DateFmt::Iso,
                _ => DateFmt::Long,
            },
            clock_display_face: s.clock_font != "ui",
            clock_size: s.clock_size,
            greeting: s.greeting.clone(),
            ntp_bg: if s.ntp_bg == "wallpaper" { NtpBg::Wallpaper } else { NtpBg::Plain },
            wallpaper_blur: s.wallpaper_blur,
            wallpaper_dim: s.wallpaper_dim,
            dial_cols: s.dial_cols,
            dial_tile: s.dial_tile as f32,
            ntp_search: s.ntp_search,
            motion: m,
        }
    }

    /// The chrome's own heights for this look.
    pub fn metrics(&self) -> Metrics {
        Metrics::new(self.kind, self.compact, self.bm_bar)
    }

    /// A UI-face font at `size`, scaled.
    pub fn font(&self, size: f32, weight: u32) -> Font {
        Font::ui(size * self.scale, weight)
    }

    /// The display face at `size`, scaled and at the chosen title weight.
    pub fn serif(&self, size: f32) -> Font {
        Font::serif_w(size * self.scale, self.title_weight)
    }

    /// The clock's size: the setting, or a share of the stage.
    pub fn clock_px(&self, stage_w: f32) -> f32 {
        if self.clock_size > 0 {
            self.clock_size as f32
        } else {
            (stage_w * 0.09).clamp(64.0, 104.0) * self.scale
        }
    }
}

/// The content column of the list pages, and its least side margin. Layout, not
/// style: a page is a fixed column whatever the person picks.
pub const ROW_H: f32 = 40.0;
pub const COL_W: f32 = 760.0;
pub const COL_MARGIN: f32 = 56.0;

/// A named look: the colours, fonts and shape of the chrome, as JSON. What
/// "Save as a theme" keeps, and what Export writes and Import reads.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ThemeFile {
    pub name: String,
    pub palette: String,
    pub accent: String,
    pub bg_color: String,
    pub side_color: String,
    pub surface_color: String,
    pub text_color: String,
    pub dim_color: String,
    pub theme: String,
    pub ui_font: String,
    pub display_font: String,
    pub mono_font: String,
    pub font_scale: u32,
    pub title_weight: u32,
    pub density: String,
    pub radius: u32,
    pub gap: u32,
    pub page_border: bool,
}

impl Default for ThemeFile {
    fn default() -> Self {
        ThemeFile::of(&Settings::default())
    }
}

impl ThemeFile {
    /// What the settings say about the look.
    pub fn of(s: &Settings) -> ThemeFile {
        ThemeFile {
            name: String::new(),
            palette: s.palette.clone(),
            accent: s.accent.clone(),
            bg_color: s.bg_color.clone(),
            side_color: s.side_color.clone(),
            surface_color: s.surface_color.clone(),
            text_color: s.text_color.clone(),
            dim_color: s.dim_color.clone(),
            theme: s.theme.clone(),
            ui_font: s.ui_font.clone(),
            display_font: s.display_font.clone(),
            mono_font: s.mono_font.clone(),
            font_scale: s.font_scale,
            title_weight: s.title_weight,
            density: s.density.clone(),
            radius: s.radius,
            gap: s.gap,
            page_border: s.page_border,
        }
    }

    /// Lay it over the settings. Only the look: layout, tabs and everything else
    /// are the person's, not the theme's.
    pub fn apply(&self, s: &mut Settings) {
        s.palette = self.palette.clone();
        s.accent = self.accent.clone();
        s.bg_color = self.bg_color.clone();
        s.side_color = self.side_color.clone();
        s.surface_color = self.surface_color.clone();
        s.text_color = self.text_color.clone();
        s.dim_color = self.dim_color.clone();
        s.theme = self.theme.clone();
        s.ui_font = self.ui_font.clone();
        s.display_font = self.display_font.clone();
        s.mono_font = self.mono_font.clone();
        s.font_scale = self.font_scale;
        s.title_weight = self.title_weight;
        s.density = self.density.clone();
        s.radius = self.radius;
        s.gap = self.gap;
        s.page_border = self.page_border;
        s.clamp();
    }

    /// Pretty JSON, for Export.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// A look from an imported file. A file that is not one is refused.
    pub fn from_json(text: &str) -> Option<ThemeFile> {
        let t: ThemeFile = serde_json::from_str(text).ok()?;
        // A file with nothing recognizable in it is not a theme.
        (t != ThemeFile::default() || text.contains("palette")).then_some(t)
    }
}

/// Where the saved looks live.
fn themes_path() -> std::path::PathBuf {
    crate::store::data_dir().join("themes.json")
}

/// Every saved look, in order. An unreadable file reads as none.
pub fn saved_themes() -> Vec<ThemeFile> {
    #[derive(Default, serde::Serialize, serde::Deserialize)]
    #[serde(default)]
    struct File {
        themes: Vec<ThemeFile>,
    }
    std::fs::read_to_string(themes_path())
        .ok()
        .and_then(|s| serde_json::from_str::<File>(&s).ok())
        .map(|f| f.themes)
        .unwrap_or_default()
}

/// Write the saved looks, atomically like every other file.
pub fn save_themes(list: &[ThemeFile]) {
    #[derive(serde::Serialize)]
    struct File<'a> {
        themes: &'a [ThemeFile],
    }
    let path = themes_path();
    let tmp = path.with_extension("tmp");
    if let Ok(json) = serde_json::to_string_pretty(&File { themes: list }) {
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

/// The private-tab mark: a muted violet, never the accent.
pub fn private_mark(dark: bool) -> Color {
    super::theme::oklch(if dark { 0.74 } else { 0.52 }, 0.14, 300.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_look_is_the_old_constant_for_constant() {
        let lo = Look::default();
        assert_eq!(lo.theme.bg, Theme::new("graphite", true, "").bg);
        assert_eq!(lo.theme.side, lo.theme.bg);
        // The spacing and radii the pages were drawn with.
        assert_eq!((lo.s1, lo.s2, lo.s3, lo.s4, lo.s5), (4.0, 8.0, 12.0, 16.0, 24.0));
        assert_eq!((lo.r_sm, lo.r_md, lo.r_lg), (6.0, 8.0, 10.0));
        // The type scale, at 100%.
        assert_eq!(lo.body.size10, 130);
        assert_eq!(lo.body.weight, 450);
        assert_eq!(lo.small.size10, 120);
        assert_eq!(lo.caption.size10, 110);
        assert_eq!(lo.title.size10, 300);
        assert_eq!(lo.h2.size10, 220);
        assert_eq!(lo.big_num.size10, 280);
        assert_eq!(lo.mono.face, Face::Mono);
        assert_eq!(lo.title.weight, 400);
        // The old defaults.
        assert_eq!(lo.kind, Kind::Vertical);
        assert_eq!(lo.tab_row_h, 32.0);
        assert_eq!(lo.page_radius, 10.0);
        assert!(lo.show_favicons && lo.footer_btns && lo.ws_row);
        assert_eq!(lo.close_btn, CloseBtn::Hover);
        assert!(lo.clock && lo.clock_24h && !lo.clock_seconds);
        assert_eq!(lo.date_fmt, DateFmt::Long);
        assert!(lo.ntp_search);
        assert_eq!(lo.dial_tile, 60.0);
        assert!(lo.motion.enabled);
    }

    #[test]
    fn a_settings_file_drives_the_whole_look() {
        let mut s = Settings::default();
        s.layout = "hybrid".into();
        s.bm_bar = true;
        s.density = "roomy".into();
        s.font_scale = 130;
        s.title_weight = 700;
        s.ui_font = "Georgia".into();
        s.bg_color = "#101418".into();
        s.side_color = "#1c2024".into();
        s.surface_color = "#24282c".into();
        s.text_color = "#f4f4f4".into();
        s.dim_color = "#9aa0a6".into();
        s.compact = true;
        s.tab_row_h = 40;
        s.show_favicons = false;
        s.close_btn = "always".into();
        s.show_memory = false;
        s.footer_btns = false;
        s.ws_row = false;
        s.clock = false;
        s.clock_24h = false;
        s.clock_seconds = true;
        s.date_format = "iso".into();
        s.clock_font = "ui".into();
        s.clock_size = 96;
        s.greeting = "Hello".into();
        s.ntp_bg = "wallpaper".into();
        s.wallpaper_blur = true;
        s.wallpaper_dim = 50;
        s.dial_cols = 6;
        s.dial_tile = 80;
        s.ntp_search = false;
        s.motion = "reduced".into();
        let lo = Look::of(&s, true, true);
        assert_eq!(lo.kind, Kind::Hybrid);
        assert!(lo.bm_bar);
        // The density moved the spacing, not the type.
        assert_eq!(lo.s3, 15.0);
        assert_eq!(lo.s5, 30.0);
        // The type scale did move the type.
        assert_eq!(lo.body.size10, 169);
        assert_eq!(lo.title.size10, 390);
        assert_eq!(lo.title.weight, 700);
        assert_eq!(lo.theme.side, Color::from_hex("#1c2024").unwrap());
        assert_eq!(lo.theme.bg, Color::from_hex("#101418").unwrap());
        assert_eq!(lo.theme.raise, Color::from_hex("#24282c").unwrap());
        assert_eq!(lo.theme.dim, Color::from_hex("#9aa0a6").unwrap());
        assert_eq!(lo.tab_row_h, 40.0);
        assert!(!lo.show_favicons && !lo.footer_btns && !lo.ws_row && !lo.show_memory);
        assert_eq!(lo.close_btn, CloseBtn::Always);
        assert!(!lo.clock && !lo.clock_24h && lo.clock_seconds);
        assert_eq!(lo.date_fmt, DateFmt::Iso);
        assert!(!lo.clock_display_face);
        assert_eq!(lo.clock_px(1000.0), 96.0);
        assert_eq!(lo.greeting, "Hello");
        assert_eq!(lo.ntp_bg, NtpBg::Wallpaper);
        assert!(lo.wallpaper_blur);
        assert_eq!(lo.wallpaper_dim, 50);
        assert_eq!(lo.dial_cols, 6);
        assert_eq!(lo.dial_tile, 80.0);
        assert!(!lo.ntp_search);
        assert!(!lo.motion.enabled, "reduced motion must not move");
        // The clock's automatic size follows the window, and the type scale.
        assert_eq!(lo.clock_px(1000.0), 96.0);
        let mut s2 = s.clone();
        s2.clock_size = 0;
        s2.font_scale = 100;
        let lo = Look::of(&s2, true, true);
        assert_eq!(lo.clock_px(1000.0), 90.0);
    }

    #[test]
    fn density_only_moves_spacing() {
        let mut s = Settings::default();
        s.density = "compact".into();
        let compact = Look::of(&s, true, true);
        s.density = "roomy".into();
        let roomy = Look::of(&s, true, true);
        assert!(compact.s3 < roomy.s3);
        assert_eq!(compact.body.size10, roomy.body.size10);
        assert_eq!(compact.r_md, roomy.r_md);
    }

    #[test]
    fn the_metrics_follow_the_layout() {
        let mut s = Settings::default();
        assert_eq!(Look::of(&s, true, true).metrics().top(), 0.0);
        s.layout = "horizontal".into();
        assert_eq!(Look::of(&s, true, true).metrics().top(), 78.0);
        s.compact = true;
        assert_eq!(Look::of(&s, true, true).metrics().top(), 66.0);
        s.bm_bar = true;
        assert_eq!(Look::of(&s, true, true).metrics().top(), 96.0);
        s.layout = "hybrid".into();
        assert_eq!(Look::of(&s, true, true).metrics().top(), 34.0);
        // The bookmarks bar is a horizontal-layout thing.
        assert_eq!(Look::of(&s, true, false).metrics().bm_h, 0.0);
    }
}
