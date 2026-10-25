//! The drawn menu.

use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP};

use super::{
    Hit, Inner, UiState,
    gfx::{Align, R},
};

type Chosen = Box<dyn FnOnce(&mut Inner, u32)>;

struct Row {
    id: u32,
    label: String,
    hint: String,
    enabled: bool,
}

pub struct MenuCard {
    rows: Vec<Row>,
    act: Option<Chosen>,
    on_cancel: Option<Box<dyn FnOnce(&mut Inner)>>,
    rect: R,
    row_rects: Vec<R>,
    sel: usize,
}

const ROW_H: f32 = 30.0;
const PAD: f32 = 6.0;
const SEP_GAP: f32 = 5.0;
const MIN_W: f32 = 170.0;

fn rows_of(items: &[(u32, String, bool)]) -> Vec<Row> {
    items
        .iter()
        .map(|(id, label, enabled)| {
            let (label, hint) = match label.split_once('\t') {
                Some((l, h)) => (l.to_string(), h.to_string()),
                None => (label.clone(), String::new()),
            };
            Row { id: *id, label, hint, enabled: *enabled }
        })
        .collect()
}

fn geometry(i: &Inner, rows: &[Row], x: f32, y: f32) -> (R, Vec<R>) {
    let (w_win, h_win) = i.size_dip();
    let lo = &i.look;
    let lf = lo.font(12.5, 500);
    let hf = lo.font(11.5, 450);
    let mut w: f32 = MIN_W - 2.0 * PAD;
    let mut h = 2.0 * PAD;
    for r in rows {
        if r.id == 0 {
            h += SEP_GAP * 2.0 + 1.0;
            continue;
        }
        let hint = if r.hint.is_empty() { 0.0 } else { i.gfx.measure(&r.hint, hf) + 24.0 };
        w = w.max(i.gfx.measure(&r.label, lf) + hint + 24.0);
        h += ROW_H;
    }
    let w = w.min(420.0);
    let x = x.min(w_win - w - PAD - 2.0).max(4.0);
    let y = y.min(h_win - h - PAD - 2.0).max(4.0);
    let rect = R::new(x, y, w, h);
    let mut rects = Vec::new();
    let mut ry = rect.y + PAD;
    for r in rows {
        if r.id == 0 {
            rects.push(R::new(rect.x + 4.0, ry, rect.w - 8.0, SEP_GAP * 2.0 + 1.0));
            ry += SEP_GAP * 2.0 + 1.0;
        } else {
            rects.push(R::new(rect.x + 4.0, ry, rect.w - 8.0, ROW_H));
            ry += ROW_H;
        }
    }
    (rect, rects)
}

pub fn open(i: &mut Inner, items: &[(u32, String)], x: f32, y: f32, act: impl FnOnce(&mut Inner, u32) + 'static) {
    let rows: Vec<(u32, String, bool)> = items.iter().map(|(id, l)| (*id, l.clone(), true)).collect();
    open_rows(i, &rows, x, y, act);
}

pub fn open_rows(i: &mut Inner, items: &[(u32, String, bool)], x: f32, y: f32, act: impl FnOnce(&mut Inner, u32) + 'static) {
    open_with(i, items, x, y, Box::new(act), None);
}

pub fn open_rows_or(i: &mut Inner, items: &[(u32, String, bool)], x: f32, y: f32, act: impl FnOnce(&mut Inner, u32) + 'static, on_cancel: impl FnOnce(&mut Inner) + 'static) {
    open_with(i, items, x, y, Box::new(act), Some(Box::new(on_cancel)));
}

fn open_with(i: &mut Inner, items: &[(u32, String, bool)], x: f32, y: f32, act: Chosen, on_cancel: Option<Box<dyn FnOnce(&mut Inner)>>) {
    i.close_cards();
    let rows = rows_of(items);
    let (rect, row_rects) = geometry(i, &rows, x, y);
    let sel = rows.iter().position(|r| r.enabled && r.id != 0).unwrap_or(0);
    i.menu = Some(MenuCard { rows, act: Some(act), on_cancel, rect, row_rects, sel });
    unsafe {
        let _ = SetFocus(Some(i.hwnd));
        SetCapture(i.hwnd);
    }
    i.invalidate();
}

pub fn cancel(i: &mut Inner) {
    let Some(card) = i.menu.take() else { return };
    unsafe {
        let _ = ReleaseCapture();
    }
    if let Some(f) = card.on_cancel {
        f(i);
    }
    i.invalidate();
}

pub fn blur(i: &mut Inner) {
    cancel(i);
}

pub fn click(i: &mut Inner, n: usize) {
    pick_row(i, n);
}

pub fn key(i: &mut Inner, vk: u32) -> bool {
    let Some(card) = i.menu.as_mut() else { return false };
    match vk {
        v if v == VK_ESCAPE.0 as u32 => {
            cancel(i);
            true
        }
        v if v == VK_RETURN.0 as u32 => {
            let sel = card.sel;
            pick_row(i, sel);
            true
        }
        v if v == VK_DOWN.0 as u32 || v == VK_UP.0 as u32 => {
            let n = card.rows.len();
            let step: isize = if v == VK_DOWN.0 as u32 { 1 } else { -1 };
            for k in 1..=n {
                let at = (card.sel as isize + step * k as isize).rem_euclid(n as isize) as usize;
                if card.rows[at].id != 0 && card.rows[at].enabled {
                    card.sel = at;
                    break;
                }
            }
            i.invalidate();
            true
        }
        _ => false,
    }
}

pub fn clicked_outside(i: &mut Inner, h: &Option<Hit>) {
    if i.menu.is_some() && !matches!(h, Some(Hit::MenuCard | Hit::MenuRow(_))) {
        cancel(i);
    }
}

fn pick_row(i: &mut Inner, n: usize) {
    let Some(mut card) = i.menu.take() else { return };
    let row = match card.rows.get(n) {
        Some(r) if r.enabled && r.id != 0 => r.id,
        Some(_) => {
            i.menu = Some(card);
            return;
        }
        None => return,
    };
    unsafe {
        let _ = ReleaseCapture();
    }
    i.invalidate();
    if let Some(act) = card.act.take() {
        act(i, row);
    }
}

pub fn draw(i: &mut Inner, _s: &UiState) {
    let Some(card) = i.menu.take() else { return };
    let lo = i.look.clone();
    let th = lo.theme;
    i.float_card(card.rect, 1.0);
    i.hit(card.rect, Hit::MenuCard);
    for (k, row) in card.rows.iter().enumerate() {
        let Some(r) = card.row_rects.get(k) else { continue };
        if row.id == 0 {
            let y = r.y + r.h / 2.0;
            i.gfx.line(card.rect.x + 8.0, y, card.rect.right() - 8.0, y, th.line, 1.0);
            continue;
        }
        let hit = Hit::MenuRow(k);
        let lit = row.enabled && (i.is_hover(&hit) || card.sel == k);
        if lit {
            i.gfx.round(*r, lo.r_sm, th.hover);
        }
        let ink = if !row.enabled { th.faint } else if lit { th.text } else { th.dim };
        i.gfx.text(&row.label, R::new(r.x + 8.0, r.y, r.w - 24.0, r.h), lo.font(12.5, 500).align(Align::Left), ink);
        if !row.hint.is_empty() {
            i.gfx.text(&row.hint, R::new(r.x, r.y, r.w - 8.0, r.h), lo.font(11.5, 450).align(Align::Right), th.faint);
        }
        i.hit(*r, hit);
    }
    i.menu = Some(card);
}
