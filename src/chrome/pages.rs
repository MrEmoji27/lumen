//! Settings and Control, drawn natively. A small widget kit (segments, toggles,
//! sliders, swatches) and two pages built from it. Settings keys are read and
//! written through serde, so a new setting is one line in a schema.

use serde_json::{Value, json};

use super::{
    Hit, Inner, UiDl, UiState, send,
    gfx::{Align, R},
    style::{self, COL_MARGIN, COL_W, ROW_H, Look},
    theme::Color,
};
use crate::store::{CLOSE_BTNS, DATE_FORMATS, DENSITIES, LAYOUTS, MOTIONS, Settings};

#[derive(Clone, Debug, PartialEq)]
pub enum PageHit {
    Seg(&'static str, &'static str),
    /// The first-run picker's Start.
    FirstRunDone,
    Toggle(&'static str),
    Slider(&'static str),
    Accent,
    AccentReset,
    /// A custom colour, by its setting key.
    Color(&'static str),
    /// Back to the palette's own for that colour.
    ColorClear(&'static str),
    /// A text setting, edited in a native dialog.
    Text(&'static str),
    /// Open the searchable font list for a setting.
    Fonts(&'static str),
    /// Choose the nth family of the list the picker last drew.
    FontPin(usize),
    FontClose,
    /// Save the current look under a name; apply or remove a saved one.
    ThemeSave,
    ThemeApply(usize),
    ThemeDelete(usize),
    ThemeExport,
    ThemeImport,
    /// Back to the defaults, for one group of the Appearance page.
    ResetGroup(&'static str),
    Engine(usize),
    Nav(usize),
    Done,
    Unload(u64),
    /// A row of a list: only there so it can light up under the mouse.
    Row(u64),
    ClearPasswords,
    KdbxOpen,
    KdbxLock,
    CopyAgent,
    Dl(&'static str, u64),
    DlClear,
    DlAddLink,
    DlTorrentPick,
    DlOpenFolder,
    DlLimit,
    DlNote,
    PermReset(usize),
    Open(String),
    Forget(String),
    /// A row of the Bookmarks page: click opens, drag moves, right click has the menu.
    Bm(u64),
    BmDel(u64),
    /// A folder's header (the empty name is Unsorted): click folds it.
    BmFolder(String),
    BmNewFolder,
    BmImport,
    ClearHistory,
    ClearData,
    /// The Drive panel: start the run, stop it, or take the tab over.
    DriveStart,
    DriveStop,
    DrivePause,
    /// Settings: write Lumen's browser entries and open Windows' Default apps page.
    DefaultMake,
    /// Settings: take Lumen's browser entries out again.
    DefaultOff,
    /// Site settings: allow, block or ask one kind for one origin.
    SitePerm(String, String, &'static str),
    /// Site settings: forget every answer a site has.
    SiteReset(String),
    /// Site settings: clear the cookies and stored data of one site.
    SiteClear(String),
    /// Site settings: forget the zoom a site is read at.
    SiteZoom(String),
    /// Privacy: forget one host's https exception.
    SiteHttp(String),
    /// Sites: put a host back under ad blocking.
    SiteAdblock(String),
    /// Privacy: what a kind does when the site has no answer: "block" or "ask".
    PermDefault(&'static str, &'static str),
    /// A settings button that opens another page ("sites").
    Panels(&'static str),
    /// A link that opens in a tab of its own.
    OpenTab(&'static str),
    /// The problem report: to zemo, onto the clipboard, or into Downloads as a file.
    BugSend,
    BugCopy,
    BugSave,
    /// The start page's crash note, put away without a report.
    CrashSeen,
    /// The start page's "what is new" note, put away unread.
    SeenVersion,
}

/// Where "Made by zemo" goes.
pub const PORTFOLIO: &str = "https://portfolio.mremoji47.workers.dev/";

#[derive(Clone, Copy)]
pub(super) enum Scale {
    Linear(f32, f32, f32),
    Steps(&'static [u32]),
}

struct Slide {
    key: &'static str,
    track: R,
    scale: Scale,
}

/// The font list, open over the settings page: which setting is being chosen,
/// the filter typed so far, and the list it last showed.
pub(super) struct FontPick {
    pub(super) key: &'static str,
    pub(super) filter: String,
    pub(super) scroll: f32,
    /// The families the last draw showed, so a click lands on one.
    pub(super) shown: Vec<String>,
}

#[derive(Default)]
pub struct Page {
    pub scroll: f32,
    /// The panel the scroll offset belongs to.
    pub(super) which: &'static str,
    pub(super) max: f32,
    pub usage: Option<Value>,
    slide: Option<Slide>,
    sliders: Vec<(&'static str, R, Scale)>,
    sections: Vec<f32>,
    view: R,
    /// A short-lived note ("Copy a link or magnet first", a failed unlock).
    pub(super) flash: Option<(String, std::time::Instant)>,
    /// Bookmark folders folded shut. Forgotten when the page closes.
    pub(super) bm_collapsed: std::collections::HashSet<String>,
    /// A bookmark row picked up: it moves when let go over another row or folder.
    bm_drag: Option<BmDrag>,
    /// What the last Bookmarks frame drew, for working out where a drag lands.
    bm_rows: Vec<DropRow>,
    /// The font list, open or not.
    pub(super) font_pick: Option<FontPick>,
    /// The name being typed for "save this look".
    pub(super) theme_name: String,
    /// The saved looks, read from disk the first time the page needs them.
    pub(super) themes: Option<Vec<style::ThemeFile>>,
    /// The changelog laid out for a width, so a frame does not measure it all again.
    changes: Option<(f32, Vec<ChangeBlock>)>,
    /// The problem report broken into lines for a width: (width, text length, lines).
    bug_lines: Option<(f32, usize, Vec<String>)>,
}

struct BmDrag {
    id: u64,
    y0: f32,
    moved: bool,
}

impl Page {
    /// Debug builds: start a page scrolled, for a screenshot of what is further
    /// down. The content's height is only known after the first draw, and the
    /// clamp must not eat the offset before that.
    #[cfg(debug_assertions)]
    pub(super) fn test_scroll(&mut self, px: f32) {
        self.scroll = px;
        self.max = f32::MAX;
    }
}

/// A row or a folder header on the Bookmarks page, as the drag sees it. `id` 0 is a header.
#[derive(Clone, Debug, PartialEq)]
struct DropRow {
    r: R,
    folder: String,
    id: u64,
}

// ------------------------------------------------------------------ schema

pub(super) enum Item {
    /// Palette presets and the custom colours: the whole Colours group.
    Colors,
    /// The saved looks: save, apply, export, import.
    Themes,
    Seg(&'static str, &'static str, &'static str, &'static [(&'static str, &'static str)]),
    /// Like Seg, but the choices wrap onto as many lines as they need. For lists
    /// that are too long for one pill (the translation languages).
    Pick(&'static str, &'static str, &'static str, &'static [(&'static str, &'static str)]),
    Toggle(&'static str, &'static str, &'static str),
    Range(&'static str, &'static str, &'static str, Scale, &'static str),
    /// A font: the current family, and a button opening the searchable list.
    Fonts(&'static str, &'static str, &'static str),
    /// A text setting, typed into a native dialog.
    Text(&'static str, &'static str, &'static str),
    /// The wallpaper file, chosen with a native open dialog.
    Wallpaper,
    /// How the chrome is arranged: the whole Layout group, with its three shapes.
    Layout,
    /// A group's "reset to default" line.
    Reset(&'static str, &'static str),
    Engine,
    Keys,
    Button(&'static str, &'static str, &'static str, PageHit),
    KeePass,
    AgentSetup,
    SitePerms,
    DefaultBrowser,
}

const RAM: &[u32] = &[512, 768, 1024, 1536, 2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768];
const NET: &[u32] = &[32, 64, 128, 256, 512, 1024, 2048, 5120, 10240, 25600, 51200, 102400];
const DL_SPEEDS: &[u32] = &[0, 1024, 5120, 10240];
/// The clock's size, in px. 0 follows the window.
const CLOCK_SIZES: &[u32] = &[0, 48, 64, 80, 96, 112, 128, 152, 176];
/// The speed-dial columns. 0 follows the width.
const DIAL_COLS: &[u32] = &[0, 3, 4, 5, 6, 7, 8];
const ENGINES: [(&str, &str); 5] = [
    ("DuckDuckGo", "https://duckduckgo.com/?q=%s"),
    ("Google", "https://www.google.com/search?q=%s"),
    ("Brave", "https://search.brave.com/search?q=%s"),
    ("Bing", "https://www.bing.com/search?q=%s"),
    ("Startpage", "https://www.startpage.com/do/search?q=%s"),
];
const KEYS: [(&str, &str); 31] = [
    ("New tab", "Ctrl+T"),
    ("New private tab", "Ctrl+Shift+N"),
    ("Reopen closed tab", "Ctrl+Shift+T"),
    ("Close tab", "Ctrl+W"),
    ("Address bar", "Ctrl+L"),
    ("Next / previous tab", "Ctrl+Tab / Ctrl+Shift+Tab"),
    ("Jump to tab", "Ctrl+1…9"),
    ("Search open tabs", "Ctrl+Shift+A"),
    ("Command palette", "Ctrl+K"),
    ("Back / forward", "Alt+← / Alt+→"),
    ("Reload", "Ctrl+R"),
    ("Compact sidebar", "Ctrl+B"),
    ("Find in page", "Ctrl+F"),
    ("Print", "Ctrl+P"),
    ("Save page", "Ctrl+S"),
    ("Zoom in / out / reset", "Ctrl++ / Ctrl+- / Ctrl+0"),
    ("Zoom with the wheel", "Ctrl+wheel"),
    ("Fullscreen", "F11"),
    ("Settings", "Ctrl+,"),
    ("Control", "Shift+Esc"),
    ("Bookmark this page", "Ctrl+D"),
    ("History / bookmarks", "Ctrl+H / Ctrl+Shift+O"),
    ("Downloads", "Ctrl+J"),
    ("Reader view", "F9"),
    ("Read aloud in reader view", "F7"),
    ("Translate page", "F8"),
    ("Darken this site / keep its colors", "Shift+F9"),
    ("Split the stage", "Ctrl+\\"),
    ("Developer tools", "F12"),
    // The page's own keys, listed so nobody has to guess.
    ("Page: switch workspace", "Ctrl+Alt+1…4"),
    ("Page: open a link in a card", "Shift+click"),
];

/// The zoom sizes Ctrl++ steps through in Chromium, for the default-zoom slider.
const ZOOM_STOPS: &[u32] = &[25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500];

pub(super) fn settings_schema() -> Vec<(&'static str, Vec<Item>)> {
    use Item::*;
    vec![
        ("General", vec![DefaultBrowser]),
        ("Appearance", vec![
            Colors,
            Themes,
            Seg("theme", "Theme", "Dark, light, or whatever Windows is doing.", &[("dark", "Dark"), ("light", "Light"), ("system", "System")]),
            Toggle("dark_pages", "Darken light sites", "When Lumen is dark, Dark Reader repaints sites that have no dark mode. Shift+F9 skips a site."),
            Reset("colors", "Reset the colours"),
        ]),
        ("Fonts", vec![
            Fonts("ui_font", "Interface font", "Everything you act on: the sidebar, the pages, the settings."),
            Fonts("mono_font", "Monospace font", "The labels, the figures and the clock, in Signal and Precise."),
            Range("font_scale", "Type size", "Every size on every screen, together.", Scale::Linear(90.0, 130.0, 5.0), "%"),
            Reset("fonts", "Reset the fonts"),
        ]),
        ("Layout", vec![
            Layout,
            Toggle("bm_bar", "Bookmarks bar", "A row of your bookmarks under the toolbar. The horizontal layout."),
            Seg("sidebar_side", "Sidebar side", "Which edge the tab sidebar sits on.", &[("left", "Left"), ("right", "Right")]),
            Range("sidebar_width", "Sidebar width", "", Scale::Linear(232.0, 400.0, 4.0), "px"),
            Toggle("compact", "Compact", "The sidebar folds to icons; the toolbar and strip tighten up. Ctrl+B toggles it."),
            Reset("layout", "Reset the layout"),
        ]),
        ("Clock and new tab", vec![
            Toggle("clock", "Clock", ""),
            Toggle("clock_24h", "24-hour clock", "Off: 12-hour with am/pm."),
            Toggle("clock_seconds", "Seconds", "A small, quiet readout beside the minutes."),
            Seg("date_format", "Date", "", &DATE_FORMATS),
            Seg("clock_font", "Clock face", "The look's own, or the interface font at the same size.", &[("display", "The look's"), ("ui", "Interface font")]),
            Range("clock_size", "Clock size", "0 follows the window's width. px", Scale::Steps(CLOCK_SIZES), "px"),
            Text("greeting", "Greeting", "A line above the clock, in your own words. Empty: none."),
            Seg("ntp_bg", "Background", "Plain, or the wallpaper image below.", &[("plain", "Plain"), ("wallpaper", "Wallpaper")]),
            Wallpaper,
            Toggle("wallpaper_blur", "Blur the wallpaper", "Drawn from a small copy, enlarged: soft, and cheap."),
            Range("wallpaper_dim", "Dim the wallpaper", "", Scale::Linear(0.0, 80.0, 5.0), "%"),
            Toggle("speed_dial", "Speed dial", "Sites you keep as tiles under the search box."),
            Range("dial_cols", "Tiles per row", "0 follows the window's width.", Scale::Steps(DIAL_COLS), ""),
            Toggle("ntp_search", "Search box", ""),
            Reset("ntp", "Reset the clock and new tab page"),
        ]),
        ("Sidebar", vec![
            Range("tab_row_h", "Tab row height", "", Scale::Linear(28.0, 48.0, 1.0), "px"),
            Toggle("show_favicons", "Site icons", "The site's favicon in a tab row. Off: the letter tile only."),
            Seg("close_btn", "Tab close button", "", &CLOSE_BTNS),
            Toggle("show_memory", "Memory readout", "The figure in the footer, above the buttons."),
            Toggle("footer_btns", "Footer buttons", "Downloads, bookmarks and settings."),
            Toggle("ws_row", "Workspace row", "The switcher at the top of the sidebar."),
            Reset("sidebar", "Reset the sidebar"),
        ]),
        ("Shape", vec![
            Seg("density", "Density", "", &DENSITIES),
            Range("gap", "Page margin", "The space around the page.", Scale::Linear(0.0, 24.0, 1.0), "px"),
            Range("radius", "Page corners", "", Scale::Linear(0.0, 20.0, 1.0), "px"),
            Toggle("page_border", "Border around the page", "A hairline, so the page is not just a hole in the window."),
            Range("default_zoom", "Default zoom", "Pages open at this size unless the site has a zoom of its own. Ctrl++ and Ctrl+- still zoom the page you are on.", Scale::Steps(ZOOM_STOPS), "%"),
            Reset("shape", "Reset the shape"),
        ]),
        ("Motion", vec![
            Seg("motion", "Animations", "Reduced draws no movement; off also rests the loading spinner.", &MOTIONS),
            Range("motion_speed", "Motion speed", "How fast every animation runs, percent of the default.", Scale::Linear(50.0, 200.0, 10.0), "%"),
            Toggle("motion_system", "Follow Windows", "Obey \"Show animations in Windows\" in Accessibility: no motion when it is off."),
            Reset("motion", "Reset the motion"),
        ]),
        ("Search", vec![
            Engine,
            Toggle("remember_text", "Search history by page text", "Lumen keeps the words of pages you open, on this PC only, so the address bar finds them by what they said. Pages with a password box are skipped."),
        ]),
        ("Translation", vec![
            Pick("translate_to", "Translate pages into", "F8, or Translate page in the tab menu. The page's own language is read off the page; the model for the pair is fetched once, then it works offline.", crate::translate::TARGETS),
        ]),
        ("Performance", vec![
            Toggle("adblock", "Block ads and trackers", "Stops them before they load. Saves memory and data."),
            Toggle("cosmetic", "Hide cookie banners and ad leftovers", "Filter-list hiding rules, applied as plain CSS."),
            Toggle("sponsorblock", "Skip sponsors on YouTube", "Fetches hashed-prefix segments and the player skips them, with a toast."),
            Toggle("userscripts", "Run my userscripts", "Your *.user.js files in the userscripts folder of Lumen's data, in your own tabs. Never private or agent tabs. A new tab picks up a change."),
            Toggle("accelerate_downloads", "Accelerate downloads", "aria2 splits files into up to 16 connections, resumes, and queues. Falls back to the built-in downloader when a site refuses it."),
            Toggle("keep_seeding", "Share torrents back after they finish", "Until ratio 1.0 or one hour, whichever first. Applies on next start."),
            Range("download_limit_kbps", "Download speed limit", "Caps aria2, the download engine. 0 is no limit.", Scale::Steps(DL_SPEEDS), "kbps"),
            Range("sleep_after_min", "Freeze hidden tabs after", "Frozen tabs stop running and wake instantly.", Scale::Linear(1.0, 60.0, 1.0), "min"),
            Range("discard_after_min", "Unload hidden tabs after", "Unloaded tabs free all memory and reload when opened.", Scale::Linear(5.0, 240.0, 5.0), "min"),
            Toggle("battery_saver", "Battery saver", "On battery, or with Windows' battery saver on: hidden tabs freeze after 1 minute and unload after 10, and motion is reduced. A leaf in the footer says so."),
        ]),
        ("Passwords", vec![
            Toggle("save_passwords", "Offer to save passwords", "Saved by WebView2, encrypted by Windows with your sign-in. Fills them in next time."),
            Button("Saved passwords", "WebView2 cannot list them. You can wipe them all.", "Clear all", PageHit::ClearPasswords),
            KeePass,
        ]),
        ("Site permissions", vec![SitePerms]),
        ("AI agent", vec![
            Toggle("agent", "Let AI agents browse", "A local MCP server. Agents open their own tabs, marked AI, and you can stop them any time."),
            Toggle("agent_shared_profile", "Agents use my profile", "Off: agent tabs have their own cookies and logins. On: their own tabs carry your logins. Your tabs stay yours until you right-click one: Let agents use this tab."),
            Toggle("agent_strict_isolation", "Strict isolation: a browser per agent", "Each agent gets its own user-data folder and browser process: nothing shared at all. Measured: about 130 MB more memory and two more processes per agent. Off, agents still get their own profiles."),
            Toggle("agent_upload", "Let agents upload files", "Off: agents cannot put files into pages. On: only files in the Lumen agent uploads folder in your home folder (agent_upload_dir in settings.json changes it)."),
            Seg("drive_cmd", "Drive panel's command", "The agent CLI the Drive panel starts, with {task} where the task goes. Presets for the CLIs set up on this PC; any other command goes in drive_cmd in settings.json.", &[
                ("opencode run --model opencode/mimo-v2.6-flash-free \"{task}\"", "opencode"),
                ("cmdc -p \"{task}\"", "cmdc"),
                ("claude -p \"{task}\"", "claude"),
            ]),
            AgentSetup,
        ]),
        ("Privacy", vec![
            Seg("tracking", "Tracking prevention", "WebView2's own tracker blocking. Balanced blocks known trackers; Strict blocks most trackers, including ones a site loads at the top level. Applies to pages as they load.", &[("off", "Off"), ("basic", "Basic"), ("balanced", "Balanced"), ("strict", "Strict")]),
            Toggle("https_first", "Try https first", "A plain http address is tried over https first. When a site has no https at all, Lumen says so and lets you continue for this once or for good. Never happens for localhost, IP addresses or home-network names."),
            Toggle("gpc", "Tell sites not to sell or share my data", "Sends the Global Privacy Control signal (Sec-GPC: 1) and Do Not Track with every request, and tells the page itself through navigator.globalPrivacyControl. Sites that honour it must treat you as opted out."),
            Button("Site settings", "What each site may do, the zoom it is read at, the data it left behind, and the http addresses you let through.", "Open…", PageHit::Panels("sites")),
            Button("Clear browsing data", "Lumen history, page text and WebView2's own browsing history and downloads. Passwords and bookmarks are never touched.", "Clear…", PageHit::ClearData),
            Toggle("clear_on_exit", "Clear history when Lumen closes", "Lumen history, the words of pages and WebView2's own browsing history go when the browser closes. Private tabs leave nothing anyway."),
        ]),
        ("Keys", vec![Keys]),
        ("About", vec![
            Button(concat!("Lumen ", env!("LUMEN_VERSION")), "What changed in this version and every one before it, from the changelog built into Lumen.", "What's new", PageHit::Panels("changes")),
            Button("Report a problem", "A plain-text report you read in full before it goes anywhere: version, Windows, your setup, recent crashes and stalls. No addresses, no page titles, nothing typed into pages.", "Make a report…", PageHit::Panels("bugreport")),
        ]),
    ]
}

fn limits_schema() -> Vec<Item> {
    use Item::*;
    vec![
        Range("ram_limit_mb", "RAM limit", "Over the limit, the hidden tab looked at longest ago unloads.", Scale::Steps(RAM), "mb"),
        Range("cpu_limit_pct", "CPU limit", "Hard cap for the whole browser, share of all cores.", Scale::Linear(5.0, 100.0, 5.0), "%"),
        Range("net_limit_kbps", "Network limit", "Bandwidth cap for each tab. Downloads are not capped.", Scale::Steps(NET), "kbps"),
        Toggle("gpu_in_process", "Graphics in browser process", "About 55 MB less memory, but video is decoded on the CPU instead of the graphics card: YouTube costs about twice the CPU and drops frames. A driver crash restarts the browser. Applies after restart."),
        Toggle("share_processes", "Share processes within a site", "Far fewer processes. One heavy tab can slow its siblings. Applies after restart."),
    ]
}

/// The on/off switch that goes with a limit slider.
fn limit_toggle(key: &str) -> Option<&'static str> {
    match key {
        "ram_limit_mb" => Some("ram_limit"),
        "cpu_limit_pct" => Some("cpu_limit"),
        "net_limit_kbps" => Some("net_limit"),
        _ => None,
    }
}

// ------------------------------------------------------------------ values

fn get(s: &Settings, k: &str) -> Value {
    serde_json::to_value(s).ok().and_then(|v| v.get(k).cloned()).unwrap_or(Value::Null)
}

pub(super) fn set(i: &mut Inner, k: &str, v: Value) {
    let Some(st) = i.state.as_mut() else { return };
    let mut all = serde_json::to_value(&st.settings).unwrap_or(json!({}));
    if all.get(k) == Some(&v) {
        return;
    }
    all[k] = v;
    let dark = st.dark;
    if let Ok(next) = serde_json::from_value::<Settings>(all) {
        st.settings = next.clone();
        // The look follows at once, so a drag previews as it moves: the theme,
        // the fonts, the spacing and the motion all come from the one struct.
        let look = Look::of(&next, dark, super::motion::system_allows());
        i.theme = look.theme;
        i.look = look;
        send(&i.proxy, json!({"t": "settings", "v": next}));
    }
    i.invalidate();
}

fn fmt_value(v: f32, unit: &str) -> String {
    match unit {
        "mb" => {
            if v >= 1024.0 { format!("{} GB", trim(v / 1024.0)) } else { format!("{} MB", v as u32) }
        }
        "kbps" => {
            if v == 0.0 {
                "No limit".to_string()
            } else if v >= 1024.0 {
                format!("{} MB/s", trim(v / 1024.0))
            } else {
                format!("{} KB/s", v as u32)
            }
        }
        "%" => format!("{}%", v as u32),
        u => format!("{} {u}", v as u32),
    }
}

fn trim(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 { format!("{}", v.round() as u32) } else { format!("{v:.1}") }
}

// ------------------------------------------------------------------- clearing

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Days since 1970-01-01 for a civil date. Howard Hinnant's algorithm.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe as i64 - 719468
}

/// Unix seconds of local midnight today.
fn midnight() -> u64 {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    let today = days_from_civil(t.wYear as i32, t.wMonth as u32, t.wDay as u32) * 86_400;
    (today - local_offset()).max(0) as u64
}

/// Seconds local time is ahead of UTC. A negative number west of Greenwich.
fn local_offset() -> i64 {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    let wall = days_from_civil(t.wYear as i32, t.wMonth as u32, t.wDay as u32) * 86_400
        + t.wHour as i64 * 3600
        + t.wMinute as i64 * 60
        + t.wSecond as i64;
    wall - unix_now() as i64
}

/// The slice of history to clear: None = everything, Some(t) = from unix `t` on.
/// The menu asks, and `done` runs with the answer once a row is chosen.
fn ask_range(i: &mut Inner, done: impl FnOnce(&mut Inner, Option<u64>) + 'static) {
    let now = unix_now();
    let items: Vec<(u32, String)> = [(1u32, "Last hour"), (2, "Today"), (3, "Last 7 days"), (4, "Last 4 weeks"), (5, "Everything")]
        .into_iter()
        .map(|(id, label)| (id, label.to_string()))
        .collect();
    let (x, y) = i.mouse;
    super::menu::open(i, &items, x, y, move |i, cmd| {
        let since = match cmd {
            1 => now - 3600,
            2 => midnight(),
            3 => now - 7 * 86_400,
            4 => now - 28 * 86_400,
            5 => {
                done(i, None);
                return;
            }
            _ => return,
        };
        done(i, Some(since));
    });
}

fn ratio(scale: Scale, v: f32) -> f32 {
    match scale {
        Scale::Linear(lo, hi, _) => ((v - lo) / (hi - lo)).clamp(0.0, 1.0),
        Scale::Steps(steps) => {
            let i = steps.iter().enumerate().min_by_key(|(_, s)| (**s as f32 - v).abs() as u32).map_or(0, |(i, _)| i);
            i as f32 / (steps.len() - 1) as f32
        }
    }
}

fn value_at(scale: Scale, r: f32) -> f32 {
    let r = r.clamp(0.0, 1.0);
    match scale {
        Scale::Linear(lo, hi, step) => (lo + ((hi - lo) * r / step).round() * step).clamp(lo, hi),
        Scale::Steps(steps) => steps[(r * (steps.len() - 1) as f32).round() as usize] as f32,
    }
}

// -------------------------------------------------------------------- draw

pub fn draw(i: &mut Inner, st: R, s: &UiState) {
    // Each page starts at its top. The scroll offset used to survive a change of page, so a
    // long Settings page left Downloads and Control scrolled off the top of a short page: blank.
    if i.page.which != s.panel {
        i.page.which = s.panel;
        i.page.scroll = 0.0;
    }
    if s.panel == "settings" && i.page.themes.is_none() {
        i.page.themes = Some(style::saved_themes());
    }
    let first = i.hits.len();
    draw_page(i, st, s);
    // The content can shrink under the offset (a finished row cleared, a window made taller).
    // The height is only known after drawing, so draw once more from the clamped offset.
    if i.page.scroll > i.page.max {
        i.page.scroll = i.page.max;
        i.hits.truncate(first);
        i.gfx.round(st, i.look.page_radius, i.theme.bg);
        draw_page(i, st, s);
    }
    // The font list sits over everything while it is open.
    draw_picker(i, st, s);
}

fn draw_page(i: &mut Inner, st: R, s: &UiState) {
    i.page.sliders.clear();
    i.page.view = st;
    match s.panel {
        "settings" => draw_settings(i, st, s),
        "downloads" => draw_downloads(i, st, s),
        "history" => draw_library(i, st, s),
        "bookmarks" => draw_bookmarks(i, st, s),
        "drive" => draw_drive(i, st, s),
        "sites" => draw_sites(i, st, s),
        "report" => super::report::draw(i, st, s),
        "dashboard" => super::dashboard::draw(i, st, s),
        "changes" => draw_changes(i, st),
        "bugreport" => draw_bugreport(i, st, s),
        _ => draw_control(i, st, s),
    }
}

/// The Drive page's column: wide enough for the steps and the agent's words side by side.
const DRIVE_W: f32 = 880.0;
/// What the Run button takes at the right of the task card.
const DRIVE_RUN_W: f32 = 104.0;

/// The task card's text part, in stage coordinates. The native text box is placed over it,
/// so what is typed and what is drawn agree; Run sits in the rest of the card.
pub fn drive_task_rect(st: R, s5: f32) -> R {
    let col = column(st, DRIVE_W);
    R::new(col.x + 10.0, st.y + s5 + 2.0 + 74.0, col.w - DRIVE_RUN_W - 10.0, 56.0)
}

/// Text broken into lines no wider than `w`, at spaces; a word longer than a line is cut
/// by the drawing, not here.
pub(super) fn wrap_lines(i: &Inner, text: &str, f: super::gfx::Font, w: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let next = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && i.gfx.measure(&next, f) > w {
            out.push(std::mem::replace(&mut line, word.to_string()));
        } else {
            line = next;
        }
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// A step's time: milliseconds while short, tenths of a second after.
fn fmt_ms(ms: u64) -> String {
    if ms < 500 { format!("{ms} ms") } else { format!("{:.1} s", ms as f64 / 1000.0) }
}

/// A quiet button with a small square before its label, both in the live colour: Stop.
pub(super) fn stop_button(i: &mut Inner, x: f32, y: f32, label: &str, hit: PageHit) -> R {
    let th = i.theme;
    let f = i.look.body.align(Align::Center);
    let r = R::new(x, y, i.gfx.measure(label, f) + 2.0 * i.look.s4 + 18.0, 30.0);
    let h = Hit::Page(hit);
    let rad = i.look.round(30.0);
    i.gfx.round(r, rad, if i.is_hover(&h) { th.hover } else { th.raise });
    i.gfx.round_stroke(r, rad, th.line, 1.0);
    i.gfx.round_stroke(R::new(r.x + i.look.s4 - 2.0, r.y + 10.0, 10.0, 10.0), 2.5, th.accent, 1.6);
    i.gfx.text(label, R::new(r.x + 18.0, r.y, r.w - 18.0, r.h), f, th.accent);
    i.hit(r, h);
    r
}

/// The Drive panel: type a task, an agent runs it in Lumen. The steps are its tool calls in
/// plain words, the card beside them is what it said. Stop ends the run; "Pause and take
/// over" hands the tab to the person and only browser_handoff gets the agent back in.
fn draw_drive(i: &mut Inner, st: R, s: &UiState) {
    let lo = i.look.clone();
    let th = i.theme;
    let col = column(st, DRIVE_W);
    i.gfx.clip(st);
    let running = s.drive.as_ref().is_some_and(|d| d.running);
    let paused = s.drive.as_ref().is_some_and(|d| d.paused);
    let sub = match &s.drive {
        Some(d) if d.running && d.paused => format!("{} paused · the tab is yours", d.name),
        Some(d) if d.running => format!("{} working · {}", d.name, d.elapsed),
        Some(d) => match d.exit {
            Some(c) if c != 0 => format!("{} finished · exit code {c}", d.name),
            _ => format!("{} finished", d.name),
        },
        None => "An agent CLI runs your task in Lumen".to_string(),
    };
    let title_y = st.y + lo.s5 + 2.0;
    title_block(i, col, title_y, "Drive", &sub);
    // While a run goes, its two ways out sit at the right of the title.
    if running {
        let f = lo.body.align(Align::Center);
        let stop_w = i.gfx.measure("Stop", f) + 2.0 * lo.s4 + 18.0;
        let x = col.right() - 48.0 - stop_w;
        stop_button(i, x, title_y + 22.0, "Stop", PageHit::DriveStop);
        if !paused {
            let w = i.gfx.measure("Pause and take over", f) + 2.0 * lo.s4;
            outline_button(i, x - lo.s2 - w, title_y + 22.0, "Pause and take over", PageHit::DrivePause, false);
        }
    }

    // The task card. The text box itself is a real Win32 edit placed over its left part.
    let field = drive_task_rect(st, lo.s5);
    let card = R::new(col.x, field.y, col.w, field.h);
    let focused = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } == i.drive_edit;
    i.gfx.round(card, lo.r_card, th.raise);
    i.gfx.round_stroke(card, lo.r_card, if focused { th.text.alpha(0.45) } else { th.line }, if focused { 1.5 } else { 1.0 });
    let f = lo.body_strong.align(Align::Center);
    let run_w = i.gfx.measure("Run", f) + 2.0 * lo.s4;
    let run = outline_button(i, card.right() - 12.0 - run_w, card.y + (card.h - 30.0) / 2.0, "Run", PageHit::DriveStart, true);
    if running {
        // One run at a time: Run waits, quietly, until this one is over.
        i.gfx.round(run, lo.round(run.h), th.raise.alpha(0.55));
    }
    // Under the card, the run scrolls: clipped there, so it never slides over the task box.
    let below = R::new(st.x, card.bottom() + 2.0, st.w, (st.bottom() - card.bottom() - 2.0).max(0.0));
    i.gfx.clip(below);
    let mut y = card.bottom() + lo.s4 - i.page.scroll;

    match &s.drive {
        None => {
            let hint = "Give it a task and press Run. It browses in its own agent tabs, in a profile of its own: it never touches your tabs or your logins. Everything it downloads lands in \"Agent downloads\" under its own name.";
            y = empty_state(i, R::new(col.x, y + lo.s4, col.w, 170.0), "send", "Nothing has run yet", hint) + lo.s4;
        }
        Some(d) => {
            // Left: the steps, numbered, in plain words, each with its time; the one going
            // now says so in the live colour. Right: what the agent said, in its own words.
            let gap = 14.0;
            let lw = ((col.w - gap) * 0.53).floor();
            let left = R::new(col.x, y, lw, 0.0);
            let right = R::new(col.x + lw + gap, y, col.w - lw - gap, 0.0);
            i.label(&format!("{} · {}", d.name, d.elapsed), R::new(left.x + 4.0, y, left.w, 18.0), th.faint);
            i.label("What it says", R::new(right.x + 4.0, y, right.w, 18.0), th.faint);
            y += 26.0;
            const ROW: f32 = 44.0;
            let rows = d.steps.len().max(1);
            let steps = R::new(left.x, y, left.w, rows as f32 * ROW);
            let mono = lo.mono_at(11.5, 450);
            let lines: Vec<String> = d.log.iter().flat_map(|l| wrap_lines(i, l, mono, right.w - 32.0)).collect();
            let words_h = 28.0 + 20.0 * lines.len().max(1) as f32;
            let h = steps.h.max(words_h).max(4.0 * ROW);
            let words = R::new(right.x, y, right.w, h);
            for c in [steps, words] {
                i.gfx.round(c, lo.r_card, th.raise);
                i.gfx.round_stroke(c, lo.r_card, th.line, 1.0);
            }
            if d.steps.is_empty() {
                i.gfx.text("No tool calls yet.", R::new(steps.x + 18.0, y, steps.w - 36.0, ROW), lo.body, th.faint);
            }
            let last = d.steps.len().saturating_sub(1);
            for (n, (what, ms)) in d.steps.iter().enumerate() {
                let r = R::new(steps.x, y + n as f32 * ROW, steps.w, ROW);
                if r.bottom() < below.y || r.y > st.bottom() {
                    continue;
                }
                if n > 0 {
                    i.gfx.line(r.x + 1.0, r.y, r.right() - 1.0, r.y, th.line, 1.0);
                }
                let now = running && n == last && ms.is_none();
                let time = match ms {
                    Some(ms) => fmt_ms(*ms),
                    None if now => "NOW".to_string(),
                    None => "—".to_string(),
                };
                let tw = i.gfx.measure(&time, mono) + 4.0;
                i.gfx.text(&format!("{:02}", n + 1), R::new(r.x + 16.0, r.y, 30.0, ROW), mono, th.faint);
                i.gfx.text(what, R::new(r.x + 52.0, r.y, (r.w - 52.0 - 18.0 - tw - 8.0).max(40.0), ROW), lo.body, th.text);
                i.gfx.text(&time, R::new(r.right() - 16.0 - tw, r.y, tw, ROW), mono.align(Align::Right), if now { th.accent } else { th.faint });
            }
            let mut ly = words.y + 14.0;
            if d.log.is_empty() {
                i.gfx.text("Nothing yet.", R::new(words.x + 16.0, ly, words.w - 32.0, 20.0), mono, th.faint);
            }
            for line in &lines {
                i.gfx.text(line, R::new(words.x + 16.0, ly, words.w - 32.0, 20.0), mono, th.dim);
                ly += 20.0;
            }
            y += h + lo.s5;
        }
    }
    // What runs, and the rule it runs under.
    i.gfx.text(&format!("Command: {}", s.settings.drive_cmd), R::new(col.x + 4.0, y, col.w, 20.0), lo.small, th.faint);
    i.gfx.text("Safety: the task Lumen sends tells the agent to call browser_handoff before paying, sending or deleting.", R::new(col.x + 4.0, y + 20.0, col.w, 20.0), lo.small, th.faint);
    y += 44.0;
    i.gfx.unclip();
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

/// A setting's name and its hint, on the left. `control_w` is what the control on the right
/// takes, so a long hint wraps to a second line instead of running under it. Returns the
/// row's height: 54, or taller when the hint needs more lines.
fn row_label(i: &Inner, r: R, label: &str, hint: &str, control_w: f32) -> f32 {
    let th = i.theme;
    let avail = (r.w - control_w - i.look.s5).max(r.w * 0.4);
    if hint.is_empty() {
        i.gfx.text(label, R::new(r.x, r.y, avail, 54.0), i.look.font(13.0, 500), th.text);
        return 54.0;
    }
    i.gfx.text(label, R::new(r.x, r.y + 8.0, avail, 20.0), i.look.font(13.0, 500), th.text);
    // Words do not fill a line to the last pixel; ask for a little more room than the sum.
    let lines = ((i.gfx.measure(hint, i.look.small) / (avail * 0.94)).ceil() as usize).clamp(1, 3);
    if lines == 1 {
        i.gfx.text(hint, R::new(r.x, r.y + 27.0, avail, 18.0), i.look.small, th.faint);
    } else {
        i.gfx.text_wrap(hint, R::new(r.x, r.y + 28.0, avail, 16.0 * lines as f32 + 2.0), i.look.small, th.faint);
    }
    28.0 + 16.0 * lines as f32 + 10.0
}

/// How wide a segmented control is.
fn seg_width(i: &Inner, opts: &[(&str, &str)]) -> f32 {
    let f = i.look.font(13.0, 450).align(Align::Center);
    opts.iter().map(|(_, l)| i.gfx.measure(l, f) + 22.0).sum::<f32>() + 4.0
}

fn draw_seg(i: &mut Inner, right: f32, cy: f32, key: &'static str, opts: &'static [(&'static str, &'static str)], cur: &str) {
    let th = i.theme;
    let f = i.look.font(13.0, 450).align(Align::Center);
    let widths: Vec<f32> = opts.iter().map(|(_, l)| i.gfx.measure(l, f) + 22.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let pill = R::new(right - total, cy - 15.0, total, 30.0);
    let lo = i.look.clone();
    i.gfx.round(pill, lo.round(30.0), th.bg);
    i.gfx.round_stroke(pill, lo.round(30.0), th.line, 1.0);
    let mut x = pill.x + 2.0;
    for ((v, l), w) in opts.iter().zip(widths) {
        let r = R::new(x, pill.y + 2.0, w, 26.0);
        let on = *v == cur;
        let h = Hit::Page(PageHit::Seg(key, v));
        if on {
            i.gfx.round(r, lo.round(26.0), th.pill);
        } else if i.is_hover(&h) {
            i.gfx.round(r, lo.round(26.0), th.hover);
        }
        let c = if on { th.pill_ink } else if i.is_hover(&h) { th.text } else { th.faint };
        i.gfx.text(l, r, f, c);
        i.hit(r, h);
        x += w;
    }
}

fn draw_toggle(i: &mut Inner, right: f32, cy: f32, key: &'static str, on: bool, enabled: bool) {
    let r = R::new(right - 38.0, cy - 11.0, 38.0, 22.0);
    let hit = enabled.then(|| Hit::Page(PageHit::Toggle(key)));
    i.switch(r, on, key, if enabled { 1.0 } else { 0.35 }, hit);
}

fn draw_slider(i: &mut Inner, right: f32, cy: f32, key: &'static str, scale: Scale, unit: &str, v: f32, dim: bool) {
    let th = i.theme;
    let val = R::new(right - 64.0, cy - 10.0, 64.0, 20.0);
    i.gfx.text(&fmt_value(v, unit), val, i.look.font(12.0, 450).align(Align::Right), if dim { th.faint } else { th.dim });
    let track = R::new(val.x - 12.0 - 160.0, cy - 2.0, 160.0, 4.0);
    let p = ratio(scale, v);
    i.gfx.round(track, 2.0, th.line);
    i.gfx.round(R::new(track.x, track.y, track.w * p, 4.0), 2.0, if dim { th.faint } else { th.text });
    i.gfx.circle(track.x + track.w * p, cy, 8.0, th.raise);
    i.gfx.ring(track.x + track.w * p, cy, 8.0, 8.0, th.text.alpha(if dim { 0.4 } else { 0.85 }), 1.5);
    let grab = R::new(track.x - 8.0, cy - 12.0, track.w + 16.0, 24.0);
    i.hit(grab, Hit::Page(PageHit::Slider(key)));
    i.page.sliders.push((key, track, scale));
}

/// The family a font setting names, in words a person reads.
fn font_shown(key: &str) -> String {
    match key {
        "segoe" => "Segoe UI (Windows)".into(),
        "instrument" => "Instrument Sans".into(),
        "figtree" => "Figtree".into(),
        "atkinson" => "Atkinson Hyperlegible".into(),
        "fraunces" => "Fraunces".into(),
        "newsreader" => "Newsreader".into(),
        "cascadia" => "Cascadia Mono".into(),
        other => other.to_string(),
    }
}

/// The key a font setting falls back to when the person picks "System default".
fn font_default_key(key: &str) -> &'static str {
    match key {
        "display_font" => "instrument",
        "mono_font" => "cascadia",
        _ => "segoe",
    }
}

/// The colour a role has right now: the custom one, or the palette's own.
fn role_color(i: &Inner, k: &str, s: &UiState) -> Color {
    let custom = get(&s.settings, k).as_str().unwrap_or("").to_string();
    if let Some(c) = Color::from_hex(&custom) {
        return c;
    }
    let th = i.theme;
    match k {
        "bg_color" => th.bg,
        "side_color" => th.side,
        "surface_color" => th.raise,
        "text_color" => th.text,
        _ => th.dim,
    }
}

/// The Colours group: the five palettes, the accent, then the custom colours.
fn draw_colors(i: &mut Inner, r: R, s: &UiState) -> f32 {
    let th = i.theme;
    let right = r.right();
    let mut y = r.y;
    let h = row_label(i, R::new(r.x, y, r.w, 70.0), "Look", "Three whole looks. Each sets colours, type, corners and motion together.", 3.0 * 100.0);
    let mut x = right - 3.0 * 100.0 + 8.0;
    for (key, name) in crate::store::LOOKS {
        let t = R::new(x, y + (h - 60.0) / 2.0, 92.0, 60.0);
        look_thumb(i, t, key, name, s.settings.look == key);
        i.hit(t, Hit::Page(PageHit::Seg("look", key)));
        x += 100.0;
    }
    y += h.max(70.0);
    // The accent, and the way back to the look's own.
    let h = row_label(i, R::new(r.x, y, r.w, 40.0), "Accent", "One colour, for what is live: a working agent, a running download, the clock's colon.", 110.0);
    let cy = y + h / 2.0;
    let sw = R::new(right - 28.0, cy - 14.0, 28.0, 28.0);
    i.gfx.round(sw, i.look.r_md, th.accent);
    i.gfx.round_stroke(sw, i.look.r_md, th.line, 1.0);
    i.hit(sw, Hit::Page(PageHit::Accent));
    if !s.settings.accent.is_empty() {
        let reset = R::new(sw.x - 56.0, cy - 10.0, 48.0, 20.0);
        let hh = self_hover(i, PageHit::AccentReset);
        i.gfx.text("Clear", reset, i.look.small.align(Align::Right), if hh { th.text } else { th.faint });
        i.hit(reset, Hit::Page(PageHit::AccentReset));
    }
    y += h;
    // The custom colours: each shows the colour in use, custom or palette's own.
    const ROLES: [(&str, &str); 5] = [
        ("bg_color", "Background"),
        ("side_color", "Sidebar and toolbar"),
        ("surface_color", "Cards and fields"),
        ("text_color", "Text"),
        ("dim_color", "Dim text"),
    ];
    for (k, label) in ROLES {
        let custom = get(&s.settings, k).as_str().unwrap_or("").to_string();
        let hint = if custom.is_empty() { "The palette's own".to_string() } else { custom.clone() };
        let h = row_label(i, R::new(r.x, y, r.w, 40.0), label, &hint, 120.0);
        let cy = y + h / 2.0;
        let chip = R::new(right - 28.0, cy - 14.0, 28.0, 28.0);
        let c = role_color(i, k, s);
        let hh = self_hover(i, PageHit::Color(k));
        i.gfx.round(chip, i.look.r_md, c);
        i.gfx.round_stroke(chip, i.look.r_md, if hh { th.accent } else { th.line }, 1.0);
        i.hit(chip, Hit::Page(PageHit::Color(k)));
        if !custom.is_empty() {
            let clear = R::new(chip.x - 56.0, cy - 10.0, 48.0, 20.0);
            let ch = self_hover(i, PageHit::Color(""));
            i.gfx.text("Clear", clear, i.look.small.align(Align::Right), if ch { th.text } else { th.faint });
            // One click target: the row's Clear clears this colour.
            i.hit(clear, Hit::Page(PageHit::ColorClear(k)));
        }
        y += h;
    }
    y - r.y
}

/// A look's small picture: its ground, its search bar, its cards, its name.
pub(super) fn look_thumb(i: &mut Inner, t: R, key: &str, name: &str, on: bool) {
    let th = i.theme;
    let hx = |h: &str| super::theme::Color::from_hex(h).unwrap_or(th.text);
    let (bg, bar, card, ink) = match key {
        "glass" => (hx("#211a3a"), super::theme::Color(1.0, 1.0, 1.0, 0.16), super::theme::Color(1.0, 1.0, 1.0, 0.10), hx("#ffffff")),
        "precise" => (hx("#0b0b0d"), hx("#16161a"), hx("#111114"), hx("#ececef")),
        _ => (hx("#f1efe9"), hx("#141414"), hx("#ffffff"), hx("#141414")),
    };
    i.gfx.round(t, 12.0, bg);
    if key == "glass" {
        // Its wallpaper, in two soft blooms, kept inside the picture.
        i.gfx.clip(t.inset(2.0, 2.0));
        i.gfx.bloom(t.x + t.w * 0.12, t.y, t.w * 0.7, hx("#5a43e6").alpha(0.9));
        i.gfx.bloom(t.right(), t.bottom(), t.w * 0.6, hx("#d2457e").alpha(0.8));
        i.gfx.unclip();
    }
    let k = (t.w / 92.0).max(1.0);
    let pad = 8.0 * k;
    let mut y = t.y + pad;
    // A big picture shows each look's clock, the thing that tells them apart at a glance.
    if t.h > 100.0 {
        let size = 34.0 * k.min(1.6);
        let (face, colon) = match key {
            "glass" => (i.look.font(size, 300), ink.alpha(0.55)),
            "precise" => (super::gfx::Font { face: super::gfx::Face::Mono, ..i.look.font(size * 0.8, 600) }, hx("#8b7bff")),
            _ => (super::gfx::Font { face: super::gfx::Face::Mono, ..i.look.font(size, 700) }, hx("#e8322b")),
        };
        let (hh, mm) = ("08", "23");
        let wh = i.gfx.measure(hh, face);
        let wc = i.gfx.measure(":", face);
        let row = R::new(t.x + pad, y, t.w, size * 1.15);
        i.gfx.text(hh, row, face, ink);
        i.gfx.text(":", R::new(row.x + wh, row.y, wc + 4.0, row.h), face, colon);
        i.gfx.text(mm, R::new(row.x + wh + wc, row.y, t.w, row.h), face, ink);
        y += size * 1.25;
    }
    let bar_h = if t.h > 100.0 { 18.0 } else { 9.0 };
    i.gfx.round(R::new(t.x + pad, y, t.w - 2.0 * pad, bar_h), bar_h / 2.0, bar);
    y += bar_h + 5.0 * k;
    let cw = (t.w - 2.0 * pad) / 3.0;
    let ch = if t.h > 100.0 { 34.0 } else { 13.0 };
    for n in 0..3 {
        i.gfx.round(R::new(t.x + pad + n as f32 * cw, y, cw - 4.0, ch), 4.0 * k.min(2.0), card);
    }
    if key == "precise" {
        i.gfx.round_stroke(t, 12.0, hx("#8b7bff").alpha(0.8), 1.0);
    }
    i.gfx.text(name, R::new(t.x + 8.0, t.bottom() - 19.0, t.w - 12.0, 16.0), i.look.font(10.5, 600), ink);
    if on {
        i.gfx.round_stroke(t.inset(-3.0, -3.0), 15.0, th.text, 2.0);
    } else {
        i.gfx.round_stroke(t, 12.0, th.line, 1.0);
    }
}

/// The first thing a new person sees: pick a look. Signal is chosen; Start keeps it.
pub fn draw_first_run(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let lo = i.look.clone();
    i.gfx.fill(st, th.bg);
    let a = i.motion.enter(super::motion::Id::Named("firstrun"), super::motion::Prop::Opacity, 0.0, 1.0, 260, super::motion::Ease::OutCubic);
    let w = (st.w - 96.0).clamp(300.0, 980.0);
    let x0 = st.x + (st.w - w) / 2.0;
    let card_w = (w - 2.0 * 18.0) / 3.0;
    let card_h = 250.0;
    let top = st.y + (st.h - (120.0 + card_h + 80.0)) / 2.0;
    i.gfx.text("Pick a look", R::new(x0, top, w, 44.0), lo.font(34.0, 700).align(Align::Center), th.text.alpha(a));
    i.gfx.text(
        "Each one is a whole Lumen: colours, type, corners and motion. Change it any time in Settings.",
        R::new(x0, top + 50.0, w, 20.0),
        lo.font(13.0, 400).align(Align::Center),
        th.faint.alpha(a),
    );
    let blurb = [("signal", "Paper and ink, one red signal. Light and dark."), ("glass", "Frosted, soft, over a colour wallpaper."), ("precise", "Sharp and dense, one violet accent.")];
    for (n, (key, name)) in crate::store::LOOKS.iter().enumerate() {
        let r = R::new(x0 + n as f32 * (card_w + 18.0), top + 100.0, card_w, card_h);
        let on = s.settings.look == *key;
        let h = Hit::Page(PageHit::Seg("look", key));
        let hov = i.is_hover(&h);
        i.gfx.round(r, lo.r_card, th.raise.alpha(a));
        i.gfx.round_stroke(r, lo.r_card, if on { th.text } else if hov { th.text.alpha(0.4) } else { th.line }, if on { 2.0 } else { 1.0 });
        look_thumb(i, R::new(r.x + 10.0, r.y + 10.0, r.w - 20.0, 170.0), key, "", false);
        i.gfx.text(name, R::new(r.x + 16.0, r.y + 190.0, r.w - 32.0, 22.0), lo.font(15.0, 700), th.text.alpha(a));
        i.gfx.text(blurb[n].1, R::new(r.x + 16.0, r.y + 214.0, r.w - 32.0, 18.0), lo.font(12.0, 400), th.faint.alpha(a));
        i.hit(r, h);
    }
    let name = crate::store::LOOKS.iter().find(|(k, _)| *k == s.settings.look).map_or("Signal", |(_, n)| n);
    let label = format!("Start with {name}");
    let f = lo.body_strong.align(Align::Center);
    let bw = i.gfx.measure(&label, f) + 2.0 * lo.s5;
    let note = "Lumen follows Windows' light or dark setting";
    let nf = lo.font(12.0, 400);
    let nw = i.gfx.measure(note, nf) + 14.0;
    let by = top + 100.0 + card_h + 30.0;
    let bx = st.x + (st.w - (nw + bw)) / 2.0 + nw;
    i.gfx.text(note, R::new(bx - nw, by, nw - 12.0, 34.0), nf.align(Align::Right), th.faint.alpha(a));
    let b = R::new(bx, by, bw, 34.0);
    let h = Hit::Page(PageHit::FirstRunDone);
    i.gfx.round(b, lo.round(34.0), if i.is_hover(&h) { th.pill } else { th.pill.alpha(0.92) });
    i.gfx.text(&label, b, f, th.pill_ink);
    i.hit(b, h);
}

/// A site's icon, the one Lumen has kept for it, or its letter on a quiet tile.
pub(super) fn site_icon(i: &mut Inner, r: R, host: &str) {
    let th = i.theme;
    let key = super::site_key(host);
    let png = i.fav_png(host, false);
    match png.and_then(|p| i.gfx.bitmap(key, &p)) {
        Some(b) => i.gfx.draw_bitmap(&b, r, 1.0),
        None => {
            i.gfx.round(r, 5.0, th.hover);
            let letter = host.trim_start_matches("www.").chars().next().unwrap_or('·').to_uppercase().to_string();
            i.gfx.text(&letter, r, i.look.font(r.h * 0.55, 700).align(Align::Center), th.dim);
        }
    }
}

/// A row's hover, for the small text actions.
fn self_hover(i: &Inner, h: PageHit) -> bool {
    i.is_hover(&Hit::Page(h))
}

/// The saved looks: save the current one, apply or remove one, export and import.
fn draw_themes(i: &mut Inner, r: R, s: &UiState) -> f32 {
    let _ = s;
    let th = i.theme;
    let right = r.right();
    let mut y = r.y;
    i.gfx.text("Saved themes", R::new(r.x, y, r.w, 30.0), i.look.body_strong, th.text);
    // Right to left: Export, Import, Save.
    let save_w = i.gfx.measure("Save this look…", i.look.body.align(Align::Center)) + 2.0 * i.look.s4;
    let export_w = i.gfx.measure("Export…", i.look.body.align(Align::Center)) + 2.0 * i.look.s4;
    let import_w = i.gfx.measure("Import…", i.look.body.align(Align::Center)) + 2.0 * i.look.s4;
    let save = outline_button(i, right - save_w, y - 2.0, "Save this look…", PageHit::ThemeSave, true).x;
    let export = outline_button(i, save - i.look.s2 - export_w, y - 2.0, "Export…", PageHit::ThemeExport, false).x;
    outline_button(i, export - i.look.s2 - import_w, y - 2.0, "Import…", PageHit::ThemeImport, false);
    y += 38.0;
    // The chips: click applies, the small × removes.
    let list = i.page.themes.clone().unwrap_or_default();
    if list.is_empty() {
        i.gfx.text("No saved looks yet. Set the colours and fonts you like, then Save.", R::new(r.x, y, r.w, 24.0), i.look.small, th.faint);
        return y + 30.0 - r.y;
    }
    let f = i.look.font(12.5, 500);
    let mut x = r.x;
    for (n, t) in list.iter().enumerate() {
        let name = if t.name.is_empty() { "Untitled".to_string() } else { t.name.clone() };
        let w = (i.gfx.measure(&name, f) + 52.0).min(220.0);
        if x + w > right {
            x = r.x;
            y += 34.0;
        }
        let chip = R::new(x, y, w, 28.0);
        let hit = Hit::Page(PageHit::ThemeApply(n));
        let hov = i.is_hover(&hit);
        i.gfx.round(chip, i.look.r_md, if hov { th.hover } else { th.bg });
        i.gfx.round_stroke(chip, i.look.r_md, th.line, 1.0);
        i.gfx.text(&name, R::new(chip.x + 8.0, chip.y, chip.w - 34.0, chip.h), f, if hov { th.text } else { th.dim });
        i.hit(chip, hit);
        let xr = R::new(chip.right() - 24.0, chip.y + 4.0, 20.0, 20.0);
        let xh = self_hover(i, PageHit::ThemeDelete(n));
        i.gfx.icon("close", xr.x + 3.0, xr.y + 3.0, 14.0, if xh { th.text } else { th.faint });
        i.hit(xr, Hit::Page(PageHit::ThemeDelete(n)));
        x += w + 6.0;
    }
    y + 38.0 - r.y
}

/// The Layout group: the three arrangements, each drawn as the shape it makes.
fn draw_layout(i: &mut Inner, r: R, s: &UiState) -> f32 {
    let th = i.theme;
    let mut y = r.y;
    i.gfx.text_wrap(
        "Where the chrome lives. Switching moves the chrome and the page at once; nothing reloads.",
        R::new(r.x, y, r.w, 34.0),
        i.look.small,
        th.faint,
    );
    y += 40.0;
    let cur = s.settings.layout.clone();
    let card_w = 168.0;
    let card_h = 108.0;
    let gap = 12.0;
    for (n, (key, label)) in LAYOUTS.iter().enumerate() {
        let cr = R::new(r.x + n as f32 * (card_w + gap), y, card_w, card_h);
        let hit = Hit::Page(PageHit::Seg("layout", key));
        let on = *key == cur;
        let hov = i.is_hover(&hit);
        i.gfx.round(cr, i.look.r_lg, if on { th.press } else if hov { th.hover } else { th.bg });
        i.gfx.round_stroke(cr, i.look.r_lg, if on { th.accent } else { th.line.alpha(if hov { 1.0 } else { 0.6 }) }, if on { 1.5 } else { 1.0 });
        draw_layout_shape(i, R::new(cr.x + 16.0, cr.y + 12.0, cr.w - 32.0, card_h - 46.0), key);
        i.gfx.text(label, R::new(cr.x, cr.bottom() - 26.0, cr.w, 20.0), i.look.font(12.0, if on { 600 } else { 450 }).align(Align::Center), if on { th.text } else { th.dim });
        i.hit(cr, hit);
    }
    y + card_h + 12.0 - r.y
}

/// The little diagram in a layout card: the chrome's strips and the page.
fn draw_layout_shape(i: &mut Inner, a: R, kind: &str) {
    let th = i.theme;
    let chrome = th.accent.alpha(0.8);
    let dim = th.accent.alpha(0.4);
    // The page: a step off the card, so the shape reads at a glance.
    let page = th.raise.mix(th.text, 0.06);
    let bar = (a.h * 0.16).max(6.0);
    let strip = (a.w * 0.26).max(10.0);
    match kind {
        "horizontal" => {
            i.gfx.round(R::new(a.x, a.y, a.w, bar), 2.0, chrome);
            i.gfx.round(R::new(a.x, a.y + bar + 3.0, a.w, bar * 0.8), 2.0, dim);
            i.gfx.round(R::new(a.x, a.y + bar * 2.2, a.w, a.h - bar * 2.2), 3.0, page);
        }
        "hybrid" => {
            i.gfx.round(R::new(a.x, a.y, a.w, bar), 2.0, chrome);
            i.gfx.round(R::new(a.x, a.y + bar + 3.0, strip * 0.7, a.h - bar - 3.0), 2.0, dim);
            i.gfx.round(R::new(a.x + strip * 0.7 + 3.0, a.y + bar + 3.0, a.w - strip * 0.7 - 3.0, a.h - bar - 3.0), 3.0, page);
        }
        _ => {
            i.gfx.round(R::new(a.x, a.y, strip, a.h), 2.0, chrome);
            i.gfx.round(R::new(a.x + strip + 3.0, a.y, a.w - strip - 3.0, a.h), 3.0, page);
        }
    }
}

/// The font list, over the settings page: a filter anyone can type into, and the
/// families of this PC under it.
pub(super) fn draw_picker(i: &mut Inner, st: R, s: &UiState) {
    let Some(pick) = i.page.font_pick.as_ref() else { return };
    let _ = s;
    let th = i.theme;
    let key = pick.key;
    let filter = pick.filter.clone();
    let scroll = pick.scroll;
    let label = match key {
        "display_font" => "Display font",
        "mono_font" => "Monospace font",
        _ => "Interface font",
    };
    // Anywhere else closes it.
    i.hit(st, Hit::Page(PageHit::FontClose));
    let card = R::new(st.x + (st.w - 540.0) / 2.0, st.y + 36.0, 540.0_f32.min((st.w - 40.0).max(200.0)), (st.h - 96.0).max(200.0));
    i.gfx.round(card.inset(-2.0, -2.0), i.look.r_lg + 2.0, th.shade.alpha(0.5));
    i.gfx.round(card, i.look.r_lg, th.raise);
    i.gfx.round_stroke(card, i.look.r_lg, th.accent.alpha(0.4), 1.0);
    i.gfx.text(label, R::new(card.x + 16.0, card.y + 8.0, card.w - 60.0, 24.0), i.look.body_strong, th.text);
    let xr = R::new(card.right() - 40.0, card.y + 8.0, 28.0, 24.0);
    let xh = self_hover(i, PageHit::FontClose);
    i.gfx.icon("close", xr.x + 6.0, xr.y + 4.0, 16.0, if xh { th.text } else { th.dim });
    i.hit(xr, Hit::Page(PageHit::FontClose));
    // The filter: type, backspace, and the list follows.
    let field = R::new(card.x + 16.0, card.y + 40.0, card.w - 32.0, 32.0);
    i.gfx.round(field, i.look.r_md, th.bg);
    i.gfx.round_stroke(field, i.look.r_md, th.acc_soft, 1.5);
    let caret = (i.born.elapsed().as_secs_f32() * 1.6).fract() < 0.5;
    let shown_text = if filter.is_empty() { "Type to search".to_string() } else { format!("{filter}{}", if caret { "|" } else { " " }) };
    i.gfx.text(&shown_text, R::new(field.x + 10.0, field.y, field.w - 20.0, field.h), i.look.font(13.0, 450), if filter.is_empty() { th.faint } else { th.text });
    // The list: the default first, then every family the filter lets through.
    let list = R::new(card.x + 8.0, field.bottom() + 8.0, card.w - 16.0, card.bottom() - field.bottom() - 20.0);
    i.gfx.clip(list);
    let row_h = 26.0;
    let f = i.look.font(13.0, 450);
    let words: Vec<String> = filter.split_whitespace().map(|w| w.to_lowercase()).collect();
    let mut shown: Vec<String> = Vec::new();
    let mut y = list.y + 2.0 - scroll;
    // Row 0 is always the way back to the default.
    let default_row = R::new(list.x + 4.0, y, list.w - 8.0, row_h);
    let dh = self_hover(i, PageHit::FontPin(0));
    if y + row_h > list.y && y < list.bottom() {
        if dh {
            i.gfx.round(default_row, i.look.r_sm, th.hover);
        }
        i.gfx.text("Windows default", R::new(default_row.x + 8.0, default_row.y, default_row.w - 16.0, row_h), f, th.dim);
        i.hit(default_row, Hit::Page(PageHit::FontPin(0)));
    }
    y += row_h;
    let mut rows = 0usize;
    let fonts = i.fonts.clone();
    for fam in fonts.iter() {
        let low = fam.to_lowercase();
        if !words.iter().all(|w| low.contains(w.as_str())) {
            continue;
        }
        let rr = R::new(list.x + 4.0, y, list.w - 8.0, row_h);
        y += row_h;
        rows += 1;
        shown.push(fam.clone());
        if rr.bottom() < list.y || rr.y > list.bottom() {
            continue;
        }
        let hit = Hit::Page(PageHit::FontPin(shown.len()));
        let hov = i.is_hover(&hit);
        if hov {
            i.gfx.round(rr, i.look.r_sm, th.hover);
        }
        i.gfx.text(fam, R::new(rr.x + 8.0, rr.y, rr.w - 16.0, row_h), f, if hov { th.text } else { th.dim });
        i.hit(rr, hit);
    }
    i.gfx.unclip();
    if rows == 0 {
        i.gfx.text("No font here has that name.", R::new(list.x, list.y + 8.0, list.w, 24.0), i.look.small, th.faint);
    }
    // The filter changes what a click means: remember what was shown.
    if let Some(p) = i.page.font_pick.as_mut() {
        p.shown = shown;
    }
}

fn draw_item(i: &mut Inner, it: &Item, r: R, s: &UiState) -> f32 {
    let th = i.theme;
    let right = r.right();
    match it {
        Item::Colors => draw_colors(i, r, s),
        Item::Themes => draw_themes(i, r, s),
        Item::Layout => draw_layout(i, r, s),
        Item::Fonts(k, label, hint) => {
            let cur = get(&s.settings, k).as_str().unwrap_or("").to_string();
            let bw = i.gfx.measure("Choose…", i.look.body.align(Align::Center)) + 2.0 * i.look.s4;
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint, bw + 150.0);
            let cy = r.y + h / 2.0;
            // The family in use, right up against the button that changes it.
            let shown = font_shown(&cur);
            let name_w = (i.gfx.measure(&shown, i.look.font(12.5, 500)) + 16.0).min(r.w * 0.5);
            i.gfx.text(&shown, R::new(right - bw - 8.0 - name_w, cy - 10.0, name_w, 20.0), i.look.font(12.5, 500).align(Align::Right), th.dim);
            draw_button(i, right, cy, "Choose…", PageHit::Fonts(k));
            h
        }
        Item::Text(k, label, hint) => {
            let cur = get(&s.settings, k).as_str().unwrap_or("").to_string();
            let bw = i.gfx.measure("Edit…", i.look.body.align(Align::Center)) + 2.0 * i.look.s4;
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint, bw + 150.0);
            let cy = r.y + h / 2.0;
            let shown = if cur.is_empty() { "None".to_string() } else { cur };
            let name_w = (i.gfx.measure(&shown, i.look.font(12.5, 500)) + 16.0).min(r.w * 0.5);
            i.gfx.text(&shown, R::new(right - bw - 8.0 - name_w, cy - 10.0, name_w, 20.0), i.look.font(12.5, 500).align(Align::Right), th.dim);
            draw_button(i, right, cy, "Edit…", PageHit::Text(k));
            h
        }
        Item::Wallpaper => {
            let file = s.settings.wallpaper.clone();
            let name = if file.is_empty() { "No image chosen".to_string() } else { file.rsplit(['\\', '/']).next().unwrap_or(&file).to_string() };
            let bw = i.gfx.measure("Choose image…", i.look.body.align(Align::Center)) + 2.0 * i.look.s4;
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), "Wallpaper image", "A file on this PC: PNG, JPEG, BMP or GIF.", bw + 150.0);
            let cy = r.y + h / 2.0;
            let name_w = (i.gfx.measure(&name, i.look.font(12.5, 500)) + 16.0).min(r.w * 0.5);
            i.gfx.text(&name, R::new(right - bw - 8.0 - name_w, cy - 10.0, name_w, 20.0), i.look.font(12.5, 500).align(Align::Right), th.dim);
            draw_button(i, right, cy, "Choose image…", PageHit::Text("wallpaper"));
            h
        }
        Item::Reset(group, hint) => {
            let bw = i.gfx.measure("Reset", i.look.body.align(Align::Center)) + 2.0 * i.look.s4;
            let h = row_label(i, R::new(r.x, r.y, r.w, 40.0), "", hint, bw);
            draw_button(i, right, r.y + h / 2.0, "Reset", PageHit::ResetGroup(group));
            h
        }
        Item::Seg(k, label, hint, opts) => {
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint, seg_width(i, opts));
            let cur = get(&s.settings, k).as_str().unwrap_or("").to_string();
            draw_seg(i, right, r.y + h / 2.0, k, opts, &cur);
            h
        }
        Item::Pick(k, label, hint, opts) => {
            // Label and hint on their own lines, then the choices wrap across the row:
            // a list this long does not fit in the half a Seg gets, and one pill wide
            // enough for it would run off the page.
            let f = i.look.font(12.5, 450).align(Align::Center);
            let cur = get(&s.settings, k).as_str().unwrap_or("").to_string();
            let mut y = r.y;
            i.gfx.text(label, R::new(r.x, y, r.w, 20.0), i.look.font(13.0, 500), th.text);
            y += 20.0;
            if !hint.is_empty() {
                i.gfx.text_wrap(hint, R::new(r.x, y, r.w, 34.0), i.look.small, th.faint);
                y += 36.0;
            }
            y += 4.0;
            let widths: Vec<f32> = opts.iter().map(|(_, l)| i.gfx.measure(l, f) + 22.0).collect();
            let mut x = r.x;
            for ((v, l), w) in opts.iter().zip(widths) {
                if x > r.x && x + w > right {
                    x = r.x;
                    y += 30.0;
                }
                let chip = R::new(x, y, w, 26.0);
                let h = Hit::Page(PageHit::Seg(k, v));
                let on = *v == cur;
                let cr = i.look.round(26.0);
                if on {
                    i.gfx.round(chip, cr, th.pill);
                } else {
                    i.gfx.round(chip, cr, if i.is_hover(&h) { th.hover } else { th.bg });
                    i.gfx.round_stroke(chip, cr, th.line, 1.0);
                }
                i.gfx.text(l, chip, f, if on { th.pill_ink } else { th.dim });
                i.hit(chip, h);
                x += w + 4.0;
            }
            y + 26.0 - r.y + 14.0
        }
        Item::Toggle(k, label, hint) => {
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint, 34.0);
            let on = get(&s.settings, k).as_bool().unwrap_or(false);
            draw_toggle(i, right, r.y + h / 2.0, k, on, true);
            h
        }
        Item::Range(k, label, hint, scale, unit) => {
            let limited = limit_toggle(k).is_some();
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint, if limited { 290.0 } else { 240.0 });
            let cy = r.y + h / 2.0;
            let v = get(&s.settings, k).as_f64().unwrap_or(0.0) as f32;
            match limit_toggle(k) {
                Some(t) => {
                    let enabled = *k != "cpu_limit_pct" || s.cpu_cap;
                    let on = get(&s.settings, t).as_bool().unwrap_or(false) && enabled;
                    draw_toggle(i, right, cy, t, on, enabled);
                    draw_slider(i, right - 50.0, cy, k, *scale, unit, v, !on);
                }
                None => draw_slider(i, right, cy, k, *scale, unit, v, false),
            }
            h
        }
        Item::Engine => {
            let f = i.look.font(13.0, 450).align(Align::Center);
            let widths: Vec<f32> = ENGINES.iter().map(|(l, _)| i.gfx.measure(l, f) + 22.0).collect();
            let total: f32 = widths.iter().sum::<f32>() + 4.0;
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), "Search engine", "", total);
            let pill = R::new(right - total, r.y + h / 2.0 - 15.0, total, 30.0);
            let rad = i.look.round(30.0);
            i.gfx.round(pill, rad, th.bg);
            i.gfx.round_stroke(pill, rad, th.line, 1.0);
            let mut x = pill.x + 2.0;
            for (n, ((label, url), w)) in ENGINES.iter().zip(widths).enumerate() {
                let rr = R::new(x, pill.y + 2.0, w, 26.0);
                let on = s.settings.search == *url;
                let h = Hit::Page(PageHit::Engine(n));
                if on {
                    i.gfx.round(rr, i.look.round(26.0), th.pill);
                }
                i.gfx.text(label, rr, f, if on { th.pill_ink } else if i.is_hover(&h) { th.text } else { th.faint });
                i.hit(rr, h);
                x += w;
            }
            h
        }
        Item::Button(label, hint, action, hit) => {
            let bw = i.gfx.measure(action, i.look.font(13.0, 450).align(Align::Center)) + 26.0;
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint, bw);
            draw_button(i, right, r.y + h / 2.0, action, hit.clone());
            h
        }
        Item::KeePass => {
            // A fresh note (a wrong password, an unreadable file) takes the
            // hint line while it lasts.
            let mut note = None;
            if let Some((msg, at)) = &i.page.flash {
                if at.elapsed().as_secs_f32() < 3.0 {
                    note = Some(msg.clone());
                }
            }
            let hint = if let Some(note) = note {
                note
            } else if s.vault > 0 {
                format!("{} entries open. Ones matching the page you are on appear by the address bar. Locks after 10 idle minutes.", s.vault)
            } else {
                "Reads a .kdbx on this PC. The password stays in memory and locks after 10 idle minutes.".to_string()
            };
            let action = if s.vault > 0 { "Lock now" } else { "Open KeePass file…" };
            let bw = i.gfx.measure(action, i.look.font(13.0, 450).align(Align::Center)) + 26.0;
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), "KeePass file", &hint, bw);
            draw_button(i, right, r.y + h / 2.0, action, if s.vault > 0 { PageHit::KdbxLock } else { PageHit::KdbxOpen });
            h
        }
        Item::AgentSetup => {
            let on = s.settings.agent;
            let hint = if on { "Claude Code, opencode, Codex, Cursor: run lumen-agent.exe mcp. Copy gives the Claude Code line." } else { "Turn the agent on first." };
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), "Connect an AI agent", hint, if on { 180.0 } else { 0.0 });
            if on {
                draw_button(i, right, r.y + h / 2.0, "Copy setup command", PageHit::CopyAgent);
            }
            h
        }
        Item::DefaultBrowser => {
            use crate::defaultapp::Status;
            let st = crate::defaultapp::status();
            let (label, mut hint) = match st {
                Status::Default => ("Lumen is your default browser", "Links from other apps and web files open here.".to_string()),
                Status::Listed => ("Not default", "Lumen is in Windows' list of browsers. Pick it there: Windows does not let an app pick itself.".to_string()),
                Status::Absent => ("Not default", "Adds Lumen to Windows' list of browsers, then opens the page where you pick it.".to_string()),
            };
            // A fresh note (what the button just did) takes the hint line while it lasts.
            if let Some((msg, at)) = &i.page.flash
                && at.elapsed().as_secs_f32() < 4.0
            {
                hint = msg.clone();
            }
            let f = i.look.font(13.0, 450).align(Align::Center);
            let make = "Make Lumen your default browser";
            let (w_make, w_off) = (i.gfx.measure(make, f) + 26.0, i.gfx.measure("Unregister", f) + 26.0);
            let both = st != Status::Absent;
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), label, &hint, w_make + if both { w_off + 8.0 } else { 0.0 });
            let cy = r.y + h / 2.0;
            draw_button(i, right, cy, make, PageHit::DefaultMake);
            if both {
                draw_button(i, right - w_make - 8.0, cy, "Unregister", PageHit::DefaultOff);
            }
            h
        }
        Item::SitePerms => {
            if s.rules.is_empty() {
                i.gfx.text_wrap("No answers remembered yet. When a site asks for your camera, location or similar, the sidebar asks you.", R::new(r.x, r.y + 6.0, r.w, 36.0), i.look.font(12.5, 400), th.faint);
                return 44.0;
            }
            let mut y = r.y;
            for (n, (origin, kind, allow)) in s.rules.iter().enumerate() {
                i.gfx.text(&super::host(origin), R::new(r.x, y, r.w * 0.45, 40.0), i.look.font(13.0, 500), th.text);
                i.gfx.text(&format!("{kind} · {}", if *allow { "Allowed" } else { "Blocked" }), R::new(r.x + r.w * 0.45, y, r.w * 0.35, 40.0), i.look.font(12.5, 400), if *allow { th.dim } else { th.faint });
                let br = R::new(right - 60.0, y + 8.0, 60.0, 24.0);
                let h = Hit::Page(PageHit::PermReset(n));
                i.gfx.text("Reset", br, i.look.small.align(Align::Right), if i.is_hover(&h) { th.text } else { th.faint });
                i.hit(br, h);
                i.gfx.line(r.x, y + 40.0, right, y + 40.0, th.line, 1.0);
                y += 41.0;
            }
            y - r.y
        }
        Item::Keys => {
            let mut y = r.y;
            for (what, keys) in KEYS {
                i.gfx.text(what, R::new(r.x, y, r.w * 0.6, 36.0), i.look.font(13.0, 400), th.dim);
                i.gfx.text(keys, R::new(r.x + r.w * 0.4, y, r.w * 0.6, 36.0), i.look.font(12.5, 450).align(Align::Right), th.text);
                i.gfx.line(r.x, y + 36.0, right, y + 36.0, th.line, 1.0);
                y += 37.0;
            }
            y - r.y
        }
    }
}

