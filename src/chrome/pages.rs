//! Settings and Control, drawn natively. A small widget kit (segments, toggles,
//! sliders, swatches) and two pages built from it. Settings keys are read and
//! written through serde, so a new setting is one line in a schema.

use serde_json::{Value, json};
use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
use windows::Win32::Foundation::COLORREF;

use super::{
    Hit, Inner, UiState, send,
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
    CopyAgent,
    Dl(&'static str, u64),
    DlClear,
    PermReset(usize),
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
    AgentSetup,
    SitePerms,
}

const RAM: &[u32] = &[512, 768, 1024, 1536, 2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768];
const NET: &[u32] = &[32, 64, 128, 256, 512, 1024, 2048, 5120, 10240, 25600, 51200, 102400];
const ENGINES: [(&str, &str); 5] = [
    ("DuckDuckGo", "https://duckduckgo.com/?q=%s"),
    ("Google", "https://www.google.com/search?q=%s"),
    ("Brave", "https://search.brave.com/search?q=%s"),
    ("Bing", "https://www.bing.com/search?q=%s"),
    ("Startpage", "https://www.startpage.com/do/search?q=%s"),
];
const KEYS: [(&str, &str); 14] = [
    ("New tab", "Ctrl+T"),
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
    ("Developer tools", "F12"),
];

fn settings_schema() -> Vec<(&'static str, Vec<Item>)> {
    use Item::*;
    vec![
        ("Appearance", vec![
            Swatch,
            Accent,
            Seg("theme", "Theme", "", &[("dark", "Dark"), ("light", "Light"), ("system", "System")]),
            Seg("sidebar_side", "Sidebar", "", &[("left", "Left"), ("right", "Right")]),
            Range("sidebar_width", "Sidebar width", "", Scale::Linear(180.0, 400.0, 4.0), "px"),
            Toggle("compact", "Compact sidebar", "Icons only. Ctrl+B toggles it."),
            Range("gap", "Page margin", "", Scale::Linear(0.0, 24.0, 1.0), "px"),
            Range("radius", "Page corners", "", Scale::Linear(0.0, 20.0, 1.0), "px"),
        ]),
        ("Search", vec![Engine]),
        ("Performance", vec![
            Toggle("adblock", "Block ads and trackers", "Stops them before they load. Saves memory and data."),
            Seg("tracking", "Tracking prevention", "", &[("off", "Off"), ("basic", "Basic"), ("balanced", "Balanced"), ("strict", "Strict")]),
            Range("sleep_after_min", "Freeze hidden tabs after", "Frozen tabs stop running and wake instantly.", Scale::Linear(1.0, 60.0, 1.0), "min"),
            Range("discard_after_min", "Unload hidden tabs after", "Unloaded tabs free all memory and reload when opened.", Scale::Linear(5.0, 240.0, 5.0), "min"),
            Toggle("show_memory", "Memory in sidebar", ""),
        ]),
        ("Passwords", vec![
            Toggle("save_passwords", "Offer to save passwords", "Saved by WebView2, encrypted by Windows with your sign-in. Fills them in next time."),
            Button("Saved passwords", "WebView2 cannot list them. You can wipe them all.", "Clear all", PageHit::ClearPasswords),
        ]),
        ("Site permissions", vec![SitePerms]),
        ("AI agent", vec![
            Toggle("agent", "Let AI agents browse", "A local MCP server. Agents open their own tabs, marked AI, and you can stop them any time."),
            Toggle("agent_shared_profile", "Agents use my profile", "Off: agent tabs have their own cookies and logins. On: they act as you."),
            AgentSetup,
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
            if v >= 1024.0 { format!("{} MB/s", trim(v / 1024.0)) } else { format!("{} KB/s", v as u32) }
        }
        "%" => format!("{}%", v as u32),
        u => format!("{} {u}", v as u32),
    }
}

fn trim(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 { format!("{}", v.round() as u32) } else { format!("{v:.1}") }
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
        Item::AgentSetup => {
            let on = s.settings.agent;
            row_label(i, R::new(r.x, r.y, r.w, 54.0), "Connect Claude Code", if on { &s.agent_endpoint } else { "Turn the agent on first." });
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
    } else {
        format!("{} KB", (b / 1024.0).round())
    }
}

fn draw_downloads(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let col = R::new(st.x + ((st.w - 760.0) / 2.0).max(40.0), st.y, st.w.min(760.0 + 80.0) - 80.0, st.h);
    i.gfx.clip(st);
    let mut y = st.y + 26.0 - i.page.scroll;
    i.gfx.text("Downloads", R::new(col.x, y, 300.0, 40.0), Font::serif(30.0), th.text);
    done_button(i, R::new(col.right() - 90.0, y + 5.0, 90.0, 30.0));
    if s.downloads.iter().any(|d| d.state == "done" || d.state == "failed") {
        let r = R::new(col.right() - 210.0, y + 10.0, 110.0, 20.0);
        let h = Hit::Page(PageHit::DlClear);
        i.gfx.text("Clear finished", r, Font::ui(12.0, 400).align(Align::Right), if i.is_hover(&h) { th.text } else { th.faint });
        i.hit(r, h);
    }
    y += 64.0;
    if s.downloads.is_empty() {
        i.gfx.text("Nothing yet. Files you download show up here.", R::new(col.x, y, col.w, 30.0), Font::ui(13.0, 400), th.faint);
    }
    for d in &s.downloads {
        let r = R::new(col.x, y, col.w, 64.0);
        i.gfx.text(&d.name, R::new(r.x, r.y + 10.0, r.w - 220.0, 22.0), Font::ui(13.5, 500), if d.state == "failed" { th.faint } else { th.text });
        let pct = if d.total > 0 { d.received as f32 / d.total as f32 } else { 0.0 };
        let status = match d.state {
            "done" => format!("Done · {}", fmt_bytes(d.received)),
            "failed" => "Failed".to_string(),
            "paused" => format!("Paused · {} of {}", fmt_bytes(d.received), fmt_bytes(d.total)),
            _ if d.total > 0 => format!("{} of {} · {}%", fmt_bytes(d.received), fmt_bytes(d.total), (pct * 100.0) as u32),
            _ => fmt_bytes(d.received),
        };
        let host = super::host(&d.url);
        i.gfx.text(&format!("{status}   {host}"), R::new(r.x, r.y + 32.0, r.w - 220.0, 18.0), Font::ui(12.0, 400), th.faint);
        if d.state == "progress" || d.state == "paused" {
            let bar = R::new(r.x, r.bottom() - 6.0, r.w - 220.0, 3.0);
            i.gfx.round(bar, 1.5, th.press);
            i.gfx.round(R::new(bar.x, bar.y, bar.w * pct.min(1.0), 3.0), 1.5, th.accent);
        }
        let acts: &[(&str, &'static str)] = match d.state {
            "progress" => &[("Pause", "pause"), ("Cancel", "cancel")],
            "paused" => &[("Resume", "resume"), ("Cancel", "cancel")],
            "done" => &[("Open", "open"), ("Show in folder", "folder")],
            _ => &[],
        };
        let mut x = r.right();
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
        PageHit::CopyAgent => super::copy_text(i.hwnd_raw(), &s.agent_cmd),
        PageHit::Dl(a, id) => send(&i.proxy, json!({"t": "dl", "a": a, "id": id})),
        PageHit::DlClear => send(&i.proxy, json!({"t": "dl", "a": "clear"})),
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
