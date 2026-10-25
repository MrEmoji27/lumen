//! The drawn form card: a title, labelled text boxes, Save and Cancel.

use std::time::Instant;

use serde_json::json;
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Input::KeyboardAndMouse::{GetFocus, SetFocus, VK_ESCAPE, VK_RETURN},
        WindowsAndMessaging::SetWindowTextW,
    },
};
use windows::core::HSTRING;

use super::{Hit, Inner, UiState, gfx::R, send, style};
use crate::speeddial;

#[derive(Clone, Debug, PartialEq)]
pub enum FormAct {
    DialAdd,
    RenameFolder { old: String },
    EditTile { i: usize },
    EditBookmark { id: u64, folder: String },
    TextSetting { key: String },
    SaveLook,
    NewFolder,
    VaultOpen { path: String },
    PagePrompt { aid: u64 },
    SignIn { aid: u64 },
    PopupSite,
    RedirectSite,
}

impl FormAct {
    fn for_page(&self) -> bool {
        matches!(self, FormAct::PagePrompt { .. } | FormAct::SignIn { .. })
    }

    fn go(&self) -> &'static str {
        match self {
            FormAct::PagePrompt { .. } => "OK",
            FormAct::SignIn { .. } => "Sign in",
            _ => "Save",
        }
    }
}

pub struct FormCard {
    title: String,
    pub(super) fields: Vec<(String, String, bool)>,
    act: FormAct,
    pub rect: R,
    pub boxes: Vec<R>,
}

const PAD: f32 = 14.0;
const BOX_H: f32 = 26.0;
const LABEL_H: f32 = 12.0;
const BTN_W: f32 = 76.0;
const BTN_H: f32 = 28.0;

fn geometry(i: &Inner, fields: usize, page: bool) -> Option<(R, Vec<R>)> {
    let area = i.float_area()?;
    let (x, w) = super::float_col(area, i.cards_right());
    let h = PAD + 14.0 + 10.0 + fields as f32 * (LABEL_H + 4.0 + BOX_H + 12.0) + BTN_H + 14.0 + PAD;
    let rect = match page.then(|| i.state.as_ref().map(|s| s.stage)).flatten() {
        Some((sx, sy, sw, _)) => {
            let w = w.max(360.0);
            R::new(sx + ((sw - w) / 2.0).max(8.0), sy + 16.0, w, h)
        }
        None => R::new(x, area.y + 8.0, w, h),
    };
    let w = rect.w;
    let mut boxes = Vec::new();
    let mut y = rect.y + PAD + 14.0 + 10.0;
    for _ in 0..fields {
        boxes.push(R::new(rect.x + PAD, y + LABEL_H + 4.0, w - 2.0 * PAD, BOX_H));
        y += LABEL_H + 4.0 + BOX_H + 12.0;
    }
    Some((rect, boxes))
}

pub fn open(i: &mut Inner, title: &str, fields: &[(&str, &str)], act: FormAct) {
    let fields: Vec<(&str, &str, bool)> = fields.iter().map(|(l, v)| (*l, *v, false)).collect();
    open_fields(i, title, &fields, act);
}

pub fn open_fields(i: &mut Inner, title: &str, fields: &[(&str, &str, bool)], act: FormAct) {
    i.close_cards();
    let fields: Vec<(String, String, bool)> = fields.iter().map(|(l, v, s)| (l.to_string(), v.to_string(), *s)).collect();
    let (rect, boxes) = match geometry(i, fields.len(), act.for_page()) {
        Some(g) => g,
        None => return,
    };
    i.form = Some(FormCard { title: title.to_string(), fields, act, rect, boxes });
    for (n, e) in i.form_edits.iter().enumerate() {
        if let Some((_, v, s)) = i.form.as_ref().and_then(|f| f.fields.get(n)) {
            let target = if *s { i.form_secret_edit } else { *e };
            unsafe {
                let _ = SetWindowTextW(target, &HSTRING::from(v.as_str()));
            }
        }
    }
    let first = match i.form.as_ref().and_then(|f| f.fields.first()) {
        Some((_, _, true)) => i.form_secret_edit,
        _ => i.form_edits[0],
    };
    i.layout_edits();
    unsafe {
        let _ = SetFocus(Some(first));
    }
    i.invalidate();
}

