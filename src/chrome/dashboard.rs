//! The dashboard: Lumen at a glance. Memory, CPU, network, tabs, downloads, privacy today and
//! the agents, one tile each. A tile opens the full page it sums up.
//!
//! The live numbers are Control's: the process sampling runs only while this page or Control
//! is open, so a closed dashboard costs nothing.

use serde_json::{Value, json};

use super::{
    Hit, Inner, UiState,
    gfx::{Align, R},
    pages::{close_button, column, fmt_bytes, title_block},
    report::fmt_time,
    send,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    /// Open a full page: "control" | "downloads" | "report" | "settings".
    Open(&'static str),
    /// Trim every hidden page now.
    FreeMemory,
    /// Stop every agent and close its tabs.
    StopAgents,
}

fn mb(b: f64) -> String {
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", b / (1024.0 * 1024.0 * 1024.0))
    } else {
        format!("{} MB", (b / (1024.0 * 1024.0)).round())
    }
}

/// Counts of the tabs by state: open, live, asleep (frozen), unloaded.
pub fn tab_counts(s: &UiState) -> (usize, usize, usize, usize) {
    let mut c = (0, 0, 0, 0);
    for t in &s.tabs {
        c.0 += 1;
        match t.status {
            "live" | "new" | "loading" => c.1 += 1,
            "asleep" => c.2 += 1,
            "off" => c.3 += 1,
            _ => {}
        }
    }
    c
}

/// One tile: a caption, a big value, a note, and the page it opens.
struct Tile {
    caption: &'static str,
    value: String,
    note: String,
    open: Option<&'static str>,
}

