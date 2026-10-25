//! The privacy report: what Lumen stopped, by day and by site.

use serde_json::json;

use super::{
    Hit, Inner, UiState,
    motion::{Ease, Id, Prop},
    gfx::{Align, R},
    group_digits,
    pages::{close_button, column, empty_state, group_label, site_icon, title_block},
    send,
    style::ROW_H,
    theme::{Color, Style, Theme},
};
use crate::privacy_stats::{Counts, SiteRow, Snapshot};

#[derive(Clone, Debug, PartialEq)]
pub enum Click {
    Range(&'static str),
    Site(String),
    AllSites,
    Reset,
}

const RANGES: [(&str, &str); 4] = [("today", "Today"), ("7", "7 days"), ("30", "30 days"), ("all", "All time")];

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

fn stopped(c: &Counts) -> u64 {
    c.ads + c.scriptlets + c.trackers + c.other + c.popups
}

fn parts(c: &Counts) -> Vec<(&'static str, u64)> {
    [("Ads", c.ads + c.scriptlets), ("Trackers", c.trackers), ("Pop-ups", c.popups), ("Other", c.other)]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .collect()
}

fn shades(th: &Theme, style: Style) -> [Color; 4] {
    match style {
        Style::Signal => [th.text, th.text.alpha(0.55), th.text.alpha(0.32), th.text.alpha(0.16)],
        Style::Glass => [th.accent, Color(1.0, 1.0, 1.0, 0.72), Color(1.0, 1.0, 1.0, 0.42), Color(1.0, 1.0, 1.0, 0.2)],
        Style::Precise => [th.accent, th.accent.alpha(0.5), th.dim, th.faint],
    }
}

pub(super) fn panel(i: &mut Inner, r: R) {
    let th = i.theme;
    match i.look.style {
        Style::Signal => i.gfx.round(r, i.look.r_lg, th.raise),
        Style::Glass => {
            i.gfx.round(r, 16.0, Color(1.0, 1.0, 1.0, 0.07));
            i.gfx.round_stroke(r, 16.0, Color(1.0, 1.0, 1.0, 0.12), 1.0);
        }
        Style::Precise => {
            i.gfx.round(r, 10.0, th.raise);
            i.gfx.round_stroke(r, 10.0, th.line, 1.0);
        }
    }
}

fn numeral(i: &Inner, size: f32) -> super::gfx::Font {
    match i.look.style {
        Style::Signal => i.look.mono_at(size, 700),
        Style::Glass => i.look.font(size, 300),
        Style::Precise => i.look.mono_at(size * 0.8, 600),
    }
}

fn blocked_panel(i: &mut Inner, col: R, y: f32, c: &Counts, prev: &Counts, range: &str, one_site: bool) -> f32 {
    let th = i.theme;
    let style = i.look.style;
    let y = group_label(i, col, y, "Blocked", 0);
    let kinds = parts(c);
    let total = stopped(c);
    let r = R::new(col.x, y, col.w, if kinds.is_empty() { 96.0 } else { 196.0 });
    panel(i, r);
    let pad = 24.0;
    if style == Style::Precise {
        i.gfx.fill(R::new(r.x + 1.0, r.y + 14.0, 2.0, r.h - 28.0), th.accent);
    }
    let inner = R::new(r.x + pad, r.y + 18.0, r.w - 2.0 * pad, r.h - 36.0);
    let big = numeral(i, 44.0);
    let num = group_digits(total);
    let w = i.gfx.measure(&num, big);
    if style == Style::Glass && total > 0 {
        i.gfx.bloom(inner.x + w / 2.0, inner.y + 26.0, (w * 0.9).max(70.0), th.accent.alpha(0.28));
    }
    i.gfx.text(&num, R::new(inner.x - 1.0, inner.y, w + 4.0, 52.0), big, th.text);
    let what = if one_site { "total on this site" } else { "total" };
    i.gfx.text(what, R::new(inner.x + w + 10.0, inner.y + 22.0, 160.0, 20.0), i.look.body, th.dim);
    if !one_site && let Some(t) = trend(total, stopped(prev), range) {
        let line = format!("{t} on the period before");
        i.gfx.text(&line, R::new(inner.right() - 260.0, inner.y + 22.0, 260.0, 20.0), i.look.small.align(Align::Right), th.faint);
    }
    if kinds.is_empty() || total == 0 {
        return r.bottom() + i.look.s4;
    }
    let shade = shades(&th, style);
    let tone = |name: &str| match name {
        "Ads" => shade[0],
        "Trackers" => shade[1],
        "Pop-ups" => shade[2],
        _ => shade[3],
    };
    let of = |n: u64| n as f32 / total as f32;
    let (bh, rad, gap) = match style {
        Style::Signal => (14.0, 7.0, 3.0),
        Style::Glass => (12.0, 6.0, 3.0),
        Style::Precise => (8.0, 0.0, 1.0),
    };
    let bar = R::new(inner.x, inner.y + 66.0, inner.w, bh);
    let grow = i.motion.enter(Id::Named("report-blocked"), Prop::Progress, 0.0, 1.0, 520, Ease::OutQuint);
    let usable = (bar.w - gap * (kinds.len() as f32 - 1.0)) * grow;
    let mut x = bar.x;
    for (k, (name, n)) in kinds.iter().enumerate() {
        let w = (usable * of(*n)).max(4.0).min(bar.right() - x);
        if style == Style::Glass && k == 0 {
            i.gfx.bloom(x + w / 2.0, bar.y + bh / 2.0, (w * 0.5).max(40.0), th.accent.alpha(0.22));
        }
        i.gfx.round(R::new(x, bar.y, w, bh), rad, tone(name));
        x += w + gap;
    }
    let cw = (inner.w / kinds.len().max(3) as f32).min(200.0);
    for (k, (name, n)) in kinds.iter().enumerate() {
        let lx = inner.x + k as f32 * cw;
        let ly = bar.bottom() + 20.0;
        let mark = R::new(lx, ly + 5.0, 9.0, 9.0);
        if style == Style::Precise {
            i.gfx.fill(mark, tone(name));
        } else {
            i.gfx.round(mark, 4.5, tone(name));
        }
        i.gfx.text(name, R::new(lx + 16.0, ly, cw - 16.0, 18.0), i.look.small, th.dim);
        let nf = numeral(i, 20.0);
        i.gfx.text(&group_digits(*n), R::new(lx + 16.0, ly + 22.0, cw - 16.0, 28.0), nf, th.text);
        if style == Style::Precise {
            let pct = format!("{}%", (of(*n) * 100.0).round() as u32);
            let nw = i.gfx.measure(&group_digits(*n), nf);
            i.gfx.text(&pct, R::new(lx + 16.0 + nw + 8.0, ly + 28.0, 60.0, 16.0), i.look.mono_at(11.0, 500), th.faint);
        }
    }
    r.bottom() + i.look.s4
}

fn rows_panel(i: &mut Inner, col: R, y: f32, title: &str, rows: &[(&str, &str, String, &str)]) -> f32 {
    let th = i.theme;
    if rows.is_empty() {
        return y;
    }
    let y = group_label(i, col, y, title, 0);
    let row_h = 62.0;
    let r = R::new(col.x, y, col.w, row_h * rows.len() as f32);
    panel(i, r);
    for (k, (icon, label, value, note)) in rows.iter().enumerate() {
        let ry = y + k as f32 * row_h;
        if k > 0 {
            i.gfx.line(r.x + 62.0, ry, r.right() - 18.0, ry, th.line, 1.0);
        }
        let ic = R::new(r.x + 18.0, ry + 14.0, 30.0, 30.0);
        match i.look.style {
            Style::Signal => i.gfx.round(ic, 9.0, th.hover),
            Style::Glass => {
                i.gfx.round(ic, 9.0, th.accent.alpha(0.18));
                i.gfx.round_stroke(ic, 9.0, Color(1.0, 1.0, 1.0, 0.14), 1.0);
            }
            Style::Precise => i.gfx.round_stroke(ic, 6.0, th.line, 1.0),
        }
        let ink = if i.look.style == Style::Glass { th.text } else { th.dim };
        i.gfx.icon(icon, ic.x + 7.0, ic.y + 7.0, 16.0, ink);
        let tx = ic.right() + 14.0;
        i.gfx.text(label, R::new(tx, ry + 12.0, r.w * 0.55, 20.0), i.look.body_strong, th.text);
        i.gfx.text(note, R::new(tx, ry + 33.0, r.w - (tx - r.x) - 150.0, 16.0), i.look.small, th.faint);
        i.gfx.text(value, R::new(r.right() - 168.0, ry, 150.0, row_h), numeral(i, 20.0).align(Align::Right), th.text);
    }
    r.bottom() + i.look.s4
}

fn other_rows(c: &Counts) -> Vec<(&'static str, &'static str, String, &'static str)> {
    [
        ("redirect", "Redirects blocked", c.redirects, "Pages that tried to send you to another site on their own."),
        ("mask", "Real IP hidden", c.webrtc + c.signals, "WebRTC and tracking headers cannot see your address. Lumen has no VPN."),
        ("https", "Moved to HTTPS", c.https_up, "Plain http pages opened over https instead."),
        ("cookie", "Cookie banners hidden", c.cookies, "Consent pop-ups that never showed."),
        ("eraser", "Page clutter hidden", c.cosmetic, "Empty ad boxes and leftovers, by the filter lists."),
        ("unlink", "Tracking parameters removed", c.removeparam, "utm_ and friends, cut from links."),
        ("next", "Sponsor segments skipped", c.sponsor, "On YouTube, by SponsorBlock."),
        ("danger", "Dangerous sites", c.danger + c.malicious, "Warnings shown, and parts of pages from the malware lists."),
        ("key", "Wrong vault password", c.wrong_password, "Failed KeePass unlocks."),
    ]
    .into_iter()
    .filter(|r| r.2 > 0)
    .map(|(icon, label, n, note)| (icon, label, group_digits(n), note))
    .collect()
}

pub fn draw(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let snap = s.report.clone().unwrap_or_default();
    let col = column(st, 860.0);
    i.gfx.clip(st);
    let mut y = st.y + i.look.s5 + 2.0 - i.page.scroll;
    y = title_block(i, col, y, "Privacy report", "What Lumen stopped, counted as it happened.");

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
        && snap.all.redirects == 0
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
            "Browse a little. Every ad, pop-up and tracker Lumen stops is counted here.",
        );
        i.gfx.unclip();
        i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
        close_button(i, st);
        return;
    }

    let c = snap.site.as_ref().map(|r| r.counts).unwrap_or(snap.totals);
    let range = if snap.site.is_some() { "all" } else { &snap.range };
    let prev = snap.prev;
    if let Some(site) = &snap.site {
        y = draw_site_head(i, col, y, site);
    }
    y = blocked_panel(i, col, y, &c, &prev, range, snap.site.is_some());
    if stopped(&c) > 0 {
        let line = "A blocked request is one your PC never downloaded and the page never waited for.";
        i.gfx.text(line, R::new(col.x, y - i.look.s2, col.w, 18.0), i.look.small, th.dim);
        y += 18.0 + i.look.s3;
    }
    y = rows_panel(i, col, y, "Other statistics", &other_rows(&c));

    if let Some(site) = &snap.site {
        y = draw_site_rest(i, col, y, site);
    } else {
        y = draw_bars(i, col, y, &snap);
        y = draw_sites(i, col, y, &snap);
        y = draw_domains(i, col, y, &snap);
    }

    if snap.agent.blocked() > 0 {
        let line = format!("AI agent tabs, counted apart: {} blocked.", group_digits(snap.agent.blocked()));
        i.gfx.text(&line, R::new(col.x, y, col.w, 20.0), i.look.small, th.faint);
        y += 28.0;
    }

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