fn draw_button(i: &mut Inner, right: f32, cy: f32, label: &str, hit: PageHit) {
    let th = i.theme;
    let f = i.look.font(13.0, 450).align(Align::Center);
    let w = i.gfx.measure(label, f) + 26.0;
    let r = R::new(right - w, cy - 15.0, w, 30.0);
    let h = Hit::Page(hit);
    let rad = i.look.round(30.0);
    i.gfx.round(r, rad, if i.is_hover(&h) { th.hover } else { th.raise });
    i.gfx.round_stroke(r, rad, th.line, 1.0);
    i.gfx.text(label, r, f, th.text);
    i.hit(r, h);
}

pub(super) fn fmt_bytes(b: i64) -> String {
    let b = b.max(0) as f64;
    if b >= 1073741824.0 {
        format!("{:.2} GB", b / 1073741824.0)
    } else if b >= 1048576.0 {
        format!("{:.1} MB", b / 1048576.0)
    } else if b >= 1024.0 {
        format!("{} KB", (b / 1024.0).round())
    } else {
        format!("{b} B")
    }
}

/// "3 min left", "40 s left"; empty when it cannot be known.
fn eta(d: &super::UiDl) -> String {
    if d.speed == 0 || d.total <= d.received {
        return String::new();
    }
    let s = (d.total - d.received) as u64 / d.speed;
    match s {
        0..=59 => format!("{s} s left"),
        60..=3599 => format!("{} min left", s.div_ceil(60)),
        _ => format!("{} h {} min left", s / 3600, (s % 3600) / 60),
    }
}

