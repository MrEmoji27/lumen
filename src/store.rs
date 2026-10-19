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
    /// Whether the first-run "pick a look" picker has been answered. Missing from a
    /// file means the file predates the picker: someone who already chose, so it
    /// reads true. Only a brand-new install, with no file at all, starts false.
    #[serde(default = "yes")]
    pub first_run_done: bool,
    /// The last version whose changes the person has seen. Behind the running one, the start
    /// page says what is new. A file without it predates the note: it reads empty, so the
    /// person hears about this version once. A new install starts at its own version.
    #[serde(default)]
    pub seen_version: String,
    /// Named palette from the UI ("graphite", "sand", "slate", "moss", "rose").
    pub palette: String,
    /// Accent color override. Empty means palette default.
    pub accent: String,
    /// The last colours picked in the colour picker, newest first. Shown as its own
    /// row of swatches.
    pub recent_colors: Vec<String>,
    /// "solid" | "mica" | "acrylic". Solid is the fastest.
    pub backdrop: String,
    /// "dark" | "light" | "system". Also drives prefers-color-scheme in pages.
    pub theme: String,
    /// The redesign's look: "signal" (warm paper and ink, the default), "glass"
    /// (a frosted sidebar over a colour wallpaper) or "precise" (near-black,
    /// hairlines). Anything else clamps to "signal".
    pub look: String,
    /// Where the chrome lives: "vertical" (the sidebar), "horizontal" (a top strip
    /// like Chrome), or "hybrid" (a vertical tab sidebar under a top toolbar).
    pub layout: String,
    /// Horizontal layout: the bookmarks bar under the toolbar.
    pub bm_bar: bool,
    /// Custom color overrides, "#rrggbb". Empty means the palette's own color.
    /// The background behind the page, the sidebar and toolbar, cards and fields,
    /// the text and the dim text.
    pub bg_color: String,
    pub side_color: String,
    pub surface_color: String,
    pub text_color: String,
    pub dim_color: String,
    /// "left" | "right"
    pub sidebar_side: String,
    pub sidebar_width: u32,
    pub compact: bool,
    /// Hide the sidebar until the mouse reaches the edge of the window. The
    /// page takes the whole width; the sidebar slides in over it.
    pub hide_sidebar: bool,
    /// Height of one tab row in the sidebar, in px.
    pub tab_row_h: u32,
    /// Draw the site's icon in a tab row (off: the letter tile only).
    pub show_favicons: bool,
    /// The tab row's close button: "hover" | "always" | "off".
    pub close_btn: String,
    /// The footer's downloads, bookmarks and settings buttons.
    pub footer_btns: bool,
    /// The workspace switcher row at the top of the sidebar.
    pub ws_row: bool,
    /// Space around the page, in px.
    pub gap: u32,
    /// Corner radius of the page, in px. 0 = square.
    pub radius: u32,
    /// "compact" | "comfortable" | "roomy": scales the spacing, not the type.
    pub density: String,
    /// A hairline border around the page, so it is not just a hole in the window.
    pub page_border: bool,
    /// Old on/off switch. `motion` is the real setting; this stays so files from
    /// before it still read as off when they said off.
    pub animations: bool,
    /// "full" | "reduced" | "off". Reduced draws no movement; off is the same,
    /// and also stops the loading spinner from turning. A file from before the
    /// setting was split reads as empty here, and the old switch above decides.
    #[serde(default = "missing")]
    pub motion: String,
    /// Motion speed, percent of the default durations. 50..200.
    pub motion_speed: u32,
    /// Follow Windows' "Show animations in Windows" (Accessibility). On: a system
    /// that asks for less motion gets "reduced", whatever the setting says.
    pub motion_system: bool,
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
    /// Run the person's userscripts: `*.user.js` files in `userscripts\` in the data
    /// folder, in their own tabs only, never an agent's or a private one.
    pub userscripts: bool,
    /// On battery, or with Windows' battery saver on: hidden tabs freeze after 1 minute
    /// and unload after 10, and the chrome's motion is reduced.
    #[serde(default = "yes")]
    pub battery_saver: bool,
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
    /// Trim the page you are watching when you are away from the mouse and keyboard:
    /// full memory while it loads, then 15 s after the last input the on-screen page
    /// goes to WebView2's low memory target, and back the moment you touch either again.
    #[serde(default = "yes")]
    pub trim_watched: bool,
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
    /// Interface font: one of the bundled keys ("segoe", "instrument", "figtree",
    /// "atkinson") or any installed Windows family name.
    pub ui_font: String,
    /// Display font for the clock and page titles: "instrument" | "fraunces" |
    /// "newsreader", or any installed Windows family name.
    pub display_font: String,
    /// Monospace font for figures (the Control table, the memory readout):
    /// "cascadia" for the bundled-ish default, or any installed family name.
    pub mono_font: String,
    /// Type scale, percent. 90..130.
    pub font_scale: u32,
    /// Weight of the display type: titles, headers, the clock. 300..800.
    pub title_weight: u32,
    /// Show the clock on the new tab page.
    pub clock: bool,
    /// 24-hour clock (off: 12-hour with am/pm).
    pub clock_24h: bool,
    /// Seconds after the minutes.
    pub clock_seconds: bool,
    /// "long" (Thursday, 17 October) | "short" (17 Oct) | "iso" (2026-10-17) | "off".
    pub date_format: String,
    /// The clock's type: "display" (the display font) | "ui".
    pub clock_font: String,
    /// Clock size in px; 0 follows the window's width.
    pub clock_size: u32,
    /// A line over the clock, your own words. Empty: none.
    pub greeting: String,
    /// New tab page background: "plain" | "wallpaper" (the file in `wallpaper`).
    pub ntp_bg: String,
    /// Soften the wallpaper (drawn from a small, upscaled copy).
    pub wallpaper_blur: bool,
    /// Dim the wallpaper, percent of black laid over it. 0..80.
    pub wallpaper_dim: u32,
    /// Columns of speed-dial tiles; 0 follows the window's width.
    pub dial_cols: u32,
    /// A speed-dial tile's size in px.
    pub dial_tile: u32,
    /// The search box on the new tab page.
    pub ntp_search: bool,
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
    /// Hosts where the person turned ad blocking off, from the switch in the site
    /// card: keyed by `blocker::site_key`, so `www.` and subdomains count as one.
    pub adblock_off_sites: Vec<String>,
    /// Send "Sec-GPC: 1" and tell pages not to sell or share what they know of you.
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
            show_memory: true,
            gpu_in_process: false,
            share_processes: true,
            trim_watched: true,
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

