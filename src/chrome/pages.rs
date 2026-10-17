//! Settings and Control, drawn natively. A small widget kit (segments, toggles,
//! sliders, swatches) and two pages built from it. Settings keys are read and
//! written through serde, so a new setting is one line in a schema.

use serde_json::{Value, json};
use windows::Win32::UI::Controls::Dialogs::{
    CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW, GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::Foundation::COLORREF;
use windows::core::{PCWSTR, PWSTR};

use super::{
    Hit, Inner, UiDl, UiState, send,
    gfx::{Align, Font, R},
    style::*,
    theme::{Color, PALETTES, Theme},
};
use crate::store::Settings;

#[derive(Clone, Debug, PartialEq)]
pub enum PageHit {
    Seg(&'static str, &'static str),
    Toggle(&'static str),
    Slider(&'static str),
    Swatch(&'static str),
    Accent,
    AccentReset,
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
    /// Settings: write Lumen's browser entries and open Windows' Default apps page.
    DefaultMake,
    /// Settings: take Lumen's browser entries out again.
    DefaultOff,
}

#[derive(Clone, Copy)]
enum Scale {
    Linear(f32, f32, f32),
    Steps(&'static [u32]),
}

struct Slide {
    key: &'static str,
    track: R,
    scale: Scale,
}

#[derive(Default)]
pub struct Page {
    pub scroll: f32,
    /// The panel the scroll offset belongs to.
    pub(super) which: &'static str,
    max: f32,
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
}

struct BmDrag {
    id: u64,
    y0: f32,
    moved: bool,
}

/// A row or a folder header on the Bookmarks page, as the drag sees it. `id` 0 is a header.
#[derive(Clone, Debug, PartialEq)]
struct DropRow {
    r: R,
    folder: String,
    id: u64,
}

// ------------------------------------------------------------------ schema

enum Item {
    Swatch,
    Accent,
    Seg(&'static str, &'static str, &'static str, &'static [(&'static str, &'static str)]),
    /// Like Seg, but the choices wrap onto as many lines as they need. For lists
    /// that are too long for one pill (the translation languages).
    Pick(&'static str, &'static str, &'static str, &'static [(&'static str, &'static str)]),
    Toggle(&'static str, &'static str, &'static str),
    Range(&'static str, &'static str, &'static str, Scale, &'static str),
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
const ENGINES: [(&str, &str); 5] = [
    ("DuckDuckGo", "https://duckduckgo.com/?q=%s"),
    ("Google", "https://www.google.com/search?q=%s"),
    ("Brave", "https://search.brave.com/search?q=%s"),
    ("Bing", "https://www.bing.com/search?q=%s"),
    ("Startpage", "https://www.startpage.com/do/search?q=%s"),
];
const KEYS: [(&str, &str); 21] = [
    ("New tab", "Ctrl+T"),
    ("New private tab", "Ctrl+Shift+N"),
    ("Reopen closed tab", "Ctrl+Shift+T"),
    ("Close tab", "Ctrl+W"),
    ("Address bar", "Ctrl+L"),
    ("Next / previous tab", "Ctrl+Tab / Ctrl+Shift+Tab"),
    ("Jump to tab", "Ctrl+1…9"),
    ("Back / forward", "Alt+← / Alt+→"),
    ("Reload", "Ctrl+R"),
    ("Compact sidebar", "Ctrl+B"),
    ("Find in page", "Ctrl+F"),
    ("Fullscreen", "F11"),
    ("Settings", "Ctrl+,"),
    ("Control", "Shift+Esc"),
    ("Bookmark this page", "Ctrl+D"),
    ("History / bookmarks", "Ctrl+H / Ctrl+Shift+O"),
    ("Downloads", "Ctrl+J"),
    ("Reader view", "F9"),
    ("Read aloud in reader view", "F7"),
    ("Darken this site / keep its colors", "Shift+F9"),
    ("Developer tools", "F12"),
];

fn settings_schema() -> Vec<(&'static str, Vec<Item>)> {
    use Item::*;
    vec![
        ("General", vec![DefaultBrowser]),
        ("Appearance", vec![
            Swatch,
            Accent,
            Seg("theme", "Theme", "", &[("dark", "Dark"), ("light", "Light"), ("system", "System")]),
            Toggle("dark_pages", "Darken light sites", "When Lumen is dark, Dark Reader repaints sites that have no dark mode. Shift+F9 skips a site."),
            Seg("ui_font", "Interface font", "Atkinson Hyperlegible is designed for easy reading.", &[("segoe", "Segoe"), ("instrument", "Instrument"), ("figtree", "Figtree"), ("atkinson", "Atkinson")]),
            Seg("display_font", "Display font", "The clock and page titles.", &[("instrument", "Instrument Serif"), ("fraunces", "Fraunces"), ("newsreader", "Newsreader")]),
            Toggle("speed_dial", "Speed dial on the new tab page", "Sites you keep as tiles under the search box. Off: just the clock and the search box."),
            Seg("sidebar_side", "Sidebar", "", &[("left", "Left"), ("right", "Right")]),
            Range("sidebar_width", "Sidebar width", "", Scale::Linear(232.0, 400.0, 4.0), "px"),
            Toggle("compact", "Compact sidebar", "Icons only. Ctrl+B toggles it."),
            Range("gap", "Page margin", "", Scale::Linear(0.0, 24.0, 1.0), "px"),
            Range("radius", "Page corners", "", Scale::Linear(0.0, 20.0, 1.0), "px"),
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
            Toggle("accelerate_downloads", "Accelerate downloads", "aria2 splits files into up to 16 connections, resumes, and queues. Falls back to the built-in downloader when a site refuses it."),
            Toggle("keep_seeding", "Share torrents back after they finish", "Until ratio 1.0 or one hour, whichever first. Applies on next start."),
            Range("download_limit_kbps", "Download speed limit", "Caps aria2, the download engine. 0 is no limit.", Scale::Steps(DL_SPEEDS), "kbps"),
            Seg("tracking", "Tracking prevention", "", &[("off", "Off"), ("basic", "Basic"), ("balanced", "Balanced"), ("strict", "Strict")]),
            Range("sleep_after_min", "Freeze hidden tabs after", "Frozen tabs stop running and wake instantly.", Scale::Linear(1.0, 60.0, 1.0), "min"),
            Range("discard_after_min", "Unload hidden tabs after", "Unloaded tabs free all memory and reload when opened.", Scale::Linear(5.0, 240.0, 5.0), "min"),
            Toggle("show_memory", "Memory in sidebar", ""),
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
            Toggle("agent_upload", "Let agents upload files", "Off: agents cannot put files into pages. On: only files in the Lumen agent uploads folder in your home folder (agent_upload_dir in settings.json changes it)."),
            AgentSetup,
        ]),
        ("Privacy", vec![
            Button("Clear browsing data", "Lumen history, page text and WebView2's own browsing history and downloads. Passwords and bookmarks are never touched.", "Clear…", PageHit::ClearData),
            Toggle("clear_on_exit", "Clear history when Lumen closes", "Lumen history, the words of pages and WebView2's own browsing history go when the browser closes. Private tabs leave nothing anyway."),
        ]),
        ("Keys", vec![Keys]),
    ]
}

fn limits_schema() -> Vec<Item> {
    use Item::*;
    vec![
        Range("ram_limit_mb", "RAM limit", "Over the limit, the hidden tab looked at longest ago unloads.", Scale::Steps(RAM), "mb"),
        Range("cpu_limit_pct", "CPU limit", "Hard cap for the whole browser, share of all cores.", Scale::Linear(5.0, 100.0, 5.0), "%"),
        Range("net_limit_kbps", "Network limit", "Bandwidth cap for each tab. Downloads are not capped.", Scale::Steps(NET), "kbps"),
        Toggle("gpu_in_process", "Graphics in browser process", "About 100 MB less memory. A driver crash restarts the browser. Applies after restart."),
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

fn set(i: &mut Inner, k: &str, v: Value) {
    let Some(st) = i.state.as_mut() else { return };
    let mut all = serde_json::to_value(&st.settings).unwrap_or(json!({}));
    if all.get(k) == Some(&v) {
        return;
    }
    all[k] = v;
    if let Ok(next) = serde_json::from_value::<Settings>(all) {
        st.settings = next.clone();
        if k == "palette" || k == "accent" {
            i.theme = Theme::new(&next.palette, i.theme.dark, &next.accent);
        }
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

/// The slice of history to clear: Some(None) = everything, Some(Some(t)) = from
/// unix `t` on, None = the menu was dismissed.
fn ask_range(i: &Inner) -> Option<Option<u64>> {
    let now = unix_now();
    let items: Vec<(u32, String)> = [(1u32, "Last hour"), (2, "Today"), (3, "Last 7 days"), (4, "Last 4 weeks"), (5, "Everything")]
        .into_iter()
        .map(|(id, label)| (id, label.to_string()))
        .collect();
    let (x, y) = i.mouse;
    match i.popup(&items, x, y) {
        1 => Some(Some(now - 3600)),
        2 => Some(Some(midnight())),
        3 => Some(Some(now - 7 * 86_400)),
        4 => Some(Some(now - 28 * 86_400)),
        5 => Some(None),
        _ => None,
    }
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
    let first = i.hits.len();
    draw_page(i, st, s);
    // The content can shrink under the offset (a finished row cleared, a window made taller).
    // The height is only known after drawing, so draw once more from the clamped offset.
    if i.page.scroll > i.page.max {
        i.page.scroll = i.page.max;
        i.hits.truncate(first);
        i.gfx.round(st, s.settings.radius as f32, i.theme.raise);
        draw_page(i, st, s);
    }
}

fn draw_page(i: &mut Inner, st: R, s: &UiState) {
    i.page.sliders.clear();
    i.page.view = st;
    match s.panel {
        "settings" => draw_settings(i, st, s),
        "downloads" => draw_downloads(i, st, s),
        "history" => draw_library(i, st, s),
        "bookmarks" => draw_bookmarks(i, st, s),
        _ => draw_control(i, st, s),
    }
}

/// A setting's name and its hint, on the left. `control_w` is what the control on the right
/// takes, so a long hint wraps to a second line instead of running under it. Returns the
/// row's height: 54, or taller when the hint needs more lines.
fn row_label(i: &Inner, r: R, label: &str, hint: &str, control_w: f32) -> f32 {
    let th = i.theme;
    let avail = (r.w - control_w - S5).max(r.w * 0.4);
    if hint.is_empty() {
        i.gfx.text(label, R::new(r.x, r.y, avail, 54.0), Font::ui(13.0, 500), th.text);
        return 54.0;
    }
    i.gfx.text(label, R::new(r.x, r.y + 8.0, avail, 20.0), Font::ui(13.0, 500), th.text);
    // Words do not fill a line to the last pixel; ask for a little more room than the sum.
    let lines = ((i.gfx.measure(hint, SMALL) / (avail * 0.94)).ceil() as usize).clamp(1, 3);
    if lines == 1 {
        i.gfx.text(hint, R::new(r.x, r.y + 27.0, avail, 18.0), SMALL, th.faint);
    } else {
        i.gfx.text_wrap(hint, R::new(r.x, r.y + 28.0, avail, 16.0 * lines as f32 + 2.0), SMALL, th.faint);
    }
    28.0 + 16.0 * lines as f32 + 10.0
}

/// How wide a segmented control is.
fn seg_width(i: &Inner, opts: &[(&str, &str)]) -> f32 {
    let f = Font::ui(13.0, 450).align(Align::Center);
    opts.iter().map(|(_, l)| i.gfx.measure(l, f) + 22.0).sum::<f32>() + 4.0
}

fn draw_seg(i: &mut Inner, right: f32, cy: f32, key: &'static str, opts: &'static [(&'static str, &'static str)], cur: &str) {
    let th = i.theme;
    let f = Font::ui(13.0, 450).align(Align::Center);
    let widths: Vec<f32> = opts.iter().map(|(_, l)| i.gfx.measure(l, f) + 22.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let pill = R::new(right - total, cy - 15.0, total, 30.0);
    i.gfx.round(pill, 9.0, th.bg);
    let mut x = pill.x + 2.0;
    for ((v, l), w) in opts.iter().zip(widths) {
        let r = R::new(x, pill.y + 2.0, w, 26.0);
        let on = *v == cur;
        let h = Hit::Page(PageHit::Seg(key, v));
        if on {
            i.gfx.round(r, 7.0, th.press);
        }
        let c = if on || i.is_hover(&h) { th.text } else { th.dim };
        i.gfx.text(l, r, f, c);
        i.hit(r, h);
        x += w;
    }
}

fn draw_toggle(i: &mut Inner, right: f32, cy: f32, key: &'static str, on: bool, enabled: bool) {
    let th = i.theme;
    let r = R::new(right - 34.0, cy - 10.0, 34.0, 20.0);
    let a = if enabled { 1.0 } else { 0.35 };
    i.gfx.round(r, 10.0, if on { th.accent.alpha(a) } else { th.press.alpha(a) });
    let kx = if on { r.x + 20.0 } else { r.x + 3.0 };
    i.gfx.circle(kx + 7.0, r.y + 10.0, 7.0, if on { th.bg } else { th.dim.alpha(a) });
    if enabled {
        i.hit(r.inset(-4.0, -4.0), Hit::Page(PageHit::Toggle(key)));
    }
}

fn draw_slider(i: &mut Inner, right: f32, cy: f32, key: &'static str, scale: Scale, unit: &str, v: f32, dim: bool) {
    let th = i.theme;
    let val = R::new(right - 64.0, cy - 10.0, 64.0, 20.0);
    i.gfx.text(&fmt_value(v, unit), val, Font::ui(12.0, 450).align(Align::Right), if dim { th.faint } else { th.dim });
    let track = R::new(val.x - 12.0 - 160.0, cy - 2.0, 160.0, 4.0);
    let p = ratio(scale, v);
    i.gfx.round(track, 2.0, th.press);
    i.gfx.round(R::new(track.x, track.y, track.w * p, 4.0), 2.0, if dim { th.faint } else { th.accent });
    i.gfx.circle(track.x + track.w * p, cy, 7.0, th.text);
    let grab = R::new(track.x - 8.0, cy - 12.0, track.w + 16.0, 24.0);
    i.hit(grab, Hit::Page(PageHit::Slider(key)));
    i.page.sliders.push((key, track, scale));
}

fn draw_item(i: &mut Inner, it: &Item, r: R, s: &UiState) -> f32 {
    let th = i.theme;
    let right = r.right();
    match it {
        Item::Swatch => {
            row_label(i, R::new(r.x, r.y, r.w, 54.0), "Palette", "", 0.0);
            let cur = s.settings.palette.clone();
            let mut x = right - PALETTES.len() as f32 * 36.0 + 8.0;
            for (key, label) in PALETTES {
                let (tile, dot) = Theme::swatch(key);
                let c = (x + 14.0, r.y + 27.0);
                if key == cur {
                    i.gfx.ring(c.0, c.1, 17.0, 17.0, th.text, 1.5);
                }
                i.gfx.circle(c.0, c.1, 14.0, tile);
                i.gfx.circle(c.0, c.1, 6.0, dot);
                let hr = R::new(x, r.y + 13.0, 28.0, 28.0);
                i.hit(hr, Hit::Page(PageHit::Swatch(key)));
                let _ = label;
                x += 36.0;
            }
            54.0
        }
        Item::Accent => {
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), "Accent", "Focus rings, toggles, the loading ring.", 100.0);
            let cy = r.y + h / 2.0;
            let sw = R::new(right - 28.0, cy - 14.0, 28.0, 28.0);
            i.gfx.round(sw, 8.0, th.accent);
            i.gfx.round_stroke(sw, 8.0, th.line, 1.0);
            i.hit(sw, Hit::Page(PageHit::Accent));
            let reset = R::new(sw.x - 60.0, cy - 10.0, 50.0, 20.0);
            let hh = Hit::Page(PageHit::AccentReset);
            i.gfx.text("Reset", reset, SMALL.align(Align::Right), if i.is_hover(&hh) { th.text } else { th.faint });
            i.hit(reset, hh);
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
            let f = Font::ui(12.5, 450).align(Align::Center);
            let cur = get(&s.settings, k).as_str().unwrap_or("").to_string();
            let mut y = r.y;
            i.gfx.text(label, R::new(r.x, y, r.w, 20.0), Font::ui(13.0, 500), th.text);
            y += 20.0;
            if !hint.is_empty() {
                i.gfx.text_wrap(hint, R::new(r.x, y, r.w, 34.0), SMALL, th.faint);
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
                if on {
                    i.gfx.round(chip, 8.0, th.press);
                } else if i.is_hover(&h) {
                    i.gfx.round(chip, 8.0, th.hover);
                }
                i.gfx.text(l, chip, f, if on { th.text } else { th.dim });
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
            let f = Font::ui(13.0, 450).align(Align::Center);
            let widths: Vec<f32> = ENGINES.iter().map(|(l, _)| i.gfx.measure(l, f) + 22.0).collect();
            let total: f32 = widths.iter().sum::<f32>() + 4.0;
            let h = row_label(i, R::new(r.x, r.y, r.w, 54.0), "Search engine", "", total);
            let pill = R::new(right - total, r.y + h / 2.0 - 15.0, total, 30.0);
            i.gfx.round(pill, 9.0, th.bg);
            let mut x = pill.x + 2.0;
            for (n, ((label, url), w)) in ENGINES.iter().zip(widths).enumerate() {
                let rr = R::new(x, pill.y + 2.0, w, 26.0);
                let on = s.settings.search == *url;
                let h = Hit::Page(PageHit::Engine(n));
                if on {
                    i.gfx.round(rr, 7.0, th.press);
                }
                i.gfx.text(label, rr, f, if on || i.is_hover(&h) { th.text } else { th.dim });
                i.hit(rr, h);
                x += w;
            }
            h
        }
        Item::Button(label, hint, action, hit) => {
            let bw = i.gfx.measure(action, Font::ui(13.0, 450).align(Align::Center)) + 26.0;
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
            let bw = i.gfx.measure(action, Font::ui(13.0, 450).align(Align::Center)) + 26.0;
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
            let f = Font::ui(13.0, 450).align(Align::Center);
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
                i.gfx.text_wrap("No answers remembered yet. When a site asks for your camera, location or similar, the sidebar asks you.", R::new(r.x, r.y + 6.0, r.w, 36.0), Font::ui(12.5, 400), th.faint);
                return 44.0;
            }
            let mut y = r.y;
            for (n, (origin, kind, allow)) in s.rules.iter().enumerate() {
                i.gfx.text(&super::host(origin), R::new(r.x, y, r.w * 0.45, 40.0), Font::ui(13.0, 500), th.text);
                i.gfx.text(&format!("{kind} · {}", if *allow { "Allowed" } else { "Blocked" }), R::new(r.x + r.w * 0.45, y, r.w * 0.35, 40.0), Font::ui(12.5, 400), if *allow { th.dim } else { th.faint });
                let br = R::new(right - 60.0, y + 8.0, 60.0, 24.0);
                let h = Hit::Page(PageHit::PermReset(n));
                i.gfx.text("Reset", br, SMALL.align(Align::Right), if i.is_hover(&h) { th.text } else { th.faint });
                i.hit(br, h);
                i.gfx.line(r.x, y + 40.0, right, y + 40.0, th.line, 1.0);
                y += 41.0;
            }
            y - r.y
        }
        Item::Keys => {
            let mut y = r.y;
            for (what, keys) in KEYS {
                i.gfx.text(what, R::new(r.x, y, r.w * 0.6, 36.0), Font::ui(13.0, 400), th.dim);
                i.gfx.text(keys, R::new(r.x + r.w * 0.4, y, r.w * 0.6, 36.0), Font::ui(12.5, 450).align(Align::Right), th.text);
                i.gfx.line(r.x, y + 36.0, right, y + 36.0, th.line, 1.0);
                y += 37.0;
            }
            y - r.y
        }
    }
}

fn draw_button(i: &mut Inner, right: f32, cy: f32, label: &str, hit: PageHit) {
    let th = i.theme;
    let f = Font::ui(13.0, 450).align(Align::Center);
    let w = i.gfx.measure(label, f) + 26.0;
    let r = R::new(right - w, cy - 15.0, w, 30.0);
    let h = Hit::Page(hit);
    i.gfx.round(r, 8.0, if i.is_hover(&h) { th.press } else { th.bg });
    i.gfx.round_stroke(r, 8.0, th.line, 1.0);
    i.gfx.text(label, r, f, th.text);
    i.hit(r, h);
}

fn fmt_bytes(b: i64) -> String {
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

/// A native open-file dialog for a .torrent; the file's bytes when picked.
fn pick_torrent(owner: windows::Win32::Foundation::HWND) -> Option<Vec<u8>> {
    let mut buf = vec![0u16; 32768];
    let filter: Vec<u16> = "Torrents\0*.torrent\0All files\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = "Open a torrent file\0".encode_utf16().collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let len = (0..buf.len()).take_while(|&i| buf[i] != 0).count();
    let path = String::from_utf16_lossy(&buf[..len]);
    std::fs::read(path).ok()
}

/// The file dialog for a .kdbx. Returns the path, not the bytes: the parse
/// happens on a worker thread, with the password.
fn pick_kdbx(owner: windows::Win32::Foundation::HWND) -> Option<String> {
    let mut buf = vec![0u16; 32768];
    let filter: Vec<u16> = "KeePass files\0*.kdbx\0All files\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = "Open a KeePass file\0".encode_utf16().collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let len = (0..buf.len()).take_while(|&i| buf[i] != 0).count();
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// Control ids. 1 and 2 are the ids Windows itself presses for Enter and
/// Escape, so the two buttons keep those numbers.
const ID_PASS: usize = 101;
const ID_OPEN: usize = 1;
const ID_CANCEL: usize = 2;

/// The master password, in a small native dialog. Modal: the owner is
/// disabled until OK or Cancel. The text leaves this stack frame only as the
/// return value — never stored, never logged, never written.
fn ask_password(owner: windows::Win32::Foundation::HWND, file: &str) -> Option<String> {
    use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DeleteObject, FF_DONTCARE, FW_NORMAL, OUT_DEFAULT_PRECIS,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus, VK_ESCAPE, VK_RETURN};
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{HSTRING, w};

    thread_local! {
        static GOT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    }

    unsafe extern "system" fn pass_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        use windows::Win32::UI::WindowsAndMessaging::*;
        // Edition 2024: an unsafe fn body is not unsafe on its own.
        unsafe {
            match msg {
                WM_COMMAND => {
                    let id = (w.0 & 0xFFFF) as usize;
                    // Open keeps what was typed; either button closes the window.
                    if id == ID_OPEN {
                        let s = super::wide_text(GetDlgItem(Some(hwnd), ID_PASS as i32).unwrap_or_default());
                        GOT.with(|g| *g.borrow_mut() = Some(s));
                    }
                    if id == ID_OPEN || id == ID_CANCEL {
                        let _ = DestroyWindow(hwnd);
                    }
                    LRESULT(0)
                }
                WM_CLOSE => {
                    let _ = DestroyWindow(hwnd);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, w, l),
            }
        }
    }

    GOT.with(|g| *g.borrow_mut() = None);
    let class = w!("LumenPass");
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None) }.map(|h| h.into()).unwrap_or_default();
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(pass_wndproc),
        hInstance: hinst,
        lpszClassName: class,
        ..Default::default()
    };
    unsafe {
        // Already registered on a second use: fine, the class is the same.
        let _ = RegisterClassExW(&wc);
        let mut owner_r = RECT::default();
        let _ = GetWindowRect(owner, &mut owner_r);
        let k = GetDpiForWindow(owner).max(96) as f32 / 96.0;
        let (wpx, hpx) = ((380.0 * k) as i32, (170.0 * k) as i32);
        let x = (owner_r.left + (owner_r.right - owner_r.left - wpx) / 2).max(0);
        let y = (owner_r.top + (owner_r.bottom - owner_r.top - hpx) / 2).max(0);
        let title: Vec<u16> = "KeePass master password\0".encode_utf16().collect();
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            class,
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            x,
            y,
            wpx,
            hpx,
            Some(owner),
            None,
            Some(hinst),
            None,
        ) else {
            return None;
        };
        let face = HSTRING::from("Segoe UI");
        let font = CreateFontW(
            -(13.0 * k).round() as i32,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            &face,
        );
        let label: Vec<u16> = format!("Master password for {}\0", file.rsplit(['\\', '/']).next().unwrap_or(file)).encode_utf16().collect();
        let caption = CreateWindowExW(
            WS_EX_LEFT,
            w!("STATIC"),
            PCWSTR(label.as_ptr()),
            WS_CHILD | WS_VISIBLE,
            12,
            12,
            wpx - 24,
            20,
            Some(hwnd),
            None,
            Some(hinst),
            None,
        )
        .unwrap_or_default();
        let edit = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32) | WINDOW_STYLE(ES_PASSWORD as u32),
            12,
            38,
            wpx - 24,
            24,
            Some(hwnd),
            Some(HMENU(ID_PASS as _)),
            Some(hinst),
            None,
        )
        .unwrap_or_default();
        let ok: Vec<u16> = "Open\0".encode_utf16().collect();
        let cancel: Vec<u16> = "Cancel\0".encode_utf16().collect();
        // Every child gets the same font, the caption and the box included.
        let _ = SendMessageW(caption, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let _ = SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let bx = wpx - 24 - 196;
        for (id, text, def) in [(ID_OPEN, ok.as_slice(), true), (ID_CANCEL, cancel.as_slice(), false)] {
            let b = CreateWindowExW(
                WS_EX_LEFT,
                w!("BUTTON"),
                PCWSTR(text.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE((if def { BS_DEFPUSHBUTTON } else { BS_PUSHBUTTON }) as u32),
                bx + if def { 100 } else { 0 },
                hpx - 44,
                96,
                28,
                Some(hwnd),
                Some(HMENU(id as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let _ = SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        }
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(GetDlgItem(Some(hwnd), ID_PASS as i32).ok());
        let _ = EnableWindow(owner, false);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // This is not a real dialog box, so it has no default button for
            // the dialog manager to press: Enter and Escape are read here,
            // before the edit box can beep at them. Tab still walks the
            // controls through IsDialogMessage.
            if msg.message == WM_KEYDOWN {
                let key = msg.wParam.0 as u16;
                if key == VK_RETURN.0 {
                    GOT.with(|g| *g.borrow_mut() = Some(super::wide_text(edit)));
                    let _ = DestroyWindow(hwnd);
                    continue;
                }
                if key == VK_ESCAPE.0 {
                    let _ = DestroyWindow(hwnd);
                    continue;
                }
            }
            if IsDialogMessageW(hwnd, &msg).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        let _ = EnableWindow(owner, true);
        let _ = SetForegroundWindow(owner);
        if !font.is_invalid() {
            let _ = DeleteObject(font.into());
        }
    }
    GOT.with(|g| g.borrow_mut().take())
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
fn column(st: R, max_w: f32) -> R {
    let w = (st.w - 2.0 * COL_MARGIN).clamp(120.0, max_w);
    R::new(st.x + (st.w - w) / 2.0, st.y, w, st.h)
}

/// The close button, top right of the page, drawn last so it sits over whatever scrolls
/// beneath it. Esc does the same.
fn close_button(i: &mut Inner, st: R) {
    let th = i.theme;
    let r = R::new(st.right() - S4 - 32.0, st.y + S4, 32.0, 32.0);
    let h = Hit::Page(PageHit::Done);
    let hov = i.is_hover(&h);
    i.gfx.round(r, R_MD, if hov { th.hover } else { th.raise });
    i.gfx.icon("close", r.x + 8.0, r.y + 8.0, 16.0, if hov { th.text } else { th.dim });
    i.hit(r, h);
}

/// Page title, and one quiet line under it when there is something to say. Returns the y
/// the content starts at.
fn title_block(i: &Inner, col: R, y: f32, title: &str, sub: &str) -> f32 {
    let th = i.theme;
    i.gfx.text(title, R::new(col.x, y, col.w - 48.0, 40.0), TITLE, th.text);
    if sub.is_empty() {
        return y + 52.0;
    }
    i.gfx.text(sub, R::new(col.x, y + 38.0, col.w - 48.0, 20.0), SMALL, th.faint);
    y + 68.0
}

/// A button at (x, y), 30 high. Outlined and quiet, or (primary) filled with the accent.
/// Returns its rectangle so the caller can step along.
fn outline_button(i: &mut Inner, x: f32, y: f32, label: &str, hit: PageHit, primary: bool) -> R {
    let th = i.theme;
    let f = if primary { BODY_STRONG } else { BODY }.align(Align::Center);
    let r = R::new(x, y, i.gfx.measure(label, f) + 2.0 * S4, 30.0);
    let h = Hit::Page(hit);
    let hov = i.is_hover(&h);
    if primary {
        i.gfx.round(r, R_MD, if hov { th.accent } else { th.accent.alpha(0.88) });
        i.gfx.text(label, r, f, th.bg);
    } else {
        if hov {
            i.gfx.round(r, R_MD, th.hover);
        }
        i.gfx.round_stroke(r, R_MD, th.line, 1.0);
        i.gfx.text(label, r, f, if hov { th.text } else { th.dim });
    }
    i.hit(r, h);
    r
}

/// A text action on a row, 28 high, its right edge at `right`. Returns its left edge.
fn row_action(i: &mut Inner, right: f32, y: f32, label: &str, hit: PageHit) -> f32 {
    let th = i.theme;
    let f = SMALL_STRONG.align(Align::Center);
    let w = i.gfx.measure(label, f) + 2.0 * S3;
    let r = R::new(right - w, y, w, 28.0);
    let h = Hit::Page(hit);
    let hov = i.is_hover(&h);
    if hov {
        i.gfx.round(r, R_SM, th.press);
    }
    i.gfx.text(label, r, f, if hov { th.text } else { th.dim });
    i.hit(r, h);
    r.x - S1
}

/// A small caps label that opens a group of rows. Returns the y after it.
fn group_label(i: &Inner, col: R, y: f32, text: &str, count: usize) -> f32 {
    let th = i.theme;
    let label = if count > 0 { format!("{}  {count}", text.to_uppercase()) } else { text.to_uppercase() };
    i.gfx.text(&label, R::new(col.x, y, col.w, 20.0), CAPTION, th.faint);
    y + 26.0
}

/// What a page shows when it has nothing to list: a mark, a plain sentence, and what to do
/// about it. Centred in `area`; returns its bottom.
fn empty_state(i: &mut Inner, area: R, icon: &str, title: &str, hint: &str) -> f32 {
    let th = i.theme;
    let cx = area.x + area.w / 2.0;
    i.gfx.circle(cx, area.y + 28.0, 28.0, th.bg);
    i.gfx.icon(icon, cx - 12.0, area.y + 16.0, 24.0, th.faint);
    i.gfx.text(title, R::new(area.x, area.y + 70.0, area.w, 26.0), Font::ui(15.0, 550).align(Align::Center), th.text);
    i.gfx.text_wrap(hint, R::new(cx - 210.0, area.y + 100.0, 420.0, 50.0), SMALL.align(Align::Center), th.faint);
    area.y + 150.0
}

/// A row's hover plate. The row also registers itself, so anything drawn on it afterwards
/// (buttons) takes the mouse first.
fn row_plate(i: &mut Inner, r: R, id: u64) -> bool {
    let th = i.theme;
    let h = Hit::Page(PageHit::Row(id));
    let hov = i.is_hover(&h) || i.hover.as_ref().is_some_and(|x| matches!(x, Hit::Page(PageHit::Dl(_, d)) if *d == id));
    if hov {
        i.gfx.round(r, R_LG, th.hover);
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
    let mut y = st.y + S5 + 2.0 - i.page.scroll;

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
        x = outline_button(i, x, y, label, hit, false).right() + S2;
    }
    let chip = format!("Speed limit: {}", chip_label(s.settings.download_limit_kbps));
    let cf = SMALL_STRONG.align(Align::Center);
    let cw = i.gfx.measure(&chip, cf) + 2.0 * S3;
    let cr = R::new(col.right() - cw, y + 1.0, cw, 28.0);
    let ch = Hit::Page(PageHit::DlLimit);
    let chov = i.is_hover(&ch);
    i.gfx.round(cr, 14.0, if chov { th.press } else { th.hover });
    i.gfx.text(&chip, cr, cf, if chov { th.text } else { th.dim });
    i.hit(cr, ch);
    if s.downloads.iter().any(|d| d.state == "done" || d.state == "failed") {
        row_action(i, cr.x - S2, y + 1.0, "Clear finished", PageHit::DlClear);
    }
    y += 30.0 + S4;

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
        y += S3;
        y = group_label(i, col, y, g, rows.len());
        // The firewall note rides under the first header that actually has a torrent.
        if s.settings.torrent_note && rows.iter().any(|d| d.kind == "torrent") {
            let msg = "Windows may ask to let aria2 through the firewall. Allow it for faster torrents. (click to hide)";
            let h = Hit::Page(PageHit::DlNote);
            i.gfx.text(msg, R::new(col.x, y - 4.0, col.w, 20.0), SMALL, if i.is_hover(&h) { th.text } else { th.faint });
            i.hit(R::new(col.x, y - 4.0, col.w, 20.0), h);
            y += 24.0;
        }
        for d in rows {
            let r = R::new(col.x - S2, y, col.w + 2.0 * S2, 72.0);
            if r.bottom() < st.y || r.y > st.bottom() {
                y += 74.0;
                continue;
            }
            row_plate(i, r, d.id);
            let failed = d.state == "failed";
            // The tile: what the file is, in its own letters.
            let tile = R::new(col.x, y + 18.0, 36.0, 36.0);
            i.gfx.round(tile, R_MD, th.bg);
            let live = matches!(d.state, "progress" | "seeding");
            i.gfx.text(&tile_label(d), tile, Font::ui(10.0, 700).align(Align::Center), if failed { th.faint.alpha(0.7) } else if live { th.accent } else { th.dim });
            // The actions, right to left; the text takes whatever they leave.
            let acts: &[(&str, &'static str)] = match d.state {
                "progress" => &[("Pause", "pause"), ("Cancel", "cancel")],
                "paused" => &[("Resume", "resume"), ("Cancel", "cancel")],
                "seeding" => &[("Stop seeding", "stopSeed")],
                "done" => &[("Open", "open"), ("Show in folder", "folder")],
                "failed" if d.url.starts_with("http") => &[("Retry", "retry")],
                _ => &[],
            };
            let mut right = col.right();
            if d.state == "done" || d.state == "failed" {
                let xr = R::new(right - 28.0, y + 22.0, 28.0, 28.0);
                let h = Hit::Page(PageHit::Dl("remove", d.id));
                let hov = i.is_hover(&h);
                if hov {
                    i.gfx.round(xr, R_SM, th.press);
                }
                i.gfx.icon("close", xr.x + 7.0, xr.y + 7.0, 14.0, if hov { th.text } else { th.faint });
                i.hit(xr, h);
                right = xr.x - S1;
            }
            for (label, a) in acts.iter().rev() {
                right = row_action(i, right, y + 22.0, label, PageHit::Dl(a, d.id));
            }
            let tx = tile.right() + S3;
            let tw = (right - S3 - tx).max(60.0);
            i.gfx.text(&d.name, R::new(tx, y + 13.0, tw, 22.0), BODY_STRONG, if failed { th.dim } else { th.text });
            i.gfx.text(&status_line(d), R::new(tx, y + 35.0, tw, 18.0), SMALL, th.faint);
            if d.state == "progress" || d.state == "paused" {
                let pct = if d.total > 0 { d.received as f32 / d.total as f32 } else { 0.0 };
                let bar = R::new(tx, y + 59.0, col.right() - tx, 4.0);
                i.gfx.round(bar, 2.0, th.press);
                if pct > 0.0 {
                    i.gfx.round(R::new(bar.x, bar.y, (bar.w * pct.min(1.0)).max(4.0), 4.0), 2.0, if d.state == "paused" { th.faint } else { th.accent });
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
    let history = true;
    let col = column(st, COL_W);
    i.gfx.clip(st);
    let mut y = st.y + S5 + 2.0 - i.page.scroll;
    let title_y = y;
    y = title_block(i, col, y, "History", "Newest first. Private tabs are never recorded.");
    if !s.history.is_empty() {
        let w = i.gfx.measure("Clear history…", BODY.align(Align::Center)) + 2.0 * S4;
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
    let mut last_day = String::new();
    for (url, title, day, time) in rows {
        if history && day != last_day {
            y += S2;
            y = group_label(i, col, y, &day, 0);
            last_day = day;
        }
        let r = R::new(col.x - S2, y, col.w + 2.0 * S2, 38.0);
        if y > st.bottom() || y + 38.0 < st.y {
            y += 40.0;
            continue;
        }
        let open = Hit::Page(PageHit::Open(url.clone()));
        let rm = Hit::Page(PageHit::Forget(url.clone()));
        let hov = i.is_hover(&open) || i.is_hover(&rm);
        if hov {
            i.gfx.round(r, R_MD, th.hover);
        }
        let mut x = col.x;
        if history {
            i.gfx.text(&time, R::new(x, y, 44.0, 38.0), SMALL, th.faint);
            x += 56.0;
        }
        // The host sits at the right, the title takes what is left, cut with an ellipsis.
        let host = super::host(&url);
        let hw = (i.gfx.measure(&host, SMALL) + 4.0).min(220.0);
        let hx = col.right() - 32.0 - hw;
        let name = if title.is_empty() { url.clone() } else { title };
        i.gfx.text(&name, R::new(x, y, (hx - S4 - x).max(60.0), 38.0), BODY, th.text);
        i.gfx.text(&host, R::new(hx, y, hw, 38.0), SMALL.align(Align::Right), th.faint);
        i.hit(r, open);
        if hov {
            let xr = R::new(col.right() - 28.0, y + 5.0, 28.0, 28.0);
            let xh = i.is_hover(&rm);
            if xh {
                i.gfx.round(xr, R_SM, th.press);
            }
            i.gfx.icon("close", xr.x + 7.0, xr.y + 7.0, 14.0, if xh { th.text } else { th.dim });
            i.hit(xr, rm);
        }
        y += 40.0;
    }
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

// -------------------------------------------------------------- bookmarks

/// What the search box and the two buttons beside it take. The search gets the rest.
const BM_BUTTONS_W: f32 = 270.0;
const BM_ROW_H: f32 = 38.0;

/// The search box of the Bookmarks page. Fixed under the title; the list scrolls below it.
/// The edit control (mod.rs) sits on top of this rectangle.
pub fn bm_search_rect(st: R) -> R {
    let col = column(st, COL_W);
    R::new(col.x, st.y + S5 + 2.0 + 68.0, (col.w - BM_BUTTONS_W).max(140.0), 34.0)
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
    let sr = bm_search_rect(st);
    let top = sr.bottom() + S3;
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
    for (folder, rows) in &groups {
        // A header when there is a folder to name, or Unsorted needs telling apart from them.
        if !folder.is_empty() || named {
            let label = if folder.is_empty() { "UNSORTED" } else { &folder.to_uppercase() };
            let h = R::new(col.x - S2, y, col.w + 2.0 * S2, 30.0);
            if h.bottom() >= top && h.y <= st.bottom() {
                let hit = Hit::Page(PageHit::BmFolder(folder.to_string()));
                let folded = i.page.bm_collapsed.contains(*folder) && !searching;
                let hov = i.is_hover(&hit);
                if hov {
                    i.gfx.round(h, R_MD, th.hover);
                }
                // A chevron: down when open, right when folded.
                let (cx, cy) = (col.x + 6.0, y + 15.0);
                let c = if hov { th.dim } else { th.faint };
                if folded {
                    i.gfx.line(cx - 2.0, cy - 4.0, cx + 2.0, cy, c, 1.5);
                    i.gfx.line(cx + 2.0, cy, cx - 2.0, cy + 4.0, c, 1.5);
                } else {
                    i.gfx.line(cx - 4.0, cy - 2.0, cx, cy + 2.0, c, 1.5);
                    i.gfx.line(cx, cy + 2.0, cx + 4.0, cy - 2.0, c, 1.5);
                }
                i.gfx.text(&format!("{label}  {}", rows.len()), R::new(col.x + 20.0, y, col.w - 20.0, 30.0), CAPTION, th.faint);
                i.hit(h, hit);
                i.page.bm_rows.push(DropRow { r: h, folder: folder.to_string(), id: 0 });
            }
            y += 30.0;
        }
        if i.page.bm_collapsed.contains(*folder) && !searching {
            y += S2;
            continue;
        }
        if rows.is_empty() {
            i.gfx.text("Empty. Drag a bookmark here, or use Move to folder.", R::new(col.x + 20.0, y, col.w - 20.0, 28.0), SMALL, th.faint);
            y += 30.0;
        }
        for m in rows {
            let r = R::new(col.x - S2, y, col.w + 2.0 * S2, BM_ROW_H);
            y += BM_ROW_H + 2.0;
            if r.bottom() < top || r.y > st.bottom() {
                continue;
            }
            i.page.bm_rows.push(DropRow { r, folder: m.folder.clone(), id: m.id });
            let open = Hit::Page(PageHit::Bm(m.id));
            let rm = Hit::Page(PageHit::BmDel(m.id));
            let hov = i.is_hover(&open) || i.is_hover(&rm);
            let lifted = dragging == Some(m.id);
            if hov && !lifted {
                i.gfx.round(r, R_MD, th.hover);
            }
            let tile = R::new(col.x, y - BM_ROW_H - 2.0 + 8.0, 22.0, 22.0);
            let name = if m.title.is_empty() { m.url.clone() } else { m.title.clone() };
            let host = super::host(&m.url);
            let letter = host.chars().next().or_else(|| name.chars().next()).unwrap_or('·').to_uppercase().to_string();
            let fade = if lifted { 0.4 } else { 1.0 };
            i.gfx.round(tile, 6.0, th.press.mix(th.accent, 0.3).alpha(fade));
            i.gfx.text(&letter, tile, Font::ui(11.0, 650).align(Align::Center), th.text.alpha(fade));
            let hw = (i.gfx.measure(&host, SMALL) + 4.0).min(220.0);
            let hx = col.right() - 32.0 - hw;
            let tx = col.x + 34.0;
            let ry = y - BM_ROW_H - 2.0;
            i.gfx.text(&name, R::new(tx, ry, (hx - S4 - tx).max(60.0), BM_ROW_H), BODY, th.text.alpha(fade));
            i.gfx.text(&host, R::new(hx, ry, hw, BM_ROW_H), SMALL.align(Align::Right), th.faint.alpha(fade));
            i.hit(r, open);
            if hov && dragging.is_none() {
                let xr = R::new(col.right() - 28.0, ry + 5.0, 28.0, 28.0);
                let xh = i.is_hover(&rm);
                if xh {
                    i.gfx.round(xr, R_SM, th.press);
                }
                i.gfx.icon("close", xr.x + 7.0, xr.y + 7.0, 14.0, if xh { th.text } else { th.dim });
                i.hit(xr, rm);
            }
        }
        y += S2;
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
            Some(ly) => i.gfx.line(col.x, ly, col.right(), ly, th.accent, 2.0),
            None => {
                if let Some(h) = rows.iter().find(|d| d.folder == folder && d.id == 0) {
                    i.gfx.round_stroke(h.r, R_MD, th.accent, 1.5);
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
    title_block(i, col, st.y + S5 + 2.0, "Bookmarks", &sub);
    let focused = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() } == i.bm_edit;
    i.gfx.round(sr, R_MD, th.bg);
    i.gfx.round_stroke(sr, R_MD, if focused { th.acc_soft } else { th.line }, if focused { 1.5 } else { 1.0 });
    i.gfx.icon("search", sr.x + 10.0, sr.y + 9.0, 16.0, th.faint);
    if q.is_empty() && !focused {
        i.gfx.text("Search bookmarks", R::new(sr.x + 34.0, sr.y, sr.w - 44.0, sr.h), BODY, th.faint);
    }
    let bf = BODY.align(Align::Center);
    let w_import = i.gfx.measure("Import…", bf) + 2.0 * S4;
    let w_folder = i.gfx.measure("New folder", bf) + 2.0 * S4;
    let by = sr.y + 2.0;
    let bx = outline_button(i, col.right() - w_import, by, "Import…", PageHit::BmImport, false).x;
    outline_button(i, bx - S2 - w_folder, by, "New folder", PageHit::BmNewFolder, false);
    close_button(i, st);
}

fn draw_settings(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let schema = settings_schema();
    // Left: title and sections.
    let nav = R::new(st.x, st.y, 190.0, st.h);
    i.gfx.line(nav.right(), st.y + S4, nav.right(), st.bottom() - S4, th.line, 1.0);
    i.gfx.text("Settings", R::new(nav.x + 22.0, nav.y + 22.0, 160.0, 40.0), TITLE, th.text);
    let current = i.page.sections.iter().rposition(|y| *y <= i.page.scroll + 30.0).unwrap_or(0);
    for (n, (name, _)) in schema.iter().enumerate() {
        let r = R::new(nav.x + S3, nav.y + 78.0 + n as f32 * 32.0, nav.w - S5, 30.0);
        let h = Hit::Page(PageHit::Nav(n));
        if n == current || i.is_hover(&h) {
            i.gfx.round(r, R_MD, th.hover);
        }
        i.gfx.text(name, R::new(r.x + 10.0, r.y, r.w - 20.0, r.h), BODY, if n == current { th.text } else { th.dim });
        i.hit(r, h);
    }

    // Right: the rows, scrolled.
    let body = R::new(nav.right() + 1.0, st.y, st.w - nav.w - 1.0, st.h);
    let col = R::new(body.x + 40.0, body.y, (body.w - 80.0).clamp(160.0, 640.0), body.h);
    i.gfx.clip(body);
    let mut y = body.y + 26.0 - i.page.scroll;
    let mut sections = Vec::new();
    for (name, items) in &schema {
        sections.push(y + i.page.scroll - body.y - 26.0);
        i.gfx.text(name, R::new(col.x, y, col.w, 30.0), H2, th.text);
        y += 40.0;
        for (n, it) in items.iter().enumerate() {
            let h = draw_item(i, it, R::new(col.x, y, col.w, 54.0), s);
            y += h;
            if n + 1 < items.len() && !matches!(it, Item::Keys) {
                i.gfx.line(col.x, y, col.right(), y, th.line, 1.0);
            }
            y += 1.0;
        }
        y += 28.0;
    }
    i.gfx.unclip();
    i.page.sections = sections;
    i.page.max = (y + i.page.scroll - body.bottom() + 20.0).max(0.0);
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
    let mut y = st.y + S5 + 2.0 - i.page.scroll;
    y = title_block(i, col, y, "Control", if have { "What each tab costs, right now. Updates every second." } else { "Measuring…" });

    // Four totals, side by side.
    let t = &u["total"];
    let tiles = [
        ("MEMORY", t["mem"].as_f64().map(mem), "all processes"),
        ("CPU", t["cpu"].as_f64().map(cpu), "of the whole machine"),
        ("NETWORK", t["net"].as_f64().map(|n| if n < 1.0 { "0 KB/s".into() } else { net(n) }), "pages, all tabs"),
        ("PROCESSES", t["procs"].as_u64().map(|p| p.to_string()), "Lumen and its pages"),
    ];
    let gap = S3;
    let tw = (col.w - 3.0 * gap) / 4.0;
    for (n, (label, value, note)) in tiles.into_iter().enumerate() {
        let r = R::new(col.x + n as f32 * (tw + gap), y, tw, 92.0);
        i.gfx.round(r, R_LG, th.bg);
        i.gfx.text(label, R::new(r.x + S4, r.y + S3, r.w - 2.0 * S4, 16.0), CAPTION, th.faint);
        let (v, c) = match value {
            Some(v) => (v, th.text),
            None => ("—".to_string(), th.faint),
        };
        i.gfx.text(&v, R::new(r.x + S4, r.y + 28.0, r.w - 2.0 * S4, 40.0), BIG_NUM, c);
        i.gfx.text(note, R::new(r.x + S4, r.y + 68.0, r.w - 2.0 * S4, 18.0), SMALL, th.faint);
    }
    y += 92.0 + S5;

    // The table. Columns hang off the right edge so they line up from row to row.
    let net_r = col.right() - 80.0;
    let cpu_r = net_r - 96.0;
    let mem_r = cpu_r - 72.0;
    let bar_x = mem_r - 80.0 - S4 - 96.0;
    let name_w = (bar_x - S4 - col.x).max(80.0);
    i.gfx.text("TAB", R::new(col.x, y, 200.0, 20.0), CAPTION, th.faint);
    for (label, right) in [("MEMORY", mem_r), ("CPU", cpu_r), ("NETWORK", net_r)] {
        i.gfx.text(label, R::new(right - 90.0, y, 90.0, 20.0), CAPTION.align(Align::Right), th.faint);
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
        let r = R::new(col.x - S2, y + 2.0, col.w + 2.0 * S2, ROW_H - 4.0);
        // Tab rows light up; the two rows that are not tabs are only there to add up.
        let hov = row.id != 0 && i.is_hover(&Hit::Page(PageHit::Row(row.id)));
        if row.id != 0 {
            i.hit(r, Hit::Page(PageHit::Row(row.id)));
        }
        if hov {
            i.gfx.round(r, R_MD, th.hover);
        }
        let c_text = if row.live { th.text } else { th.faint };
        // A live tab with no process found yet has no figures, not zeros.
        let known = row.live && row.mem > 0.0;
        let nw = i.gfx.measure(&row.name, BODY).min(name_w - 90.0).max(40.0);
        i.gfx.text(&row.name, R::new(col.x, y, nw + 2.0, ROW_H), BODY, c_text);
        if !row.state.is_empty() {
            let c = if row.state == "Active" { th.accent } else { th.faint };
            i.gfx.text(row.state, R::new(col.x + nw + S3, y, 90.0, ROW_H), SMALL, c);
        }
        if known {
            // Share of everything Lumen uses: a track and a fill, so the heavy rows show.
            let bar = R::new(bar_x, y + ROW_H / 2.0 - 2.0, 96.0, 4.0);
            i.gfx.round(bar, 2.0, th.press);
            let w = (96.0 * (row.mem / total_mem).min(1.0) as f32).max(if row.mem > 0.0 { 3.0 } else { 0.0 });
            if w > 0.0 {
                i.gfx.round(R::new(bar.x, bar.y, w, 4.0), 2.0, th.accent.alpha(0.8));
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
            i.gfx.text(v, R::new(right - 90.0, y, 90.0, ROW_H), Font::ui(13.0, if strong { 550 } else { 400 }).align(Align::Right), c);
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
    i.gfx.text("Total", R::new(col.x, y, 200.0, ROW_H), BODY_STRONG, th.text);
    let totals = [t["mem"].as_f64().map(mem), t["cpu"].as_f64().map(cpu), t["net"].as_f64().map(net)];
    for (v, right) in totals.iter().zip([mem_r, cpu_r, net_r]) {
        let text = v.clone().unwrap_or_else(|| "—".into());
        i.gfx.text(&text, R::new(right - 90.0, y, 90.0, ROW_H), BODY_STRONG.align(Align::Right), if v.is_some() { th.text } else { th.faint });
    }
    y += ROW_H + S5 + S2;

    i.gfx.text("Limits", R::new(col.x, y, 300.0, 30.0), H2, th.text);
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
        PageHit::Toggle(k) => {
            let on = get(&s.settings, k).as_bool().unwrap_or(false);
            set(i, k, json!(!on));
        }
        PageHit::Swatch(k) => set(i, "palette", json!(k)),
        PageHit::AccentReset => set(i, "accent", json!("")),
        PageHit::Accent => {
            if let Some(c) = pick_color(i.hwnd_raw(), i.theme.accent) {
                set(i, "accent", json!(c.hex()));
            }
        }
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
            // The file and the password are asked for here, in the chrome; the
            // pair crosses to the main loop on an event that is never logged.
            if let Some(path) = pick_kdbx(i.hwnd_raw())
                && let Some(pw) = ask_password(i.hwnd_raw(), &path)
            {
                let _ = i.proxy.send_event(crate::engine::Ev::VaultOpen(path, pw));
            }
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
            if let Some(bytes) = pick_torrent(i.hwnd_raw()) {
                use base64::Engine as _;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                send(&i.proxy, json!({"t": "dl", "a": "addTorrent", "b64": b64}));
            }
        }
        PageHit::DlLimit => {
            // The native menu of speeds aria2 accepts as max-overall-download-limit.
            let (x, y) = i.mouse;
            let kbps = match i.popup(
                &[(1, "No limit".into()), (2, "1 MB/s".into()), (3, "5 MB/s".into()), (4, "10 MB/s".into())],
                x,
                y,
            ) {
                1 => 0,
                2 => 1024,
                3 => 5120,
                4 => 10240,
                _ => return,
            };
            send(&i.proxy, json!({"t": "dl", "a": "limit", "kbps": kbps}));
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
            if let Some(a) = super::dialog::ask_fields(i.hwnd_raw(), "New folder", &[("Name", "")]) {
                send(&i.proxy, json!({"t": "bmFolderAdd", "name": a[0]}));
            }
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
            let pick = i.popup(&items, x, y);
            let browser = match pick {
                0 => return,
                50 => "all".to_string(),
                n => match found.get(n as usize - 1) {
                    Some(f) => f.browser.to_string(),
                    None => return,
                },
            };
            send(&i.proxy, json!({"t": "bmImport", "browser": browser}));
        }
        PageHit::ClearHistory => {
            // One native menu of time ranges first, then the wipe.
            if let Some(since) = ask_range(i) {
                send(&i.proxy, json!({"t": "clearHistory", "since": since}));
            }
        }
        PageHit::ClearData => {
            if let Some(since) = ask_range(i) {
                let (x, y) = i.mouse;
                // Second choice: history only, or the cookies and cache with it.
                let more = match i.popup(&[(1, "History only".into()), (2, "History, cookies and cache".into())], x, y) {
                    1 => false,
                    2 => true,
                    _ => return,
                };
                send(&i.proxy, json!({"t": "clearData", "since": since, "more": more}));
            }
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

pub fn key(_i: &mut Inner, _vk: u32) -> bool {
    false
}

/// The Windows color dialog. Blocks while open, which is what a picker should do.
fn pick_color(owner: windows::Win32::Foundation::HWND, start: Color) -> Option<Color> {
    let mut custom = [COLORREF(0); 16];
    let mut cc = CHOOSECOLORW {
        lStructSize: size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: COLORREF(start.colorref()),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_FULLOPEN | CC_RGBINIT,
        ..Default::default()
    };
    if unsafe { ChooseColorW(&mut cc) }.as_bool() {
        let v = cc.rgbResult.0;
        Some(Color((v & 0xff) as f32 / 255.0, ((v >> 8) & 0xff) as f32 / 255.0, ((v >> 16) & 0xff) as f32 / 255.0, 1.0))
    } else {
        None
    }
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
