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
    Unbookmark(String),
    ClearHistory,
    ClearData,
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
    max: f32,
    pub usage: Option<Value>,
    slide: Option<Slide>,
    sliders: Vec<(&'static str, R, Scale)>,
    sections: Vec<f32>,
    view: R,
    /// A short-lived note ("Copy a link or magnet first", a failed unlock).
    pub(super) flash: Option<(String, std::time::Instant)>,
}

// ------------------------------------------------------------------ schema

enum Item {
    Swatch,
    Accent,
    Seg(&'static str, &'static str, &'static str, &'static [(&'static str, &'static str)]),
    Toggle(&'static str, &'static str, &'static str),
    Range(&'static str, &'static str, &'static str, Scale, &'static str),
    Engine,
    Keys,
    Button(&'static str, &'static str, &'static str, PageHit),
    KeePass,
    AgentSetup,
    SitePerms,
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
        ("Appearance", vec![
            Swatch,
            Accent,
            Seg("theme", "Theme", "", &[("dark", "Dark"), ("light", "Light"), ("system", "System")]),
            Toggle("dark_pages", "Darken light sites", "When Lumen is dark, Dark Reader repaints sites that have no dark mode. Shift+F9 skips a site."),
            Seg("ui_font", "Interface font", "Atkinson Hyperlegible is designed for easy reading.", &[("segoe", "Segoe"), ("instrument", "Instrument"), ("figtree", "Figtree"), ("atkinson", "Atkinson")]),
            Seg("display_font", "Display font", "The clock and page titles.", &[("instrument", "Instrument Serif"), ("fraunces", "Fraunces"), ("newsreader", "Newsreader")]),
            Seg("sidebar_side", "Sidebar", "", &[("left", "Left"), ("right", "Right")]),
            Range("sidebar_width", "Sidebar width", "", Scale::Linear(180.0, 400.0, 4.0), "px"),
            Toggle("compact", "Compact sidebar", "Icons only. Ctrl+B toggles it."),
            Range("gap", "Page margin", "", Scale::Linear(0.0, 24.0, 1.0), "px"),
            Range("radius", "Page corners", "", Scale::Linear(0.0, 20.0, 1.0), "px"),
        ]),
        ("Search", vec![
            Engine,
            Toggle("remember_text", "Search history by page text", "Lumen keeps the words of pages you open, on this PC only, so the address bar finds them by what they said. Pages with a password box are skipped."),
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
            Toggle("agent_shared_profile", "Agents use my profile", "Off: agent tabs have their own cookies and logins. On: they act as you."),
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
    i.page.sliders.clear();
    i.page.view = st;
    match s.panel {
        "settings" => draw_settings(i, st, s),
        "downloads" => draw_downloads(i, st, s),
        "history" | "bookmarks" => draw_library(i, st, s),
        _ => draw_control(i, st, s),
    }
}

fn row_label(i: &Inner, r: R, label: &str, hint: &str) {
    let th = i.theme;
    if hint.is_empty() {
        i.gfx.text(label, R::new(r.x, r.y, r.w * 0.55, r.h), Font::ui(13.0, 500), th.text);
    } else {
        i.gfx.text(label, R::new(r.x, r.y + 8.0, r.w * 0.55, 20.0), Font::ui(13.0, 500), th.text);
        i.gfx.text(hint, R::new(r.x, r.y + 27.0, r.w * 0.6, 18.0), Font::ui(12.0, 400), th.faint);
    }
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
            row_label(i, R::new(r.x, r.y, r.w, 54.0), "Palette", "");
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
            row_label(i, R::new(r.x, r.y, r.w, 54.0), "Accent", "Focus rings, toggles, the loading ring.");
            let sw = R::new(right - 28.0, r.y + 13.0, 28.0, 28.0);
            i.gfx.round(sw, 8.0, th.accent);
            i.gfx.round_stroke(sw, 8.0, th.line, 1.0);
            i.hit(sw, Hit::Page(PageHit::Accent));
            let reset = R::new(sw.x - 60.0, r.y + 17.0, 50.0, 20.0);
            let h = Hit::Page(PageHit::AccentReset);
            i.gfx.text("Reset", reset, Font::ui(12.0, 400).align(Align::Right), if i.is_hover(&h) { th.text } else { th.faint });
            i.hit(reset, h);
            54.0
        }
        Item::Seg(k, label, hint, opts) => {
            row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint);
            let cur = get(&s.settings, k).as_str().unwrap_or("").to_string();
            draw_seg(i, right, r.y + 27.0, k, opts, &cur);
            54.0
        }
        Item::Toggle(k, label, hint) => {
            row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint);
            let on = get(&s.settings, k).as_bool().unwrap_or(false);
            draw_toggle(i, right, r.y + 27.0, k, on, true);
            54.0
        }
        Item::Range(k, label, hint, scale, unit) => {
            row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint);
            let v = get(&s.settings, k).as_f64().unwrap_or(0.0) as f32;
            match limit_toggle(k) {
                Some(t) => {
                    let enabled = *k != "cpu_limit_pct" || s.cpu_cap;
                    let on = get(&s.settings, t).as_bool().unwrap_or(false) && enabled;
                    draw_toggle(i, right, r.y + 27.0, t, on, enabled);
                    draw_slider(i, right - 50.0, r.y + 27.0, k, *scale, unit, v, !on);
                }
                None => draw_slider(i, right, r.y + 27.0, k, *scale, unit, v, false),
            }
            54.0
        }
        Item::Engine => {
            row_label(i, R::new(r.x, r.y, r.w, 54.0), "Search engine", "");
            let f = Font::ui(13.0, 450).align(Align::Center);
            let widths: Vec<f32> = ENGINES.iter().map(|(l, _)| i.gfx.measure(l, f) + 22.0).collect();
            let total: f32 = widths.iter().sum::<f32>() + 4.0;
            let pill = R::new(right - total, r.y + 12.0, total, 30.0);
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
            54.0
        }
        Item::Button(label, hint, action, hit) => {
            row_label(i, R::new(r.x, r.y, r.w, 54.0), label, hint);
            draw_button(i, right, r.y + 27.0, action, hit.clone());
            54.0
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
            if let Some(note) = note {
                row_label(i, R::new(r.x, r.y, r.w, 54.0), "KeePass file", &note);
            } else if s.vault > 0 {
                let hint = format!("{} entries open. Ones matching the page you are on appear by the address bar. Locks after 10 idle minutes.", s.vault);
                row_label(i, R::new(r.x, r.y, r.w, 54.0), "KeePass file", &hint);
            } else {
                row_label(
                    i,
                    R::new(r.x, r.y, r.w, 54.0),
                    "KeePass file",
                    "Reads a .kdbx on this PC. The password stays in memory and locks after 10 idle minutes.",
                );
            }
            if s.vault > 0 {
                draw_button(i, right, r.y + 27.0, "Lock now", PageHit::KdbxLock);
            } else {
                draw_button(i, right, r.y + 27.0, "Open KeePass file…", PageHit::KdbxOpen);
            }
            54.0
        }
        Item::AgentSetup => {
            let on = s.settings.agent;
            row_label(i, R::new(r.x, r.y, r.w, 54.0), "Connect an AI agent", if on { "Claude Code, opencode, Codex, Cursor: run lumen-agent.exe mcp. Copy gives the Claude Code line." } else { "Turn the agent on first." });
            if on {
                draw_button(i, right, r.y + 27.0, "Copy setup command", PageHit::CopyAgent);
            }
            54.0
        }
        Item::SitePerms => {
            if s.rules.is_empty() {
                i.gfx.text("No answers remembered yet. When a site asks for your camera, location or similar, the sidebar asks you.", R::new(r.x, r.y, r.w, 36.0), Font::ui(12.5, 400), th.faint);
                return 40.0;
            }
            let mut y = r.y;
            for (n, (origin, kind, allow)) in s.rules.iter().enumerate() {
                i.gfx.text(&super::host(origin), R::new(r.x, y, r.w * 0.45, 40.0), Font::ui(13.0, 500), th.text);
                i.gfx.text(&format!("{kind} · {}", if *allow { "Allowed" } else { "Blocked" }), R::new(r.x + r.w * 0.45, y, r.w * 0.35, 40.0), Font::ui(12.5, 400), if *allow { th.dim } else { th.faint });
                let br = R::new(right - 60.0, y + 8.0, 60.0, 24.0);
                let h = Hit::Page(PageHit::PermReset(n));
                i.gfx.text("Reset", br, Font::ui(12.0, 400).align(Align::Right), if i.is_hover(&h) { th.text } else { th.faint });
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
fn summary(active: usize, down: i64, up: i64) -> String {
    if active == 0 {
        return String::new();
    }
    format!("{active} active · {}/s down · {}/s up", fmt_bytes(down), fmt_bytes(up))
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
        s.push_str(&format!("   {host}"));
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

/// A quiet text button in the header row, right-aligned from `x` inwards.
fn head_action(i: &mut Inner, x: f32, y: f32, label: &str, h: Hit) -> f32 {
    let th = i.theme;
    let w = i.gfx.measure(label, Font::ui(12.0, 400));
    let r = R::new(x - w, y, w, 20.0);
    i.gfx.text(label, r, Font::ui(12.0, 400).align(Align::Right), if i.is_hover(&h) { th.text } else { th.faint });
    i.hit(r, h);
    x - w - 18.0
}

fn draw_downloads(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let col = R::new(st.x + ((st.w - 760.0) / 2.0).max(40.0), st.y, st.w.min(760.0 + 80.0) - 80.0, st.h);
    i.gfx.clip(st);
    let mut y = st.y + 26.0 - i.page.scroll;
    i.gfx.text("Downloads", R::new(col.x, y, 300.0, 40.0), Font::serif(30.0), th.text);
    // Actions right of Done, laid out right to left so new ones never collide.
    done_button(i, R::new(col.right() - 90.0, y + 5.0, 90.0, 30.0));
    let mut x = col.right() - 90.0 - 18.0;
    if s.downloads.iter().any(|d| d.state == "done" || d.state == "failed") {
        x = head_action(i, x, y + 10.0, "Clear finished", Hit::Page(PageHit::DlClear));
    }
    x = head_action(i, x, y + 10.0, "Open folder", Hit::Page(PageHit::DlOpenFolder));
    x = head_action(i, x, y + 10.0, "Open torrent…", Hit::Page(PageHit::DlTorrentPick));
    head_action(i, x, y + 10.0, "Add link", Hit::Page(PageHit::DlAddLink));
    y += 52.0;

    // The summary line, or the short-lived "copy a link" note in its place.
    let mut flash_gone = false;
    if let Some((msg, at)) = &i.page.flash {
        if at.elapsed().as_secs_f32() < 3.0 {
            i.gfx.text(msg, R::new(col.x, y, col.w - 160.0, 22.0), Font::ui(12.5, 400), th.faint);
        } else {
            flash_gone = true;
        }
    } else {
        let active: Vec<&UiDl> = s.downloads.iter().filter(|d| matches!(d.state, "progress" | "paused")).collect();
        let down: i64 = active.iter().map(|d| d.speed as i64).sum();
        let up: i64 = s.downloads.iter().filter(|d| matches!(d.state, "progress" | "paused" | "seeding")).map(|d| d.up as i64).sum();
        let t = summary(active.len(), down, up);
        if !t.is_empty() {
            i.gfx.text(&t, R::new(col.x, y, col.w - 160.0, 22.0), Font::ui(12.5, 400), th.faint);
        }
    }
    if flash_gone {
        i.page.flash = None;
    }
    // The speed-limit chip: what aria2 is capped at, if anything.
    let chip = chip_label(s.settings.download_limit_kbps);
    let cw = i.gfx.measure(chip, Font::ui(11.5, 500)) + 16.0;
    let cr = R::new(col.right() - cw, y - 1.0, cw, 22.0);
    let ch = Hit::Page(PageHit::DlLimit);
    i.gfx.round(cr, 11.0, if i.is_hover(&ch) { th.hover } else { th.press });
    i.gfx.text(chip, cr, Font::ui(11.5, 500).align(Align::Center), th.dim);
    i.hit(cr, ch);
    y += 34.0;

    if s.downloads.is_empty() {
        i.gfx.text("Nothing yet. Files you download show up here.", R::new(col.x, y, col.w, 30.0), Font::ui(13.0, 400), th.faint);
        y += 30.0;
    }
    for g in group_order() {
        let rows: Vec<&UiDl> = s.downloads.iter().filter(|d| group(d.state) == g).collect();
        if rows.is_empty() {
            continue;
        }
        y += 8.0;
        i.gfx.text(&g.to_uppercase(), R::new(col.x, y, col.w, 22.0), Font::ui(11.0, 600), th.faint);
        y += 26.0;
        // The firewall note rides under the first header that actually has a torrent.
        if s.settings.torrent_note && rows.iter().any(|d| d.kind == "torrent") {
            let msg = "Windows may ask to let aria2 through the firewall; allow it for faster torrents.";
            let h = Hit::Page(PageHit::DlNote);
            i.gfx.text(msg, R::new(col.x, y, col.w, 20.0), Font::ui(11.5, 400), if i.is_hover(&h) { th.text } else { th.faint });
            i.hit(R::new(col.x, y, col.w, 20.0), h);
            y += 24.0;
        }
        for d in rows {
            let r = R::new(col.x, y, col.w, 64.0);
            let mut tx = r.x;
            // The kind tag: File / Torrent / Video, small caps, quiet.
            let kind = match d.kind {
                "torrent" => "Torrent",
                "video" => "Video",
                _ => "File",
            };
            let kw = i.gfx.measure(kind, Font::ui(9.5, 700)) + 12.0;
            let kr = R::new(tx, r.y + 10.0, kw, 16.0);
            i.gfx.round(kr, 4.0, th.press);
            i.gfx.text(kind, kr, Font::ui(9.5, 700).align(Align::Center), th.faint);
            tx += kw + 8.0;
            i.gfx.text(&d.name, R::new(tx, r.y + 8.0, r.right() - 220.0 - tx, 22.0), Font::ui(13.5, 500), if d.state == "failed" { th.faint } else { th.text });
            i.gfx.text(&status_line(d), R::new(r.x, r.y + 32.0, r.w - 220.0, 18.0), Font::ui(12.0, 400), th.faint);
            if d.state == "progress" || d.state == "paused" {
                let pct = if d.total > 0 { d.received as f32 / d.total as f32 } else { 0.0 };
                let bar = R::new(r.x, r.bottom() - 6.0, r.w - 220.0, 3.0);
                i.gfx.round(bar, 1.5, th.press);
                i.gfx.round(R::new(bar.x, bar.y, bar.w * pct.min(1.0), 3.0), 1.5, th.accent);
            }
            let acts: &[(&str, &'static str)] = match d.state {
                "progress" => &[("Pause", "pause"), ("Cancel", "cancel")],
                "paused" => &[("Resume", "resume"), ("Cancel", "cancel")],
                "seeding" => &[("Stop seeding", "stopSeed")],
                "done" => &[("Open", "open"), ("Show in folder", "folder")],
                "failed" if d.url.starts_with("http") => &[("Retry", "retry")],
                _ => &[],
            };
            // Finished rows can go: a quiet x at the far right.
            let mut x = r.right();
            if d.state == "done" || d.state == "failed" {
                let xr = R::new(x - 26.0, r.y + 21.0, 22.0, 22.0);
                let h = Hit::Page(PageHit::Dl("remove", d.id));
                if i.is_hover(&h) {
                    i.gfx.round(xr, 6.0, th.press);
                }
                i.gfx.icon("close", xr.x + 4.0, xr.y + 4.0, 14.0, if i.is_hover(&h) { th.text } else { th.faint });
                i.hit(xr, h);
                x -= 30.0;
            }
            for (label, a) in acts.iter().rev() {
                let w = i.gfx.measure(label, Font::ui(12.5, 450)) + 18.0;
                let br = R::new(x - w, r.y + 18.0, w, 28.0);
                let h = Hit::Page(PageHit::Dl(a, d.id));
                if i.is_hover(&h) {
                    i.gfx.round(br, 7.0, th.hover);
                }
                i.gfx.text(label, br, Font::ui(12.5, 450).align(Align::Center), th.dim);
                i.hit(br, h);
                x -= w + 4.0;
            }
            i.gfx.line(col.x, r.bottom(), col.right(), r.bottom(), th.line, 1.0);
            y += 65.0;
        }
    }
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
}

/// History (grouped by day) or bookmarks: one quiet list, click to open, x to remove.
fn draw_library(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let history = s.panel == "history";
    let col = R::new(st.x + ((st.w - 760.0) / 2.0).max(40.0), st.y, st.w.min(760.0 + 80.0) - 80.0, st.h);
    i.gfx.clip(st);
    let mut y = st.y + 26.0 - i.page.scroll;
    i.gfx.text(if history { "History" } else { "Bookmarks" }, R::new(col.x, y, 300.0, 40.0), Font::serif(30.0), th.text);
    done_button(i, R::new(col.right() - 90.0, y + 5.0, 90.0, 30.0));
    if history && !s.history.is_empty() {
        let r = R::new(col.right() - 210.0, y + 10.0, 110.0, 20.0);
        let h = Hit::Page(PageHit::ClearHistory);
        i.gfx.text("Clear history", r, Font::ui(12.0, 400).align(Align::Right), if i.is_hover(&h) { th.text } else { th.faint });
        i.hit(r, h);
    }
    y += 60.0;
    let rows: Vec<(String, String, String, String)> = if history {
        s.history.iter().map(|v| (v.url.clone(), v.title.clone(), v.day.clone(), v.time.clone())).collect()
    } else {
        s.bookmarks.iter().map(|(u, t)| (u.clone(), t.clone(), String::new(), String::new())).collect()
    };
    if rows.is_empty() {
        let msg = if history { "Nothing yet. Pages you visit show up here." } else { "No bookmarks yet. Press Ctrl+D or the star in the address bar." };
        i.gfx.text(msg, R::new(col.x, y, col.w, 30.0), Font::ui(13.0, 400), th.faint);
    }
    let mut last_day = String::new();
    for (url, title, day, time) in rows {
        if history && day != last_day {
            y += 8.0;
            i.gfx.text(&day.to_uppercase(), R::new(col.x, y, col.w, 22.0), Font::ui(11.0, 600), th.faint);
            y += 28.0;
            last_day = day;
        }
        let r = R::new(col.x - 8.0, y, col.w + 16.0, 36.0);
        if y > st.bottom() || y + 36.0 < st.y {
            y += 37.0;
            continue;
        }
        let open = Hit::Page(PageHit::Open(url.clone()));
        let rm = Hit::Page(if history { PageHit::Forget(url.clone()) } else { PageHit::Unbookmark(url.clone()) });
        let hov = i.is_hover(&open) || i.is_hover(&rm);
        if hov {
            i.gfx.round(r, 8.0, th.hover);
        }
        let mut x = col.x;
        if history {
            i.gfx.text(&time, R::new(x, y, 44.0, 36.0), Font::ui(12.0, 400), th.faint);
            x += 54.0;
        }
        let name = if title.is_empty() { url.clone() } else { title };
        let host = super::host(&url);
        let hw = i.gfx.measure(&host, Font::ui(12.0, 400)) + 20.0;
        let nw = (col.right() - x - hw - 40.0).max(80.0);
        i.gfx.text(&name, R::new(x, y, nw, 36.0), Font::ui(13.0, 450), th.text);
        let used = i.gfx.measure(&name, Font::ui(13.0, 450)).min(nw);
        i.gfx.text(&host, R::new(x + used + 12.0, y, hw, 36.0), Font::ui(12.0, 400), th.faint);
        i.hit(r, open);
        if hov {
            let xr = R::new(col.right() - 26.0, y + 7.0, 22.0, 22.0);
            if i.is_hover(&rm) {
                i.gfx.round(xr, 6.0, th.press);
            }
            i.gfx.icon("close", xr.x + 4.0, xr.y + 4.0, 14.0, th.dim);
            i.hit(xr, rm);
        }
        y += 37.0;
    }
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
}

fn draw_settings(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let schema = settings_schema();
    // Left: title and sections.
    let nav = R::new(st.x, st.y, 190.0, st.h);
    i.gfx.line(nav.right(), st.y + 16.0, nav.right(), st.bottom() - 16.0, th.line, 1.0);
    i.gfx.text("Settings", R::new(nav.x + 22.0, nav.y + 22.0, 160.0, 40.0), Font::serif(28.0), th.text);
    let current = i.page.sections.iter().rposition(|y| *y <= i.page.scroll + 30.0).unwrap_or(0);
    for (n, (name, _)) in schema.iter().enumerate() {
        let r = R::new(nav.x + 12.0, nav.y + 78.0 + n as f32 * 32.0, nav.w - 24.0, 30.0);
        let h = Hit::Page(PageHit::Nav(n));
        if n == current || i.is_hover(&h) {
            i.gfx.round(r, 7.0, th.hover);
        }
        i.gfx.text(name, R::new(r.x + 10.0, r.y, r.w - 20.0, r.h), Font::ui(13.0, 450), if n == current { th.text } else { th.dim });
        i.hit(r, h);
    }
    done_button(i, R::new(nav.x + 12.0, nav.bottom() - 46.0, nav.w - 24.0, 30.0));

    // Right: the rows, scrolled.
    let body = R::new(nav.right() + 1.0, st.y, st.w - nav.w - 1.0, st.h);
    let col = R::new(body.x + 40.0, body.y, (body.w - 80.0).min(640.0), body.h);
    i.gfx.clip(body);
    let mut y = body.y + 26.0 - i.page.scroll;
    let mut sections = Vec::new();
    for (name, items) in &schema {
        sections.push(y + i.page.scroll - body.y - 26.0);
        i.gfx.text(name, R::new(col.x, y, col.w, 30.0), Font::serif(22.0), th.text);
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
}

fn done_button(i: &mut Inner, r: R) {
    let th = i.theme;
    let h = Hit::Page(PageHit::Done);
    if i.is_hover(&h) {
        i.gfx.round(r, 7.0, th.hover);
    }
    i.gfx.text("Done", R::new(r.x + 10.0, r.y, 60.0, r.h), Font::ui(13.0, 450), th.dim);
    i.gfx.text("Esc", R::new(r.x, r.y, r.w - 10.0, r.h), Font::ui(11.5, 400).align(Align::Right), th.faint);
    i.hit(r, h);
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

fn draw_control(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let u = i.page.usage.clone().unwrap_or(json!({}));
    let col = R::new(st.x + ((st.w - 860.0) / 2.0).max(40.0), st.y, st.w.min(860.0 + 80.0) - 80.0, st.h);
    i.gfx.clip(st);
    let mut y = st.y + 26.0 - i.page.scroll;
    i.gfx.text("Control", R::new(col.x, y, 300.0, 40.0), Font::serif(30.0), th.text);
    done_button(i, R::new(col.right() - 90.0, y + 5.0, 90.0, 30.0));
    y += 60.0;
    // Totals.
    let t = &u["total"];
    let stats = [
        (t["mem"].as_f64().map(mem).unwrap_or("—".into()), "MEMORY"),
        (t["cpu"].as_f64().map(cpu).unwrap_or("—".into()), "CPU"),
        (t["net"].as_f64().map(net).unwrap_or("—".into()), "NETWORK"),
        (t["procs"].as_u64().map(|p| p.to_string()).unwrap_or("—".into()), "PROCESSES"),
    ];
    let mut x = col.x;
    for (v, label) in stats {
        let w = i.gfx.measure(&v, Font::serif(30.0)).max(90.0) + 40.0;
        i.gfx.text(&v, R::new(x, y, w, 36.0), Font::serif(30.0), th.text);
        i.gfx.text(label, R::new(x, y + 38.0, w, 16.0), Font::ui(11.0, 500), th.faint);
        x += w;
    }
    y += 74.0;
    i.gfx.line(col.x, y, col.right(), y, th.line, 1.0);
    y += 12.0;
    // Table.
    let cols = [col.right() - 290.0, col.right() - 200.0, col.right() - 100.0];
    let head = Font::ui(11.0, 500);
    i.gfx.text("TAB", R::new(col.x, y, 200.0, 24.0), head, th.faint);
    for (n, h) in ["MEMORY", "CPU", "NETWORK"].iter().enumerate() {
        i.gfx.text(h, R::new(cols[n] - 80.0, y, 80.0, 24.0), head.align(Align::Right), th.faint);
    }
    y += 30.0;
    let total_mem = t["mem"].as_f64().unwrap_or(1.0).max(1.0);
    let mut rows: Vec<(String, String, f64, f64, f64, bool, Option<u64>)> = Vec::new();
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
        rows.push((
            name,
            state.into(),
            x["mem"].as_f64().unwrap_or(0.0),
            x["cpu"].as_f64().unwrap_or(0.0),
            x["net"].as_f64().unwrap_or(0.0),
            live,
            (live && tab.id != s.active).then_some(tab.id),
        ));
    }
    rows.sort_by(|a, b| b.2.total_cmp(&a.2));
    let ui = &u["ui"];
    let core = &u["core"];
    rows.push(("Lumen".into(), "interface".into(), ui["mem"].as_f64().unwrap_or(0.0), ui["cpu"].as_f64().unwrap_or(0.0), -1.0, true, None));
    rows.push(("Browser core".into(), String::new(), core["mem"].as_f64().unwrap_or(0.0), core["cpu"].as_f64().unwrap_or(0.0), -1.0, true, None));
    for (name, state, m, c, n, live, unload) in rows {
        let r = R::new(col.x, y, col.w, 40.0);
        let c_text = if live { th.text } else { th.faint };
        let nw = i.gfx.measure(&name, Font::ui(13.0, 450)).min(cols[0] - 120.0 - col.x - 120.0);
        i.gfx.text(&name, R::new(r.x, r.y, nw + 2.0, r.h), Font::ui(13.0, 450), c_text);
        if !state.is_empty() {
            i.gfx.text(&state, R::new(r.x + nw + 10.0, r.y, 110.0, r.h), Font::ui(11.5, 400), th.faint);
        }
        let vals = [
            if live { mem(m) } else { "—".into() },
            if live { cpu(c) } else { "—".into() },
            if n < 0.0 || !live { "—".into() } else { net(n) },
        ];
        for (k, v) in vals.iter().enumerate() {
            i.gfx.text(v, R::new(cols[k] - 90.0, r.y, 90.0, r.h), Font::ui(13.0, if k == 0 { 500 } else { 400 }).align(Align::Right), if k == 0 { c_text } else { th.dim });
        }
        if live {
            let bw = 90.0 * (m / total_mem).min(1.0) as f32;
            i.gfx.round(R::new(cols[0] - bw, r.bottom() - 7.0, bw, 2.0), 1.0, th.acc_soft);
        }
        if let Some(id) = unload {
            let br = R::new(col.right() - 70.0, r.y + 8.0, 70.0, 24.0);
            let h = Hit::Page(PageHit::Unload(id));
            i.gfx.text("Unload", br, Font::ui(12.0, 400).align(Align::Right), if i.is_hover(&h) { th.text } else { th.faint });
            i.hit(br, h);
        }
        i.gfx.line(col.x, r.bottom(), col.right(), r.bottom(), th.line, 1.0);
        y += 41.0;
    }
    y += 26.0;
    i.gfx.text("Limits", R::new(col.x, y, 300.0, 30.0), Font::serif(22.0), th.text);
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
    let _: Color = th.text;
}

// ------------------------------------------------------------------- input

pub fn press(i: &mut Inner, p: PageHit, x: f32, _y: f32) {
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

pub fn drag_move(i: &mut Inner, x: f32, _y: f32) -> bool {
    if i.page.slide.is_none() {
        return false;
    }
    drag_to(i, x);
    true
}

pub fn drag_end(i: &mut Inner) -> bool {
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
        PageHit::Unbookmark(url) => send(&i.proxy, json!({"t": "unbookmark", "url": url})),
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
    use super::{classify_link, days_from_civil, fmt_bytes, group, midnight, summary, unix_now};
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
        assert_eq!(summary(3, 4_404_019, 317_440), "3 active · 4.2 MB/s down · 310 KB/s up");
        // Nothing running: no line at all.
        assert_eq!(summary(0, 0, 0), "");
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
        assert_eq!(super::status_line(&d), "Seeding · up 300 KB/s · ratio 0.42 · 9 peers   example.com");

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
}