/// The looks that may be chosen, so a bad file cannot pick a fifth one.
pub const LAYOUTS: [(&str, &str); 3] = [("vertical", "Vertical"), ("horizontal", "Horizontal"), ("hybrid", "Hybrid")];
/// The motion modes. Reduced means no movement; off also stops the spinner.
pub const MOTIONS: [(&str, &str); 3] = [("full", "Full"), ("reduced", "Reduced"), ("off", "Off")];
/// The three spacing densities.
pub const DENSITIES: [(&str, &str); 3] = [("compact", "Compact"), ("comfortable", "Comfortable"), ("roomy", "Roomy")];
/// The tab row's close button.
pub const CLOSE_BTNS: [(&str, &str); 3] = [("hover", "On hover"), ("always", "Always"), ("off", "Hidden")];
/// The date line under the clock.
pub const DATE_FORMATS: [(&str, &str); 4] = [("long", "Thursday, 17 October"), ("short", "17 Oct"), ("iso", "2026-10-17"), ("off", "Hidden")];
/// The three looks of the redesign: Signal (the default), Glass and Precise.
/// The first-run picker and the Settings list show them in this order.
pub const LOOKS: [(&str, &str); 3] = [("signal", "Signal"), ("glass", "Glass"), ("precise", "Precise")];

/// A key a file from before this setting did not have: empty, so the reader can
/// tell "not there" from "chose the default".
fn missing() -> String {
    String::new()
}

/// What a missing `first_run_done` reads as: a settings.json without the key is
/// someone who already chose, never a brand-new install. A new install has no
/// file at all, so it gets Settings::default() and the picker shows.
fn yes() -> bool {
    true
}

/// An option list holds one of its keys, or `default` when the file said
/// something else.
fn one_of(v: &mut String, keys: &[(&str, &str)], default: &str) {
    if !keys.iter().any(|(k, _)| *k == v.as_str()) {
        *v = default.to_string();
    }
}

/// A "#rrggbb" override, or empty. Anything else is treated as no override.
fn clean_color(v: &mut String) {
    let s = v.trim().to_ascii_lowercase();
    let ok = s.is_empty()
        || (s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit()));
    *v = if ok { s } else { String::new() };
}

/// A font name from the settings: a bundled key or an installed family. Trimmed
/// and bounded; a name is never more than a line. Empty falls back to the default.
fn clean_font(v: &mut String, default: &str) {
    *v = v.trim().chars().take(64).collect();
    if v.is_empty() {
        *v = default.to_string();
    }
}

impl Settings {
    /// Version 2 is the minimal redesign. Old look values were defaults, not choices.
    /// Version 3 splits the motion switch into full/reduced/off.
    /// Version 4 turns graphics in the browser process off: it took the GPU's video decoding
    /// with it, and every video was decoded on the CPU. It was a default, not a choice.
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

