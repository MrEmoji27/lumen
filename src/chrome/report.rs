//! The privacy report: what Lumen stopped, by day and by site, like another browser's.
//!
//! The numbers come from `privacy_stats::Snapshot`, filled into `UiState.report` only while
//! this page is open. Bytes and time saved are estimates, and the page says so.

use serde_json::json;

use super::{
    Hit, Inner, UiState,
    gfx::{Align, R},
    pages::{close_button, column, empty_state, fmt_bytes, group_label, title_block},
    send,
    style::ROW_H,
    theme::Theme,
};
use crate::privacy_stats::{Counts, SiteRow, Snapshot};

/// What can be clicked on the report page.
#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    /// A range: "today" | "7" | "30" | "all".
    Range(&'static str),
    /// A row of the top sites: show that site alone.
    Site(String),
    /// Back from one site to the whole range.
    AllSites,
    /// Forget every count.
    Reset,
}

const RANGES: [(&str, &str); 4] = [("today", "Today"), ("7", "7 days"), ("30", "30 days"), ("all", "All time")];

/// Milliseconds as a person reads them: "40 ms", "12 s", "4 min", "1.2 h".
pub fn fmt_time(ms: u64) -> String {
    match ms {
        0..=999 => format!("{ms} ms"),
        1_000..=59_999 => format!("{} s", ms / 1_000),
        60_000..=3_599_999 => format!("{} min", ms / 60_000),
        _ => format!("{:.1} h", ms as f64 / 3_600_000.0),
    }
}