/// "3 active · 4.2 MB/s down · 310 KB/s up"; empty when nothing is running.
/// While torrents run, appends " · port open" or " · port closed".
fn summary(active: usize, down: i64, up: i64, upnp_port_open: bool) -> String {
    if active == 0 {
        return String::new();
    }
    let mut s = format!("{active} active · {}/s down · {}/s up", fmt_bytes(down), fmt_bytes(up));
    if upnp_port_open {
        s.push_str(" · port open");
    } else {
        s.push_str(" · port closed");
    }
    s
}

/// The group a row falls in; the page draws them in this order, empty ones hidden.
fn group(state: &str) -> &'static str {
    match state {
        "progress" | "paused" => "Active",
        "seeding" => "Seeding",
        "done" => "Done",
        _ => "Failed",
    }
}

fn group_order() -> [&'static str; 4] {
    ["Active", "Seeding", "Done", "Failed"]
}

/// One status line per kind, per the brief: files get connections, torrents get peers.
fn status_line(d: &UiDl) -> String {
    let host = super::host(&d.url);
    let pct = if d.total > 0 { d.received as f32 / d.total as f32 } else { 0.0 };
    let mut s = match d.state {
        "done" => format!("Done · {}", fmt_bytes(d.received)),
        "failed" if !d.note.is_empty() => format!("Failed · {}", d.note),
        "failed" => "Failed".to_string(),
        "seeding" => {
            let mut s = format!("Seeding · up {}/s", fmt_bytes(d.up as i64));
            if d.ratio > 0.0 {
                s.push_str(&format!(" · ratio {:.2}", d.ratio));
            }
            if d.peers > 0 {
                s.push_str(&format!(" · {} peers", d.peers));
            }
            s
        }
        "paused" => {
            let mut s = format!("Paused · {} of {}", fmt_bytes(d.received), fmt_bytes(d.total));
            if !d.note.is_empty() {
                s.push_str(&format!(" · {}", d.note));
            }
            s
        }
        _ if d.total > 0 => {
            let mut s = format!("{} of {} · {}%", fmt_bytes(d.received), fmt_bytes(d.total), (pct * 100.0) as u32);
            if d.speed > 0 {
                s.push_str(&format!(" · {}/s", fmt_bytes(d.speed as i64)));
            }
            let left = eta(d);
            if !left.is_empty() {
                s.push_str(&format!(" · {left}"));
            }
            match d.kind {
                "torrent" => {
                    if d.peers > 0 || d.seeds > 0 {
                        s.push_str(&format!(" · {} peers, {} seeds", d.peers, d.seeds));
                    }
                    if d.up > 0 {
                        s.push_str(&format!(" · up {}/s", fmt_bytes(d.up as i64)));
                    }
                }
                _ => {
                    if d.peers > 0 {
                        s.push_str(&format!(" · {} connections", d.peers));
                    }
                }
            }
            if !d.note.is_empty() {
                s.push_str(&format!(" · {}", d.note));
            }
            s
        }
        _ if !d.note.is_empty() => d.note.clone(),
        _ => fmt_bytes(d.received),
    };
    if !host.is_empty() {
        s.push_str(&format!(" · {host}"));
    }
    s
}

