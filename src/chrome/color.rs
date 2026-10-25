//! The colour picker's card.

use serde_json::json;
use windows::Win32::UI::{
    Input::KeyboardAndMouse::{GetFocus, ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE, VK_RETURN},
    WindowsAndMessaging::SetWindowTextW,
};
use windows::core::HSTRING;

use super::{
    Hit, Inner, UiState,
    gfx::R,
    send,
    theme::Color,
};

const W: f32 = 264.0;
const PAD: f32 = 14.0;
const SV_H: f32 = 148.0;
const HUE_H: f32 = 12.0;
const SW: f32 = 26.0;
const GAP: f32 = 8.0;

pub struct ColorCard {
    key: String,
    was: String,
    color: Color,
    h: f32,
    s: f32,
    v: f32,
    drag: Option<u8>,
    anchor: Option<R>,
    rect: R,
    sv: R,
    hue: R,
}

impl ColorCard {
    pub fn hex_box(&self) -> R {
        R::new(self.rect.x + PAD, self.rect.bottom() - PAD - 26.0, 130.0, 26.0)
    }
}

fn presets() -> [&'static str; 7] {
    ["#e8322b", "#ff7ab8", "#8b7bff", "#141414", "#8e8a80", "#f1efe9", "#ffffff"]
}

fn title(key: &str) -> &'static str {
    match key {
        "accent" => "Accent",
        "bg_color" => "Background",
        "side_color" => "Sidebar and toolbar",
        "surface_color" => "Cards and fields",
        "text_color" => "Text",
        "dim_color" => "Dim text",
        _ => "Colour",
    }
}