/// The quiet line under the Blocked tiles: the other things Lumen did, only those that happened.
pub fn extras(c: &Counts) -> String {
    let parts: Vec<String> = [
        (c.cookies, "cookie banners hidden"),
        (c.cosmetic, "page clutter hidden"),
        (c.removeparam, "tracking parameters removed"),
        (c.scriptlets, "YouTube ad blocks"),
        (c.sponsor, "sponsor segments skipped"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{n} {what}"))
    .collect();
    parts.join(" · ")
}

/// A small trend for one number against the period before it. None for "all",
/// or when there is nothing to compare.
fn trend(now: u64, prev: u64, range: &str) -> Option<String> {
    if range == "all" || (now == 0 && prev == 0) {
        return None;
    }
    if prev == 0 {
        return Some("new".into());
    }
    let pct = ((now as f64 - prev as f64) / prev as f64 * 100.0).round() as i64;
    if pct > 0 {
        Some(format!("↑{pct}%"))
    } else if pct < 0 {
        Some(format!("↓{}%", -pct))
    } else {
        Some("–".into())
    }
}

/// One tile of a section: caption, big number, and a note under it.
fn tile(i: &mut Inner, th: Theme, r: R, label: &str, value: &str, note: &str) {
    i.gfx.round(r, i.look.r_lg, th.bg);
    i.gfx.text(label, R::new(r.x + i.look.s4, r.y + i.look.s3, r.w - 2.0 * i.look.s4, 16.0), i.look.caption, th.faint);
    i.gfx.text(value, R::new(r.x + i.look.s4, r.y + 28.0, r.w - 2.0 * i.look.s4, 40.0), i.look.big_num, th.text);
    if !note.is_empty() {
        i.gfx.text_wrap(note, R::new(r.x + i.look.s4, r.y + 68.0, r.w - 2.0 * i.look.s4, 30.0), i.look.small, th.faint);
    }
}

/// A row of tiles for one section. `n` tiles, gap between them, height ~100.
fn tile_row(i: &mut Inner, th: Theme, col: R, y: f32, tiles: &[(&str, &str, &str)]) -> f32 {
    let gap = i.look.s3;
    let n = tiles.len() as f32;
    let tw = (col.w - (n - 1.0) * gap) / n;
    for (k, (label, value, note)) in tiles.iter().enumerate() {
        let r = R::new(col.x + k as f32 * (tw + gap), y, tw, 100.0);
        tile(i, th, r, label, value, note);
    }
    y + 100.0 + i.look.s4
}

pub fn draw(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let snap = s.report.clone().unwrap_or_default();
    let col = column(st, 860.0);
    i.gfx.clip(st);
    let mut y = st.y + i.look.s5 + 2.0 - i.page.scroll;
    y = title_block(i, col, y, "Privacy report", "What Lumen stopped. Data and time saved are estimates.");

    // The range, as a row of small pills.
    let mut x = col.x;
    for (key, label) in RANGES {
        let on = snap.range == key || (snap.range.is_empty() && key == "today");
        let f = i.look.small_strong.align(Align::Center);
        let r = R::new(x, y, i.gfx.measure(label, f) + 2.0 * i.look.s3, 28.0);
        let h = Hit::Report(Click::Range(key));
        let hov = i.is_hover(&h);
        if on {
            i.gfx.round(r, i.look.round(r.h), th.pill);
        } else if hov {
            i.gfx.round(r, i.look.r_md, th.hover);
        }
        i.gfx.text(label, r, f, if on { th.pill_ink } else if hov { th.text } else { th.dim });
        i.hit(r, h);
        x = r.right() + i.look.s1;
    }
    y += 28.0 + i.look.s4;

    let empty = snap.all.blocked() == 0
        && snap.all.cosmetic == 0
        && snap.all.cookies == 0
        && snap.all.popups == 0
        && snap.all.webrtc == 0
        && snap.all.signals == 0
        && snap.all.wrong_password == 0
        && snap.all.malicious == 0
        && snap.all.danger == 0
        && snap.all.https_up == 0;
    if empty {
        y = empty_state(
            i,
            R::new(col.x, y + i.look.s5, col.w, 160.0),
            "shield",
            "Nothing blocked yet",
            "Browse a little. Every ad, popup and tracker Lumen stops is counted here, with the data and time it saved you.",
        );
        i.gfx.unclip();
        i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
        close_button(i, st);
        return;
    }

    // The counts for the range, or the one site picked. Trends only for the range.
    let c = snap.site.as_ref().map(|r| r.counts).unwrap_or(snap.totals);
    let range = if snap.site.is_some() { "all" } else { &snap.range };
    let prev = snap.prev;
    let note = |now: u64, was: u64, quiet: &str| -> String {
        match trend(now, was, range) {
            Some(t) => t,
            None => quiet.to_string(),
        }
    };

    // 1. Blocked
    y = group_label(i, col, y, "Blocked", 0);
    let (ads_n, ads_t) = ((c.ads + c.scriptlets).to_string(), note(c.ads + c.scriptlets, prev.ads + prev.scriptlets, "ad rules"));
    let (pop_n, pop_t) = (c.popups.to_string(), note(c.popups, prev.popups, "blocked popups"));
    let (trk_n, trk_t) = (c.trackers.to_string(), note(c.trackers, prev.trackers, "tracker rules"));
    y = tile_row(
        i,
        th,
        col,
        y,
        &[
            ("ADS", &ads_n, &ads_t),
            ("POP-UPS", &pop_n, &pop_t),
            ("TRACKERS", &trk_n, &trk_t),
        ],
    );
    let more = extras(&c);
    if !more.is_empty() {
        i.gfx.text_wrap(&more, R::new(col.x, y - i.look.s2, col.w, 36.0), i.look.small, th.dim);
        y += 24.0;
    }

    // 2. Saved
    y = group_label(i, col, y, "Saved", 0);
    let (data_n, time_n) = (fmt_bytes(c.bytes as i64), fmt_time(c.ms));
    y = tile_row(
        i,
        th,
        col,
        y,
        &[
            ("DATA SAVED", &data_n, "estimated"),
            ("TIME SAVED", &time_n, "estimated"),
        ],
    );

    // 3. Privacy stats
    y = group_label(i, col, y, "Privacy stats", 0);
    let ip = c.webrtc + c.signals;
    let bad = c.danger + c.malicious;
    y = tile_row(
        i,
        th,
        col,
        y,
        &[
            (
                "IP LEAKS PREVENTED",
                &ip.to_string(),
                "Lumen has no VPN. WebRTC and tracking headers cannot see your local address.",
            ),
            ("WRONG PASSWORD", &c.wrong_password.to_string(), "Failed KeePass unlocks of the vault."),
            ("DANGEROUS SITES", &bad.to_string(), "Warnings shown, and subresources from the malware lists."),
            ("HTTPS UPGRADES", &c.https_up.to_string(), "Plain http pages moved to https."),
        ],
    );
    y += i.look.s2;

    if let Some(site) = &snap.site {
        y = draw_site(i, col, y, site);
    } else {
        y = draw_bars(i, col, y, &snap);
        y = draw_sites(i, col, y, &snap);
        y = draw_domains(i, col, y, &snap);
    }

    if snap.agent.blocked() > 0 {
        let line = format!("AI agent tabs, counted apart: {} blocked, {} saved.", snap.agent.blocked(), fmt_bytes(snap.agent.bytes as i64));
        i.gfx.text(&line, R::new(col.x, y, col.w, 20.0), i.look.small, th.faint);
        y += 28.0;
    }

    // Reset, quiet, at the end.
    let label = "Reset statistics";
    let f = i.look.small_strong.align(Align::Center);
    let r = R::new(col.x, y + i.look.s2, i.gfx.measure(label, f) + 2.0 * i.look.s3, 28.0);
    let h = Hit::Report(Click::Reset);
    let hov = i.is_hover(&h);
    if hov {
        i.gfx.round(r, i.look.r_sm, th.press);
    }
    i.gfx.round_stroke(r, i.look.r_sm, th.line, 1.0);
    i.gfx.text(label, r, f, if hov { th.text } else { th.dim });
    i.hit(r, h);
    y = r.bottom() + i.look.s5;

    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

/// Blocked requests per day, oldest at the left. Returns the y after it.
fn draw_bars(i: &mut Inner, col: R, y: f32, snap: &Snapshot) -> f32 {
    let th = i.theme;
    if snap.bars.len() < 2 {
        return y;
    }
    let y = group_label(i, col, y, "By day", 0);
    let bars = &snap.bars[snap.bars.len().saturating_sub(60)..];
    let h = 96.0;
    let top = bars.iter().map(|b| b.blocked).max().unwrap_or(0).max(1) as f32;
    let step = col.w / bars.len() as f32;
    let w = (step * 0.7).clamp(2.0, 18.0);
    for (n, b) in bars.iter().enumerate() {
        let bh = (b.blocked as f32 / top * h).max(if b.blocked > 0 { 2.0 } else { 0.0 });
        let x = col.x + n as f32 * step + (step - w) / 2.0;
        i.gfx.round(R::new(x, y + h - bh, w, bh.max(1.0)), 2.0, if b.blocked > 0 { th.accent.alpha(0.75) } else { th.line });
    }
    let (first, last) = (&bars[0].day, &bars[bars.len() - 1].day);
    i.gfx.text(first, R::new(col.x, y + h + i.look.s1, 120.0, 16.0), i.look.caption, th.faint);
    i.gfx.text(last, R::new(col.right() - 120.0, y + h + i.look.s1, 120.0, 16.0), i.look.caption.align(Align::Right), th.faint);
    y + h + 24.0 + i.look.s4
}

/// Sites where the most was blocked; a click shows one site alone.
fn draw_sites(i: &mut Inner, col: R, y: f32, snap: &Snapshot) -> f32 {
    let th = i.theme;
    if snap.sites.is_empty() {
        return y;
    }
    let mut y = group_label(i, col, y, "Sites", snap.sites.len().min(10));
    for row in snap.sites.iter().take(10) {
        let r = R::new(col.x - i.look.s2, y, col.w + 2.0 * i.look.s2, ROW_H);
        let h = Hit::Report(Click::Site(row.host.clone()));
        if i.is_hover(&h) {
            i.gfx.round(r, i.look.r_lg, th.hover);
        }
        i.hit(r, h);
        let cy = y + (ROW_H - 18.0) / 2.0;
        i.gfx.text(&row.host, R::new(col.x, cy, col.w * 0.5, 18.0), i.look.body, th.text);
        let bar = R::new(col.x + col.w * 0.55, y + ROW_H / 2.0 - 2.0, col.w * 0.25, 4.0);
        i.gfx.round(bar, 2.0, th.line);
        i.gfx.round(R::new(bar.x, bar.y, (bar.w * row.share).max(2.0), 4.0), 2.0, th.text.alpha(0.85));
        let n = format!("{} blocked", row.counts.blocked());
        i.gfx.text(&n, R::new(col.right() - 120.0, cy, 120.0, 18.0), i.look.small.align(Align::Right), th.dim);
        y += ROW_H;
    }
    y + i.look.s4
}

/// Tracker domains stopped most often.
fn draw_domains(i: &mut Inner, col: R, y: f32, snap: &Snapshot) -> f32 {
    let th = i.theme;
    if snap.domains.is_empty() {
        return y;
    }
    let mut y = group_label(i, col, y, "Blocked most often", snap.domains.len().min(10));
    for (domain, n) in snap.domains.iter().take(10) {
        let cy = y + (32.0 - 18.0) / 2.0;
        i.gfx.text(domain, R::new(col.x, cy, col.w - 140.0, 18.0), i.look.body, th.text);
        i.gfx.text(&n.to_string(), R::new(col.right() - 120.0, cy, 120.0, 18.0), i.look.small.align(Align::Right), th.dim);
        y += 32.0;
    }
    y + i.look.s4
}

/// One site, everything counted for it, and the way back.
fn draw_site(i: &mut Inner, col: R, y: f32, site: &SiteRow) -> f32 {
    let th = i.theme;
    let f = i.look.small_strong.align(Align::Center);
    let label = "← All sites";
    let back = R::new(col.x, y, i.gfx.measure(label, f) + 2.0 * i.look.s3, 28.0);
    let h = Hit::Report(Click::AllSites);
    if i.is_hover(&h) {
        i.gfx.round(back, i.look.r_sm, th.press);
    }
    i.gfx.text(label, back, f, th.dim);
    i.hit(back, h);
    let mut y = back.bottom() + i.look.s3;
    i.gfx.text(&site.host, R::new(col.x, y, col.w, 30.0), i.look.h2, th.text);
    y += 40.0;
    let c = &site.counts;
    for (label, v) in [
        ("Ads", c.ads),
        ("Trackers", c.trackers),
        ("Other requests", c.other),
        ("Pop-ups blocked", c.popups),
        ("Page clutter hidden", c.cosmetic),
        ("Cookie banners hidden", c.cookies),
        ("Tracking parameters removed", c.removeparam),
        ("YouTube ad blocks", c.scriptlets),
        ("Sponsor segments skipped", c.sponsor),
        ("IP leaks prevented", c.webrtc + c.signals),
        ("Dangerous sites warned", c.danger),
        ("Dangerous subresources blocked", c.malicious),
        ("HTTPS upgrades", c.https_up),
    ] {
        if v == 0 {
            continue;
        }
        i.gfx.text(label, R::new(col.x, y + 7.0, col.w - 140.0, 18.0), i.look.body, th.dim);
        i.gfx.text(&v.to_string(), R::new(col.right() - 120.0, y + 7.0, 120.0, 18.0), i.look.body_strong.align(Align::Right), th.text);
        y += 32.0;
    }
    if !site.last.is_empty() {
        i.gfx.text(&format!("Last seen {}", site.last), R::new(col.x, y + i.look.s2, col.w, 18.0), i.look.small, th.faint);
        y += 32.0;
    }
    y + i.look.s4
}

/// A click on the page, sent to the main loop.
pub fn click(i: &mut Inner, c: Click) {
    let p = &i.proxy;
    match c {
        Click::Range(r) => send(p, json!({"t": "report", "range": r})),
        Click::Site(h) => send(p, json!({"t": "report", "site": h})),
        Click::AllSites => send(p, json!({"t": "report", "site": ""})),
        Click::Reset => {
            // The drawn question; its Reset sends the wipe on the way out.
            super::confirm::open(i, super::confirm::ConfirmKind::ResetStats);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_reads_like_a_person_says_it() {
        assert_eq!(fmt_time(40), "40 ms");
        assert_eq!(fmt_time(12_400), "12 s");
        assert_eq!(fmt_time(240_000), "4 min");
        assert_eq!(fmt_time(4_320_000), "1.2 h");
    }

    #[test]
    fn extras_name_only_what_happened() {
        let c = Counts { cookies: 3, sponsor: 1, ..Default::default() };
        assert_eq!(extras(&c), "3 cookie banners hidden · 1 sponsor segments skipped");
        assert_eq!(extras(&Counts::default()), "");
    }

    #[test]
    fn trend_reads_a_change_against_the_period_before() {
        assert_eq!(trend(10, 5, "7").as_deref(), Some("↑100%"));
        assert_eq!(trend(5, 10, "7").as_deref(), Some("↓50%"));
        assert_eq!(trend(7, 7, "7").as_deref(), Some("–"));
        assert_eq!(trend(3, 0, "7").as_deref(), Some("new"));
        assert_eq!(trend(0, 0, "7"), None);
        assert_eq!(trend(9, 3, "all"), None, "all time has no previous period");
    }
}
