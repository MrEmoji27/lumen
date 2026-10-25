//! Where the chrome lives and where the page goes.

use super::gfx::R;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Vertical,
    Horizontal,
    Hybrid,
}

impl Kind {
    pub fn of(v: &str) -> Kind {
        match v {
            "horizontal" => Kind::Horizontal,
            "hybrid" => Kind::Hybrid,
            _ => Kind::Vertical,
        }
    }

    pub fn top_toolbar(self) -> bool {
        !matches!(self, Kind::Vertical)
    }

    pub fn sidebar(self) -> bool {
        matches!(self, Kind::Vertical | Kind::Hybrid)
    }

    pub fn tab_strip(self) -> bool {
        matches!(self, Kind::Horizontal)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub strip_h: f32,
    pub tool_h: f32,
    pub bm_h: f32,
}

impl Metrics {
    pub fn new(kind: Kind, compact: bool, bm_bar: bool) -> Metrics {
        let (strip, tool) = match kind {
            Kind::Vertical => (0.0, 0.0),
            Kind::Horizontal => {
                if compact {
                    (32.0, 34.0)
                } else {
                    (38.0, 40.0)
                }
            }
            Kind::Hybrid => {
                if compact {
                    (0.0, 34.0)
                } else {
                    (0.0, 40.0)
                }
            }
        };
        let bm = if kind.tab_strip() && bm_bar {
            if compact { 30.0 } else { 34.0 }
        } else {
            0.0
        };
        Metrics { strip_h: strip, tool_h: tool, bm_h: bm }
    }

    pub fn top(&self) -> f32 {
        self.strip_h + self.tool_h + self.bm_h
    }
}

pub fn stage(kind: Kind, w: f32, h: f32, side_w: f32, side_right: bool, top: f32, gap: f32) -> (f32, f32, f32, f32) {
    let g = gap;
    match kind {
        Kind::Vertical => {
            let x = if side_right { g } else { side_w };
            (x, g, (w - side_w - g).max(1.0), (h - 2.0 * g).max(1.0))
        }
        Kind::Horizontal => (g, top + g, (w - 2.0 * g).max(1.0), (h - top - 2.0 * g).max(1.0)),
        Kind::Hybrid => {
            let x = if side_right { g } else { side_w };
            (x, top + g, (w - side_w - g).max(1.0), (h - top - 2.0 * g).max(1.0))
        }
    }
}

pub fn tab_width(avail: f32, n: usize, compact: bool) -> f32 {
    let (max, min) = if compact { (190.0, 44.0) } else { (230.0, 56.0) };
    if n == 0 {
        return max;
    }
    (avail / n as f32).clamp(min, max)
}

const GAP: f32 = 4.0;

pub const EDGE_BAND: f32 = 8.0;

pub fn strip_fixed(compact: bool, ws: bool) -> f32 {
    let win = if compact { 26.0 } else { 30.0 };
    let plus = if compact { 30.0 } else { 34.0 };
    let chip = if ws { if compact { 34.0 } else { 40.0 } } else { 0.0 };
    3.0 * (win + 1.0) + plus + chip + GAP
}

#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    pub pinned: Vec<R>,
    pub tabs: Vec<R>,
    pub plus: R,
    pub ws: Option<R>,
    pub wins: R,
}