pub fn rgb_to_hsv(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (c.0.clamp(0.0, 1.0), c.1.clamp(0.0, 1.0), c.2.clamp(0.0, 1.0));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        (60.0 * ((g - b) / d)) % 360.0
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    (h, s, max)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color {
    let h = (h.rem_euclid(360.0)) / 60.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    Color(r + m, g + m, b + m, 1.0)
}

pub fn open(i: &mut Inner, key: &str, was: String, color: Color, anchor: Option<R>) {
    let (h, s, v) = rgb_to_hsv(color);
    i.color = Some(ColorCard { key: key.to_string(), was, color, h, s, v, drag: None, anchor, rect: R::default(), sv: R::default(), hue: R::default() });
    let rect = match &i.state {
        Some(s) => rect_of(i, s, anchor),
        None => R::default(),
    };
    i.color.as_mut().unwrap().rect = rect;
    unsafe {
        let _ = SetWindowTextW(i.color_edit, &HSTRING::from(color.hex()));
    }
    i.layout_edits();
    i.invalidate();
}

pub fn close(i: &mut Inner, keep: bool) {
    let Some(card) = i.color.take() else { return };
    if !keep {
        super::pages::set(i, &card.key, json!(card.was));
    }
    unsafe {
        if GetFocus() == i.color_edit {
            let _ = SetFocus(Some(i.hwnd));
        }
    }
    i.layout_edits();
    i.invalidate();
}

pub fn done(i: &mut Inner) {
    let Some(card) = i.color.as_ref() else { return };
    remember(i, &card.color.hex());
    save(i);
    close(i, true);
}

pub fn cancel(i: &mut Inner) {
    close(i, false);
}

pub fn pick_preset(i: &mut Inner, n: usize) {
    if let Some(hex) = presets().get(n) {
        pick(i, hex);
    }
}

pub fn pick_recent(i: &mut Inner, n: usize) {
    if let Some(hex) = i.state.as_ref().and_then(|s| s.settings.recent_colors.get(n)).cloned() {
        pick(i, &hex);
    }
}

fn pick(i: &mut Inner, hex: &str) {
    let Some(c) = Color::from_hex(hex) else { return };
    if let Some(card) = i.color.as_mut() {
        let (h, s, v) = rgb_to_hsv(c);
        (card.h, card.s, card.v, card.color) = (h, s, v, c);
    }
    remember(i, hex);
    live(i);
    save(i);
    close(i, true);
}

pub fn press(i: &mut Inner, which: u8, x: f32, y: f32) {
    if let Some(card) = i.color.as_mut() {
        card.drag = Some(which);
    }
    unsafe {
        SetCapture(i.hwnd);
    }
    drag_move(i, x, y);
}

pub fn drag_move(i: &mut Inner, x: f32, y: f32) -> bool {
    let mut changed = false;
    if let Some(card) = i.color.as_mut() {
        match card.drag {
            Some(0) if card.sv.w > 0.0 => {
                card.s = ((x - card.sv.x) / card.sv.w).clamp(0.0, 1.0);
                card.v = (1.0 - (y - card.sv.y) / card.sv.h).clamp(0.0, 1.0);
                changed = true;
            }
            Some(1) if card.hue.w > 0.0 => {
                card.h = (((x - card.hue.x) / card.hue.w).clamp(0.0, 1.0) * 360.0).min(359.999);
                changed = true;
            }
            _ => {}
        }
        if changed {
            card.color = hsv_to_rgb(card.h, card.s, card.v);
        }
    }
    if changed {
        live(i);
        i.invalidate();
    }
    changed
}

pub fn drag_end(i: &mut Inner) -> bool {
    let Some(card) = i.color.as_mut() else { return false };
    if card.drag.take().is_none() {
        return false;
    }
    unsafe {
        let _ = ReleaseCapture();
    }
    i.invalidate();
    true
}

pub fn key(i: &mut Inner, vk: u32) -> bool {
    if i.color.is_none() {
        return false;
    }
    match vk {
        v if v == VK_ESCAPE.0 as u32 => {
            cancel(i);
            true
        }
        v if v == VK_RETURN.0 as u32 => {
            done(i);
            true
        }
        _ => false,
    }
}

pub fn typed(i: &mut Inner) {
    let text = super::wide_text(i.color_edit);
    let Some(c) = Color::from_hex(&text) else { return };
    let mut changed = false;
    if let Some(card) = i.color.as_mut() {
        let (h, s, v) = rgb_to_hsv(c);
        if (h, s, v) != (card.h, card.s, card.v) {
            (card.h, card.s, card.v, card.color) = (h, s, v, c);
            changed = true;
        }
    }
    if changed {
        live(i);
    }
    i.invalidate();
}

fn live(i: &mut Inner) {
    let Some(card) = i.color.as_ref() else { return };
    let (key, hex) = (card.key.clone(), card.color.hex());
    super::pages::set(i, &key, json!(hex));
    if unsafe { GetFocus() } != i.color_edit {
        unsafe {
            let _ = SetWindowTextW(i.color_edit, &HSTRING::from(hex));
        }
    }
}

fn remember(i: &mut Inner, hex: &str) {
    let Some(s) = i.state.as_mut() else { return };
    let hex = hex.to_lowercase();
    s.settings.recent_colors.retain(|c| c.to_lowercase() != hex);
    s.settings.recent_colors.insert(0, hex);
    s.settings.recent_colors.truncate(6);
}

fn save(i: &mut Inner) {
    if let Some(s) = &i.state {
        send(&i.proxy, json!({"t": "settings", "v": serde_json::to_value(&s.settings).unwrap_or_default()}));
    }
}

fn height(s: &UiState) -> f32 {
    let recents = if s.settings.recent_colors.is_empty() { 0.0 } else { 16.0 + 12.0 + 4.0 + SW };
    PAD + 14.0 + 10.0 + SV_H + 12.0 + HUE_H + 16.0 + 12.0 + 4.0 + SW + recents + 16.0 + 26.0 + PAD
}

fn rect_of(i: &Inner, s: &UiState, anchor: Option<R>) -> R {
    let (w, h) = i.size_dip();
    let rh = height(s);
    let (x, y) = match anchor {
        Some(a) => {
            let x = (a.right() - W).clamp(8.0, (w - W - 8.0).max(8.0));
            let below = a.bottom() + 8.0;
            let y = if below + rh <= h - 8.0 { below } else { (a.y - rh - 8.0).max(8.0) };
            (x, y)
        }
        None => ((w - W - 24.0).max(8.0), 80.0),
    };
    R::new(x, y, W, rh)
}

pub fn draw(i: &mut Inner, s: &UiState) {
    let Some(mut card) = i.color.take() else { return };
    let lo = i.look.clone();
    let th = lo.theme;
    let rect = rect_of(i, s, card.anchor);
    card.rect = rect;
    i.float_card(rect, 1.0);
    i.label(title(&card.key), R::new(rect.x + PAD, rect.y + PAD + 1.0, 200.0, 14.0), th.faint);
    i.hit(rect, Hit::ColorCard);

    let sv = R::new(rect.x + PAD, rect.y + PAD + 14.0 + 10.0, W - 2.0 * PAD, SV_H);
    card.sv = sv;
    let pure = hsv_to_rgb(card.h, 1.0, 1.0);
    i.gfx.round(sv, 8.0, Color(1.0, 1.0, 1.0, 1.0));
    i.gfx.grad(sv, 8.0, &[(0.0, Color(1.0, 1.0, 1.0, 1.0)), (1.0, pure)], false);
    i.gfx.grad(sv, 8.0, &[(0.0, Color(0.0, 0.0, 0.0, 0.0)), (1.0, Color(0.0, 0.0, 0.0, 1.0))], true);
    i.gfx.round_stroke(sv, 8.0, th.line, 1.0);
    let (kx, ky) = (sv.x + card.s * sv.w, sv.bottom() - card.v * sv.h);
    i.gfx.circle(kx, ky, 6.0, card.color);
    i.gfx.ring(kx, ky, 6.0, 6.0, Color(1.0, 1.0, 1.0, 1.0), 2.0);
    i.gfx.ring(kx, ky, 7.5, 7.5, th.shade.alpha(0.5), 1.0);
    i.hit(sv, Hit::ColorArea);

    let hue = R::new(sv.x, sv.bottom() + 12.0, sv.w, HUE_H);
    card.hue = hue;
    let spoke = |n: f32| hsv_to_rgb(n * 60.0, 1.0, 1.0);
    i.gfx.grad(hue, HUE_H / 2.0, &[(0.0, spoke(0.0)), (1.0 / 6.0, spoke(1.0)), (2.0 / 6.0, spoke(2.0)), (0.5, spoke(3.0)), (4.0 / 6.0, spoke(4.0)), (5.0 / 6.0, spoke(5.0)), (1.0, spoke(6.0))], false);
    i.gfx.round_stroke(hue, HUE_H / 2.0, th.line, 1.0);
    let hx = hue.x + card.h / 360.0 * hue.w;
    i.gfx.circle(hx, hue.y + HUE_H / 2.0, 6.0, Color(1.0, 1.0, 1.0, 1.0));
    i.gfx.ring(hx, hue.y + HUE_H / 2.0, 6.0, 6.0, th.shade.alpha(0.5), 1.0);
    i.hit(R::new(hue.x, hue.y - 4.0, hue.w, hue.h + 8.0), Hit::ColorHue);

    let mut y = hue.bottom() + 16.0;
    i.label("Presets", R::new(sv.x, y, 100.0, 12.0), th.faint);
    y += 12.0 + 4.0;
    let row = R::new(sv.x + 3.0, y, sv.w - 6.0, SW);
    for (n, hex) in presets().iter().enumerate() {
        let r = R::new(row.x + n as f32 * (SW + GAP), row.y, SW, SW);
        swatch(i, r, Color::from_hex(hex).unwrap_or(th.text), hex, card.color.hex());
        i.hit(r, Hit::ColorPreset(n));
    }
    y += SW;
    if !s.settings.recent_colors.is_empty() {
        y += 16.0;
        i.label("Recent", R::new(sv.x, y, 100.0, 12.0), th.faint);
        y += 12.0 + 4.0;
        let n = s.settings.recent_colors.len().min(6);
        let row = R::new(sv.x + (sv.w - (n as f32 * SW + (n as f32 - 1.0) * GAP)) / 2.0, y, sv.w, SW);
        for (k, hex) in s.settings.recent_colors.iter().take(6).enumerate() {
            let r = R::new(row.x + k as f32 * (SW + GAP), row.y, SW, SW);
            swatch(i, r, Color::from_hex(hex).unwrap_or(th.text), hex, card.color.hex());
            i.hit(r, Hit::ColorRecent(k));
        }
        y += SW;
    }

    y += 16.0;
    let hex_rect = card.hex_box();
    i.gfx.round(hex_rect, 6.0, th.bg);
    i.gfx.round_stroke(hex_rect, 6.0, th.line, 1.0);
    i.gfx.text(&card.color.hex(), R::new(hex_rect.x + 8.0, hex_rect.y, hex_rect.w - 16.0, hex_rect.h), lo.font(13.0, 450), th.text);
    i.hit(hex_rect, Hit::ColorCard);
    let done = R::new(rect.right() - PAD - 92.0, y, 92.0, 26.0);
    i.pill_button(done, "Done", Hit::ColorDone, true, 1.0);
    i.color = Some(card);
}

fn swatch(i: &mut Inner, r: R, c: Color, hex: &str, current: String) {
    let th = i.theme;
    i.gfx.round(r, 7.0, c);
    if hex.eq_ignore_ascii_case(&current) {
        i.gfx.round_stroke(r.inset(-1.0, -1.0), 8.0, th.text, 2.0);
    } else {
        i.gfx.round_stroke(r, 7.0, th.line, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::{hsv_to_rgb, rgb_to_hsv};
    use crate::chrome::theme::Color;

    fn close(a: (f32, f32, f32), b: (f32, f32, f32)) {
        assert!((a.0 - b.0).abs() < 0.01, "{a:?} != {b:?}");
        assert!((a.1 - b.1).abs() < 0.01, "{a:?} != {b:?}");
        assert!((a.2 - b.2).abs() < 0.01, "{a:?} != {b:?}");
    }

    #[test]
    fn primaries_have_their_wheel_places() {
        close(rgb_to_hsv(Color(1.0, 0.0, 0.0, 1.0)), (0.0, 1.0, 1.0));
        close(rgb_to_hsv(Color(0.0, 1.0, 0.0, 1.0)), (120.0, 1.0, 1.0));
        close(rgb_to_hsv(Color(0.0, 0.0, 1.0, 1.0)), (240.0, 1.0, 1.0));
        close(rgb_to_hsv(Color(1.0, 1.0, 0.0, 1.0)), (60.0, 1.0, 1.0));
        close(rgb_to_hsv(Color(1.0, 0.0, 1.0, 1.0)), (300.0, 1.0, 1.0));
    }

    #[test]
    fn greys_have_no_hue() {
        close(rgb_to_hsv(Color(0.5, 0.5, 0.5, 1.0)), (0.0, 0.0, 0.5));
        close(rgb_to_hsv(Color(0.0, 0.0, 0.0, 1.0)), (0.0, 0.0, 0.0));
        close(rgb_to_hsv(Color(1.0, 1.0, 1.0, 1.0)), (0.0, 0.0, 1.0));
    }

    #[test]
    fn hsv_and_rgb_round_trip() {
        for hex in ["#e8322b", "#2f6bff", "#13a36b", "#8b7bff", "#f1efe9", "#121211", "#808080", "#00ff88", "#000000", "#ffffff"] {
            let c = Color::from_hex(hex).expect("readable");
            let (h, s, v) = rgb_to_hsv(c);
            assert_eq!(hsv_to_rgb(h, s, v).hex(), hex, "{hex} did not come back");
        }
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0).hex(), "#ff0000");
        assert_eq!(hsv_to_rgb(120.0, 1.0, 1.0).hex(), "#00ff00");
        assert_eq!(hsv_to_rgb(240.0, 1.0, 1.0).hex(), "#0000ff");
        assert_eq!(hsv_to_rgb(360.0, 1.0, 1.0).hex(), "#ff0000");
        assert_eq!(hsv_to_rgb(-60.0, 1.0, 1.0).hex(), "#ff00ff");
    }
}
