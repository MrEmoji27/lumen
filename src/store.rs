//! Settings and session, saved as JSON under %APPDATA%\Lumen.

use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

pub fn data_dir() -> PathBuf {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Lumen");
    let _ = fs::create_dir_all(&dir);
    dir
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Shortcut {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Settings format. Old files lack it and read as 0, which triggers the migration.
    #[serde(default)]
    pub version: u32,
    /// Named palette from the UI ("graphite", "sand", "slate", "moss", "rose").
    pub palette: String,
    /// Accent color override. Empty means palette default.
    pub accent: String,
    /// "solid" | "mica" | "acrylic". Solid is the fastest.
    pub backdrop: String,
    /// "dark" | "light" | "system". Also drives prefers-color-scheme in pages.
    pub theme: String,
    /// "left" | "right"
    pub sidebar_side: String,
    pub sidebar_width: u32,
    pub compact: bool,
    /// Space around the page, in px.
    pub gap: u32,
    /// Corner radius of the page, in px. 0 = square.
    pub radius: u32,
    pub animations: bool,
    /// Search URL, `%s` is replaced by the query.
    pub search: String,
    /// Wallpaper image URL for the new tab page. Empty = none.
    pub wallpaper: String,
    /// Minutes a hidden tab waits before it is suspended (frozen, memory trimmed).
    pub sleep_after_min: u32,
    /// Minutes a hidden tab waits before it is discarded (process freed, reload on click).
    pub discard_after_min: u32,
    /// "strict" | "balanced" | "basic" | "off"
    pub tracking: String,
    /// Block ads and trackers at the network layer.
    pub adblock: bool,
    pub show_memory: bool,
    pub shortcuts: Vec<Shortcut>,
}

impl Default for Settings {
    fn default() -> Self {
        let s = |t: &str, u: &str| Shortcut { title: t.into(), url: u.into() };
        Self {
            version: SETTINGS_VERSION,
            palette: "graphite".into(),
            accent: String::new(),
            backdrop: "solid".into(),
            theme: "dark".into(),
            sidebar_side: "left".into(),
            sidebar_width: 240,
            compact: false,
            gap: 8,
            radius: 10,
            animations: true,
            search: "https://duckduckgo.com/?q=%s".into(),
            wallpaper: String::new(),
            sleep_after_min: 5,
            discard_after_min: 30,
            tracking: "balanced".into(),
            adblock: true,
            show_memory: true,
            shortcuts: vec![
                s("YouTube", "https://www.youtube.com"),
                s("GitHub", "https://github.com"),
                s("Reddit", "https://www.reddit.com"),
                s("Gmail", "https://mail.google.com"),
                s("Wikipedia", "https://en.wikipedia.org"),
            ],
        }
    }
}

const SETTINGS_VERSION: u32 = 2;

impl Settings {
    /// Version 2 is the minimal redesign. Old look values were defaults, not choices.
    fn migrate(&mut self) {
        if self.version < 2 {
            let d = Settings::default();
            self.palette = d.palette;
            self.backdrop = d.backdrop;
            self.sidebar_width = d.sidebar_width;
            self.accent.clear();
        }
        self.version = SETTINGS_VERSION;
    }

    /// Keep values inside sane bounds. A bad file must not break the browser.
    pub fn clamp(&mut self) {
        self.sidebar_width = self.sidebar_width.clamp(180, 400);
        self.gap = self.gap.clamp(0, 32);
        self.radius = self.radius.clamp(0, 20);
        self.sleep_after_min = self.sleep_after_min.clamp(1, 240);
        self.discard_after_min = self.discard_after_min.clamp(self.sleep_after_min, 1440);
        if !self.search.contains("%s") {
            self.search = Settings::default().search;
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTab {
    pub url: String,
    pub title: String,
    pub favicon: String,
    pub pinned: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

fn load<T: Default + for<'de> Deserialize<'de>>(name: &str) -> T {
    fs::read_to_string(data_dir().join(name))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save<T: Serialize>(name: &str, value: &T) {
    let path = data_dir().join(name);
    let tmp = path.with_extension("tmp");
    if let Ok(json) = serde_json::to_string_pretty(value) {
        // Write then rename, so a crash never leaves half a file.
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

pub fn load_settings() -> Settings {
    let mut s: Settings = load("settings.json");
    s.migrate();
    s.clamp();
    s
}

pub fn save_settings(s: &Settings) {
    save("settings.json", s);
}

pub fn load_session() -> Session {
    load("session.json")
}

pub fn save_session(s: &Session) {
    save("session.json", s);
}