fn box_of(i: &Inner, n: usize) -> HWND {
    match i.form.as_ref().and_then(|f| f.fields.get(n)) {
        Some((_, _, true)) => i.form_secret_edit,
        _ => i.form_edits[n.min(i.form_edits.len() - 1)],
    }
}

pub fn cancel(i: &mut Inner) {
    let Some(card) = i.form.take() else { return };
    leave(i);
    match card.act {
        FormAct::PagePrompt { aid } => send(&i.proxy, json!({"t": "pageAnswer", "aid": aid, "ok": false})),
        FormAct::SignIn { aid } => {
            let _ = i.proxy.send_event(crate::engine::Ev::SignInAnswer(aid, None));
        }
        _ => {}
    }
}

pub fn put_away(i: &mut Inner) {
    if i.form.as_ref().is_some_and(|f| f.act.for_page()) {
        i.form = None;
        leave(i);
    }
}

pub fn save(i: &mut Inner) {
    let n = i.form.as_ref().map(|f| f.fields.len()).unwrap_or(0);
    let values: Vec<String> = (0..n).map(|k| super::wide_text(box_of(i, k))).collect();
    let Some(card) = i.form.take() else { return };
    leave(i);
    submit(i, &card.act, &values);
}

pub fn key(i: &mut Inner, vk: u32) -> bool {
    if i.form.is_none() {
        return false;
    }
    match vk {
        v if v == VK_ESCAPE.0 as u32 => {
            cancel(i);
            true
        }
        v if v == VK_RETURN.0 as u32 => {
            save(i);
            true
        }
        _ => false,
    }
}

pub fn tab(i: &mut Inner, back: bool) {
    let n = i.form.as_ref().map(|f| f.fields.len()).unwrap_or(0);
    if n == 0 {
        return;
    }
    let focus = unsafe { GetFocus() };
    let at = (0..n).position(|k| box_of(i, k) == focus).unwrap_or(0);
    let next = if back { (at + n - 1) % n } else { (at + 1) % n };
    let target = box_of(i, next);
    unsafe {
        let _ = SetFocus(Some(target));
    }
}

pub fn clicked_outside(i: &mut Inner, h: &Option<Hit>) {
    if i.form.is_some() && !matches!(h, Some(Hit::FormCard | Hit::FormSave | Hit::FormCancel)) {
        cancel(i);
    }
}

fn leave(i: &mut Inner) {
    unsafe {
        let focus = GetFocus();
        if i.form_edits.contains(&focus) || focus == i.form_secret_edit {
            let _ = SetFocus(Some(i.hwnd));
        }
    }
    i.layout_edits();
    i.invalidate();
}

