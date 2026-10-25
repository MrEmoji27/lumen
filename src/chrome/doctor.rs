//! The check-up page: one row per check, its mark, what was found, and what to do.

use super::{
    Inner, UiState,
    gfx::{Align, R},
    pages::{PageHit, close_button, column, empty_state, outline_button, title_block},
    report::panel,
    theme::{Color, Style},
};
use crate::doctor::{Mark, Row};

pub fn draw(i: &mut Inner, st: R, s: &UiState) {
    let th = i.theme;
    let lo = i.look.clone();
    let col = column(st, 720.0);
    i.gfx.clip(st);
    let mut y = st.y + lo.s5 + 2.0 - i.page.scroll;
    let title_y = y;
    y = title_block(i, col, y, "Doctor", "What is wrong with this Lumen, and what to do about it");
    let Some(report) = s.doctor.as_ref().and_then(|v| v.report.as_ref()) else {
        empty_state(
            i,
            R::new(col.x, y + lo.s5, col.w, 170.0),
            "clock",
            "Checking…",
            "WebView2, the data folder, the filter lists, the helpers, Windows Hello, the default browser, the agent door, and the crashes.",
        );
        i.gfx.unclip();
        i.page.max = 0.0;
        close_button(i, st);
        return;
    };
    // Running it again sits level with the title, so it is there before and after a scroll.
    let f = lo.body.align(Align::Center);
    let w = i.gfx.measure("Check again", f) + 2.0 * lo.s4;
    outline_button(i, col.right() - w - 48.0, title_y + 4.0, "Check again", PageHit::DoctorRun, false);
    // The version and build stamp on the left, how the checks came out on the right.
    let mono = lo.mono_at(11.0, 450);
    let sum = report.summary();
    let summary_w = i.gfx.measure(&sum, lo.small) + 4.0;
    let line = R::new(col.x + 4.0, y, col.w - 8.0, 18.0);
    i.gfx.text(&report.head, R::new(line.x, y, line.w - summary_w - 12.0, 18.0), mono, th.faint);
    i.gfx.text(&sum, line, lo.small.align(Align::Right), th.dim);
    y += 22.0;
    for row in &report.rows {
        y = draw_row(i, col, y, row);
    }
    y += lo.s5;
    i.gfx.unclip();
    i.page.max = (y + i.page.scroll - st.bottom() + 30.0).max(0.0);
    close_button(i, st);
}

/// One check: a card, its mark, its name, what it found, and the fix when it needs one.
fn draw_row(i: &mut Inner, col: R, y: f32, row: &Row) -> f32 {
    let th = i.theme;
    let lo = i.look.clone();
    let tall = row.mark != Mark::Ok && !row.fix.is_empty();
    let card = R::new(col.x, y, col.w, if tall { 72.0 } else { 54.0 });
    panel(i, card);
    let tile = R::new(card.x + 18.0, card.y + 12.0, 30.0, 30.0);
    mark_tile(i, tile, row.mark);
    let tx = tile.right() + 14.0;
    let w = card.right() - 20.0 - tx;
    i.gfx.text(row.name, R::new(tx, card.y + 7.0, w, 19.0), lo.body_strong, th.text);
    i.gfx.text(&row.found, R::new(tx, card.y + 26.0, w, 17.0), lo.small, if row.mark == Mark::Fail { th.text } else { th.dim });
    if tall {
        let ink = if row.mark == Mark::Fail { th.text } else { th.dim };
        i.gfx.text(&format!("→ {}", row.fix), R::new(tx, card.y + 44.0, w, 17.0), lo.small, ink);
    }
    card.bottom() + lo.s2
}

/// The mark in its tile: ok is quiet, a warning is ink, a failure is a solid tile with the
/// mark cut out of it.
fn mark_tile(i: &mut Inner, r: R, mark: Mark) {
    let th = i.theme;
    let rad = if i.look.style == Style::Precise { i.look.round(30.0) } else { 9.0 };
    match mark {
        Mark::Ok => {
            match i.look.style {
                Style::Signal => i.gfx.round(r, rad, th.hover),
                Style::Glass => {
                    i.gfx.round(r, rad, th.accent.alpha(0.18));
                    i.gfx.round_stroke(r, rad, Color(1.0, 1.0, 1.0, 0.14), 1.0);
                }
                Style::Precise => i.gfx.round_stroke(r, rad, th.line, 1.0),
            }
            let ink = if i.look.style == Style::Glass { th.text } else { th.dim };
            i.gfx.icon("check", r.x + 7.0, r.y + 7.0, 16.0, ink);
        }
        Mark::Warn => {
            i.gfx.round(r, rad, th.hover);
            i.gfx.round_stroke(r, rad, th.line, 1.0);
            i.gfx.icon("alert", r.x + 7.0, r.y + 7.0, 16.0, th.text);
        }
        Mark::Fail => {
            i.gfx.round(r, rad, th.text);
            i.gfx.icon("alert", r.x + 7.0, r.y + 7.0, 16.0, th.bg);
        }
    }
}