pub fn draw(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let u: Value = i.page.usage.clone().unwrap_or(json!({}));
    let t = &u["total"];
    let col = column(st, 860.0);
    i.gfx.clip(st);
    let mut y = st.y + i.look.s5 + 2.0 - i.page.scroll;
    y = title_block(i, col, y, "Dashboard", &format!("Lumen {} at a glance. Live while this page is open.", crate::VERSION));

    let (open, live, asleep, off) = tab_counts(s);
    let active_dl: Vec<_> = s.downloads.iter().filter(|d| d.state == "progress").collect();
    let speed = active_dl.iter().map(|d| d.speed).sum::<u64>() as i64;
    let agents = s.tabs.iter().filter(|t| t.agent).count();
    let privacy = s.privacy_today.unwrap_or_default();

    let tiles = [
        Tile {
            caption: "MEMORY",
            value: t["mem"].as_f64().map(mb).or_else(|| s.mem.map(|(b, _)| mb(b as f64))).unwrap_or_else(|| "—".into()),
            note: u["ui"]["mem"].as_f64().map(|m| format!("Lumen itself {}", mb(m))).unwrap_or_else(|| "Measuring…".into()),
            open: Some("control"),
        },
        Tile {
            caption: "CPU",
            value: t["cpu"].as_f64().map(|c| format!("{c:.1}%")).unwrap_or_else(|| "—".into()),
            note: "of the whole machine".into(),
            open: Some("control"),
        },
        Tile {
            caption: "NETWORK",
            value: t["net"].as_f64().map(|n| format!("{}/s", fmt_bytes(n as i64))).unwrap_or_else(|| "—".into()),
            note: "pages, all tabs".into(),
            open: Some("control"),
        },
        Tile {
            caption: "TABS",
            value: open.to_string(),
            note: format!("{live} live · {asleep} asleep · {off} unloaded"),
            open: None,
        },
        Tile {
            caption: "DOWNLOADS",
            value: active_dl.len().to_string(),
            note: if active_dl.is_empty() { "nothing running".into() } else { format!("{}/s", fmt_bytes(speed)) },
            open: Some("downloads"),
        },
        Tile {
            caption: "BLOCKED TODAY",
            value: privacy.blocked().to_string(),
            note: format!("{} saved · {}", fmt_bytes(privacy.bytes as i64), fmt_time(privacy.ms)),
            open: Some("report"),
        },
    ];

    let gap = i.look.s3;
    let per = 3usize;
    let tw = (col.w - (per as f32 - 1.0) * gap) / per as f32;
    for (n, tile) in tiles.iter().enumerate() {
        let r = R::new(col.x + (n % per) as f32 * (tw + gap), y + (n / per) as f32 * (104.0 + gap), tw, 104.0);
        let h = tile.open.map(|p| Hit::Dash(Click::Open(p)));
        let hov = h.as_ref().is_some_and(|h| i.is_hover(h));
        i.gfx.round(r, i.look.r_lg, if hov { th.hover } else { th.bg });
        i.gfx.text(tile.caption, R::new(r.x + i.look.s4, r.y + i.look.s3, r.w - 2.0 * i.look.s4, 16.0), i.look.caption, th.faint);
        i.gfx.text(&tile.value, R::new(r.x + i.look.s4, r.y + 30.0, r.w - 2.0 * i.look.s4, 40.0), i.look.big_num, th.text);
        i.gfx.text(&tile.note, R::new(r.x + i.look.s4, r.y + 74.0, r.w - 2.0 * i.look.s4, 18.0), i.look.small, th.faint);
        if let Some(h) = h {
            i.hit(r, h);
        }
    }
    y += 2.0 * 104.0 + gap + i.look.s5;

    // Actions under the tiles, and the pages that left the footer for here.
    let mut x = col.x;
    for (label, c) in [
        ("Free memory", Click::FreeMemory),
        ("Open Control", Click::Open("control")),
        ("Bookmarks", Click::Open("bookmarks")),
        ("Privacy report", Click::Open("report")),
    ] {
        let f = i.look.small_strong.align(Align::Center);
        let r = R::new(x, y, i.gfx.measure(label, f) + 2.0 * i.look.s3, 30.0);
        let h = Hit::Dash(c);
        let hov = i.is_hover(&h);
        if hov {
            i.gfx.round(r, i.look.r_md, th.hover);
        }
        i.gfx.round_stroke(r, i.look.r_md, th.line, 1.0);
        i.gfx.text(label, r, f, if hov { th.text } else { th.dim });
        i.hit(r, h);
        x = r.right() + i.look.s2;
    }
    y += 30.0 + i.look.s5;

    // Agents: who is working right now.
    i.gfx.text("AGENTS", R::new(col.x, y, col.w, 20.0), i.look.caption, th.faint);
    y += 26.0;
    if agents == 0 {
        i.gfx.text("No agent is working. Agents connect through lumen-agent (Settings > AI agent).", R::new(col.x, y, col.w, 20.0), i.look.small, th.dim);
        y += 30.0;
    } else {
        let line = format!("{agents} agent tab(s) open{}", if s.agent_busy { " · working now" } else { "" });
        i.gfx.text(&line, R::new(col.x, y + 5.0, col.w - 160.0, 20.0), i.look.body, th.text);
        let label = "Stop all agents";
        let f = i.look.small_strong.align(Align::Center);
        let w = i.gfx.measure(label, f) + 2.0 * i.look.s3;
        let r = R::new(col.right() - w, y, w, 30.0);
        let h = Hit::Dash(Click::StopAgents);
        let hov = i.is_hover(&h);
        i.gfx.round(r, i.look.r_md, if hov { th.accent } else { th.accent.alpha(0.85) });
        i.gfx.text(label, r, f, th.bg);
        i.hit(r, h);
        y += 30.0 + i.look.s2;
        for t in s.tabs.iter().filter(|t| t.agent).take(8) {
            let name = if t.title.is_empty() { super::host(&t.url) } else { t.title.clone() };
            i.gfx.text(&format!("·  {name}"), R::new(col.x, y, col.w, 20.0), i.look.small, th.dim);
            y += 22.0;
        }
        y += i.look.s3;
    }

    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

pub fn click(i: &mut Inner, c: Click) {
    let p = &i.proxy;
    match c {
        Click::Open(which) => send(p, json!({"t": "panel", "open": true, "which": which})),
        Click::FreeMemory => send(p, json!({"t": "freeMemory"})),
        Click::StopAgents => send(p, json!({"t": "agentStop"})),
    }
}

#[cfg(test)]
mod tests {
    use super::mb;

    #[test]
    fn memory_reads_in_mb_then_gb() {
        assert_eq!(mb(300.0 * 1024.0 * 1024.0), "300 MB");
        assert_eq!(mb(1.5 * 1024.0 * 1024.0 * 1024.0), "1.5 GB");
    }
}
