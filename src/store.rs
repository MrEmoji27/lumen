//! Settings and session, saved as JSON under %APPDATA%\Lumen.

use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

/// Where everything lives. LUMEN_DATA points tests at a throwaway folder,
/// so they never touch the real profile, settings or session.
pub fn data_dir() -> PathBuf {
    let dir = std::env::var_os("LUMEN_DATA").map(PathBuf::from).unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Lumen")
    });
    let _ = fs::create_dir_all(&dir);
    dir
}

/// Where downloads land. LUMEN_DOWNLOADS points tests at a throwaway folder,
/// so a live test never touches the real Downloads.
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
    /// Skip sponsor segments on YouTube (SponsorBlock).
    pub sponsorblock: bool,
    /// Hide ad leftovers and cookie banners (cosmetic rules).
    pub cosmetic: bool,
    /// Count what the blocker stops, per day and per site, for the report.
    pub privacy_stats: bool,
    /// The running "N blocked" note at the bottom of the sidebar.
    pub page_chip: bool,
    pub show_memory: bool,
    /// The speed dial: tiles under the new tab page's search box.
    pub shortcuts: Vec<Shortcut>,
    /// Show the speed dial. Off: the new tab page is the clock and the search box.
    pub speed_dial: bool,
    /// Run GPU work inside the browser process. About 100 MB less. Applies on restart.
    pub gpu_in_process: bool,
    /// Tabs of the same site share one renderer process. Fewer processes, less memory.
    /// Applies on restart.
    pub share_processes: bool,
    /// Offer to save passwords and fill them in (WebView2's own, encrypted by Windows).
    pub save_passwords: bool,
    /// Hand http(s) downloads to aria2: many connections per file, resume, queue.
    pub accelerate_downloads: bool,
    /// Cap aria2's total download speed, in KB/s. 0 = no limit.
    pub download_limit_kbps: u32,
    /// Share torrents back after they finish (until ratio 1.0 or one hour).
    /// Applies on the next start.
    pub keep_seeding: bool,
    /// The port aria2 listens on for torrents. Picked once in 20000..60000, then saved.
    pub bt_port: u16,
    /// Open that port on the router with UPnP so peers can reach us.
    pub upnp: bool,
    /// Show the "Windows may ask to let aria2 through the firewall" note once.
    pub torrent_note: bool,
    /// Keep the text of opened pages so history can be searched by it.
    pub remember_text: bool,
    /// Clear history (Lumen's, page text and WebView2's own) when Lumen closes.
    pub clear_on_exit: bool,
    /// When Lumen is dark, darken light sites too (Dark Reader).
    pub dark_pages: bool,
    /// Hosts Dark Reader leaves alone.
    pub dark_skip: Vec<String>,
    /// "Translate page" (F8) target language, two letters: a Spanish page reads in
    /// English. The pair has to be one the model registry carries.
    pub translate_to: String,
    /// Hosts that open in Servo instead of WebView2 ("Servo sites"). Light pages,
    /// where Servo's smaller footprint pays. Empty means everything is WebView2.
    /// Needs a build with the `servo` feature; without it the list does nothing.
    pub servo_sites: Vec<String>,
    /// Interface font: "segoe" | "instrument" | "figtree" | "atkinson".
    pub ui_font: String,
    /// Display font for the clock and page titles: "instrument" | "fraunces" | "newsreader".
    pub display_font: String,
    /// Let AI agents browse through the built-in MCP server on localhost.
    pub agent: bool,
    pub agent_port: u32,
    /// Agent tabs use the user's profile (logins, cookies) instead of their own.
    pub agent_shared_profile: bool,
    /// Every agent gets its own WebView2 environment: its own user-data folder, so its
    /// own browser process. The hardest wall between agents; costs memory per agent.
    pub agent_strict_isolation: bool,
    /// Let agents put files into web pages (browser_upload). Off by default.
    pub agent_upload: bool,
    /// The one folder agents may upload from. Empty means "Lumen agent uploads" in the home folder.
    pub agent_upload_dir: String,
    /// The Drive panel's command. `{task}` is replaced by the person's task.
    pub drive_cmd: String,
    /// Keep a local usage journal: speed, memory, crashes and errors, on this PC
    /// only, hosts instead of URLs. Nothing is ever sent anywhere.
    pub journal: bool,
    /// Soft RAM limit: over it, the oldest hidden tabs are unloaded.
    pub ram_limit: bool,
    pub ram_limit_mb: u32,
    /// Hard CPU cap for the whole browser, percent of all cores (Windows Job Object).
    pub cpu_limit: bool,
    pub cpu_limit_pct: u32,
    /// Bandwidth cap per tab, KB/s.
    pub net_limit: bool,
    pub net_limit_kbps: u32,
    /// What a kind of permission question does when the site has no answer of its own:
    /// "block", or absent for ask. Keys are the kinds of `sites::KINDS`.
    pub perm_defaults: std::collections::BTreeMap<String, String>,
    /// The zoom pages open at, in percent, unless the site has a zoom of its own.
    pub default_zoom: u32,
    /// Try https first for plain http pages, and warn when a site has none.
    pub https_first: bool,
    /// Hosts the person said may stay on plain http.
    pub http_ok: Vec<String>,
    /// Send "Sec-GPC: 1" and tell pages not to sell or share what they know of you.
    pub gpc: bool,
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
            sponsorblock: true,
            show_memory: true,
            gpu_in_process: true,
            share_processes: true,
            save_passwords: true,
            accelerate_downloads: true,
            download_limit_kbps: 0,
            keep_seeding: true,
            bt_port: 0,
            upnp: true,
            torrent_note: true,
            cosmetic: true,
            privacy_stats: true,
            page_chip: true,
            dark_pages: false,
            remember_text: true,
            clear_on_exit: false,
            dark_skip: Vec::new(),
            translate_to: "en".into(),
            servo_sites: Vec::new(),
            ui_font: "segoe".into(),
            display_font: "instrument".into(),
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