/// What the clipboard holds: a link aria2 can take, a magnet, or nothing useful.
fn classify_link(text: &str) -> &'static str {
    let t = text.trim();
    if t.starts_with("magnet:?") {
        "magnet"
    } else if t.starts_with("http://") || t.starts_with("https://") {
        "url"
    } else {
        "other"
    }
}

/// Apply an imported look: keep it among the saved ones, and put it on.
pub(super) fn apply_theme(i: &mut Inner, t: style::ThemeFile) {
    let Some(s) = i.state.clone() else { return };
    let mut list = style::saved_themes();
    if !t.name.is_empty() {
        list.push(t.clone());
        style::save_themes(&list);
        i.page.themes = Some(list);
    }
    let mut next = s.settings.clone();
    t.apply(&mut next);
    send(&i.proxy, json!({"t": "settings", "v": serde_json::to_value(&next).unwrap_or_default()}));
    i.page.flash = Some(("Imported".into(), std::time::Instant::now()));
}

/// The settings one "Reset" button owns. Not the layout: a look is colours,
/// fonts, clock, sidebar shape and motion, never where the chrome lives.
fn group_keys(group: &str) -> &'static [&'static str] {
    match group {
        "colors" => &["palette", "accent", "bg_color", "side_color", "surface_color", "text_color", "dim_color"],
        "fonts" => &["ui_font", "display_font", "mono_font", "font_scale", "title_weight"],
        "layout" => &["layout", "bm_bar", "sidebar_side", "sidebar_width", "compact"],
        "ntp" => &[
            "clock", "clock_24h", "clock_seconds", "date_format", "clock_font", "clock_size", "greeting", "ntp_bg", "wallpaper",
            "wallpaper_blur", "wallpaper_dim", "speed_dial", "dial_cols", "dial_tile", "ntp_search",
        ],
        "sidebar" => &["tab_row_h", "show_favicons", "close_btn", "show_memory", "footer_btns", "ws_row"],
        "shape" => &["density", "gap", "radius", "page_border"],
        "motion" => &["motion", "motion_speed", "motion_system"],
        _ => &[],
    }
}

/// Put one group of the Appearance page back to its defaults, in one message.
fn reset_group(i: &mut Inner, group: &str) {
    let Some(st) = i.state.as_ref().map(|s| s.settings.clone()) else { return };
    let keys = group_keys(group);
    if keys.is_empty() {
        return;
    }
    let mut all = serde_json::to_value(&st).unwrap_or(json!({}));
    let defaults = serde_json::to_value(Settings::default()).unwrap_or(json!({}));
    for k in keys {
        if let Some(v) = defaults.get(*k) {
            all[*k] = v.clone();
        }
    }
    if let Ok(next) = serde_json::from_value::<Settings>(all) {
        let look = Look::of(&next, i.theme.dark, super::motion::system_allows());
        i.theme = look.theme;
        i.look = look;
        if let Some(s) = i.state.as_mut() {
            s.settings = next.clone();
        }
        send(&i.proxy, json!({"t": "settings", "v": next}));
        i.page.flash = Some(("Back to the defaults".into(), std::time::Instant::now()));
    }
    i.invalidate();
}

fn chip_label(kbps: u32) -> &'static str {
    match kbps {
        0 => "No limit",
        1024 => "1 MB/s",
        5120 => "5 MB/s",
        _ => "10 MB/s",
    }
}

// ------------------------------------------------------------ page parts
//
// What every list page is made of: a centred column, a title with an optional line under
// it, toolbar buttons, rows that light up under the mouse, an empty state that says what to
// do, and a close button that stays put while the page scrolls.

/// The content column of a list page: centred, capped, and never closer to the edge than
/// the margin.
pub(super) fn column(st: R, max_w: f32) -> R {
    let w = (st.w - 2.0 * COL_MARGIN).clamp(120.0, max_w);
    R::new(st.x + (st.w - w) / 2.0, st.y, w, st.h)
}

/// The close button, top right of the page, drawn last so it sits over whatever scrolls
/// beneath it. Esc does the same.
pub(super) fn close_button(i: &mut Inner, st: R) {
    let th = i.theme;
    let r = R::new(st.right() - i.look.s4 - 32.0, st.y + i.look.s4, 32.0, 32.0);
    let h = Hit::Page(PageHit::Done);
    let hov = i.is_hover(&h);
    i.gfx.round(r, i.look.round(32.0), if hov { th.hover } else { th.raise });
    i.gfx.round_stroke(r, i.look.round(32.0), th.line, 1.0);
    i.gfx.icon("close", r.x + 8.0, r.y + 8.0, 16.0, if hov { th.text } else { th.dim });
    i.hit(r, h);
}

/// Page title, and one quiet line under it when there is something to say. Returns the y
/// the content starts at.
pub(super) fn title_block(i: &Inner, col: R, y: f32, title: &str, sub: &str) -> f32 {
    let th = i.theme;
    let short = sub.chars().count() <= 64;
    if !sub.is_empty() && short {
        i.label(&sub.trim_end_matches('.').replace(". ", " · "), R::new(col.x, y, col.w - 48.0, 18.0), th.faint);
        i.gfx.text(title, R::new(col.x, y + 18.0, col.w - 48.0, 42.0), i.look.title, th.text);
        return y + 74.0;
    }
    i.gfx.text(title, R::new(col.x, y, col.w - 48.0, 42.0), i.look.title, th.text);
    if sub.is_empty() {
        return y + 56.0;
    }
    i.gfx.text(sub, R::new(col.x, y + 42.0, col.w - 48.0, 20.0), i.look.small, th.faint);
    y + 74.0
}

/// A button at (x, y), 30 high. Outlined and quiet, or (primary) filled with the accent.
/// Returns its rectangle so the caller can step along.
pub(super) fn outline_button(i: &mut Inner, x: f32, y: f32, label: &str, hit: PageHit, primary: bool) -> R {
    let th = i.theme;
    let f = if primary { i.look.body_strong } else { i.look.body }.align(Align::Center);
    let r = R::new(x, y, i.gfx.measure(label, f) + 2.0 * i.look.s4, 30.0);
    let h = Hit::Page(hit);
    let hov = i.is_hover(&h);
    let rad = i.look.round(30.0);
    if primary {
        let fill = if i.look.style == super::theme::Style::Signal { th.pill } else { th.accent };
        i.gfx.round(r, rad, if hov { fill } else { fill.alpha(0.9) });
        i.gfx.text(label, r, f, if i.look.style == super::theme::Style::Signal { th.pill_ink } else { th.bg });
    } else {
        i.gfx.round(r, rad, if hov { th.hover } else { th.raise });
        i.gfx.round_stroke(r, rad, th.line, 1.0);
        i.gfx.text(label, r, f, th.text);
    }
    i.hit(r, h);
    r
}

/// A text action on a row, 28 high, its right edge at `right`. Returns its left edge.
pub(super) fn row_action(i: &mut Inner, right: f32, y: f32, label: &str, hit: PageHit) -> f32 {
    let th = i.theme;
    let f = i.look.small_strong.align(Align::Center);
    let w = i.gfx.measure(label, f) + 2.0 * i.look.s3;
    let r = R::new(right - w, y, w, 28.0);
    let h = Hit::Page(hit);
    let hov = i.is_hover(&h);
    if hov {
        i.gfx.round(r, i.look.round(28.0), th.hover);
    }
    i.gfx.text(label, r, f, if hov { th.text } else { th.dim });
    i.hit(r, h);
    r.x - i.look.s1
}

