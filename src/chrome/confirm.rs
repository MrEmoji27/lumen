//! The drawn question card: a title, a line or two, and the buttons. A destructive
//! first button is drawn in the look's danger colour. Like the form, it is state on
//! the chrome plus a pending action, never a window with a loop of its own.

use serde_json::json;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE, VK_RETURN};

use super::{Hit, Inner, gfx::R, send};
use crate::{engine::Ev, extlink};

/// What the question is for: the action a chosen button runs.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmKind {
    /// Forget every count in the privacy report.
    ResetStats,
    /// A site asked to open an app. The answer goes back to the main loop, which
    /// finishes the question it started.
    External { origin: String, uri: String, remember: bool, scheme: String },
}

/// One open question: its title, its words, and its action.
pub struct ConfirmCard {
    title: String,
    text: String,
    act: ConfirmKind,
    /// Where the card, and each button, was drawn. Filled every frame.
    pub rect: R,
    pub buttons: Vec<R>,
}

/// The card's sides, and the parts that do not move.
const PAD: f32 = 14.0;
const BTN_H: f32 = 28.0;
const GAP: f32 = 8.0;

/// The buttons of a question, in the order they read: label, whether it is the
/// destructive one. The last is always Cancel.
fn buttons_of(act: &ConfirmKind) -> Vec<String> {
    match act {
        ConfirmKind::ResetStats => vec!["Reset".into(), "Cancel".into()],
        ConfirmKind::External { remember, .. } => {
            let mut b = vec!["Open".to_string()];
            if *remember {
                b.push("Always for this site".into());
            }
            b.push("Cancel".into());
            b
        }
    }
}

/// Whether the n-th button is the destructive one: the first, when what it runs
/// cannot be taken back.
fn destructive(act: &ConfirmKind, n: usize) -> bool {
    matches!(act, ConfirmKind::ResetStats) && n == 0
}

/// The title, the words, and the button labels of the external-link question.
pub fn external_parts(origin: &str, scheme: &str) -> (String, String) {
    extlink::prompt_words(origin, scheme)
}

/// Where the card and its buttons sit: the top of the floating cards' column, wide
/// enough for the button row.
fn geometry(i: &Inner, act: &ConfirmKind) -> Option<(R, Vec<R>)> {
    let area = i.float_area()?;
    let (x, foath) = super::float_col(area, i.cards_right());
    let labels = buttons_of(act);
    let f = i.look.font(12.5, 600);
    let widths: Vec<f32> = labels.iter().map(|l| (i.gfx.measure(l, f) + 30.0).max(64.0)).collect();
    let row = widths.iter().sum::<f32>() + GAP * (labels.len() as f32 - 1.0);
    let w = foath.max(row + 2.0 * PAD);
    let h = PAD + 14.0 + 8.0 + 36.0 + 16.0 + BTN_H + PAD;
    let rect = R::new(x, area.y + 8.0, w, h);
    // Right to left: Cancel at the edge, the first button leftmost.
    let mut buttons: Vec<R> = Vec::new();
    let mut bx = rect.right() - PAD;
    for width in widths.iter().rev() {
        bx -= width;
        buttons.push(R::new(bx, rect.bottom() - PAD - BTN_H, *width, BTN_H));
        bx -= GAP;
    }
    buttons.reverse();
    Some((rect, buttons))
}

/// Open a question. One card at a time: whatever else was open goes away.
pub fn open(i: &mut Inner, act: ConfirmKind) {
    i.close_cards();
    let (title, text) = match &act {
        ConfirmKind::ResetStats => ("Reset statistics".into(), "Forget every count in the privacy report?".into()),
        ConfirmKind::External { origin, scheme, .. } => external_parts(origin, scheme),
    };
    let (rect, buttons) = match geometry(i, &act) {
        Some(g) => g,
        None => return,
    };
    i.confirm = Some(ConfirmCard { title, text, act, rect, buttons });
    i.layout_edits();
    unsafe {
        let _ = SetFocus(Some(i.hwnd));
    }
    i.invalidate();
}

/// Put the question away unanswered.
pub fn cancel(i: &mut Inner) {
    if i.confirm.take().is_none() {
        return;
    }
    i.invalidate();
}

/// A button was pressed: the pending action runs.
pub fn pick(i: &mut Inner, n: usize) {
    let Some(card) = i.confirm.take() else { return };
    match card.act {
        ConfirmKind::ResetStats => {
            if n == 0 {
                send(&i.proxy, json!({"t": "reportReset"}));
            }
        }
        ConfirmKind::External { origin, uri, remember, .. } => {
            let answer = match n {
                0 => extlink::Answer::Once,
                1 if remember => extlink::Answer::Always,
                _ => extlink::Answer::Cancel,
            };
            let _ = i.proxy.send_event(Ev::ExternalAnswer { origin, uri, answer });
        }
    }
    i.invalidate();
}

/// Escape cancels, Enter is the first button. True while a question is open.
pub fn key(i: &mut Inner, vk: u32) -> bool {
    if i.confirm.is_none() {
        return false;
    }
    match vk {
        v if v == VK_ESCAPE.0 as u32 => {
            cancel(i);
            true
        }
        v if v == VK_RETURN.0 as u32 => {
            pick(i, 0);
            true
        }
        _ => false,
    }
}

/// The pointer came up outside the card: that cancels it, as Escape does.
pub fn clicked_outside(i: &mut Inner, h: &Option<Hit>) {
    if i.confirm.is_some() && !matches!(h, Some(Hit::ConfirmCard | Hit::ConfirmButton(_))) {
        cancel(i);
    }
}

/// Draw the card: its title, its words on two lines at most, and its buttons.
pub fn draw(i: &mut Inner, _s: &super::UiState) {
    let Some(mut card) = i.confirm.take() else { return };
    let lo = i.look.clone();
    let th = lo.theme;
    let Some((rect, buttons)) = geometry(i, &card.act) else {
        i.confirm = Some(card);
        return;
    };
    card.rect = rect;
    card.buttons = buttons;
    i.float_card(rect, 1.0);
    i.hit(rect, Hit::ConfirmCard);
    i.label(&card.title, R::new(rect.x + PAD, rect.y + PAD + 1.0, rect.w - 2.0 * PAD, 14.0), th.faint);
    i.gfx.text_wrap(&card.text, R::new(rect.x + PAD, rect.y + PAD + 14.0 + 8.0, rect.w - 2.0 * PAD, 36.0), lo.font(12.5, 450), th.text);
    let labels = buttons_of(&card.act);
    for (n, r) in card.buttons.iter().enumerate() {
        let label = labels.get(n).cloned().unwrap_or_default();
        if destructive(&card.act, n) {
            // The one button that cannot be taken back, in the danger colour.
            let hov = i.is_hover(&Hit::ConfirmButton(n));
            i.gfx.round(*r, lo.round(r.h), th.danger.alpha(if hov { 1.0 } else { 0.92 }));
            i.gfx.text(&label, *r, lo.font(12.5, 600).align(super::gfx::Align::Center), th.pill_ink);
            i.hit(*r, Hit::ConfirmButton(n));
        } else {
            i.pill_button(*r, &label, Hit::ConfirmButton(n), n == 0, 1.0);
        }
    }
    i.confirm = Some(card);
}