fn submit(i: &mut Inner, act: &FormAct, a: &[String]) {
    let Some(s) = i.state.clone() else { return };
    match act {
        FormAct::DialAdd => {
            if a.len() < 2 {
                return;
            }
            let mut st = s.settings.clone();
            if speeddial::add(&mut st.shortcuts, &a[1], &a[0]) {
                send(&i.proxy, json!({"t": "settings", "v": st}));
            } else {
                i.page.flash = Some(("Not added: that is not an address, it is already a tile, or the page is full".into(), Instant::now()));
            }
        }
        FormAct::RenameFolder { old } => {
            if let Some(new) = a.first() {
                send(&i.proxy, json!({"t": "bmFolderRename", "old": old, "new": new}));
            }
        }
        FormAct::EditTile { i: n } => {
            if a.len() < 2 {
                return;
            }
            let mut st = s.settings.clone();
            if speeddial::edit(&mut st.shortcuts, *n, &a[1], &a[0]) {
                send(&i.proxy, json!({"t": "settings", "v": st}));
            }
        }
        FormAct::EditBookmark { id, folder } => {
            if a.len() < 2 {
                return;
            }
            send(&i.proxy, json!({"t": "bmEdit", "id": id, "title": a[0], "url": a[1], "folder": folder}));
        }
        FormAct::TextSetting { key } => {
            if let Some(v) = a.first() {
                super::pages::set(i, key, json!(v));
            }
        }
        FormAct::SaveLook => {
            let Some(name) = a.first().map(|v| v.trim().to_string()).filter(|n| !n.is_empty()) else { return };
            let mut list = style::saved_themes();
            let mut file = style::ThemeFile::of(&s.settings);
            file.name = name.clone();
            if let Some(at) = list.iter().position(|t| t.name == name) {
                list[at] = file;
            } else {
                list.push(file);
            }
            style::save_themes(&list);
            i.page.themes = Some(list);
            i.page.flash = Some((format!("Saved \"{name}\""), Instant::now()));
        }
        FormAct::NewFolder => {
            if let Some(name) = a.first() {
                send(&i.proxy, json!({"t": "bmFolderAdd", "name": name}));
            }
        }
        FormAct::VaultOpen { path } => {
            if let Some(pw) = a.first() {
                let _ = i.proxy.send_event(crate::engine::Ev::VaultOpen(path.clone(), pw.clone()));
            }
        }
        FormAct::PagePrompt { aid } => {
            send(&i.proxy, json!({"t": "pageAnswer", "aid": aid, "ok": true, "text": a.first().cloned().unwrap_or_default()}));
        }
        FormAct::SignIn { aid } => {
            let with = (a.len() >= 2).then(|| (a[0].clone(), a[1].clone()));
            let _ = i.proxy.send_event(crate::engine::Ev::SignInAnswer(*aid, with));
        }
        FormAct::PopupSite => {
            let Some(host) = a.first().map(|v| crate::popup::host_of(v)) else { return };
            if host.is_empty() {
                i.page.flash = Some(("That is not a site name".into(), Instant::now()));
                return;
            }
            let mut list = s.settings.popup_sites.clone();
            if list.contains(&host) {
                return;
            }
            list.push(host);
            super::pages::set(i, "popup_sites", json!(list));
        }
        FormAct::RedirectSite => {
            let Some(host) = a.first().map(|v| crate::popup::host_of(v)) else { return };
            if host.is_empty() {
                i.page.flash = Some(("That is not a site name".into(), Instant::now()));
                return;
            }
            let mut list = s.settings.redirect_sites.clone();
            if list.contains(&host) {
                return;
            }
            list.push(host);
            super::pages::set(i, "redirect_sites", json!(list));
        }
    }
}

pub fn draw(i: &mut Inner, _s: &UiState) {
    let Some(mut card) = i.form.take() else { return };
    let lo = i.look.clone();
    let th = lo.theme;
    let Some((rect, boxes)) = geometry(i, card.fields.len(), card.act.for_page()) else {
        i.form = Some(card);
        return;
    };
    card.rect = rect;
    card.boxes = boxes;
    i.float_card(rect, 1.0);
    i.label(&card.title, R::new(rect.x + PAD, rect.y + PAD + 1.0, rect.w - 2.0 * PAD, 14.0), th.faint);
    i.hit(rect, Hit::FormCard);
    for (n, (label, _, secret)) in card.fields.iter().enumerate() {
        let Some(b) = card.boxes.get(n) else { break };
        i.gfx.text(label, R::new(b.x, b.y - LABEL_H - 4.0, b.w, LABEL_H), lo.small, th.dim);
        i.gfx.round(*b, 6.0, th.bg);
        i.gfx.round_stroke(*b, 6.0, th.line, 1.0);
        let edit = if *secret { i.form_secret_edit } else { i.form_edits[n.min(i.form_edits.len() - 1)] };
        let value = super::wide_text(edit);
        let shown = if *secret { "•".repeat(value.chars().count()) } else { value };
        i.gfx.text(&shown, R::new(b.x + 8.0, b.y, b.w - 16.0, b.h), lo.font(13.0, 450), th.text);
    }
    let by = rect.bottom() - PAD - BTN_H;
    let cancel = R::new(rect.right() - PAD - BTN_W, by, BTN_W, BTN_H);
    let save = R::new(cancel.x - 8.0 - BTN_W, by, BTN_W, BTN_H);
    i.pill_button(save, card.act.go(), Hit::FormSave, true, 1.0);
    i.pill_button(cancel, "Cancel", Hit::FormCancel, false, 1.0);
    i.form = Some(card);
}