const SETTINGS_VERSION: u32 = 2;
/// Where the torrent port may live. High enough to dodge the usual listeners.
pub(crate) const BT_PORT_MIN: u16 = 20000;
pub(crate) const BT_PORT_MAX: u16 = 60000;

/// The Drive panel's default command: an agent CLI already installed, run once with the
/// task. Presets for other CLIs live in the panel's settings row.
pub const DEFAULT_DRIVE_CMD: &str = "opencode run --model opencode/mimo-v2.6-flash-free \"{task}\"";
/// What a Drive command must not be: run with no task and no room for one.
pub(crate) fn drive_cmd_usable(cmd: &str) -> bool {
    let c = cmd.trim();
    !c.is_empty() && c.contains("{task}") && c.len() <= 600 && !c.chars().any(char::is_control)
}

/// The task Lumen hands the Drive agent: the person's own words, plus the one rule that
/// stands above the task. The text carries no URL and nothing about the person.
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

/// A command template as a program and its arguments. Double quotes group words (they
/// are not passed on); `{task}` becomes the full prompt. None when there is nothing to run.
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
        self.sidebar_width = self.sidebar_width.clamp(232, 400);
        self.gap = self.gap.clamp(0, 32);
        self.radius = self.radius.clamp(0, 20);
        self.sleep_after_min = self.sleep_after_min.clamp(1, 240);
        self.discard_after_min = self.discard_after_min.clamp(self.sleep_after_min, 1440);
        self.ram_limit_mb = self.ram_limit_mb.clamp(512, 32768);
        self.agent_port = self.agent_port.clamp(1024, 65535);
        self.cpu_limit_pct = self.cpu_limit_pct.clamp(5, 100);
        self.net_limit_kbps = self.net_limit_kbps.clamp(32, 1_000_000);
        self.download_limit_kbps = self.download_limit_kbps.min(1_000_000);
        self.default_zoom = crate::sites::clamp_pct(self.default_zoom);
        // Only "block" means anything in the defaults; ask is the absence of a line.
        self.perm_defaults.retain(|k, v| v == "block" && !k.is_empty());
        self.http_ok.iter_mut().for_each(|h| *h = h.trim().to_ascii_lowercase());
        self.http_ok.retain(|h| !h.is_empty() && !h.contains(['/', ' ', '*']));
        self.http_ok.sort();
        self.http_ok.dedup();
        self.http_ok.truncate(500);
        // 0 means "never chosen"; a real port stays inside the range.
        if self.bt_port != 0 {
            self.bt_port = self.bt_port.clamp(BT_PORT_MIN, BT_PORT_MAX);
        }
        if !self.search.contains("%s") {
            self.search = Settings::default().search;
        }
        // The translation target is a two-letter language code, or English.
        let two = self.translate_to.len() == 2 && self.translate_to.chars().all(|c| c.is_ascii_alphabetic());
        if !two {
            self.translate_to = Settings::default().translate_to;
        } else {
            self.translate_to.make_ascii_lowercase();
        }
        // A host list is hosts: no spaces, no blanks, and not a thousand of them.
        self.servo_sites.iter_mut().for_each(|h| *h = h.trim().to_string());
        self.servo_sites.retain(|h| !h.is_empty() && !h.contains(['/', ' ', '*']));
        self.servo_sites.truncate(500);
        // A Drive command without `{task}` could never run anything useful.
        if !drive_cmd_usable(&self.drive_cmd) {
            self.drive_cmd = DEFAULT_DRIVE_CMD.into();
        }
    }

    /// The torrent port, chosen once and saved. 0 means it has never been chosen,
    /// so the first call picks one and writes it; later calls return the same one.
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
    /// The page a pin was pinned on. Empty in files from before pins remembered it.
    pub pin_url: String,
}