/// A small caps label that opens a group of rows. Returns the y after it.
pub(super) fn group_label(i: &Inner, col: R, y: f32, text: &str, count: usize) -> f32 {
    let th = i.theme;
    let label = if count > 0 { format!("{}  {count}", text) } else { text.to_string() };
    i.label(&label, R::new(col.x + 4.0, y, col.w, 20.0), th.faint);
    y + 28.0
}

/// What a page shows when it has nothing to list: a mark, a plain sentence, and what to do
/// about it. Centred in `area`; returns its bottom.
pub(super) fn empty_state(i: &mut Inner, area: R, icon: &str, title: &str, hint: &str) -> f32 {
    let th = i.theme;
    let cx = area.x + area.w / 2.0;
    i.gfx.circle(cx, area.y + 28.0, 28.0, th.raise);
    i.gfx.ring(cx, area.y + 28.0, 28.0, 28.0, th.line, 1.0);
    i.gfx.icon(icon, cx - 12.0, area.y + 16.0, 24.0, th.faint);
    i.gfx.text(title, R::new(area.x, area.y + 70.0, area.w, 26.0), i.look.font(15.0, 550).align(Align::Center), th.text);
    i.gfx.text_wrap(hint, R::new(cx - 210.0, area.y + 100.0, 420.0, 50.0), i.look.small.align(Align::Center), th.faint);
    area.y + 150.0
}

/// A row's hover plate. The row also registers itself, so anything drawn on it afterwards
/// (buttons) takes the mouse first.
pub(super) fn row_plate(i: &mut Inner, r: R, id: u64) -> bool {
    let th = i.theme;
    let h = Hit::Page(PageHit::Row(id));
    let hov = i.is_hover(&h) || i.hover.as_ref().is_some_and(|x| matches!(x, Hit::Page(PageHit::Dl(_, d)) if *d == id));
    if hov {
        i.gfx.round(r, i.look.r_lg, th.hover);
    }
    i.hit(r, h);
    hov
}

/// Letters for a download's tile: its extension, or what kind of thing it is.
fn tile_label(d: &UiDl) -> String {
    let ext = d.name.rsplit_once('.').map(|(_, e)| e).filter(|e| !e.is_empty() && e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric()));
    match (ext, d.kind) {
        (Some(e), _) => e.to_uppercase(),
        (None, "torrent") => "TOR".into(),
        (None, "video") => "VID".into(),
        _ => "FILE".into(),
    }
}

fn draw_downloads(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let col = column(st, COL_W);
    i.gfx.clip(st);
    let mut y = st.y + i.look.s5 + 2.0 - i.page.scroll;

    // The line under the title: the short-lived "copy a link" note, else what is running.
    let mut flash_gone = false;
    let mut sub = String::new();
    if let Some((msg, at)) = &i.page.flash {
        if at.elapsed().as_secs_f32() < 3.0 {
            sub = msg.clone();
        } else {
            flash_gone = true;
        }
    }
    if flash_gone {
        i.page.flash = None;
    }
    if sub.is_empty() {
        let active: Vec<&UiDl> = s.downloads.iter().filter(|d| matches!(d.state, "progress" | "paused")).collect();
        let down: i64 = active.iter().map(|d| d.speed as i64).sum();
        let up: i64 = s.downloads.iter().filter(|d| matches!(d.state, "progress" | "paused" | "seeding")).map(|d| d.up as i64).sum();
        sub = summary(active.len(), down, up, s.upnp_port_open);
    }
    y = title_block(i, col, y, "Downloads", &sub);

    // Toolbar: things to add on the left, the cap and the cleanup on the right.
    let mut x = col.x;
    for (label, hit) in [("Add link", PageHit::DlAddLink), ("Open torrent…", PageHit::DlTorrentPick), ("Open folder", PageHit::DlOpenFolder)] {
        x = outline_button(i, x, y, label, hit, false).right() + i.look.s2;
    }
    let chip = format!("Speed limit: {}", chip_label(s.settings.download_limit_kbps));
    let cf = i.look.small_strong.align(Align::Center);
    let cw = i.gfx.measure(&chip, cf) + 2.0 * i.look.s3;
    let cr = R::new(col.right() - cw, y + 1.0, cw, 28.0);
    let ch = Hit::Page(PageHit::DlLimit);
    let chov = i.is_hover(&ch);
    i.gfx.round(cr, 14.0, if chov { th.press } else { th.hover });
    i.gfx.text(&chip, cr, cf, if chov { th.text } else { th.dim });
    i.hit(cr, ch);
    if s.downloads.iter().any(|d| d.state == "done" || d.state == "failed") {
        row_action(i, cr.x - i.look.s2, y + 1.0, "Clear finished", PageHit::DlClear);
    }
    y += 30.0 + i.look.s4;

    if s.downloads.is_empty() {
        let area = R::new(col.x, y + 56.0, col.w, 200.0);
        empty_state(i, area, "download", "No downloads yet", "Files you download show up here. To start one yourself, copy a link or magnet and press Add link.");
        i.gfx.unclip();
        i.page.max = 0.0;
        close_button(i, st);
        return;
    }

    for g in group_order() {
        let rows: Vec<&UiDl> = s.downloads.iter().filter(|d| group(d.state) == g).collect();
        if rows.is_empty() {
            continue;
        }
        y += i.look.s3;
        y = group_label(i, col, y, g, rows.len());
        // The group's card, under its rows.
        let card = R::new(col.x - 6.0, y, col.w + 12.0, rows.len() as f32 * 74.0 + 6.0 + if s.settings.torrent_note && rows.iter().any(|d| d.kind == "torrent") { 30.0 } else { 0.0 });
        if card.bottom() >= st.y && card.y <= st.bottom() {
            i.gfx.round(card, i.look.r_card, th.raise);
            i.gfx.round_stroke(card, i.look.r_card, th.line, 1.0);
        }
        y += 3.0;
        // The firewall note rides under the first header that actually has a torrent.
        if s.settings.torrent_note && rows.iter().any(|d| d.kind == "torrent") {
            let msg = "Windows may ask to let aria2 through the firewall. Allow it for faster torrents. (click to hide)";
            let h = Hit::Page(PageHit::DlNote);
            i.gfx.text(msg, R::new(col.x + 14.0, y + 4.0, col.w - 28.0, 22.0), i.look.small, if i.is_hover(&h) { th.text } else { th.faint });
            i.hit(R::new(col.x, y + 4.0, col.w, 22.0), h);
            y += 30.0;
        }
        for d in rows {
            let r = R::new(col.x - i.look.s2, y, col.w + 2.0 * i.look.s2, 72.0);
            if r.bottom() < st.y || r.y > st.bottom() {
                y += 74.0;
                continue;
            }
            row_plate(i, r.inset(6.0, 3.0), d.id);
            let failed = d.state == "failed";
            // The tile: what the file is, in its own letters.
            let tile = R::new(col.x + 10.0, y + 14.0, 44.0, 44.0);
            i.gfx.round(tile, i.look.r_md, th.bg);
            i.gfx.round_stroke(tile, i.look.r_md, th.line, 1.0);
            i.gfx.text(&tile_label(d), tile, i.look.mono_at(10.0, 700).align(Align::Center), if failed { th.faint.alpha(0.7) } else { th.text });
            // The actions, right to left; the text takes whatever they leave.
            let acts: &[(&str, &'static str)] = match d.state {
                "progress" => &[("Pause", "pause"), ("Cancel", "cancel")],
                "paused" => &[("Resume", "resume"), ("Cancel", "cancel")],
                "seeding" => &[("Stop seeding", "stopSeed")],
                "done" => &[("Open", "open"), ("Show in folder", "folder")],
                "failed" if d.url.starts_with("http") => &[("Retry", "retry")],
                _ => &[],
            };
            let mut right = col.right() - 10.0;
            if d.state == "done" || d.state == "failed" {
                let xr = R::new(right - 28.0, y + 22.0, 28.0, 28.0);
                let h = Hit::Page(PageHit::Dl("remove", d.id));
                let hov = i.is_hover(&h);
                if hov {
                    i.gfx.round(xr, i.look.r_sm, th.press);
                }
                i.gfx.icon("close", xr.x + 7.0, xr.y + 7.0, 14.0, if hov { th.text } else { th.faint });
                i.hit(xr, h);
                right = xr.x - i.look.s1;
            }
            for (label, a) in acts.iter().rev() {
                right = row_action(i, right, y + 22.0, label, PageHit::Dl(a, d.id));
            }
            let tx = tile.right() + i.look.s3;
            let tw = (right - i.look.s3 - tx).max(60.0);
            i.gfx.text(&d.name, R::new(tx, y + 13.0, tw, 22.0), i.look.body_strong, if failed { th.dim } else { th.text });
            i.gfx.text(&status_line(d), R::new(tx, y + 35.0, tw, 18.0), i.look.small, th.faint);
            if d.state == "progress" || d.state == "paused" {
                let pct = if d.total > 0 { d.received as f32 / d.total as f32 } else { 0.0 };
                let bar = R::new(tx, y + 59.0, col.right() - 10.0 - tx, 4.0);
                i.gfx.round(bar, 2.0, th.line);
                if pct > 0.0 {
                    i.gfx.round(R::new(bar.x, bar.y, (bar.w * pct.min(1.0)).max(4.0), 4.0), 2.0, if d.state == "paused" { th.faint } else { th.text });
                }
            }
            y += 74.0;
        }
    }
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

/// History, grouped by day: one quiet list, click to open, x to remove.
fn draw_library(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let col = column(st, COL_W);
    i.gfx.clip(st);
    let mut y = st.y + i.look.s5 + 2.0 - i.page.scroll;
    let title_y = y;
    y = title_block(i, col, y, "History", "Newest first. Private tabs are never recorded.");
    if !s.history.is_empty() {
        let w = i.gfx.measure("Clear history…", i.look.body.align(Align::Center)) + 2.0 * i.look.s4;
        outline_button(i, col.right() - w - 48.0, title_y + 4.0, "Clear history…", PageHit::ClearHistory, false);
    }
    let rows: Vec<(String, String, String, String)> =
        s.history.iter().map(|v| (v.url.clone(), v.title.clone(), v.day.clone(), v.time.clone())).collect();
    if rows.is_empty() {
        empty_state(i, R::new(col.x, y + 56.0, col.w, 200.0), "clock", "No history yet", "Pages you visit show up here, newest first.");
        i.gfx.unclip();
        i.page.max = 0.0;
        close_button(i, st);
        return;
    }
    // The days, in order, each its rows in one card.
    let mut days: Vec<(String, Vec<(String, String, String)>)> = Vec::new();
    for (url, title, day, time) in rows {
        match days.last_mut() {
            Some((d, list)) if *d == day => list.push((url, title, time)),
            _ => days.push((day, vec![(url, title, time)])),
        }
    }
    let lo = i.look.clone();
    const ROW: f32 = 46.0;
    for (day, list) in days {
        y += lo.s3;
        y = group_label(i, col, y, &day, 0);
        let card = R::new(col.x, y, col.w, list.len() as f32 * ROW);
        if card.bottom() >= st.y && card.y <= st.bottom() {
            i.gfx.round(card, lo.r_card, th.raise);
            i.gfx.round_stroke(card, lo.r_card, th.line, 1.0);
        }
        for (n, (url, title, time)) in list.into_iter().enumerate() {
            let r = R::new(card.x, y, card.w, ROW);
            y += ROW;
            if r.y > st.bottom() || r.bottom() < st.y {
                continue;
            }
            if n > 0 {
                i.gfx.line(card.x + 1.0, r.y, card.right() - 1.0, r.y, th.line, 1.0);
            }
            let open = Hit::Page(PageHit::Open(url.clone()));
            let rm = Hit::Page(PageHit::Forget(url.clone()));
            let hov = i.is_hover(&open) || i.is_hover(&rm);
            if hov {
                i.gfx.round(r.inset(4.0, 4.0), lo.r_md, th.hover);
            }
            let mut x = r.x + 18.0;
            i.gfx.text(&time, R::new(x, r.y, 48.0, ROW), lo.mono_at(11.5, 450), th.faint);
            x += 52.0;
            let host = super::host(&url);
            site_icon(i, R::new(x, r.y + (ROW - 18.0) / 2.0, 18.0, 18.0), &host);
            x += 30.0;
            // The host sits at the right, the title takes what is left, cut with an ellipsis.
            let hf = lo.mono_at(11.5, 450);
            let hw = (i.gfx.measure(&host, hf) + 4.0).min(240.0);
            let hx = r.right() - 18.0 - hw - if hov { 30.0 } else { 0.0 };
            let name = if title.is_empty() { url.clone() } else { title };
            i.gfx.text(&name, R::new(x, r.y, (hx - lo.s4 - x).max(60.0), ROW), lo.body, th.text);
            i.gfx.text(&host, R::new(hx, r.y, hw, ROW), hf.align(Align::Right), th.faint);
            i.hit(r, open);
            if hov {
                let xr = R::new(r.right() - 42.0, r.y + 9.0, 28.0, 28.0);
                let xh = i.is_hover(&rm);
                if xh {
                    i.gfx.round(xr, lo.round(28.0), th.press);
                }
                i.gfx.icon("close", xr.x + 7.0, xr.y + 7.0, 14.0, if xh { th.text } else { th.dim });
                i.hit(xr, rm);
            }
        }
    }
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

// -------------------------------------------------------------- bookmarks

/// What the search box and the two buttons beside it take. The search gets the rest.
const BM_BUTTONS_W: f32 = 270.0;
const BM_ROW_H: f32 = 46.0;

/// The search box of the Bookmarks page. Fixed under the title; the list scrolls below it.
/// The edit control (mod.rs) sits on top of this rectangle. `top` is the space the
/// title block takes, from the look that is drawing.
pub fn bm_search_rect(st: R, top: f32) -> R {
    let col = column(st, COL_W);
    R::new(col.x, st.y + top + 2.0 + 68.0, (col.w - BM_BUTTONS_W).max(140.0), 34.0)
}

/// Every word typed is somewhere in the title or the address.
fn bm_matches(q: &str, m: &super::UiMark) -> bool {
    let hay = format!("{} {}", m.title, m.url).to_lowercase();
    q.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
}

/// The page's groups, in order: Unsorted (when it holds something), then each folder. While
/// searching, only the groups that have a match; otherwise every folder, empty ones too.
fn bm_groups<'a>(marks: &'a [super::UiMark], folders: &'a [String], q: &str) -> Vec<(&'a str, Vec<&'a super::UiMark>)> {
    let searching = !q.trim().is_empty();
    let pick = |folder: &str| -> Vec<&'a super::UiMark> { marks.iter().filter(|m| m.folder == folder && (!searching || bm_matches(q, m))).collect() };
    let mut out = Vec::new();
    let loose = pick("");
    if !loose.is_empty() {
        out.push(("", loose));
    }
    for f in folders {
        let rows = pick(f);
        if !searching || !rows.is_empty() {
            out.push((f.as_str(), rows));
        }
    }
    out
}

/// Where a dragged bookmark lands when let go with the pointer at `y`: the folder, and the
/// bookmark it goes before (None: the end of the folder). Over a header it goes to the end of
/// that folder; over the upper half of a row, before it; over the lower half, before the next
/// row of the same folder.
fn bm_drop(rows: &[DropRow], y: f32) -> Option<(String, Option<u64>)> {
    let n = rows.iter().position(|d| y >= d.r.y && y < d.r.bottom())?;
    let d = &rows[n];
    if d.id == 0 {
        return Some((d.folder.clone(), None));
    }
    if y < d.r.y + d.r.h / 2.0 {
        return Some((d.folder.clone(), Some(d.id)));
    }
    let next = rows[n + 1..].iter().find(|x| x.id != 0 && x.folder == d.folder).map(|x| x.id);
    Some((d.folder.clone(), next))
}

fn draw_bookmarks(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let col = column(st, COL_W);
    let q = super::wide_text(i.bm_edit);
    let q = q.trim().to_string();
    let sr = bm_search_rect(st, i.look.s5);
    let top = sr.bottom() + i.look.s3;
    let list = R::new(st.x, top, st.w, st.bottom() - top);
    let groups = bm_groups(&s.bookmarks, &s.bm_folders, &q);
    let total: usize = groups.iter().map(|g| g.1.len()).sum();
    let named = !s.bm_folders.is_empty();
    i.page.bm_rows.clear();

    // The list first, clipped under the search row; the header is drawn after it so its
    // buttons win the mouse wherever a half-scrolled row reaches up under them.
    i.gfx.clip(list);
    let mut y = top - i.page.scroll;
    if total == 0 && (!q.is_empty() || s.bm_folders.is_empty()) {
        let (head, hint) = if q.is_empty() {
            ("No bookmarks yet", "Press Ctrl+D, or the star in the address bar, to keep a page here. Import brings over what Chrome, Edge, Brave or Opera GX had.")
        } else {
            ("Nothing matches", "Try fewer words. The title and the address are searched.")
        };
        empty_state(i, R::new(col.x, top + 40.0, col.w, 200.0), if q.is_empty() { "star" } else { "search" }, head, hint);
        y = top;
    }
    let searching = !q.is_empty();
    let dragging = i.page.bm_drag.as_ref().filter(|d| d.moved).map(|d| d.id);
    let lo = i.look.clone();
    for (folder, rows) in &groups {
        // A header when there is a folder to name, or Unsorted needs telling apart from them.
        if !folder.is_empty() || named {
            let label = if folder.is_empty() { "Unsorted" } else { folder };
            let h = R::new(col.x, y, col.w, 30.0);
            if h.bottom() >= top && h.y <= st.bottom() {
                let hit = Hit::Page(PageHit::BmFolder(folder.to_string()));
                let folded = i.page.bm_collapsed.contains(*folder) && !searching;
                let hov = i.is_hover(&hit);
                if hov {
                    i.gfx.round(h, lo.round(h.h), th.hover);
                }
                // A chevron: down when open, right when folded.
                let (cx, cy) = (col.x + 12.0, y + 15.0);
                let c = if hov { th.dim } else { th.faint };
                if folded {
                    i.gfx.line(cx - 2.0, cy - 4.0, cx + 2.0, cy, c, 1.5);
                    i.gfx.line(cx + 2.0, cy, cx - 2.0, cy + 4.0, c, 1.5);
                } else {
                    i.gfx.line(cx - 4.0, cy - 2.0, cx, cy + 2.0, c, 1.5);
                    i.gfx.line(cx, cy + 2.0, cx + 4.0, cy - 2.0, c, 1.5);
                }
                i.label(&format!("{label}  {}", rows.len()), R::new(col.x + 26.0, y + 7.0, col.w - 30.0, 16.0), if hov { th.dim } else { th.faint });
                i.hit(h, hit);
                i.page.bm_rows.push(DropRow { r: h, folder: folder.to_string(), id: 0 });
            }
            y += 34.0;
        }
        if i.page.bm_collapsed.contains(*folder) && !searching {
            y += lo.s2;
            continue;
        }
        // The folder's bookmarks, in one card.
        let card = R::new(col.x, y, col.w, rows.len().max(1) as f32 * BM_ROW_H);
        if card.bottom() >= top && card.y <= st.bottom() {
            i.gfx.round(card, lo.r_card, th.raise);
            i.gfx.round_stroke(card, lo.r_card, th.line, 1.0);
        }
        if rows.is_empty() {
            i.gfx.text("Empty. Drag a bookmark here, or use Move to folder.", R::new(card.x + 18.0, y, card.w - 36.0, BM_ROW_H), lo.small, th.faint);
            y += BM_ROW_H;
        }
        for (n, m) in rows.iter().enumerate() {
            let r = R::new(card.x, y, card.w, BM_ROW_H);
            y += BM_ROW_H;
            if r.bottom() < top || r.y > st.bottom() {
                continue;
            }
            if n > 0 {
                i.gfx.line(card.x + 1.0, r.y, card.right() - 1.0, r.y, th.line, 1.0);
            }
            i.page.bm_rows.push(DropRow { r, folder: m.folder.clone(), id: m.id });
            let open = Hit::Page(PageHit::Bm(m.id));
            let rm = Hit::Page(PageHit::BmDel(m.id));
            let hov = i.is_hover(&open) || i.is_hover(&rm);
            let lifted = dragging == Some(m.id);
            if hov && !lifted {
                i.gfx.round(r.inset(4.0, 4.0), lo.r_md, th.hover);
            }
            let name = if m.title.is_empty() { m.url.clone() } else { m.title.clone() };
            let host = super::host(&m.url);
            let fade = if lifted { 0.4 } else { 1.0 };
            site_icon(i, R::new(r.x + 18.0, r.y + (BM_ROW_H - 18.0) / 2.0, 18.0, 18.0), &host);
            // The host at the right, the title takes what is left, cut with an ellipsis.
            let hf = lo.mono_at(11.5, 450);
            let hw = (i.gfx.measure(&host, hf) + 4.0).min(240.0);
            let hx = r.right() - 18.0 - hw - if hov { 30.0 } else { 0.0 };
            let tx = r.x + 48.0;
            i.gfx.text(&name, R::new(tx, r.y, (hx - lo.s4 - tx).max(60.0), BM_ROW_H), lo.body, th.text.alpha(fade));
            i.gfx.text(&host, R::new(hx, r.y, hw, BM_ROW_H), hf.align(Align::Right), th.faint.alpha(fade));
            i.hit(r, open);
            if hov && dragging.is_none() {
                let xr = R::new(r.right() - 42.0, r.y + (BM_ROW_H - 28.0) / 2.0, 28.0, 28.0);
                let xh = i.is_hover(&rm);
                if xh {
                    i.gfx.round(xr, lo.round(28.0), th.press);
                }
                i.gfx.icon("close", xr.x + 7.0, xr.y + 7.0, 14.0, if xh { th.text } else { th.dim });
                i.hit(xr, rm);
            }
        }
        y += lo.s4;
    }
    // Where a dragged row would land: a line between rows, or the folder lit.
    if dragging.is_some()
        && let Some((folder, before)) = bm_drop(&i.page.bm_rows, i.mouse.1)
    {
        let rows = i.page.bm_rows.clone();
        let target = match before {
            Some(b) => rows.iter().find(|d| d.id == b).map(|d| d.r.y - 1.0),
            None => rows.iter().filter(|d| d.folder == folder && d.id != 0).map(|d| d.r.bottom() + 1.0).last(),
        };
        match target {
            Some(ly) => i.gfx.line(col.x + 12.0, ly, col.right() - 12.0, ly, th.text, 2.0),
            None => {
                if let Some(h) = rows.iter().find(|d| d.folder == folder && d.id == 0) {
                    i.gfx.round_stroke(h.r, i.look.round(h.r.h), th.text, 1.5);
                }
            }
        }
    }
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);

    // The header: title, one line under it, the search box and its two buttons.
    let flash = i.page.flash.as_ref().filter(|(_, at)| at.elapsed().as_secs_f32() < 8.0).map(|(m, _)| m.clone());
    let sub = flash.unwrap_or_else(|| match s.bookmarks.len() {
        0 => "Ctrl+D, or the star in the address bar, adds one.".to_string(),
        1 => "1 bookmark. Ctrl+Shift+O opens this page.".to_string(),
        n => format!("{n} bookmarks. Ctrl+Shift+O opens this page."),
    });
    title_block(i, col, st.y + i.look.s5 + 2.0, "Bookmarks", &sub);
    let focused = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } == i.bm_edit;
    i.gfx.round(sr, i.look.round(sr.h), th.raise);
    i.gfx.round_stroke(sr, i.look.round(sr.h), if focused { th.text.alpha(0.45) } else { th.line }, if focused { 1.5 } else { 1.0 });
    i.gfx.icon("search", sr.x + 12.0, sr.y + 10.0, 14.0, th.faint);
    if q.is_empty() && !focused {
        i.gfx.text("Search bookmarks", R::new(sr.x + 34.0, sr.y, sr.w - 44.0, sr.h), i.look.body, th.faint);
    }
    let bf = i.look.body.align(Align::Center);
    let w_import = i.gfx.measure("Import…", bf) + 2.0 * i.look.s4;
    let w_folder = i.gfx.measure("New folder", bf) + 2.0 * i.look.s4;
    let by = sr.y + 2.0;
    let bx = outline_button(i, col.right() - w_import, by, "Import…", PageHit::BmImport, false).x;
    outline_button(i, bx - i.look.s2 - w_folder, by, "New folder", PageHit::BmNewFolder, false);
    close_button(i, st);
}