    /// Keep values inside sane bounds. A bad file must not break the browser.
    pub fn clamp(&mut self) {
        // A file from before the motion setting: the old on/off switch decides.
        if self.motion.is_empty() {
            self.motion = if self.animations { "full" } else { "off" }.into();
        }
        one_of(&mut self.layout, &LAYOUTS, "vertical");
        one_of(&mut self.look, &LOOKS, "signal");
        one_of(&mut self.motion, &MOTIONS, "full");
        one_of(&mut self.density, &DENSITIES, "comfortable");
        one_of(&mut self.close_btn, &CLOSE_BTNS, "hover");
        one_of(&mut self.date_format, &DATE_FORMATS, "long");
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
        // A per-site switch stores the host the way the blocker compares it, so a
        // hand-edited file and the site card's own copy land on the same words.
        self.adblock_off_sites.iter_mut().for_each(|h| *h = crate::blocker::site_key(h));
        self.adblock_off_sites.retain(|h| !h.is_empty() && !h.contains(['/', ' ', '*']));
        self.adblock_off_sites.sort();
        self.adblock_off_sites.dedup();
        self.adblock_off_sites.truncate(500);
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
    /// The tab is an Essential: it keeps the site open in every workspace.
    pub essential: bool,
}

/// A named set of tabs. Saved in session.json so workspaces survive a restart.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub name: String,
    pub tabs: Vec<SavedTab>,
    /// Index into `tabs` of the active tab in this workspace.
    pub active: usize,
    /// The colour this workspace wears: a number in the palette, empty for none.
    pub colour: String,
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
    /// The sites kept open in every workspace: the essentials. The same row in
    /// every workspace, whichever one is open.
    pub essentials: Vec<SavedTab>,
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

    /// The chrome sends the whole Settings back as JSON when one thing changes (a new
    /// speed-dial tile, a toggle). A value that does not read back is dropped silently.
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

    /// A settings.json from before userscripts still loads, and reads as on: the field
    /// is new, not a choice the person made.
    #[test]
    fn an_old_settings_file_has_userscripts_on() {
        let mut v = serde_json::to_value(Settings::default()).expect("settings to json");
        v.as_object_mut().expect("an object").remove("userscripts");
        let s: Settings = serde_json::from_value(v).expect("old file loads");
        assert!(s.userscripts);
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

    /// Graphics in the browser process cost the GPU's video decoding. A file that has it on from
    /// the old default gets it off, once; after that the person's choice stands.
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

    /// A file from before every customization field still loads: the layout is
    /// vertical, the fonts are the defaults, the clock is on, motion is full.
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
        // The choices that were in the file are still the person's.
        assert_eq!(s.palette, "moss");
        assert_eq!(s.sidebar_width, 300);
        assert_eq!(s.ui_font, "figtree");
        assert!(!s.speed_dial);
        // The custom colours are unset, so the palette paints.
        assert!(s.bg_color.is_empty() && s.side_color.is_empty() && s.surface_color.is_empty());
        assert!(s.text_color.is_empty() && s.dim_color.is_empty());
    }

    /// The old animations switch still speaks: a file that said off reads as
    /// motion off; a file that said on reads as full.
    #[test]
    fn the_old_animations_switch_migrates_to_motion() {
        for (animations, want) in [(false, "off"), (true, "full")] {
            let old = format!(r#"{{"version":2,"palette":"graphite","animations":{animations}}}"#);
            let mut s: Settings = serde_json::from_str(&old).expect("old file loads");
            s.migrate();
            s.clamp();
            assert_eq!(s.motion, want, "animations={animations}");
        }
        // A file that already has a motion choice keeps it.
        let old = r#"{"version":2,"animations":false,"motion":"reduced"}"#;
        let mut s: Settings = serde_json::from_str(old).expect("old file loads");
        s.migrate();
        s.clamp();
        assert_eq!(s.motion, "reduced");
    }

    /// Every customization survives a save and a load.
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

    /// Nonsense in the file is clamped, never trusted: a layout that is not one
    /// of the three becomes vertical, colours that are not colours go away.
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

    /// A brand-new install has no settings.json at all, so it reads the struct
    /// defaults: the first-run picker has not been answered, and the look is
    /// Signal, the redesign's default.
    #[test]
    fn a_new_install_starts_at_the_first_run_picker() {
        let s = Settings::default();
        assert!(!s.first_run_done);
        assert_eq!(s.look, "signal");
    }

    /// Anyone who already has a settings.json chose long ago, so they never see
    /// the picker: the missing key reads as done. A missing look reads as the
    /// default, Signal.
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

    /// A look that is not one of the three is no choice at all: it clamps to
    /// Signal. The three real ones are the person's and are kept.
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

    /// The chosen look and the answered first-run flag survive a save and a load.
    /// A flag that is false is a value, not a missing key: it stays false.
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