/// A named set of tabs. Saved in session.json so workspaces survive a restart.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub name: String,
    pub tabs: Vec<SavedTab>,
    /// Index into `tabs` of the active tab in this workspace.
    pub active: usize,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// Kept on disk for the old single-workspace format. Populated on save from
    /// the active workspace so a pre-workspaces build can still read the file.
    pub tabs: Vec<SavedTab>,
    pub active: usize,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
    /// New format: one entry per workspace. Empty when the file predates workspaces.
    pub workspaces: Vec<Workspace>,
    pub active_workspace: usize,
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

/// Remembered site permissions: origin -> permission -> allowed.
pub type Rules = std::collections::BTreeMap<String, std::collections::BTreeMap<String, bool>>;

pub fn load_rules() -> Rules {
    load("permissions.json")
}

pub fn save_rules(r: &Rules) {
    save("permissions.json", r);
}

/// The zoom of each site, in percent: zoom.json. Only sites that differ from the default zoom.
pub fn load_zooms() -> crate::sites::Zooms {
    load("zoom.json")
}

pub fn save_zooms(z: &crate::sites::Zooms) {
    save("zoom.json", z);
}

/// A finished download, kept so the Downloads page survives a restart.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SavedDl {
    pub url: String,
    pub path: String,
    /// "done" | "failed"
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

/// Set when Lumen closed with "clear history on exit" on and the WebView2 call
/// could not be made to finish first. The next start wipes it and drops the flag.
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
    fn clear_on_exit_is_off_by_default() {
        assert!(!Settings::default().clear_on_exit);
    }

    /// A settings.json from before the speed dial switch still loads, with the speed dial on
    /// and the person's own tiles untouched.
    #[test]
    fn an_old_settings_file_shows_the_speed_dial() {
        let mut v = serde_json::to_value(Settings::default()).expect("settings to json");
        v.as_object_mut().expect("an object").remove("speed_dial");
        v["shortcuts"] = serde_json::json!([{"title": "Mine", "url": "https://mine.test/"}]);
        let s: Settings = serde_json::from_value(v).expect("old file loads");
        assert!(s.speed_dial);
        assert_eq!(s.shortcuts.len(), 1);
    }

    /// A session.json from before pins remembered their page still loads: the pin
    /// is there, with no pinned URL yet (the restore fills it in).
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

    /// The pinned page rides through a save and a load, apart from the page the tab is on.
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

    /// The Drive command becomes a program and arguments, with the task inside the
    /// prompt and the handoff rule always in it.
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
        // A quote in the task cannot break out of the template's own quotes.
        let (program, args) = drive_command("cmdc -p \"{task}\"", "say \"hi\" now").unwrap();
        assert_eq!(program, "cmdc");
        assert_eq!(args.len(), 2, "{args:?}");
        assert!(args[1].contains("say 'hi' now"), "{:?}", args[1]);
        // A template with nothing in it, and one with no {task}, are not runnable.
        assert!(drive_command("", "x").is_none());
        assert!(drive_command("   ", "x").is_none());
        assert!(!drive_cmd_usable("opencode run"));
        assert!(drive_cmd_usable(DEFAULT_DRIVE_CMD));
        // The prompt never carries a newline the command line cannot hold.
        assert!(!drive_prompt("a\nb").contains('\r'));
    }

    /// A settings.json written before the field existed still loads, and reads as off.
    #[test]
    fn an_old_settings_file_without_clear_on_exit_loads() {
        let mut v = serde_json::to_value(Settings::default()).expect("settings to json");
        v.as_object_mut().expect("an object").remove("clear_on_exit");
        let s: Settings = serde_json::from_value(v).expect("old file loads");
        assert!(!s.clear_on_exit);
        // The rest of the file came through untouched.
        assert_eq!(s.search, Settings::default().search);
        // And turning it on survives a round trip.
        let mut s = s;
        s.clear_on_exit = true;
        s.clamp();
        assert!(s.clear_on_exit);
    }
}