pub fn strip(area: R, pins: usize, tabs: usize, compact: bool, ws: bool) -> Strip {
    let pad = if compact { 2.0 } else { 4.0 };
    let win = if compact { 26.0 } else { 30.0 };
    let wins_w = 3.0 * (win + 1.0);
    let wins = R::new(area.right() - pad - wins_w, area.y, wins_w, area.h);
    let pin_w = if compact { 32.0 } else { 38.0 };
    let plus_w = if compact { 30.0 } else { 34.0 };
    let chip_w = if compact { 34.0 } else { 40.0 };
    let mut x = area.x + pad;
    let mut pinned = Vec::new();
    for _ in 0..pins {
        if x + pin_w > wins.x {
            break;
        }
        pinned.push(R::new(x, area.y + 1.0, pin_w - 2.0, area.h - 2.0));
        x += pin_w;
    }
    let plus_x = area.right() - pad - strip_fixed(compact, ws);
    let chip_x = plus_x + GAP + plus_w;
    let avail = (plus_x - x).max(0.0);
    let tw = tab_width(avail, tabs, compact);
    let mut tabs_r = Vec::new();
    for _ in 0..tabs {
        if x + tw > plus_x + 0.01 {
            break;
        }
        tabs_r.push(R::new(x, area.y + 1.0, tw, area.h - 2.0));
        x += tw;
    }
    let plus = R::new(x.min(plus_x), area.y, plus_w, area.h);
    let ws_r = ws.then(|| R::new(chip_x, area.y, chip_w, area.h));
    Strip { pinned, tabs: tabs_r, plus, ws: ws_r, wins }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_leaves_room_for_the_sidebar_and_the_chrome() {
        let (x, y, w, h) = stage(Kind::Vertical, 1200.0, 800.0, 240.0, false, 0.0, 8.0);
        assert_eq!((x, y), (240.0, 8.0));
        assert_eq!((w, h), (952.0, 784.0));
        let (x, _, w, _) = stage(Kind::Vertical, 1200.0, 800.0, 240.0, true, 0.0, 8.0);
        assert_eq!((x, w), (8.0, 952.0));
        let m = Metrics::new(Kind::Horizontal, false, true);
        assert_eq!((m.strip_h, m.tool_h, m.bm_h), (38.0, 40.0, 34.0));
        let top = m.top();
        let (x, y, w, h) = stage(Kind::Horizontal, 1200.0, 800.0, 0.0, false, top, 8.0);
        assert_eq!((x, y), (8.0, 120.0));
        assert_eq!((w, h), (1184.0, 672.0));
        let m = Metrics::new(Kind::Hybrid, false, true);
        assert_eq!(m.top(), 40.0);
        let (x, y, w, h) = stage(Kind::Hybrid, 1200.0, 800.0, 240.0, false, m.top(), 8.0);
        assert_eq!((x, y), (240.0, 48.0));
        assert_eq!((w, h), (952.0, 744.0));
        let m = Metrics::new(Kind::Horizontal, true, false);
        assert_eq!((m.strip_h, m.tool_h, m.bm_h), (32.0, 34.0, 0.0));
        let (_, _, w, h) = stage(Kind::Horizontal, 200.0, 100.0, 0.0, false, 66.0, 8.0);
        assert!(w >= 1.0 && h >= 1.0);
    }

    #[test]
    fn tabs_shrink_as_they_multiply_and_stop_at_the_floor() {
        assert_eq!(tab_width(2000.0, 1, false), 230.0);
        assert_eq!(tab_width(900.0, 5, false), 180.0);
        let mut last = f32::MAX;
        for n in 1..40 {
            let w = tab_width(1000.0, n, false);
            assert!(w <= last, "width is not shrinking at n={n}");
            assert!(w >= 56.0, "width below the floor at n={n}: {w}");
            last = w;
        }
        assert_eq!(tab_width(10.0, 3, false), 56.0);
        assert!(tab_width(120.0, 3, true) < tab_width(120.0, 3, false));
        assert_eq!(tab_width(10.0, 3, true), 44.0);
    }

    #[test]
    fn the_strip_keeps_its_parts_apart_and_inside() {
        let area = R::new(0.0, 0.0, 1200.0, 38.0);
        let s = strip(area, 3, 7, false, true);
        assert_eq!(s.pinned.len(), 3);
        assert_eq!(s.tabs.len(), 7);
        let mut last_right = 0.0;
        for r in s.pinned.iter().chain(s.tabs.iter()) {
            assert!(r.x >= last_right, "rects overlap or run backwards: {r:?} after {last_right}");
            assert!(r.right() <= s.wins.x, "a tab runs into the window buttons");
            assert!(r.y >= area.y && r.bottom() <= area.bottom());
            last_right = r.right();
        }
        assert!(s.pinned[0].w < s.tabs[0].w);
        assert!(s.plus.x >= last_right - 0.5);
        assert!(s.ws.is_some_and(|w| w.x >= s.plus.right()));
        assert!(s.plus.right() <= s.wins.x + 0.01);
        assert!(s.wins.right() <= area.right());
        assert!(s.wins.x > s.plus.x);
        let s = strip(area, 0, 60, false, false);
        assert!(s.tabs.len() < 60);
        assert!(s.tabs.iter().all(|r| r.right() <= s.wins.x));
        assert!(s.tabs.iter().all(|r| r.w >= 56.0 - 0.01), "a tab fell below the floor");
        let s = strip(R::new(0.0, 0.0, 300.0, 38.0), 0, 2, false, false);
        assert!(s.tabs.iter().all(|r| r.w > 0.0 && r.right() <= s.wins.x));
        assert!(s.plus.w > 0.0);
    }

    #[test]
    fn hit_areas_are_where_the_parts_are_drawn() {
        let area = R::new(0.0, 0.0, 1000.0, 38.0);
        let s = strip(area, 1, 4, false, true);
        let mut all: Vec<R> = s.pinned.clone();
        all.extend(s.tabs.iter().copied());
        for (n, r) in all.iter().enumerate() {
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let inside = all.iter().filter(|o| o.contains(cx, cy)).count();
            assert_eq!(inside, 1, "rect {n} shares its centre with another");
        }
        if let Some(ws) = s.ws {
            assert!(ws.x >= s.plus.right(), "the workspace chip overlaps the + button");
            assert!(ws.right() <= s.wins.x + 0.01, "the workspace chip runs into the window buttons");
        }
    }

    #[test]
    fn the_strip_gives_the_tabs_the_room_they_need() {
        let area = R::new(0.0, 0.0, 1280.0, 38.0);
        let s = strip(area, 0, 8, false, true);
        assert_eq!(s.tabs.len(), 8);
        let last = s.tabs.last().expect("tabs");
        assert!((last.right() - s.plus.x).abs() < 0.5, "the tabs do not fill the strip: {last:?} plus {:?}", s.plus);
        let ws = s.ws.expect("a chip");
        assert_eq!(ws.right(), s.wins.x, "the chip does not stand by the window buttons");
        assert!(ws.x >= s.plus.right(), "the chip sits on the + button");
        let s = strip(area, 0, 8, false, false);
        assert!(s.ws.is_none());
        assert!(s.plus.right() <= s.wins.x);
        let s = strip(area, 0, 8, true, true);
        assert_eq!(s.tabs.len(), 8);
        assert!(s.ws.is_some_and(|w| w.x >= s.plus.right()));
    }

    #[test]
    fn a_tab_is_never_cut_in_half() {
        let area = R::new(0.0, 0.0, 1280.0, 38.0);
        let s = strip(area, 0, 40, false, false);
        assert!(s.tabs.len() < 40, "all forty fit? the room is wrong");
        assert!(s.tabs.iter().all(|r| r.w >= 56.0 - 0.01), "a tab fell below the floor");
        let w = s.tabs[0].w;
        assert!(s.tabs.iter().all(|r| (r.w - w).abs() < 0.01), "the tabs are not even");
        assert!(s.tabs.last().expect("tabs").right() <= s.plus.x + 0.01);
    }
}