fn draw_bars(i: &mut Inner, col: R, y: f32, snap: &Snapshot) -> f32 {
    let th = i.theme;
    let style = i.look.style;
    if snap.bars.len() < 2 {
        return y;
    }
    let y = group_label(i, col, y, "By day", 0);
    let bars = &snap.bars[snap.bars.len().saturating_sub(60)..];
    let frame = R::new(col.x, y, col.w, 168.0);
    panel(i, frame);
    let inner = R::new(frame.x + 20.0, frame.y + 18.0, frame.w - 40.0, 112.0);
    let top = bars.iter().map(|b| b.blocked).max().unwrap_or(0).max(1) as f32;
    let rise = i.motion.enter(Id::Named("report-days"), Prop::Progress, 0.0, 1.0, 620, Ease::OutQuint);
    if style == Style::Precise {
        for k in 0..3 {
            let gy = inner.y + inner.h * k as f32 / 3.0;
            i.gfx.line(inner.x, gy, inner.right(), gy, th.line.alpha(0.6), 1.0);
        }
        i.gfx.line(inner.x, inner.bottom(), inner.right(), inner.bottom(), th.line, 1.0);
        let peak = group_digits(top as u64);
        i.gfx.text(&peak, R::new(inner.right() - 80.0, inner.y - 16.0, 80.0, 14.0), i.look.mono_at(10.0, 500).align(Align::Right), th.faint);
    }
    let step = inner.w / bars.len() as f32;
    let last = bars.len() - 1;
    for (n, b) in bars.iter().enumerate() {
        let today = n == last;
        let bh = (b.blocked as f32 / top * inner.h).max(if b.blocked > 0 { 3.0 } else { 0.0 }) * rise;
        let cx = inner.x + n as f32 * step + step / 2.0;
        match style {
            Style::Signal => {
                let w = (step * 0.62).clamp(3.0, 26.0);
                let c = if today { th.accent } else if b.blocked > 0 { th.text.alpha(0.82) } else { th.line };
                i.gfx.round(R::new(cx - w / 2.0, inner.bottom() - bh.max(2.0), w, bh.max(2.0)), (w / 2.0).min(5.0), c);
            }
            Style::Glass => {
                let w = (step * 0.58).clamp(3.0, 24.0);
                if today && b.blocked > 0 {
                    i.gfx.bloom(cx, inner.bottom() - bh / 2.0, (bh * 0.7).max(30.0), th.accent.alpha(0.35));
                }
                let c = if today { th.accent } else { Color(1.0, 1.0, 1.0, if b.blocked > 0 { 0.38 } else { 0.1 }) };
                i.gfx.round(R::new(cx - w / 2.0, inner.bottom() - bh.max(2.0), w, bh.max(2.0)), w / 2.0, c);
            }
            Style::Precise => {
                let w = if step > 14.0 { 2.0 } else { 1.0 };
                let c = if today { th.accent } else { th.dim };
                i.gfx.fill(R::new(cx - w / 2.0, inner.bottom() - bh, w, bh), c);
                if b.blocked > 0 {
                    i.gfx.fill(R::new(cx - 3.0, inner.bottom() - bh - 1.0, 6.0, 2.0), c);
                }
            }
        }
    }
    let (first, last_day) = (&bars[0].day, &bars[last].day);
    let foot = inner.bottom() + 10.0;
    let f = if style == Style::Glass { i.look.caption } else { i.look.mono_at(10.5, 500) };
    i.gfx.text(first, R::new(inner.x, foot, 120.0, 16.0), f, th.faint);
    let today = format!("{} today · {last_day}", group_digits(bars[last].blocked));
    i.gfx.text(&today, R::new(inner.right() - 240.0, foot, 240.0, 16.0), f.align(Align::Right), th.dim);
    frame.bottom() + i.look.s4
}

