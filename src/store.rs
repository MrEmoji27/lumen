//! Settings and session, saved as JSON under %APPDATA%\Lumen.

use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

pub fn data_dir() -> PathBuf {
    let dir = std::env::var_os("LUMEN_DATA").map(PathBuf::from).unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Lumen")
    });
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn download_dir() -> PathBuf {
    let dir = std::env::var_os("LUMEN_DOWNLOADS")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::download_dir().unwrap_or_else(data_dir));
    let _ = fs::create_dir_all(&dir);
    dir
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shortcut {
    pub title: String,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(default)]
    pub version: u32,
    #[serde(default = "yes")]
    pub first_run_done: bool,
    #[serde(default)]
    pub seen_version: String,
    pub palette: String,
    pub accent: String,
    pub recent_colors: Vec<String>,
    pub backdrop: String,
    pub theme: String,
    pub look: String,
    pub layout: String,
    pub bm_bar: bool,
    pub bg_color: String,
    pub side_color: String,
    pub surface_color: String,
    pub text_color: String,
    pub dim_color: String,
    pub sidebar_side: String,
    pub sidebar_width: u32,
    pub compact: bool,
    pub hide_sidebar: bool,
    pub tab_row_h: u32,
    pub show_favicons: bool,
    pub close_btn: String,
    pub footer_btns: bool,
    pub ws_row: bool,
    pub gap: u32,
    pub radius: u32,
    pub density: String,
    pub page_border: bool,
    pub animations: bool,
    #[serde(default = "missing")]
    pub motion: String,
    pub motion_speed: u32,
    pub motion_system: bool,
    pub search: String,
    pub wallpaper: String,
    pub sleep_after_min: u32,
    pub discard_after_min: u32,
    pub tracking: String,
    pub adblock: bool,
    pub sponsorblock: bool,
    pub userscripts: bool,
    #[serde(default = "yes")]
    pub battery_saver: bool,
    pub lock_app: bool,
    pub show_credit: bool,
    pub cosmetic: bool,
    pub privacy_stats: bool,
    #[serde(default = "yes")]
    pub block_popups: bool,
    #[serde(default)]
    pub popup_sites: Vec<String>,
    #[serde(default = "yes")]
    pub block_redirects: bool,
    #[serde(default)]
    pub redirect_sites: Vec<String>,
    #[serde(default)]
    pub private_tabs: bool,
    pub page_chip: bool,
    pub show_memory: bool,
    pub shortcuts: Vec<Shortcut>,
    pub speed_dial: bool,
    pub gpu_in_process: bool,
    pub share_processes: bool,
    #[serde(default = "yes")]
    pub trim_watched: bool,
    pub archive_after: String,
    pub save_passwords: bool,
    pub accelerate_downloads: bool,
    pub download_limit_kbps: u32,
    pub keep_seeding: bool,
    pub bt_port: u16,
    pub upnp: bool,
    pub torrent_note: bool,
    pub remember_text: bool,
    pub clear_on_exit: bool,
    pub dark_pages: bool,
    pub dark_skip: Vec<String>,
    pub translate_to: String,
    pub servo_sites: Vec<String>,
    pub ui_font: String,
    pub display_font: String,
    pub mono_font: String,
    pub font_scale: u32,
    pub title_weight: u32,
    pub clock: bool,
    pub clock_24h: bool,
    pub clock_seconds: bool,
    pub date_format: String,
    pub clock_font: String,
    pub clock_size: u32,
    pub greeting: String,
    pub ntp_bg: String,
    pub wallpaper_blur: bool,
    pub wallpaper_dim: u32,
    pub dial_cols: u32,
    pub dial_tile: u32,
    pub ntp_search: bool,
    pub agent: bool,
    pub agent_port: u32,
    pub agent_shared_profile: bool,
    pub agent_strict_isolation: bool,
    pub agent_upload: bool,
    pub agent_upload_dir: String,
    pub drive_cmd: String,
    pub journal: bool,
    pub ram_limit: bool,
    pub ram_limit_mb: u32,
    pub cpu_limit: bool,
    pub cpu_limit_pct: u32,
    pub net_limit: bool,
    pub net_limit_kbps: u32,
    pub perm_defaults: std::collections::BTreeMap<String, String>,
    pub default_zoom: u32,
    pub https_first: bool,
    pub http_ok: Vec<String>,
    pub adblock_off_sites: Vec<String>,
    pub gpc: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let s = |t: &str, u: &str| Shortcut { title: t.into(), url: u.into() };
        Self {
            version: SETTINGS_VERSION,
            first_run_done: false,
            seen_version: env!("LUMEN_VERSION").into(),
            palette: "graphite".into(),
            accent: String::new(),
            recent_colors: Vec::new(),
            backdrop: "solid".into(),
            theme: "dark".into(),
            look: "signal".into(),
            layout: "vertical".into(),
            bm_bar: false,
            bg_color: String::new(),
            side_color: String::new(),
            surface_color: String::new(),
            text_color: String::new(),
            dim_color: String::new(),
            sidebar_side: "left".into(),
            sidebar_width: 240,
            compact: false,
            hide_sidebar: false,
            tab_row_h: 32,
            show_favicons: true,
            close_btn: "hover".into(),
            footer_btns: true,
            ws_row: true,
            gap: 8,
            radius: 10,
            density: "comfortable".into(),
            page_border: false,
            animations: true,
            motion: "full".into(),
            motion_speed: 100,
            motion_system: true,
            search: "https://duckduckgo.com/?q=%s".into(),
            wallpaper: String::new(),
            sleep_after_min: 5,
            discard_after_min: 30,
            tracking: "balanced".into(),
            adblock: true,
            sponsorblock: true,
            userscripts: true,
            battery_saver: true,
            lock_app: false,
            show_credit: true,
            show_memory: true,
            gpu_in_process: false,
            share_processes: true,
            trim_watched: true,
            archive_after: crate::archive::DEFAULT.into(),
            save_passwords: true,
            accelerate_downloads: true,
            download_limit_kbps: 0,
            keep_seeding: true,
            bt_port: 0,
            upnp: true,
            torrent_note: true,
            cosmetic: true,
            privacy_stats: true,
            block_popups: true,
            popup_sites: Vec::new(),
            block_redirects: true,
            redirect_sites: Vec::new(),
            private_tabs: false,
            page_chip: true,
            dark_pages: false,
            remember_text: true,
            clear_on_exit: false,
            dark_skip: Vec::new(),
            translate_to: "en".into(),
            servo_sites: Vec::new(),
            ui_font: "segoe".into(),
            display_font: "instrument".into(),
            mono_font: "cascadia".into(),
            font_scale: 100,
            title_weight: 400,
            clock: true,
            clock_24h: true,
            clock_seconds: false,
            date_format: "long".into(),
            clock_font: "display".into(),
            clock_size: 0,
            greeting: String::new(),
            ntp_bg: "plain".into(),
            wallpaper_blur: false,
            wallpaper_dim: 25,
            dial_cols: 0,
            dial_tile: 60,
            ntp_search: true,
            agent: false,
            agent_port: 47615,
            agent_shared_profile: false,
            agent_strict_isolation: false,
            agent_upload: false,
            agent_upload_dir: String::new(),
            drive_cmd: DEFAULT_DRIVE_CMD.into(),
            journal: true,
            ram_limit: false,
            ram_limit_mb: 2048,
            cpu_limit: false,
            cpu_limit_pct: 50,
            net_limit: false,
            net_limit_kbps: 2048,
            perm_defaults: Default::default(),
            default_zoom: 100,
            https_first: true,
            http_ok: Vec::new(),
            adblock_off_sites: Vec::new(),
            gpc: true,
            speed_dial: true,
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

const SETTINGS_VERSION: u32 = 4;
pub(crate) const BT_PORT_MIN: u16 = 20000;
pub(crate) const BT_PORT_MAX: u16 = 60000;

pub const DEFAULT_DRIVE_CMD: &str = "opencode run --model opencode/mimo-v2.6-flash-free \"{task}\"";
pub(crate) fn drive_cmd_usable(cmd: &str) -> bool {
    let c = cmd.trim();
    !c.is_empty() && c.contains("{task}") && c.len() <= 600 && !c.chars().any(char::is_control)
}

pub fn drive_prompt(task: &str) -> String {
    let task: String = task
        .chars()
        .map(|c| if c == '"' { '\'' } else if c.is_control() { ' ' } else { c })
        .take(2000)
        .collect();
    format!(
        "{}\n\nYou are working in the person's Lumen browser: the browser_* tools are MCP calls into it. \
Work only in the tabs you open. Before you pay, send a message, delete anything, or log in, stop and call \
browser_handoff with one line for the person, and wait for them to hand the tab back. Never do those alone. \
When the task is done, say what you did in one line.",
        task.trim()
    )
}

pub fn drive_command(template: &str, task: &str) -> Option<(String, Vec<String>)> {
    let line = template.replace("{task}", &drive_prompt(task));
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in line.trim().chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    let mut it = tokens.into_iter();
    let program = it.next()?;
    Some((program, it.collect()))
}

pub const LAYOUTS: [(&str, &str); 3] = [("vertical", "Vertical"), ("horizontal", "Horizontal"), ("hybrid", "Hybrid")];
pub const MOTIONS: [(&str, &str); 3] = [("full", "Full"), ("reduced", "Reduced"), ("off", "Off")];
pub const DENSITIES: [(&str, &str); 3] = [("compact", "Compact"), ("comfortable", "Comfortable"), ("roomy", "Roomy")];
pub const CLOSE_BTNS: [(&str, &str); 3] = [("hover", "On hover"), ("always", "Always"), ("off", "Hidden")];
pub const DATE_FORMATS: [(&str, &str); 4] = [("long", "Thursday, 17 October"), ("short", "17 Oct"), ("iso", "2026-10-17"), ("off", "Hidden")];
pub const LOOKS: [(&str, &str); 3] = [("signal", "Signal"), ("glass", "Glass"), ("precise", "Precise")];

fn missing() -> String {
    String::new()
}

fn yes() -> bool {
    true
}

fn one_of(v: &mut String, keys: &[(&str, &str)], default: &str) {
    if !keys.iter().any(|(k, _)| *k == v.as_str()) {
        *v = default.to_string();
    }
}

fn clean_color(v: &mut String) {
    let s = v.trim().to_ascii_lowercase();
    let ok = s.is_empty()
        || (s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit()));
    *v = if ok { s } else { String::new() };
}