// -------------------------------------------------------------- about

/// One release as the What's new page lays it out: its heading, and its lines, each
/// (text, kind): 0 a paragraph line, 1 a bullet's first line, 2 a bullet's next lines,
/// 3 the gap after a paragraph.
pub(super) struct ChangeBlock {
    version: String,
    date: String,
    lines: Vec<(String, u8)>,
}

const CHANGE_LINE: f32 = 21.0;

/// The changelog, broken into lines for a card `w` wide.
fn change_blocks(i: &Inner, w: f32) -> Vec<ChangeBlock> {
    let f = i.look.body;
    crate::changelog::releases()
        .into_iter()
        .map(|r| {
            let mut lines = Vec::new();
            for p in &r.intro {
                lines.extend(wrap_lines(i, p, f, w).into_iter().map(|l| (l, 0)));
                lines.push((String::new(), 3));
            }
            for it in &r.items {
                for (n, l) in wrap_lines(i, it, f, w - 16.0).into_iter().enumerate() {
                    lines.push((l, if n == 0 { 1 } else { 2 }));
                }
            }
            if lines.last().is_some_and(|l| l.1 == 3) {
                lines.pop();
            }
            ChangeBlock { version: r.version, date: r.date, lines }
        })
        .collect()
}

/// What's new: the changelog that ships inside Lumen, newest first, a card per version.
fn draw_changes(i: &mut Inner, st: R) {
    let th = i.theme;
    let lo = i.look.clone();
    let col = column(st, COL_W);
    let text_w = col.w - 36.0;
    if i.page.changes.as_ref().is_none_or(|(w, _)| (*w - text_w).abs() > 0.5) {
        let blocks = change_blocks(i, text_w);
        i.page.changes = Some((text_w, blocks));
    }
    let now = env!("LUMEN_VERSION");
    i.gfx.clip(st);
    let mut y = st.y + lo.s5 + 2.0 - i.page.scroll;
    y = title_block(i, col, y, "What's new", &format!("Lumen {now} · the changelog, built in"));
    let blocks = i.page.changes.take().map(|(w, b)| (w, b)).unwrap_or((text_w, Vec::new()));
    for b in &blocks.1 {
        let label = if b.date.is_empty() { b.version.clone() } else { format!("{} · {}", b.version, b.date) };
        let label = if b.version == now { format!("{label} · this version") } else { label };
        let card_h = 28.0 + b.lines.len() as f32 * CHANGE_LINE - b.lines.iter().filter(|l| l.1 == 3).count() as f32 * (CHANGE_LINE - 8.0);
        if y + 28.0 + card_h >= st.y && y <= st.bottom() {
            i.label(&label, R::new(col.x + 4.0, y, col.w, 18.0), if b.version == now { th.text } else { th.faint });
            let card = R::new(col.x, y + 26.0, col.w, card_h);
            i.gfx.round(card, lo.r_card, th.raise);
            i.gfx.round_stroke(card, lo.r_card, th.line, 1.0);
            let mut ly = card.y + 14.0;
            for (text, kind) in &b.lines {
                match kind {
                    3 => {
                        ly += 8.0;
                        continue;
                    }
                    1 => {
                        i.gfx.circle(card.x + 22.0, ly + CHANGE_LINE / 2.0, 2.0, th.dim);
                        i.gfx.text(text, R::new(card.x + 34.0, ly, text_w - 16.0, CHANGE_LINE), lo.body, th.text);
                    }
                    2 => i.gfx.text(text, R::new(card.x + 34.0, ly, text_w - 16.0, CHANGE_LINE), lo.body, th.text),
                    _ => i.gfx.text(text, R::new(card.x + 18.0, ly, text_w, CHANGE_LINE), lo.body, th.dim),
                }
                ly += CHANGE_LINE;
            }
        }
        y += 26.0 + card_h + lo.s5;
    }
    i.page.changes = Some(blocks);
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

/// Where the report page's "what happened" box sits, in stage coordinates. The edit control
/// (mod.rs) is placed over it; the page's head does not scroll, so neither does the box.
pub fn bug_story_rect(st: R, s5: f32) -> R {
    let col = column(st, COL_W);
    R::new(col.x, st.y + s5 + 2.0 + 74.0 + 40.0 + 22.0, col.w, 44.0)
}

/// Report a problem: the whole report on the page, so the person reads what it says before
/// it goes anywhere; a box for what happened; Send, Copy, Save.
fn draw_bugreport(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let lo = i.look.clone();
    let col = column(st, COL_W);
    let story = super::wide_text(i.bug_edit);
    let flash = i.page.flash.as_ref().filter(|(_, at)| at.elapsed().as_secs_f32() < 8.0).map(|(m, _)| m.clone());
    let sub = flash.unwrap_or_else(|| "Read it all, then send it".to_string());
    let mut y = title_block(i, col, st.y + lo.s5 + 2.0, "Report a problem", &sub);
    i.gfx.text_wrap(
        "Nothing leaves this PC until you press Send, and then only the text below, to zemo's inbox. It holds no addresses, no page titles and nothing typed into pages.",
        R::new(col.x + 4.0, y - 4.0, col.w - 8.0, 36.0),
        lo.small,
        th.faint,
    );
    y += 40.0;
    i.label("What happened", R::new(col.x + 4.0, y, col.w, 16.0), th.faint);
    // The box itself is a real edit window over this card.
    let field = bug_story_rect(st, lo.s5);
    let focused = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } == i.bug_edit;
    i.gfx.round(field, lo.r_card, th.raise);
    i.gfx.round_stroke(field, lo.r_card, if focused { th.text.alpha(0.45) } else { th.line }, if focused { 1.5 } else { 1.0 });
    if story.is_empty() && !focused {
        i.gfx.text("What you did, what you expected, what Lumen did instead", R::new(field.x + 16.0, field.y, field.w - 32.0, field.h), lo.body, th.faint);
    }
    y = field.bottom() + lo.s3;
    let mut x = col.x;
    let sent = s.bug_sent;
    x = outline_button(i, x, y, if sent { "Sent" } else { "Send to zemo" }, PageHit::BugSend, true).right() + lo.s2;
    if sent {
        // Sent once is enough: the button goes quiet until the page opens again.
        let r = R::new(col.x, y, x - lo.s2 - col.x, 30.0);
        i.gfx.round(r, lo.round(30.0), th.raise.alpha(0.55));
    }
    x = outline_button(i, x, y, "Copy report", PageHit::BugCopy, false).right() + lo.s2;
    outline_button(i, x, y, "Save to Downloads", PageHit::BugSave, false);
    y += 30.0 + lo.s4;

    // The report as it will be sent, with the words from the box in it, in the monospace face.
    let below = R::new(st.x, y - 4.0, st.w, (st.bottom() - y + 4.0).max(0.0));
    i.gfx.clip(below);
    let top = y;
    let mut y = top - i.page.scroll;
    let mono = lo.mono_at(12.0, 450);
    let text = s.bug_report.as_deref().map(|r| crate::bugreport::with_story(r, &story)).unwrap_or_else(|| "Gathering…".to_string());
    let w = col.w - 36.0;
    if i.page.bug_lines.as_ref().is_none_or(|(bw, n, _)| (*bw - w).abs() > 0.5 || *n != text.len()) {
        let lines: Vec<String> = text.lines().flat_map(|l| if l.trim().is_empty() { vec![String::new()] } else { wrap_lines(i, l, mono, w) }).collect();
        i.page.bug_lines = Some((w, text.len(), lines));
    }
    let lines = i.page.bug_lines.as_ref().map(|b| b.2.clone()).unwrap_or_default();
    const LINE: f32 = 19.0;
    let card = R::new(col.x, y, col.w, 28.0 + lines.len() as f32 * LINE);
    i.gfx.round(card, lo.r_card, th.raise);
    i.gfx.round_stroke(card, lo.r_card, th.line, 1.0);
    for (n, l) in lines.iter().enumerate() {
        let ly = card.y + 14.0 + n as f32 * LINE;
        if ly + LINE < below.y || ly > st.bottom() {
            continue;
        }
        i.gfx.text(l, R::new(card.x + 18.0, ly, w, LINE), mono, if n == 0 { th.text } else { th.dim });
    }
    y = card.bottom() + lo.s5;
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

/// A Settings section's icon in the menu.
pub(super) fn section_icon(name: &str) -> &'static str {
    match name {
        "General" => "globe",
        "Appearance" => "palette",
        "Fonts" => "type",
        "Layout" => "grid",
        "Clock and new tab" => "clock",
        "Sidebar" => "sidebar",
        "Shape" => "shapes",
        "Motion" => "wind",
        "Search" => "search",
        "Translation" => "languages",
        "Performance" => "zap",
        "Passwords" => "key",
        "Site permissions" => "lock",
        "AI agent" => "bot",
        "Privacy" => "shield",
        "Keys" => "keyboard",
        "About" => "info",
        _ => "sliders",
    }
}

/// The Settings search box, in the menu under the title. The edit control (mod.rs) sits on
/// top of this rectangle.
pub fn set_search_rect(st: R) -> R {
    R::new(st.x + 14.0, st.y + 90.0, SET_NAV_W - 28.0, 34.0)
}

const SET_NAV_W: f32 = 224.0;

/// The words a setting is found by: its name, its hint, its choices; for the rows that are
/// a whole widget, what they are about.
fn item_words(it: &Item) -> String {
    use Item::*;
    match it {
        Seg(_, l, h, o) | Pick(_, l, h, o) => format!("{l} {h} {}", o.iter().map(|(_, n)| *n).collect::<Vec<_>>().join(" ")),
        Toggle(_, l, h) | Range(_, l, h, _, _) | Fonts(_, l, h) | Text(_, l, h) => format!("{l} {h}"),
        Button(l, h, b, _) => format!("{l} {h} {b}"),
        Reset(_, l) => l.to_string(),
        Colors => "look signal glass precise colours colors palette accent".into(),
        Themes => "saved looks themes export import".into(),
        Wallpaper => "wallpaper image picture background".into(),
        Layout => "layout vertical horizontal sidebar toolbar".into(),
        Engine => format!("search engine {}", ENGINES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" ")),
        Keys => format!("keys keyboard shortcuts {}", KEYS.iter().map(|(n, k)| format!("{n} {k}")).collect::<Vec<_>>().join(" ")),
        KeePass => "keepass kdbx passwords vault".into(),
        AgentSetup => "agent mcp setup lumen-agent claude".into(),
        SitePerms => "site permissions camera microphone location notifications".into(),
        DefaultBrowser => "default browser windows".into(),
    }
}

/// Every word typed is in the section's name or the setting's words.
pub(super) fn item_matches(q: &str, section: &str, it: &Item) -> bool {
    let hay = format!("{section} {}", item_words(it)).to_lowercase();
    q.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
}

/// The name a setting shows in the command palette: the words its row is called by,
/// for the rows that are a whole widget.
pub(super) fn pal_label(it: &Item) -> String {
    use Item::*;
    match it {
        Seg(_, l, _, _) | Pick(_, l, _, _) | Toggle(_, l, _) | Range(_, l, _, _, _) | Fonts(_, l, _) | Text(_, l, _) => l.to_string(),
        Button(l, _, _, _) | Reset(_, l) => l.to_string(),
        Colors => "Look".into(),
        Themes => "Saved themes".into(),
        Wallpaper => "Wallpaper".into(),
        Layout => "Layout".into(),
        Engine => "Search engine".into(),
        Keys => "Keyboard shortcuts".into(),
        KeePass => "KeePass".into(),
        AgentSetup => "Agent setup".into(),
        SitePerms => "Site permissions".into(),
        DefaultBrowser => "Default browser".into(),
    }
}

fn draw_settings(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let lo = i.look.clone();
    let q = super::wide_text(i.set_edit).trim().to_string();
    let searching = !q.is_empty();
    // While searching, each section keeps only the settings that match; a section with
    // none is left out of the page and dimmed in the menu.
    let schema: Vec<(&'static str, Vec<Item>)> =
        settings_schema().into_iter().map(|(name, items)| (name, items.into_iter().filter(|it| !searching || item_matches(&q, name, it)).collect())).collect();
    // Left: the title, the build, and the sections as a menu with icons.
    let nav = R::new(st.x, st.y, SET_NAV_W, st.h);
    i.gfx.line(nav.right(), st.y + lo.s4, nav.right(), st.bottom() - lo.s4, th.line, 1.0);
    i.gfx.text("Settings", R::new(nav.x + 24.0, nav.y + 26.0, nav.w - 32.0, 36.0), lo.font(26.0, 700), th.text);
    // Which build this is, under the title: the question people ask before reporting a bug.
    let build = env!("LUMEN_BUILD").split_whitespace().next().unwrap_or("");
    i.label(&format!("Lumen {} · {build}", env!("LUMEN_VERSION")), R::new(nav.x + 24.0, nav.y + 64.0, nav.w - 32.0, 16.0), th.faint);
    // The search box: a pill, the box itself is the edit control over it.
    let sr = set_search_rect(st);
    let focused = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } == i.set_edit;
    i.gfx.round(sr, lo.round(sr.h), th.raise);
    i.gfx.round_stroke(sr, lo.round(sr.h), if focused { th.text.alpha(0.45) } else { th.line }, if focused { 1.5 } else { 1.0 });
    i.gfx.icon("search", sr.x + 12.0, sr.y + 10.0, 14.0, th.faint);
    if q.is_empty() && !focused {
        i.gfx.text("Search settings", R::new(sr.x + 34.0, sr.y, sr.w - 44.0, sr.h), lo.body, th.faint);
    }
    let current = if searching { usize::MAX } else { i.page.sections.iter().rposition(|y| *y <= i.page.scroll + 30.0).unwrap_or(0) };
    for (n, (name, items)) in schema.iter().enumerate() {
        let r = R::new(nav.x + lo.s3, sr.bottom() + 12.0 + n as f32 * 34.0, nav.w - 2.0 * lo.s3, 32.0);
        let h = Hit::Page(PageHit::Nav(n));
        let on = n == current;
        let empty = items.is_empty();
        if on {
            i.gfx.round(r, lo.round(r.h), th.pill);
        } else if i.is_hover(&h) && !empty {
            i.gfx.round(r, lo.round(r.h), th.hover);
        }
        let ink = if on { th.pill_ink } else if empty { th.faint.alpha(0.6) } else { th.dim };
        i.gfx.icon(section_icon(name), r.x + 12.0, r.y + 8.0, 16.0, ink.alpha(if on { 1.0 } else { 0.85 }));
        i.gfx.text(name, R::new(r.x + 38.0, r.y, r.w - 44.0, r.h), lo.body, ink);
        i.hit(r, h);
    }

    // At the foot of the menu, the credit: one quiet line. Only the name is a link, to the
    // portfolio; nothing else about it asks to be clicked.
    let credit = R::new(nav.x + lo.s3 + 12.0, st.bottom() - 40.0, nav.w - 2.0 * lo.s3 - 12.0, 22.0);
    let last = sr.bottom() + 12.0 + schema.len() as f32 * 34.0;
    if credit.y > last + 8.0 {
        let f = lo.font(11.0, 450);
        let lead = "made by ";
        let lw = i.gfx.measure(lead, f);
        let nw = i.gfx.measure("zemo", f);
        i.gfx.text(lead, R::new(credit.x, credit.y, lw + 2.0, credit.h), f, th.faint.alpha(0.8));
        let name = R::new(credit.x + lw, credit.y, nw + 2.0, credit.h);
        let h = Hit::Page(PageHit::OpenTab(PORTFOLIO));
        let hov = i.is_hover(&h);
        i.gfx.text("zemo", name, f, if hov { th.text } else { th.faint.alpha(0.8) });
        if hov {
            let y = name.y + name.h / 2.0 + 7.0;
            i.gfx.line(name.x, y, name.x + nw, y, th.text, 1.0);
        }
        i.hit(name, h);
    }

    // Right: each section a label and a card of rows, scrolled.
    let body = R::new(nav.right() + 1.0, st.y, st.w - nav.w - 1.0, st.h);
    let col = R::new(body.x + 40.0, body.y, (body.w - 96.0).clamp(160.0, 720.0), body.h);
    let inner = |r: R| R::new(r.x + 18.0, r.y, r.w - 36.0, r.h);
    // How tall each section's card is: its rows drawn once out of sight, their hits dropped.
    let mark = i.hits.len();
    i.gfx.clip(R::new(-10.0, -10.0, 1.0, 1.0));
    let heights: Vec<Vec<f32>> = schema
        .iter()
        .map(|(_, items)| items.iter().map(|it| draw_item(i, it, R::new(inner(col).x, -2000.0, inner(col).w, 54.0), s) + 1.0).collect())
        .collect();
    i.gfx.unclip();
    i.hits.truncate(mark);
    i.gfx.clip(body);
    let mut y = body.y + 34.0 - i.page.scroll;
    let mut sections = Vec::new();
    if searching && schema.iter().all(|(_, items)| items.is_empty()) {
        empty_state(i, R::new(col.x, y + 40.0, col.w, 200.0), "search", "Nothing matches", "Try fewer words, or another name for it.");
    }
    for ((name, items), hs) in schema.iter().zip(heights) {
        sections.push(y + i.page.scroll - body.y - 34.0);
        if items.is_empty() {
            continue;
        }
        i.label(name, R::new(col.x + 4.0, y, col.w, 18.0), th.faint);
        y += 26.0;
        let card_h: f32 = hs.iter().sum::<f32>() + 10.0;
        let card = R::new(col.x, y, col.w, card_h);
        if card.bottom() >= body.y && card.y <= body.bottom() {
            i.gfx.round(card, lo.r_card, th.raise);
            i.gfx.round_stroke(card, lo.r_card, th.line, 1.0);
        }
        y += 5.0;
        for (n, (it, h)) in items.iter().zip(hs.iter()).enumerate() {
            if y + h >= body.y && y <= body.bottom() {
                draw_item(i, it, R::new(inner(col).x, y, inner(col).w, 54.0), s);
            }
            y += h;
            if n + 1 < items.len() && !matches!(it, Item::Keys) {
                i.gfx.line(card.x + 1.0, y - 1.0, card.right() - 1.0, y - 1.0, th.line, 1.0);
            }
        }
        y += 5.0 + 30.0;
    }
    i.gfx.unclip();
    i.page.sections = sections;
    i.page.max = (y + i.page.scroll - body.bottom() + 20.0).max(0.0);
    close_button(i, st);
}

// -------------------------------------------------------------- site settings

/// What a permission row acts on: one site's answer, or the default for a kind.
#[derive(Clone, Copy)]
enum PermTarget<'a> {
    Site(&'a str, &'a str),
    Default(&'static str),
}