fn draw_sites(i: &mut Inner, col: R, y: f32, snap: &Snapshot) -> f32 {
    let th = i.theme;
    if snap.sites.is_empty() {
        return y;
    }
    let mut y = group_label(i, col, y, "Sites", snap.sites.len().min(10));
    let shade = shades(&th, i.look.style)[0];
    for row in snap.sites.iter().take(10) {
        let r = R::new(col.x - i.look.s2, y, col.w + 2.0 * i.look.s2, ROW_H);
        let h = Hit::Report(Click::Site(row.host.clone()));
        if i.is_hover(&h) {
            i.gfx.round(r, i.look.r_lg, th.hover);
        }
        i.hit(r, h);
        site_icon(i, R::new(col.x, y + (ROW_H - 20.0) / 2.0, 20.0, 20.0), &row.host);
        let cy = y + (ROW_H - 18.0) / 2.0;
        i.gfx.text(&row.host, R::new(col.x + 32.0, cy, col.w * 0.5 - 32.0, 18.0), i.look.body, th.text);
        let bar = R::new(col.x + col.w * 0.55, y + ROW_H / 2.0 - 2.0, col.w * 0.25, 4.0);
        let rad = if i.look.style == Style::Precise { 0.0 } else { 2.0 };
        i.gfx.round(bar, rad, th.line);
        i.gfx.round(R::new(bar.x, bar.y, (bar.w * row.share).max(2.0), 4.0), rad, shade);
        let n = format!("{} blocked", group_digits(row.counts.blocked()));
        i.gfx.text(&n, R::new(col.right() - 120.0, cy, 120.0, 18.0), i.look.small.align(Align::Right), th.dim);
        y += ROW_H;
    }
    y + i.look.s4
}