fn clean_font(v: &mut String, default: &str) {
    *v = v.trim().chars().take(64).collect();
    if v.is_empty() {
        *v = default.to_string();
    }
}

impl Settings {
    fn migrate(&mut self) {
        if self.version < 2 {
            let d = Settings::default();
            self.palette = d.palette;
            self.backdrop = d.backdrop;
            self.sidebar_width = d.sidebar_width;
            self.accent.clear();
        }
        if self.version < 4 {
            self.gpu_in_process = false;
        }
        self.version = SETTINGS_VERSION;
    }

    pub fn clamp(&mut self) {
        if self.motion.is_empty() {
            self.motion = if self.animations { "full" } else { "off" }.into();
        }
        one_of(&mut self.layout, &LAYOUTS, "vertical");
        one_of(&mut self.look, &LOOKS, "signal");
        one_of(&mut self.motion, &MOTIONS, "full");
        one_of(&mut self.density, &DENSITIES, "comfortable");
        one_of(&mut self.close_btn, &CLOSE_BTNS, "hover");
        one_of(&mut self.date_format, &DATE_FORMATS, "long");
        one_of(&mut self.archive_after, &crate::archive::CHOICES, crate::archive::DEFAULT);
        for c in [&mut self.bg_color, &mut self.side_color, &mut self.surface_color, &mut self.text_color, &mut self.dim_color] {
            clean_color(c);
        }
        self.recent_colors.truncate(6);
        clean_font(&mut self.ui_font, "segoe");
        clean_font(&mut self.display_font, "instrument");
        clean_font(&mut self.mono_font, "cascadia");
        self.font_scale = self.font_scale.clamp(90, 130);
        self.title_weight = self.title_weight.clamp(300, 800);
        self.motion_speed = self.motion_speed.clamp(50, 200);
        self.clock_size = if self.clock_size == 0 { 0 } else { self.clock_size.clamp(36, 200) };
        self.wallpaper_dim = self.wallpaper_dim.min(80);
        self.wallpaper = self.wallpaper.trim().to_string();
        self.greeting = self.greeting.trim().chars().take(60).collect();
        self.ntp_bg = if self.ntp_bg.trim().eq_ignore_ascii_case("wallpaper") { "wallpaper".into() } else { "plain".into() };
        self.dial_cols = if self.dial_cols == 0 { 0 } else { self.dial_cols.clamp(3, 8) };
        self.dial_tile = self.dial_tile.clamp(44, 96);
        self.tab_row_h = self.tab_row_h.clamp(28, 48);
        self.sidebar_width = self.sidebar_width.clamp(232, 400);
        self.gap = self.gap.clamp(0, 32);
        self.radius = self.radius.clamp(0, 20);
        if self.sleep_after_min != 0 {
            self.sleep_after_min = self.sleep_after_min.clamp(1, 240);
        }
        if self.discard_after_min != 0 {
            self.discard_after_min = self.discard_after_min.clamp(self.sleep_after_min.max(1), 1440);
        }
        self.ram_limit_mb = self.ram_limit_mb.clamp(512, 32768);
        self.agent_port = self.agent_port.clamp(1024, 65535);
        self.cpu_limit_pct = self.cpu_limit_pct.clamp(5, 100);
        self.net_limit_kbps = self.net_limit_kbps.clamp(32, 1_000_000);
        self.download_limit_kbps = self.download_limit_kbps.min(1_000_000);
        self.default_zoom = crate::sites::clamp_pct(self.default_zoom);
        self.perm_defaults.retain(|k, v| v == "block" && !k.is_empty());
        self.http_ok.iter_mut().for_each(|h| *h = h.trim().to_ascii_lowercase());
        self.http_ok.retain(|h| !h.is_empty() && !h.contains(['/', ' ', '*']));
        self.http_ok.sort();
        self.http_ok.dedup();
        self.http_ok.truncate(500);
        self.adblock_off_sites.iter_mut().for_each(|h| *h = crate::blocker::site_key(h));
        self.adblock_off_sites.retain(|h| !h.is_empty() && !h.contains(['/', ' ', '*']));
        self.adblock_off_sites.sort();
        self.adblock_off_sites.dedup();
        self.adblock_off_sites.truncate(500);
        self.popup_sites.iter_mut().for_each(|h| *h = crate::blocker::site_key(h));
        self.popup_sites.retain(|h| !h.is_empty() && !h.contains(['/', ' ', '*']));
        self.popup_sites.sort();
        self.popup_sites.dedup();
        self.popup_sites.truncate(500);
        self.redirect_sites.iter_mut().for_each(|h| *h = crate::blocker::site_key(h));
        self.redirect_sites.retain(|h| !h.is_empty() && !h.contains(['/', ' ', '*']));
        self.redirect_sites.sort();
        self.redirect_sites.dedup();
        self.redirect_sites.truncate(500);
        if self.bt_port != 0 {
            self.bt_port = self.bt_port.clamp(BT_PORT_MIN, BT_PORT_MAX);
        }
        if !self.search.contains("%s") {
            self.search = Settings::default().search;
        }
        let two = self.translate_to.len() == 2 && self.translate_to.chars().all(|c| c.is_ascii_alphabetic());
        if !two {
            self.translate_to = Settings::default().translate_to;
        } else {
            self.translate_to.make_ascii_lowercase();
        }
        self.servo_sites.iter_mut().for_each(|h| *h = h.trim().to_string());
        self.servo_sites.retain(|h| !h.is_empty() && !h.contains(['/', ' ', '*']));
        self.servo_sites.truncate(500);
        if !drive_cmd_usable(&self.drive_cmd) {
            self.drive_cmd = DEFAULT_DRIVE_CMD.into();
        }
    }