/// The Allow / Block / Ask choice of one permission row, as a segmented control centred on
/// `cy`. `cur` is what is remembered (None = ask). A default row offers Block and Ask only:
/// there is nothing to allow before a site asks.
fn perm_chips(i: &mut Inner, right: f32, cy: f32, target: PermTarget<'_>, cur: Option<bool>) -> f32 {
    let th = i.theme;
    let lo = i.look.clone();
    let f = lo.font(12.5, 450).align(Align::Center);
    let opts: &[(&'static str, Option<bool>)] =
        if matches!(target, PermTarget::Site(..)) { &[("Allow", Some(true)), ("Block", Some(false)), ("Ask", None)] } else { &[("Block", Some(false)), ("Ask", None)] };
    let widths: Vec<f32> = opts.iter().map(|(l, _)| i.gfx.measure(l, f) + 22.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let pill = R::new(right - total, cy - 15.0, total, 30.0);
    i.gfx.round(pill, lo.round(30.0), th.bg);
    i.gfx.round_stroke(pill, lo.round(30.0), th.line, 1.0);
    let mut x = pill.x + 2.0;
    for ((label, v), w) in opts.iter().zip(widths) {
        let r = R::new(x, pill.y + 2.0, w, 26.0);
        let h = Hit::Page(match target {
            PermTarget::Site(origin, kind) => PageHit::SitePerm(origin.to_string(), kind.to_string(), label),
            PermTarget::Default(kind) => PageHit::PermDefault(kind, if *label == "Block" { "block" } else { "ask" }),
        });
        let on = *v == cur;
        if on {
            i.gfx.round(r, lo.round(26.0), th.pill);
        } else if i.is_hover(&h) {
            i.gfx.round(r, lo.round(26.0), th.hover);
        }
        i.gfx.text(label, r, f, if on { th.pill_ink } else { th.dim });
        i.hit(r, h);
        x += w;
    }
    pill.x
}

/// A verdict as a chip state.
fn chip_state(v: crate::sites::Verdict) -> Option<bool> {
    match v {
        crate::sites::Verdict::Allow => Some(true),
        crate::sites::Verdict::Block => Some(false),
        crate::sites::Verdict::Ask => None,
    }
}

/// A card of `rows` rows of `row` height at y, with hairlines between them. Returns the
/// card; the rows are drawn by the caller, on top.
fn rows_card(i: &mut Inner, col: R, y: f32, rows: usize, row: f32) -> R {
    let th = i.theme;
    let card = R::new(col.x, y, col.w, rows.max(1) as f32 * row);
    i.gfx.round(card, i.look.r_card, th.raise);
    i.gfx.round_stroke(card, i.look.r_card, th.line, 1.0);
    for n in 1..rows {
        let ly = y + n as f32 * row;
        i.gfx.line(card.x + 1.0, ly, card.right() - 1.0, ly, th.line, 1.0);
    }
    card
}

/// Site settings: what each site may do, the zoom it is read at, and the data it left behind.
/// The defaults come first, so the page answers "how do I stop notifications?" without a site.
fn draw_sites(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let lo = i.look.clone();
    let col = column(st, COL_W);
    i.gfx.clip(st);
    let mut y = st.y + lo.s5 + 2.0 - i.page.scroll;
    let origins: Vec<&str> = {
        let mut o: Vec<&str> = s.rules.iter().map(|(o, _, _)| o.as_str()).collect();
        o.dedup();
        o
    };
    let sub = if origins.is_empty() && s.zooms.is_empty() {
        "How sites behave, and what to do about one in particular.".to_string()
    } else {
        format!("{} sites with answers · {} at their own zoom", origins.len(), s.zooms.len())
    };
    y = title_block(i, col, y, "Site settings", &sub);
    const ROW: f32 = 50.0;
    let inner = |r: R| R::new(r.x + 18.0, r.y, r.w - 36.0, r.h);

    // What pages open at, unless a site has a zoom of its own. Measured out of sight first,
    // so its card is drawn under it at the right height.
    y = group_label(i, col, y, "Pages open at", 0);
    let range = Item::Range(
        "default_zoom",
        "Default zoom",
        "Every site follows this unless it has a zoom of its own. Ctrl++ and Ctrl+- set one; the chip in the address bar takes it back to 100%.",
        Scale::Steps(ZOOM_STOPS),
        "%",
    );
    let mark = i.hits.len();
    i.gfx.clip(R::new(-10.0, -10.0, 1.0, 1.0));
    let h = draw_item(i, &range, R::new(inner(col).x, -2000.0, inner(col).w, 54.0), s);
    i.gfx.unclip();
    i.hits.truncate(mark);
    rows_card(i, col, y, 1, h + 10.0);
    draw_item(i, &range, R::new(inner(col).x, y + 5.0, inner(col).w, 54.0), s);
    y += h + 10.0 + lo.s5;

    // The default answer per kind.
    y = group_label(i, col, y, "When a site has no answer", 0);
    let card = rows_card(i, col, y, crate::sites::KINDS.len(), ROW);
    for (n, kind) in crate::sites::KINDS.iter().enumerate() {
        let r = R::new(card.x, y + n as f32 * ROW, card.w, ROW);
        if r.y > st.bottom() || r.bottom() < st.y {
            continue;
        }
        let cur = chip_state(crate::sites::default_for(&s.settings.perm_defaults, kind.key));
        i.gfx.text(kind.label, R::new(r.x + 18.0, r.y, r.w * 0.5, ROW), lo.font(13.0, 500), th.text);
        perm_chips(i, r.right() - 14.0, r.y + ROW / 2.0, PermTarget::Default(kind.key), cur);
    }
    y = card.bottom() + lo.s5;

    // One card per site: its name and what can be done about it, then its answers.
    y = group_label(i, col, y, "Answers remembered", origins.len());
    if origins.is_empty() {
        let card = rows_card(i, col, y, 1, 64.0);
        i.gfx.text_wrap(
            "No site has an answer yet. When a site asks for your camera, location or similar, the answer is remembered here and can be changed.",
            R::new(card.x + 18.0, card.y + 14.0, card.w - 36.0, 40.0),
            lo.small,
            th.faint,
        );
        y = card.bottom() + lo.s3;
    }
    for origin in origins {
        let answers: Vec<(&str, bool)> = s.rules.iter().filter(|(o, _, _)| o == origin).map(|(_, k, a)| (k.as_str(), *a)).collect();
        let card = rows_card(i, col, y, answers.len() + 1, ROW);
        if card.bottom() >= st.y && card.y <= st.bottom() {
            let host = super::host(origin);
            site_icon(i, R::new(card.x + 18.0, y + (ROW - 18.0) / 2.0, 18.0, 18.0), &host);
            i.gfx.text(&host, R::new(card.x + 48.0, y, card.w * 0.5, ROW), lo.body_strong, th.text);
            let right = row_action(i, card.right() - 12.0, y + (ROW - 28.0) / 2.0, "Clear data", PageHit::SiteClear(origin.to_string()));
            row_action(i, right, y + (ROW - 28.0) / 2.0, "Reset all", PageHit::SiteReset(origin.to_string()));
            for (n, (kind, allow)) in answers.iter().enumerate() {
                let r = R::new(card.x, y + (n + 1) as f32 * ROW, card.w, ROW);
                i.gfx.text(&crate::sites::label(kind), R::new(r.x + 48.0, r.y, r.w * 0.5, ROW), lo.body, th.dim);
                perm_chips(i, r.right() - 14.0, r.y + ROW / 2.0, PermTarget::Site(origin, kind), Some(*allow));
            }
        }
        y = card.bottom() + lo.s3;
    }
    y += lo.s5 - lo.s3;

    // Zoom by site: what the person chose, and the way back to the default.
    y = group_label(i, col, y, "Zoom by site", s.zooms.len());
    let card = rows_card(i, col, y, s.zooms.len(), ROW);
    if s.zooms.is_empty() {
        i.gfx.text("Every site opens at the default zoom.", R::new(card.x + 18.0, y, card.w - 36.0, ROW), lo.small, th.faint);
    }
    for (n, (host, pct)) in s.zooms.iter().enumerate() {
        let r = R::new(card.x, y + n as f32 * ROW, card.w, ROW);
        if r.y > st.bottom() || r.bottom() < st.y {
            continue;
        }
        site_icon(i, R::new(r.x + 18.0, r.y + (ROW - 18.0) / 2.0, 18.0, 18.0), host);
        i.gfx.text(host, R::new(r.x + 48.0, r.y, r.w * 0.6, ROW), lo.body, th.text);
        let left = row_action(i, r.right() - 12.0, r.y + (ROW - 28.0) / 2.0, "Reset", PageHit::SiteZoom(host.clone()));
        i.gfx.text(&format!("{pct}%"), R::new(left - 80.0, r.y, 72.0, ROW), lo.mono_at(12.0, 500).align(Align::Right), th.dim);
    }
    y = card.bottom() + lo.s5;

    // The hosts that may stay on plain http.
    if !s.settings.http_ok.is_empty() {
        y = group_label(i, col, y, "May stay on plain http", s.settings.http_ok.len());
        let card = rows_card(i, col, y, s.settings.http_ok.len(), ROW);
        for (n, host) in s.settings.http_ok.iter().enumerate() {
            let r = R::new(card.x, y + n as f32 * ROW, card.w, ROW);
            if r.y > st.bottom() || r.bottom() < st.y {
                continue;
            }
            i.gfx.text(host, R::new(r.x + 18.0, r.y, r.w * 0.6, ROW), lo.body, th.text);
            row_action(i, r.right() - 12.0, r.y + (ROW - 28.0) / 2.0, "Reset", PageHit::SiteHttp(host.clone()));
        }
        y = card.bottom() + lo.s5;
    }

    // The hosts where the person turned ad blocking off, and the way back.
    if !s.settings.adblock_off_sites.is_empty() {
        y = group_label(i, col, y, "Ads allowed on", s.settings.adblock_off_sites.len());
        let card = rows_card(i, col, y, s.settings.adblock_off_sites.len(), ROW);
        for (n, host) in s.settings.adblock_off_sites.iter().enumerate() {
            let r = R::new(card.x, y + n as f32 * ROW, card.w, ROW);
            if r.y > st.bottom() || r.bottom() < st.y {
                continue;
            }
            site_icon(i, R::new(r.x + 18.0, r.y + (ROW - 18.0) / 2.0, 18.0, 18.0), host);
            i.gfx.text(host, R::new(r.x + 48.0, r.y, r.w * 0.6, ROW), lo.body, th.text);
            row_action(i, r.right() - 12.0, r.y + (ROW - 28.0) / 2.0, "Reset", PageHit::SiteAdblock(host.clone()));
        }
        y = card.bottom() + lo.s5;
    }

    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

fn mem(b: f64) -> String {
    let mb = b / 1048576.0;
    if mb >= 1024.0 {
        format!("{:.2} GB", mb / 1024.0)
    } else if mb >= 10.0 {
        format!("{} MB", mb.round())
    } else {
        format!("{mb:.1} MB")
    }
}

fn cpu(c: f64) -> String {
    if c >= 10.0 { format!("{}%", c.round()) } else { format!("{c:.1}%") }
}

fn net(bps: f64) -> String {
    if bps < 1.0 {
        "—".into()
    } else if bps >= 1048576.0 {
        format!("{:.1} MB/s", bps / 1048576.0)
    } else {
        format!("{} KB/s", (bps / 1024.0).round().max(1.0))
    }
}

/// One line of the Control table. `id` is the tab's id; 0 for the rows that are not tabs.
struct UsageRow {
    id: u64,
    name: String,
    state: &'static str,
    mem: f64,
    cpu: f64,
    /// Bytes per second, or None for rows that have no network figure of their own.
    net: Option<f64>,
    live: bool,
    /// The tab can be unloaded from here.
    unload: bool,
}

fn draw_control(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let have = i.page.usage.is_some();
    let u = i.page.usage.clone().unwrap_or(json!({}));
    let col = column(st, 860.0);
    i.gfx.clip(st);
    let mut y = st.y + i.look.s5 + 2.0 - i.page.scroll;
    y = title_block(i, col, y, "Control", if have { "What each tab costs, right now. Updates every second." } else { "Measuring…" });

    // Four totals, side by side.
    let t = &u["total"];
    let tiles = [
        ("MEMORY", t["mem"].as_f64().map(mem), "all processes"),
        ("CPU", t["cpu"].as_f64().map(cpu), "of the whole machine"),
        ("NETWORK", t["net"].as_f64().map(|n| if n < 1.0 { "0 KB/s".into() } else { net(n) }), "pages, all tabs"),
        ("PROCESSES", t["procs"].as_u64().map(|p| p.to_string()), "Lumen and its pages"),
    ];
    let gap = i.look.s3;
    let tw = (col.w - 3.0 * gap) / 4.0;
    for (n, (label, value, note)) in tiles.into_iter().enumerate() {
        let r = R::new(col.x + n as f32 * (tw + gap), y, tw, 92.0);
        i.gfx.round(r, i.look.r_card, th.raise);
        i.gfx.round_stroke(r, i.look.r_card, th.line, 1.0);
        i.label(label, R::new(r.x + i.look.s4, r.y + i.look.s3, r.w - 2.0 * i.look.s4, 16.0), th.faint);
        let (v, c) = match value {
            Some(v) => (v, th.text),
            None => ("—".to_string(), th.faint),
        };
        i.gfx.text(&v, R::new(r.x + i.look.s4, r.y + 28.0, r.w - 2.0 * i.look.s4, 40.0), i.look.big_num, c);
        i.gfx.text(note, R::new(r.x + i.look.s4, r.y + 68.0, r.w - 2.0 * i.look.s4, 18.0), i.look.small, th.faint);
    }
    y += 92.0 + i.look.s5;

    // The table. Columns hang off the right edge so they line up from row to row.
    let net_r = col.right() - 80.0;
    let cpu_r = net_r - 96.0;
    let mem_r = cpu_r - 72.0;
    let bar_x = mem_r - 80.0 - i.look.s4 - 96.0;
    let name_w = (bar_x - i.look.s4 - col.x).max(80.0);
    i.label("Tab", R::new(col.x, y, 200.0, 20.0), th.faint);
    for (label, right) in [("MEMORY", mem_r), ("CPU", cpu_r), ("NETWORK", net_r)] {
        let lw = i.gfx.measure_tracked(&label.to_uppercase(), i.look.label, i.look.label_track);
        i.label(label, R::new(right - lw, y, lw + 4.0, 20.0), th.faint);
    }
    y += 24.0;
    i.gfx.line(col.x, y, col.right(), y, th.line, 1.0);

    let total_mem = t["mem"].as_f64().unwrap_or(0.0).max(1.0);
    let mut rows: Vec<UsageRow> = Vec::new();
    for tab in &s.tabs {
        let x = &u["tabs"][tab.id.to_string()];
        let live = tab.status == "live" || tab.status == "asleep";
        let state = match tab.status {
            "off" => "Unloaded",
            "asleep" => "Frozen",
            "crashed" => "Crashed",
            _ if tab.id == s.active => "Active",
            _ => "",
        };
        let name = if tab.title.is_empty() { if tab.status == "new" { "New tab".into() } else { super::host(&tab.url) } } else { tab.title.clone() };
        rows.push(UsageRow {
            id: tab.id,
            name,
            state,
            mem: x["mem"].as_f64().unwrap_or(0.0),
            cpu: x["cpu"].as_f64().unwrap_or(0.0),
            net: Some(x["net"].as_f64().unwrap_or(0.0)),
            live,
            unload: live && tab.id != s.active,
        });
    }
    rows.sort_by(|a, b| b.mem.total_cmp(&a.mem));
    let (ui, core) = (&u["ui"], &u["core"]);
    for (name, state, v) in [("Lumen", "interface", ui), ("Browser core", "shared", core)] {
        rows.push(UsageRow { id: 0, name: name.into(), state, mem: v["mem"].as_f64().unwrap_or(0.0), cpu: v["cpu"].as_f64().unwrap_or(0.0), net: None, live: true, unload: false });
    }
    for (n, row) in rows.iter().enumerate() {
        let r = R::new(col.x - i.look.s2, y + 2.0, col.w + 2.0 * i.look.s2, ROW_H - 4.0);
        // Tab rows light up; the two rows that are not tabs are only there to add up.
        let hov = row.id != 0 && i.is_hover(&Hit::Page(PageHit::Row(row.id)));
        if row.id != 0 {
            i.hit(r, Hit::Page(PageHit::Row(row.id)));
        }
        if hov {
            i.gfx.round(r, i.look.r_md, th.hover);
        }
        let c_text = if row.live { th.text } else { th.faint };
        // A live tab with no process found yet has no figures, not zeros.
        let known = row.live && row.mem > 0.0;
        let nw = i.gfx.measure(&row.name, i.look.body).min(name_w - 90.0).max(40.0);
        i.gfx.text(&row.name, R::new(col.x, y, nw + 2.0, ROW_H), i.look.body, c_text);
        if !row.state.is_empty() {
            let c = if row.state == "Active" { th.text } else { th.faint };
            i.gfx.text(row.state, R::new(col.x + nw + i.look.s3, y, 90.0, ROW_H), i.look.small, c);
        }
        if known {
            // Share of everything Lumen uses: a track and a fill, so the heavy rows show.
            let bar = R::new(bar_x, y + ROW_H / 2.0 - 2.0, 96.0, 4.0);
            i.gfx.round(bar, 2.0, th.press);
            let w = (96.0 * (row.mem / total_mem).min(1.0) as f32).max(if row.mem > 0.0 { 3.0 } else { 0.0 });
            if w > 0.0 {
                i.gfx.round(R::new(bar.x, bar.y, w, 4.0), 2.0, th.text.alpha(0.85));
            }
        }
        let vals = [
            if known { mem(row.mem) } else { "—".into() },
            if known { cpu(row.cpu) } else { "—".into() },
            match row.net {
                Some(b) if known => net(b),
                _ => "—".into(),
            },
        ];
        for (k, (v, right)) in vals.iter().zip([mem_r, cpu_r, net_r]).enumerate() {
            let strong = k == 0 && row.live;
            let c = if !row.live || v == "—" { th.faint } else if k == 0 { th.text } else { th.dim };
            i.gfx.text(v, R::new(right - 90.0, y, 90.0, ROW_H), i.look.font(13.0, if strong { 550 } else { 400 }).align(Align::Right), c);
        }
        if row.unload {
            row_action(i, col.right(), y + (ROW_H - 28.0) / 2.0, "Unload", PageHit::Unload(row.id));
        }
        y += ROW_H;
        if n + 1 < rows.len() {
            i.gfx.line(col.x, y, col.right(), y, th.line.alpha(0.6), 1.0);
        }
    }
    // The sum, set apart: what the whole browser is using.
    i.gfx.line(col.x, y, col.right(), y, th.faint.alpha(0.5), 1.0);
    i.gfx.text("Total", R::new(col.x, y, 200.0, ROW_H), i.look.body_strong, th.text);
    let totals = [t["mem"].as_f64().map(mem), t["cpu"].as_f64().map(cpu), t["net"].as_f64().map(net)];
    for (v, right) in totals.iter().zip([mem_r, cpu_r, net_r]) {
        let text = v.clone().unwrap_or_else(|| "—".into());
        i.gfx.text(&text, R::new(right - 90.0, y, 90.0, ROW_H), i.look.mono_strong.align(Align::Right), if v.is_some() { th.text } else { th.faint });
    }
    y += ROW_H + i.look.s5 + i.look.s2;

    i.gfx.text("Limits", R::new(col.x, y, 300.0, 30.0), i.look.h2, th.text);
    y += 40.0;
    let items = limits_schema();
    for (n, it) in items.iter().enumerate() {
        let h = draw_item(i, it, R::new(col.x, y, col.w, 54.0), s);
        y += h;
        if n + 1 < items.len() {
            i.gfx.line(col.x, y, col.right(), y, th.line, 1.0);
        }
        y += 1.0;
    }
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}


// ------------------------------------------------------------------- input

pub fn press(i: &mut Inner, p: PageHit, x: f32, y: f32) {
    if let PageHit::Bm(id) = p {
        i.page.bm_drag = Some(BmDrag { id, y0: y, moved: false });
        // Keep the mouse while it is down, so a drag that leaves the window still ends.
        unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(i.hwnd);
        }
        return;
    }
    if let PageHit::Slider(key) = p
        && let Some((_, track, scale)) = i.page.sliders.iter().find(|(k, _, _)| *k == key).map(|t| (t.0, t.1, t.2))
    {
        i.page.slide = Some(Slide { key, track, scale });
        drag_to(i, x);
    }
}

fn drag_to(i: &mut Inner, x: f32) {
    let Some(sl) = &i.page.slide else { return };
    let v = value_at(sl.scale, (x - sl.track.x) / sl.track.w);
    let key = sl.key;
    set(i, key, json!(v.round() as u32));
}

pub fn drag_move(i: &mut Inner, x: f32, y: f32) -> bool {
    if let Some(d) = &mut i.page.bm_drag {
        // A few pixels of wobble is still a click.
        if !d.moved && (y - d.y0).abs() > 6.0 {
            d.moved = true;
        }
        if d.moved {
            i.invalidate();
            return true;
        }
        return false;
    }
    if i.page.slide.is_none() {
        return false;
    }
    drag_to(i, x);
    true
}

pub fn drag_end(i: &mut Inner, y: f32) -> bool {
    if let Some(d) = i.page.bm_drag.take() {
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
        if !d.moved {
            return false;
        }
        if let Some((folder, before)) = bm_drop(&i.page.bm_rows, y) {
            send(&i.proxy, json!({"t": "bmPlace", "id": d.id, "folder": folder, "before": before}));
        }
        i.invalidate();
        return true;
    }
    i.page.slide.take().is_some()
}

pub fn click(i: &mut Inner, p: PageHit) {
    let Some(s) = i.state.clone() else { return };
    match p {
        PageHit::Seg(k, v) => set(i, k, json!(v)),
        PageHit::FirstRunDone => set(i, "first_run_done", json!(true)),
        PageHit::Toggle(k) => {
            let on = get(&s.settings, k).as_bool().unwrap_or(false);
            set(i, k, json!(!on));
        }
        PageHit::AccentReset => set(i, "accent", json!("")),
        PageHit::Accent => {
            // The card opens under the swatch that was clicked.
            let anchor = i.hits.iter().find(|(_, h)| *h == Hit::Page(PageHit::Accent)).map(|(r, _)| *r);
            super::color::open(i, "accent", s.settings.accent.clone(), i.theme.accent, anchor);
        }
        PageHit::Color(k) => {
            let start = role_color(i, k, &s);
            let was = get(&s.settings, k).as_str().unwrap_or("").to_string();
            let anchor = i.hits.iter().find(|(_, h)| *h == Hit::Page(PageHit::Color(k))).map(|(r, _)| *r);
            super::color::open(i, k, was, start, anchor);
        }
        PageHit::ColorClear(k) => set(i, k, json!("")),
        PageHit::Text("wallpaper") => {
            super::files::open(i, super::files::FileWhat::Image);
        }
        PageHit::Text(k) => {
            let cur = get(&s.settings, k).as_str().unwrap_or("").to_string();
            super::form::open(i, "Edit", &[("Text", &cur)], super::form::FormAct::TextSetting { key: k.to_string() });
        }
        PageHit::Fonts(k) => {
            i.page.font_pick = Some(FontPick { key: k, filter: String::new(), scroll: 0.0, shown: Vec::new() });
            i.invalidate();
        }
        PageHit::FontPin(n) => {
            let (key, family) = match i.page.font_pick.as_ref() {
                Some(p) if n == 0 => (p.key, font_default_key(p.key).to_string()),
                Some(p) => match p.shown.get(n - 1) {
                    Some(f) => (p.key, f.clone()),
                    None => return,
                },
                None => return,
            };
            i.page.font_pick = None;
            set(i, key, json!(family));
        }
        PageHit::FontClose => {
            i.page.font_pick = None;
            i.invalidate();
        }
        PageHit::ThemeSave => {
            let cur = i.page.theme_name.clone();
            super::form::open(i, "Save this look", &[("Name", &cur)], super::form::FormAct::SaveLook);
        }
        PageHit::ThemeApply(n) => {
            let list = style::saved_themes();
            let Some(t) = list.get(n) else { return };
            let mut next = s.settings.clone();
            t.apply(&mut next);
            set(i, "palette", json!(next.palette));
            // set() sends one key at a time; the rest go with it in one message.
            send(&i.proxy, json!({"t": "settings", "v": serde_json::to_value(&next).unwrap_or_default()}));
        }
        PageHit::ThemeDelete(n) => {
            let mut list = style::saved_themes();
            if n < list.len() {
                let gone = list.remove(n);
                style::save_themes(&list);
                i.page.themes = Some(list);
                i.page.flash = Some((format!("Removed \"{}\"", gone.name), std::time::Instant::now()));
            }
            i.invalidate();
        }
        PageHit::ThemeExport => {
            let mut file = style::ThemeFile::of(&s.settings);
            file.name = i.page.theme_name.clone();
            let suggested = format!("{}.json", if file.name.is_empty() { "lumen-theme" } else { &file.name });
            super::files::open(i, super::files::FileWhat::ThemeSave { text: file.to_json(), suggested });
        }
        PageHit::ThemeImport => {
            super::files::open(i, super::files::FileWhat::ThemeOpen);
        }
        PageHit::ResetGroup(group) => reset_group(i, group),
        PageHit::Engine(n) => set(i, "search", json!(ENGINES[n].1)),
        PageHit::Nav(n) => {
            if let Some(y) = i.page.sections.get(n) {
                i.page.scroll = y.clamp(0.0, i.page.max);
                i.invalidate();
            }
        }
        PageHit::Done => send(&i.proxy, json!({"t": "panel", "open": false})),
        PageHit::Unload(id) => send(&i.proxy, json!({"t": "sleep", "id": id})),
        PageHit::Row(_) => {}
        PageHit::Slider(_) => {}
        PageHit::ClearPasswords => send(&i.proxy, json!({"t": "clearPasswords"})),
        PageHit::KdbxOpen => {
            // The file card picks the .kdbx; the password form follows it. The pair
            // crosses to the main loop on an event that is never logged.
            super::files::open(i, super::files::FileWhat::Kdbx);
        }
        PageHit::KdbxLock => send(&i.proxy, json!({"t": "kdbxLock"})),
        PageHit::CopyAgent => super::copy_text(i.hwnd_raw(), &s.agent_cmd),
        PageHit::Dl(a, id) => send(&i.proxy, json!({"t": "dl", "a": a, "id": id})),
        PageHit::DlClear => send(&i.proxy, json!({"t": "dl", "a": "clear"})),
        PageHit::DlOpenFolder => send(&i.proxy, json!({"t": "dl", "a": "openFolder"})),
        PageHit::DlAddLink => {
            // The clipboard decides: a link goes to aria2, anything else gets a note.
            let text = super::read_clipboard(i.hwnd_raw());
            match classify_link(&text) {
                "magnet" | "url" => send(&i.proxy, json!({"t": "dl", "a": "addUrl", "url": text})),
                _ => {
                    i.page.flash = Some(("Copy a link or magnet first".into(), std::time::Instant::now()));
                    i.invalidate();
                }
            }
        }
        PageHit::DlTorrentPick => {
            super::files::open(i, super::files::FileWhat::Torrent);
        }
        PageHit::DlLimit => {
            // The drawn menu of speeds aria2 accepts as max-overall-download-limit.
            let (x, y) = i.mouse;
            super::menu::open(
                i,
                &[(1, "No limit".into()), (2, "1 MB/s".into()), (3, "5 MB/s".into()), (4, "10 MB/s".into())],
                x,
                y,
                |i, cmd| {
                    let kbps = match cmd {
                        1 => 0,
                        2 => 1024,
                        3 => 5120,
                        4 => 10240,
                        _ => return,
                    };
                    send(&i.proxy, json!({"t": "dl", "a": "limit", "kbps": kbps}));
                },
            );
        }
        PageHit::DlNote => {
            // The firewall line is dismissed for good; it never comes back.
            set(i, "torrent_note", json!(false));
        }
        PageHit::Open(url) => send(&i.proxy, json!({"t": "nav", "url": url})),
        PageHit::Forget(url) => send(&i.proxy, json!({"t": "forget", "url": url})),
        PageHit::Bm(id) => {
            if let Some(m) = s.bookmarks.iter().find(|m| m.id == id) {
                send(&i.proxy, json!({"t": "nav", "url": m.url}));
            }
        }
        PageHit::BmDel(id) => send(&i.proxy, json!({"t": "bmDelete", "id": id})),
        PageHit::BmFolder(name) => {
            if !i.page.bm_collapsed.remove(&name) {
                i.page.bm_collapsed.insert(name);
            }
            i.invalidate();
        }
        PageHit::BmNewFolder => {
            super::form::open(i, "New folder", &[("Name", "")], super::form::FormAct::NewFolder);
        }
        PageHit::BmImport => {
            // Read-only look at the other browsers' files: which are here, and how many each.
            let found = crate::bookmarks::scan();
            if found.is_empty() {
                i.page.flash = Some(("No Chrome, Edge, Brave or Opera GX bookmarks found on this PC".into(), std::time::Instant::now()));
                i.invalidate();
                return;
            }
            let mut items: Vec<(u32, String)> = found.iter().enumerate().map(|(n, f)| (n as u32 + 1, format!("{}  ({} bookmarks)", f.browser, f.items.len()))).collect();
            if found.len() > 1 {
                items.push((0, String::new()));
                items.push((50, "All of them".into()));
            }
            let (x, y) = i.mouse;
            super::menu::open(i, &items, x, y, move |i, cmd| {
                let browser = match cmd {
                    0 => return,
                    50 => "all".to_string(),
                    n => match found.get(n as usize - 1) {
                        Some(f) => f.browser.to_string(),
                        None => return,
                    },
                };
                send(&i.proxy, json!({"t": "bmImport", "browser": browser}));
            });
        }
        PageHit::DriveStart => send(&i.proxy, json!({"t": "driveStart"})),
        PageHit::DriveStop => send(&i.proxy, json!({"t": "driveStop"})),
        PageHit::DrivePause => send(&i.proxy, json!({"t": "drivePause"})),
        PageHit::ClearHistory => {
            // One drawn menu of time ranges first, then the wipe.
            ask_range(i, |i, since| send(&i.proxy, json!({"t": "clearHistory", "since": since})));
        }
        PageHit::ClearData => {
            ask_range(i, |i, since| {
                let (x, y) = i.mouse;
                // Second choice: history only, or the cookies and cache with it.
                super::menu::open(i, &[(1, "History only".into()), (2, "History, cookies and cache".into())], x, y, move |i, cmd| {
                    let more = match cmd {
                        1 => false,
                        2 => true,
                        _ => return,
                    };
                    send(&i.proxy, json!({"t": "clearData", "since": since, "more": more}));
                });
            });
        }
        PageHit::DefaultMake => {
            let note = crate::defaultapp::make_default();
            i.page.flash = Some((note, std::time::Instant::now()));
            i.invalidate();
        }
        PageHit::DefaultOff => {
            let note = crate::defaultapp::unregister();
            i.page.flash = Some((note, std::time::Instant::now()));
            i.invalidate();
        }
        PageHit::PermReset(n) => {
            if let Some((origin, kind, _)) = s.rules.get(n) {
                send(&i.proxy, json!({"t": "permReset", "origin": origin, "kind": kind}));
            }
        }
        PageHit::SitePerm(origin, kind, v) => send(&i.proxy, json!({"t": "permSet", "origin": origin, "kind": kind, "v": v})),
        PageHit::SiteReset(origin) => send(&i.proxy, json!({"t": "permResetSite", "origin": origin})),
        PageHit::SiteClear(origin) => send(&i.proxy, json!({"t": "siteClear", "origin": origin})),
        PageHit::SiteZoom(host) => send(&i.proxy, json!({"t": "zoomForget", "host": host})),
        PageHit::SiteHttp(host) => send(&i.proxy, json!({"t": "httpForget", "host": host})),
        PageHit::SiteAdblock(host) => {
            // Back under blocking: the list goes through set(), which saves and tells main.
            let mut list = s.settings.adblock_off_sites.clone();
            list.retain(|h| *h != host);
            set(i, "adblock_off_sites", json!(list));
        }
        PageHit::PermDefault(kind, v) => send(&i.proxy, json!({"t": "permDefault", "kind": kind, "v": v})),
        PageHit::Panels(which) => {
            // Opening the changelog is reading it: the start page's note goes.
            if which == "changes" && s.settings.seen_version != env!("LUMEN_VERSION") {
                set(i, "seen_version", json!(env!("LUMEN_VERSION")));
            }
            send(&i.proxy, json!({"t": "panel", "open": true, "which": which}));
        }
        PageHit::OpenTab(url) => send(&i.proxy, json!({"t": "openTab", "url": url})),
        PageHit::SeenVersion => set(i, "seen_version", json!(env!("LUMEN_VERSION"))),
        PageHit::BugSend => {
            if !s.bug_sent {
                send(&i.proxy, json!({"t": "bugSend", "what": super::wide_text(i.bug_edit)}));
            }
        }
        PageHit::CrashSeen => send(&i.proxy, json!({"t": "crashSeen"})),
        PageHit::BugCopy => {
            if let Some(report) = s.bug_report.as_deref() {
                let text = crate::bugreport::with_story(report, &super::wide_text(i.bug_edit));
                super::copy_text(i.hwnd, &text);
                i.page.flash = Some(("Copied. Paste it wherever you report it.".into(), std::time::Instant::now()));
                i.invalidate();
            }
        }
        PageHit::BugSave => send(&i.proxy, json!({"t": "bugSave"})),
    }
}

pub fn wheel(i: &mut Inner, delta: i32) -> bool {
    let Some(s) = &i.state else { return false };
    let over_page = s.panel != "none" && i.page.view.contains(i.mouse.0, i.mouse.1);
    if !over_page {
        return false;
    }
    let next = (i.page.scroll - delta as f32 / 120.0 * 60.0).clamp(0.0, i.page.max);
    if next != i.page.scroll {
        i.page.scroll = next;
        i.invalidate();
    }
    true
}

/// A key while the font list is open: Escape closes, Backspace thins the filter,
/// Enter takes the first family, the arrows walk the list. Everything else is
/// swallowed so typing never reaches the browser's shortcuts.
pub fn key(i: &mut Inner, vk: u32) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_BACK, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_MENU, VK_RETURN, VK_UP};
    let down = |k: u16| unsafe { GetKeyState(k as i32) } < 0;
    if i.page.font_pick.is_none() || down(VK_CONTROL.0) || down(VK_MENU.0) {
        return false;
    }
    match vk {
        v if v == VK_ESCAPE.0 as u32 => i.page.font_pick = None,
        v if v == VK_BACK.0 as u32 => {
            if let Some(p) = i.page.font_pick.as_mut() {
                p.filter.pop();
            }
        }
        v if v == VK_RETURN.0 as u32 => {
            let (key, family) = match i.page.font_pick.as_ref() {
                Some(p) => (p.key, p.shown.first().cloned()),
                None => return false,
            };
            match family {
                Some(f) => {
                    i.page.font_pick = None;
                    set(i, key, json!(f));
                }
                None => i.page.font_pick = None,
            }
        }
        v if v == VK_DOWN.0 as u32 => {
            if let Some(p) = i.page.font_pick.as_mut() {
                p.scroll += 26.0;
            }
        }
        v if v == VK_UP.0 as u32 => {
            if let Some(p) = i.page.font_pick.as_mut() {
                p.scroll = (p.scroll - 26.0).max(0.0);
            }
        }
        _ => {}
    }
    i.invalidate();
    true
}

/// A typed character while the font list is open: it goes into the filter.
pub fn pick_char(i: &mut Inner, ch: u32) -> bool {
    let Some(p) = i.page.font_pick.as_mut() else { return false };
    if let Some(c) = char::from_u32(ch)
        && !c.is_control()
        && p.filter.chars().count() < 40
    {
        p.filter.push(c);
        p.scroll = 0.0;
        i.invalidate();
    }
    true
}

impl Inner {
    fn hwnd_raw(&self) -> windows::Win32::Foundation::HWND {
        self.hwnd
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_link, column, days_from_civil, fmt_bytes, group, midnight, summary, tile_label, unix_now};
    use crate::chrome::gfx::R;
    use crate::chrome::UiDl;

    fn dl(state: &'static str, kind: &'static str) -> UiDl {
        UiDl {
            id: 1,
            name: "x".into(),
            url: "https://example.com/f.bin".into(),
            received: 0,
            total: 0,
            state,
            speed: 0,
            note: String::new(),
            kind,
            up: 0,
            peers: 0,
            seeds: 0,
            ratio: 0.0,
        }
    }

    #[test]
    fn civil_days_are_the_epoch_count() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 1, 2), 1);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        // A leap day and a century rule that is not a leap.
        assert_eq!(days_from_civil(2024, 2, 29), days_from_civil(2024, 3, 1) - 1);
        assert_eq!(days_from_civil(1900, 3, 1), days_from_civil(1900, 2, 28) + 1);
    }

    #[test]
    fn midnight_is_the_start_of_today() {
        let now = unix_now();
        let m = midnight();
        assert!(m <= now, "midnight is not in the future");
        assert!(now - m < 86_400, "midnight is not within the last day");
        // Local time-of-day at midnight is 00:00, which is what the ranges need.
        assert_eq!((now - m) / 3600, ((now - m) / 3600).min(23));
    }

    /// The summary line, exactly as the brief spells it.
    #[test]
    fn summary_counts_and_speeds() {
        assert_eq!(summary(3, 4_404_019, 317_440, true), "3 active · 4.2 MB/s down · 310 KB/s up · port open");
        assert_eq!(summary(3, 4_404_019, 317_440, false), "3 active · 4.2 MB/s down · 310 KB/s up · port closed");
        // Nothing running: no line at all.
        assert_eq!(summary(0, 0, 0, true), "");
    }

    /// Every state lands in a group; the page draws them in a fixed order.
    #[test]
    fn groups_cover_every_state() {
        assert_eq!(group("progress"), "Active");
        assert_eq!(group("paused"), "Active");
        assert_eq!(group("seeding"), "Seeding");
        assert_eq!(group("done"), "Done");
        assert_eq!(group("failed"), "Failed");
    }

    /// Clipboard text is a link, a magnet, or nothing aria2 wants.
    #[test]
    fn links_classify_by_scheme() {
        assert_eq!(classify_link("https://example.com/f.bin"), "url");
        assert_eq!(classify_link("  http://a.test/x  "), "url");
        assert_eq!(classify_link("magnet:?xt=urn:btih:abc"), "magnet");
        assert_eq!(classify_link("hello world"), "other");
        assert_eq!(classify_link("ftp://a.test/x"), "other");
        assert_eq!(classify_link(""), "other");
    }

    /// The seeding and torrent status lines spell out the brief's fields.
    #[test]
    fn status_lines_name_peers_and_ratio() {
        let mut d = dl("seeding", "torrent");
        d.up = 300 * 1024;
        d.ratio = 0.42;
        d.peers = 9;
        // The host rides along at the end, like every status line on this page.
        assert_eq!(super::status_line(&d), "Seeding · up 300 KB/s · ratio 0.42 · 9 peers · example.com");

        let mut d = dl("progress", "torrent");
        d.received = 120 * 1048576;
        d.total = 1_503_238_553; // 1.4 GiB
        d.speed = 4308992;
        d.peers = 23;
        d.seeds = 7;
        d.up = 300 * 1024;
        let s = super::status_line(&d);
        assert!(s.contains("120.0 MB of 1.40 GB"), "{s}");
        assert!(s.contains("4.1 MB/s"), "{s}");
        assert!(s.contains("23 peers, 7 seeds"), "{s}");
        assert!(s.contains("up 300 KB/s"), "{s}");

        let mut d = dl("progress", "file");
        d.peers = 16;
        d.received = 120 * 1048576;
        d.total = 1_503_238_553;
        let s = super::status_line(&d);
        assert!(s.contains("16 connections"), "{s}");
        assert!(!s.contains("peers"), "{s}");
    }

    #[test]
    fn fmt_bytes_rounds_like_the_brief() {
        assert_eq!(fmt_bytes(4_404_019), "4.2 MB");
        assert_eq!(fmt_bytes(317_440), "310 KB");
    }

    #[test]
    fn tile_says_what_the_file_is() {
        let named = |n: &str, kind: &'static str| UiDl { name: n.into(), ..dl("done", kind) };
        assert_eq!(tile_label(&named("report.pdf", "file")), "PDF");
        assert_eq!(tile_label(&named("a.tar.gz", "file")), "GZ");
        // No extension, or one too long to be one: say the kind instead.
        assert_eq!(tile_label(&named("README", "file")), "FILE");
        assert_eq!(tile_label(&named("some file.not an ext", "file")), "FILE");
        assert_eq!(tile_label(&named("movie", "video")), "VID");
        assert_eq!(tile_label(&named("ubuntu", "torrent")), "TOR");
    }

    fn mark(id: u64, title: &str, url: &str, folder: &str) -> crate::chrome::UiMark {
        crate::chrome::UiMark { id, title: title.into(), url: url.into(), folder: folder.into() }
    }

    #[test]
    fn search_wants_every_word_in_the_title_or_the_address() {
        let m = mark(1, "The Rust Programming Language", "https://doc.rust-lang.org/book/", "");
        assert!(super::bm_matches("rust book", &m));
        assert!(super::bm_matches("  LANGUAGE ", &m));
        assert!(super::bm_matches("rust-lang.org", &m));
        assert!(!super::bm_matches("rust python", &m));
    }

    #[test]
    fn groups_run_unsorted_then_folders_and_hide_what_does_not_match() {
        let marks = vec![
            mark(1, "A", "https://a.test/", ""),
            mark(2, "B", "https://b.test/", "Work"),
            mark(3, "C", "https://c.test/", "Work"),
        ];
        let folders = vec!["Work".to_string(), "Empty".to_string()];
        let g = super::bm_groups(&marks, &folders, "");
        let names: Vec<(&str, usize)> = g.iter().map(|(f, r)| (*f, r.len())).collect();
        // Every folder shows when not searching, an empty one too.
        assert_eq!(names, [("", 1), ("Work", 2), ("Empty", 0)]);
        // Searching keeps only the groups with a match.
        let g = super::bm_groups(&marks, &folders, "c.test");
        assert_eq!(g.iter().map(|(f, r)| (*f, r.len())).collect::<Vec<_>>(), [("Work", 1)]);
        // Nothing in Unsorted: no Unsorted group.
        let g = super::bm_groups(&marks[1..], &folders, "");
        assert_eq!(g[0].0, "Work");
    }

    #[test]
    fn a_dragged_bookmark_lands_before_the_row_under_it_or_at_the_end_of_the_folder() {
        use super::DropRow;
        let row = |y: f32, folder: &str, id: u64| DropRow { r: R::new(0.0, y, 100.0, 40.0), folder: folder.into(), id };
        let rows = vec![row(0.0, "", 0), row(40.0, "", 1), row(80.0, "", 2), row(120.0, "Work", 0), row(160.0, "Work", 3)];
        // Upper half of a row: before it.
        assert_eq!(super::bm_drop(&rows, 45.0), Some(("".into(), Some(1))));
        // Lower half: before the next row of the same folder.
        assert_eq!(super::bm_drop(&rows, 70.0), Some(("".into(), Some(2))));
        // Lower half of the last one: the end of its folder.
        assert_eq!(super::bm_drop(&rows, 110.0), Some(("".into(), None)));
        assert_eq!(super::bm_drop(&rows, 195.0), Some(("Work".into(), None)));
        // A header: the end of that folder.
        assert_eq!(super::bm_drop(&rows, 130.0), Some(("Work".into(), None)));
        // Off the list: nowhere.
        assert_eq!(super::bm_drop(&rows, 500.0), None);
    }

    #[test]
    fn column_is_centred_capped_and_keeps_its_margin() {
        let wide = column(R::new(300.0, 0.0, 1400.0, 800.0), 760.0);
        assert_eq!(wide.w, 760.0);
        assert_eq!(wide.x, 300.0 + (1400.0 - 760.0) / 2.0);
        // Narrow: never closer to either edge than the margin the close button needs.
        let narrow = column(R::new(0.0, 0.0, 400.0, 800.0), 760.0);
        assert!(narrow.x >= 56.0 && narrow.right() <= 400.0 - 56.0 + 0.01);
    }
}