fn draw_domains(i: &mut Inner, col: R, y: f32, snap: &Snapshot) -> f32 {
    let th = i.theme;
    if snap.domains.is_empty() {
        return y;
    }
    let mut y = group_label(i, col, y, "Blocked most often", snap.domains.len().min(10));
    let f = if i.look.style == Style::Glass { i.look.body } else { i.look.mono_at(12.5, 400) };
    for (domain, n) in snap.domains.iter().take(10) {
        let cy = y + (32.0 - 18.0) / 2.0;
        i.gfx.text(domain, R::new(col.x, cy, col.w - 140.0, 18.0), f, th.text);
        i.gfx.text(&group_digits(*n), R::new(col.right() - 120.0, cy, 120.0, 18.0), i.look.small.align(Align::Right), th.dim);
        y += 32.0;
    }
    y + i.look.s4
}

fn draw_site_head(i: &mut Inner, col: R, y: f32, site: &SiteRow) -> f32 {
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
    let y = back.bottom() + i.look.s3;
    site_icon(i, R::new(col.x, y + 3.0, 26.0, 26.0), &site.host);
    i.gfx.text(&site.host, R::new(col.x + 38.0, y, col.w - 38.0, 32.0), i.look.h2, th.text);
    y + 44.0
}

fn draw_site_rest(i: &mut Inner, col: R, y: f32, site: &SiteRow) -> f32 {
    if site.last.is_empty() {
        return y;
    }
    let th = i.theme;
    i.gfx.text(&format!("Last seen {}", site.last), R::new(col.x, y, col.w, 18.0), i.look.small, th.faint);
    y + 32.0 + i.look.s4
}