    pub fn ensure_bt_port(&mut self) -> u16 {
        if self.bt_port == 0 {
            self.bt_port = crate::upnp::random_port();
            save_settings(self);
        }
        self.bt_port
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTab {
    pub url: String,
    pub title: String,
    pub favicon: String,
    pub pinned: bool,
    pub pin_url: String,
    pub essential: bool,
    pub idle_ms: u64,
    pub locked: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchivedTab {
    pub url: String,
    pub title: String,
    pub favicon: String,
    pub when: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub name: String,
    pub tabs: Vec<SavedTab>,
    pub active: usize,
    pub colour: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
    pub essentials: Vec<SavedTab>,
    pub archived: Vec<ArchivedTab>,
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

pub type Rules = std::collections::BTreeMap<String, std::collections::BTreeMap<String, bool>>;

pub fn load_rules() -> Rules {
    load("permissions.json")
}

pub fn save_rules(r: &Rules) {
    save("permissions.json", r);
}

pub fn load_zooms() -> crate::sites::Zooms {
    load("zoom.json")
}

pub fn save_zooms(z: &crate::sites::Zooms) {
    save("zoom.json", z);
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SavedDl {
    pub url: String,
    pub path: String,
    pub state: String,
    pub size: i64,
    #[serde(default)]
    pub note: String,
}

pub fn load_downloads() -> Vec<SavedDl> {
    load("downloads.json")
}

pub fn save_downloads(d: &[SavedDl]) {
    save("downloads.json", &d);
}

pub fn load_session() -> Session {
    load("session.json")
}

pub fn save_session(s: &Session) {
    save("session.json", s);
}

const CLEAR_FLAG: &str = "clear-history.flag";

pub fn clear_history_pending() -> bool {
    data_dir().join(CLEAR_FLAG).exists()
}

pub fn set_clear_history_pending(pending: bool) {
    if pending {
        save(CLEAR_FLAG, &true);
    } else {
        let _ = fs::remove_file(data_dir().join(CLEAR_FLAG));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_survives_the_clamp() {
        let mut s = Settings { sleep_after_min: 0, discard_after_min: 0, ..Settings::default() };
        s.clamp();
        assert_eq!((s.sleep_after_min, s.discard_after_min), (0, 0));
        let mut s = Settings { sleep_after_min: 60, discard_after_min: 10, ..Settings::default() };
        s.clamp();
        assert_eq!((s.sleep_after_min, s.discard_after_min), (60, 60));
        let mut s = Settings { sleep_after_min: 0, discard_after_min: 30, ..Settings::default() };
        s.clamp();
        assert_eq!((s.sleep_after_min, s.discard_after_min), (0, 30));
    }

    #[test]
    fn settings_survive_the_chrome_round_trip() {
        let mut s = Settings::default();
        s.clamp();
        let back: Result<Settings, _> = serde_json::from_value(serde_json::to_value(&s).unwrap());
        assert!(back.is_ok(), "{:?}", back.err());
        if let Ok(text) = std::env::var("LUMEN_TEST_SETTINGS_FILE").and_then(|p| std::fs::read_to_string(p).map_err(|_| std::env::VarError::NotPresent)) {
            let mut real: Settings = serde_json::from_str(&text).unwrap();
            real.clamp();
            let back: Result<Settings, _> = serde_json::from_value(serde_json::to_value(&real).unwrap());
            assert!(back.is_ok(), "real file: {:?}", back.err());
        }
    }

    #[test]
    fn clear_on_exit_is_off_by_default() {
        assert!(!Settings::default().clear_on_exit);
    }

    #[test]
    fn an_old_settings_file_has_userscripts_on() {
        let mut v = serde_json::to_value(Settings::default()).expect("settings to json");
        v.as_object_mut().expect("an object").remove("userscripts");
        let s: Settings = serde_json::from_value(v).expect("old file loads");
        assert!(s.userscripts);
    }

    #[test]
    fn an_old_settings_file_shows_the_speed_dial() {
        let mut v = serde_json::to_value(Settings::default()).expect("settings to json");
        v.as_object_mut().expect("an object").remove("speed_dial");
        v["shortcuts"] = serde_json::json!([{"title": "Mine", "url": "https://mine.test/"}]);
        let s: Settings = serde_json::from_value(v).expect("old file loads");
        assert!(s.speed_dial);
        assert_eq!(s.shortcuts.len(), 1);
    }

    #[test]
    fn an_old_session_with_pins_loads() {
        let old = r#"{"tabs":[{"url":"https://music.youtube.com/","title":"Music","favicon":"","pinned":true},
            {"url":"https://a.test/","title":"A","favicon":"","pinned":false}],"active":1,"width":900.0,"height":700.0,"maximized":false}"#;
        let s: Session = serde_json::from_str(old).expect("old session loads");
        assert_eq!(s.tabs.len(), 2);
        assert!(s.tabs[0].pinned);
        assert_eq!(s.tabs[0].pin_url, "");
        assert_eq!(crate::pins::restored_pin_url(s.tabs[0].pinned, &s.tabs[0].pin_url, &s.tabs[0].url), "https://music.youtube.com/");
    }

    #[test]
    fn the_pinned_page_survives_the_session_file() {
        let tab = SavedTab {
            url: "https://www.youtube.com/watch?v=1".into(),
            pinned: true,
            pin_url: "https://music.youtube.com/".into(),
            ..Default::default()
        };
        let back: SavedTab = serde_json::from_str(&serde_json::to_string(&tab).expect("to json")).expect("from json");
        assert_eq!(back.pin_url, "https://music.youtube.com/");
        assert_eq!(back.url, "https://www.youtube.com/watch?v=1");
    }

    #[test]
    fn a_drive_command_carries_the_task_and_the_rule() {
        let (program, args) = drive_command(DEFAULT_DRIVE_CMD, "buy the cheapest kettle").unwrap();
        assert_eq!(program, "opencode");
        assert_eq!(args[0], "run");
        assert_eq!(args[1], "--model");
        assert_eq!(args[2], "opencode/mimo-v2.6-flash-free");
        assert!(args[3].starts_with("buy the cheapest kettle"), "{}", args[3]);
        assert!(args[3].contains("browser_handoff"), "{}", args[3]);
        assert!(args[3].contains("pay"), "{}", args[3]);
        let (program, args) = drive_command("cmdc -p \"{task}\"", "say \"hi\" now").unwrap();
        assert_eq!(program, "cmdc");
        assert_eq!(args.len(), 2, "{args:?}");
        assert!(args[1].contains("say 'hi' now"), "{:?}", args[1]);
        assert!(drive_command("", "x").is_none());
        assert!(drive_command("   ", "x").is_none());
        assert!(!drive_cmd_usable("opencode run"));
        assert!(drive_cmd_usable(DEFAULT_DRIVE_CMD));
        assert!(!drive_prompt("a\nb").contains('\r'));
    }

    #[test]
    fn an_old_settings_file_without_clear_on_exit_loads() {
        let mut v = serde_json::to_value(Settings::default()).expect("settings to json");
        v.as_object_mut().expect("an object").remove("clear_on_exit");
        let s: Settings = serde_json::from_value(v).expect("old file loads");
        assert!(!s.clear_on_exit);
        assert_eq!(s.search, Settings::default().search);
        let mut s = s;
        s.clear_on_exit = true;
        s.clamp();
        assert!(s.clear_on_exit);
    }

    #[test]
    fn graphics_in_the_browser_process_goes_off_once() {
        let mut s: Settings = serde_json::from_str(r#"{"version":3,"gpu_in_process":true}"#).expect("loads");
        s.migrate();
        assert!(!s.gpu_in_process);
        assert_eq!(s.version, SETTINGS_VERSION);
        let mut t: Settings = serde_json::from_str(r#"{"version":4,"gpu_in_process":true}"#).expect("loads");
        t.migrate();
        assert!(t.gpu_in_process, "a choice made after the change stands");
        assert!(!Settings::default().gpu_in_process);
    }

    #[test]
    fn an_old_settings_file_gets_vertical_and_the_default_look() {
        let old = r#"{"version":2,"palette":"moss","theme":"dark","sidebar_width":300,"gap":4,"radius":0,
            "ui_font":"figtree","display_font":"fraunces","animations":true,"speed_dial":false}"#;
        let mut s: Settings = serde_json::from_str(old).expect("old file loads");
        assert_eq!(s.version, 2);
        s.migrate();
        s.clamp();
        assert_eq!(s.layout, "vertical");
        assert_eq!(s.motion, "full");
        assert_eq!(s.font_scale, 100);
        assert_eq!(s.title_weight, 400);
        assert!(s.clock && s.clock_24h && !s.clock_seconds);
        assert_eq!(s.date_format, "long");
        assert!(s.ntp_search);
        assert_eq!(s.dial_tile, 60);
        assert_eq!(s.tab_row_h, 32);
        assert!(s.show_favicons);
        assert_eq!(s.close_btn, "hover");
        assert!(s.footer_btns && s.ws_row);
        assert_eq!(s.density, "comfortable");
        assert!(!s.page_border && !s.bm_bar);
        assert_eq!(s.palette, "moss");
        assert_eq!(s.sidebar_width, 300);
        assert_eq!(s.ui_font, "figtree");
        assert!(!s.speed_dial);
        assert!(s.bg_color.is_empty() && s.side_color.is_empty() && s.surface_color.is_empty());
        assert!(s.text_color.is_empty() && s.dim_color.is_empty());
    }

    #[test]
    fn the_old_animations_switch_migrates_to_motion() {
        for (animations, want) in [(false, "off"), (true, "full")] {
            let old = format!(r#"{{"version":2,"palette":"graphite","animations":{animations}}}"#);
            let mut s: Settings = serde_json::from_str(&old).expect("old file loads");
            s.migrate();
            s.clamp();
            assert_eq!(s.motion, want, "animations={animations}");
        }
        let old = r#"{"version":2,"animations":false,"motion":"reduced"}"#;
        let mut s: Settings = serde_json::from_str(old).expect("old file loads");
        s.migrate();
        s.clamp();
        assert_eq!(s.motion, "reduced");
    }

    #[test]
    fn the_customization_round_trips() {
        let mut s = Settings::default();
        s.layout = "hybrid".into();
        s.bm_bar = true;
        s.bg_color = "#101820".into();
        s.side_color = "#181818".into();
        s.surface_color = "#20242c".into();
        s.text_color = "#f2f2f2".into();
        s.dim_color = "#9aa0a6".into();
        s.ui_font = "Segoe UI Variable Text".into();
        s.display_font = "Georgia".into();
        s.mono_font = "Cascadia Mono".into();
        s.font_scale = 115;
        s.title_weight = 600;
        s.clock = false;
        s.clock_24h = false;
        s.clock_seconds = true;
        s.date_format = "iso".into();
        s.clock_font = "ui".into();
        s.clock_size = 120;
        s.greeting = "Hello".into();
        s.ntp_bg = "wallpaper".into();
        s.wallpaper_blur = true;
        s.wallpaper_dim = 40;
        s.dial_cols = 5;
        s.dial_tile = 72;
        s.ntp_search = false;
        s.tab_row_h = 40;
        s.show_favicons = false;
        s.close_btn = "always".into();
        s.footer_btns = false;
        s.ws_row = false;
        s.density = "roomy".into();
        s.page_border = true;
        s.motion = "reduced".into();
        s.motion_speed = 150;
        s.motion_system = false;
        s.clamp();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).expect("to json")).expect("from json");
        assert_eq!(back.layout, "hybrid");
        assert!(back.bm_bar);
        assert_eq!(back.bg_color, "#101820");
        assert_eq!(back.side_color, "#181818");
        assert_eq!(back.surface_color, "#20242c");
        assert_eq!(back.text_color, "#f2f2f2");
        assert_eq!(back.dim_color, "#9aa0a6");
        assert_eq!(back.ui_font, "Segoe UI Variable Text");
        assert_eq!(back.display_font, "Georgia");
        assert_eq!(back.mono_font, "Cascadia Mono");
        assert_eq!(back.font_scale, 115);
        assert_eq!(back.title_weight, 600);
        assert!(!back.clock && !back.clock_24h && back.clock_seconds);
        assert_eq!(back.date_format, "iso");
        assert_eq!(back.clock_font, "ui");
        assert_eq!(back.clock_size, 120);
        assert_eq!(back.greeting, "Hello");
        assert_eq!(back.ntp_bg, "wallpaper");
        assert!(back.wallpaper_blur);
        assert_eq!(back.wallpaper_dim, 40);
        assert_eq!(back.dial_cols, 5);
        assert_eq!(back.dial_tile, 72);
        assert!(!back.ntp_search);
        assert_eq!(back.tab_row_h, 40);
        assert!(!back.show_favicons);
        assert_eq!(back.close_btn, "always");
        assert!(!back.footer_btns && !back.ws_row);
        assert_eq!(back.density, "roomy");
        assert!(back.page_border);
        assert_eq!(back.motion, "reduced");
        assert_eq!(back.motion_speed, 150);
        assert!(!back.motion_system);
    }

    #[test]
    fn a_bad_file_is_clamped() {
        let old = r##"{"layout":"diagonal","motion":"warp","density":"huge","close_btn":"sometimes",
            "date_format":"yesterday","bg_color":"not a color","text_color":"#GGGGGG",
            "font_scale":900,"title_weight":90000,"motion_speed":0,"clock_size":5000,
            "dial_cols":99,"dial_tile":1,"tab_row_h":400,"wallpaper_dim":900,"ntp_bg":"video"}"##;
        let mut s: Settings = serde_json::from_str(old).expect("file loads");
        s.clamp();
        assert_eq!(s.layout, "vertical");
        assert_eq!(s.motion, "full");
        assert_eq!(s.density, "comfortable");
        assert_eq!(s.close_btn, "hover");
        assert_eq!(s.date_format, "long");
        assert!(s.bg_color.is_empty() && s.text_color.is_empty());
        assert_eq!(s.font_scale, 130);
        assert_eq!(s.title_weight, 800);
        assert_eq!(s.motion_speed, 50);
        assert_eq!(s.clock_size, 200);
        assert_eq!(s.dial_cols, 8);
        assert_eq!(s.dial_tile, 44);
        assert_eq!(s.tab_row_h, 48);
        assert_eq!(s.wallpaper_dim, 80);
        assert_eq!(s.ntp_bg, "plain");
    }

    #[test]
    fn a_new_install_starts_at_the_first_run_picker() {
        let s = Settings::default();
        assert!(!s.first_run_done);
        assert_eq!(s.look, "signal");
    }

    #[test]
    fn an_old_settings_file_never_sees_the_picker() {
        let mut v = serde_json::to_value(Settings::default()).expect("settings to json");
        let obj = v.as_object_mut().expect("an object");
        obj.remove("first_run_done");
        obj.remove("look");
        let s: Settings = serde_json::from_value(v).expect("old file loads");
        assert!(s.first_run_done);
        assert_eq!(s.look, "signal");
    }

    #[test]
    fn a_bad_look_clamps_to_signal() {
        let mut s: Settings = serde_json::from_str(r#"{"look":"neon"}"#).expect("file loads");
        s.clamp();
        assert_eq!(s.look, "signal");
        for look in ["signal", "glass", "precise"] {
            let mut s: Settings = serde_json::from_str(&format!(r#"{{"look":"{look}"}}"#)).expect("file loads");
            s.clamp();
            assert_eq!(s.look, look);
        }
    }

    #[test]
    fn the_look_and_first_run_round_trip() {
        let mut s = Settings::default();
        s.look = "glass".into();
        s.first_run_done = true;
        s.clamp();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).expect("to json")).expect("from json");
        assert_eq!(back.look, "glass");
        assert!(back.first_run_done);
        let back: Settings = serde_json::from_str(&serde_json::to_string(&Settings::default()).expect("to json")).expect("from json");
        assert!(!back.first_run_done, "an unanswered flag does not read as answered");
        assert_eq!(back.look, "signal");
    }
}