pub fn click(i: &mut Inner, c: Click) {
    let p = &i.proxy;
    match c {
        Click::Range(r) => send(p, json!({"t": "report", "range": r})),
        Click::Site(h) => send(p, json!({"t": "report", "site": h})),
        Click::AllSites => send(p, json!({"t": "report", "site": ""})),
        Click::Reset => {
            super::confirm::open(i, super::confirm::ConfirmKind::ResetStats);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trend_reads_a_change_against_the_period_before() {
        assert_eq!(trend(10, 5, "7").as_deref(), Some("↑100%"));
        assert_eq!(trend(5, 10, "7").as_deref(), Some("↓50%"));
        assert_eq!(trend(7, 7, "7").as_deref(), Some("–"));
        assert_eq!(trend(3, 0, "7").as_deref(), Some("new"));
        assert_eq!(trend(0, 0, "7"), None);
        assert_eq!(trend(9, 3, "all"), None, "all time has no previous period");
    }

    #[test]
    fn the_hero_counts_what_was_stopped_and_what_it_was() {
        let c = Counts { ads: 10, scriptlets: 2, trackers: 5, popups: 0, other: 1, cookies: 9, ..Default::default() };
        assert_eq!(stopped(&c), 18);
        assert_eq!(parts(&c), vec![("Ads", 12), ("Trackers", 5), ("Other", 1)]);
        assert!(parts(&Counts::default()).is_empty());
    }
}
