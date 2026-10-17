//! Where the chrome lives and where the page goes. Pure math: main.rs asks for
//! the page rectangle, the drawing code asks for the same numbers plus the
//! strip's element rectangles, and the tests pin both down. Nothing here reads
//! the window or the settings; that is `Look`'s job.

use super::gfx::R;

/// The three ways the chrome can be arranged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// The sidebar: navigation, the address bar, tabs and the footer in a strip
    /// down one side. Lumen's own look.
    Vertical,
    /// Like Chrome: a tab strip across the top, a toolbar under it, the page below.
    Horizontal,
    /// A vertical tab sidebar with the toolbar (address bar and all) across the top.
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

    /// The address bar lives in a top toolbar rather than the sidebar.
    pub fn top_toolbar(self) -> bool {
        !matches!(self, Kind::Vertical)
    }

    /// A tab list down one side of the window.
    pub fn sidebar(self) -> bool {
        matches!(self, Kind::Vertical | Kind::Hybrid)
    }

    /// A tab strip across the top of the window.
    pub fn tab_strip(self) -> bool {
        matches!(self, Kind::Horizontal)
    }
}

/// The chrome's own heights, for a layout and a density. The page rectangle and
/// every drawn row count from these, so the two never disagree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// The tab strip's height (horizontal only).
    pub strip_h: f32,
    /// The toolbar's height (the layouts with a top toolbar).
    pub tool_h: f32,
    /// The bookmarks bar's height; 0 when it is off or there is no toolbar.
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

    /// Everything the chrome takes off the top of the window.
    pub fn top(&self) -> f32 {
        self.strip_h + self.tool_h + self.bm_h
    }
}

/// The page rectangle, in window coordinates. `side_w` is the sidebar's width,
/// `side_right` which edge it is on, `top` the chrome's top rows, `gap` the
/// margin around the page. The same numbers the chrome draws against.
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

/// One tab's width in the strip. Tabs share what is left over; past the cap they
/// stop growing, past the floor they keep shrinking until the strip is full.
pub fn tab_width(avail: f32, n: usize, compact: bool) -> f32 {
    let (max, min) = if compact { (190.0, 44.0) } else { (230.0, 56.0) };
    if n == 0 {
        return max;
    }
    (avail / n as f32).clamp(min, max)
}

/// What the strip's fixed parts take, so the tabs know what is left.
pub fn strip_fixed(compact: bool, ws: bool) -> f32 {
    let wins = if compact { 3.0 * 32.0 } else { 3.0 * 40.0 };
    let plus = if compact { 30.0 } else { 34.0 };
    let chip = if ws { if compact { 34.0 } else { 40.0 } } else { 0.0 };
    wins + plus + chip + 8.0
}

/// The horizontal strip's rectangles: pinned tabs (icon only), the tabs, the
/// "+", the workspaces chip, then the window buttons, right to left.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    /// One rectangle per pinned tab, in order.
    pub pinned: Vec<R>,
    /// One rectangle per unpinned tab, in order.
    pub tabs: Vec<R>,
    pub plus: R,
    /// The workspaces chip, when there is one.
    pub ws: Option<R>,
    pub wins: R,
}

/// Lay the strip out. `area` is the strip's row; `tabs` is the visible tab count
/// (unpinned), `pins` the pinned count. Tabs never overlap and never leave the
/// strip; the window buttons always get their room.
pub fn strip(area: R, pins: usize, tabs: usize, compact: bool, ws: bool) -> Strip {
    let (win_btn, pad) = if compact { (30.0, 2.0) } else { (38.0, 4.0) };
    let wins = R::new(area.right() - pad - 3.0 * win_btn, area.y, 3.0 * win_btn, area.h);
    let mut x = area.x + pad;
    let pin_w = if compact { 32.0 } else { 38.0 };
    let mut pinned = Vec::new();
    for _ in 0..pins {
        let r = R::new(x, area.y + 1.0, pin_w - 2.0, area.h - 2.0);
        pinned.push(r);
        x += pin_w;
        if x + pin_w > wins.x {
            break;
        }
    }
    let fixed = strip_fixed(compact, ws) + pins as f32 * pin_w;
    let avail = (wins.x - area.x - fixed).max(0.0);
    let tw = tab_width(avail, tabs, compact);
    let mut tabs_r = Vec::new();
    for _ in 0..tabs {
        if x + (if compact { 20.0 } else { 28.0 }) > wins.x {
            break;
        }
        let w = tw.min(wins.x - x);
        tabs_r.push(R::new(x, area.y + 1.0, w, area.h - 2.0));
        x += w;
    }
    let plus_w = if compact { 30.0 } else { 34.0 };
    let plus = R::new(x.min(wins.x - plus_w), area.y, plus_w, area.h);
    let ws_r = ws.then(|| R::new((plus.x - if compact { 34.0 } else { 40.0 }).max(area.x), area.y, if compact { 34.0 } else { 40.0 }, area.h));
    Strip { pinned, tabs: tabs_r, plus, ws: ws_r, wins }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_leaves_room_for_the_sidebar_and_the_chrome() {
        // Vertical: the sidebar takes one side, the margin the rest.
        let (x, y, w, h) = stage(Kind::Vertical, 1200.0, 800.0, 240.0, false, 0.0, 8.0);
        assert_eq!((x, y), (240.0, 8.0));
        assert_eq!((w, h), (952.0, 784.0));
        // The sidebar on the right swaps the margin to the left.
        let (x, _, w, _) = stage(Kind::Vertical, 1200.0, 800.0, 240.0, true, 0.0, 8.0);
        assert_eq!((x, w), (8.0, 952.0));
        // Horizontal: the strip and toolbar take the top, the margin the sides.
        let m = Metrics::new(Kind::Horizontal, false, true);
        assert_eq!((m.strip_h, m.tool_h, m.bm_h), (38.0, 40.0, 34.0));
        let top = m.top();
        let (x, y, w, h) = stage(Kind::Horizontal, 1200.0, 800.0, 0.0, false, top, 8.0);
        assert_eq!((x, y), (8.0, 120.0));
        assert_eq!((w, h), (1184.0, 672.0));
        // Hybrid: the sidebar, the toolbar, the margin.
        let m = Metrics::new(Kind::Hybrid, false, true);
        assert_eq!(m.top(), 40.0);
        let (x, y, w, h) = stage(Kind::Hybrid, 1200.0, 800.0, 240.0, false, m.top(), 8.0);
        assert_eq!((x, y), (240.0, 48.0));
        assert_eq!((w, h), (952.0, 744.0));
        // Compact trims the chrome's rows.
        let m = Metrics::new(Kind::Horizontal, true, false);
        assert_eq!((m.strip_h, m.tool_h, m.bm_h), (32.0, 34.0, 0.0));
        // A stage never collapses, whatever the window is.
        let (_, _, w, h) = stage(Kind::Horizontal, 200.0, 100.0, 0.0, false, 66.0, 8.0);
        assert!(w >= 1.0 && h >= 1.0);
    }

    #[test]
    fn tabs_shrink_as_they_multiply_and_stop_at_the_floor() {
        // One tab: capped, not stretched across the strip.
        assert_eq!(tab_width(2000.0, 1, false), 230.0);
        // A few: they share the room.
        assert_eq!(tab_width(900.0, 5, false), 180.0);
        // Many: shrinking, never below the floor.
        let mut last = f32::MAX;
        for n in 1..40 {
            let w = tab_width(1000.0, n, false);
            assert!(w <= last, "width is not shrinking at n={n}");
            assert!(w >= 56.0, "width below the floor at n={n}: {w}");
            last = w;
        }
        assert_eq!(tab_width(10.0, 3, false), 56.0);
        // Compact tabs shrink further before they stop.
        assert!(tab_width(120.0, 3, true) < tab_width(120.0, 3, false));
        assert_eq!(tab_width(10.0, 3, true), 44.0);
    }

    #[test]
    fn the_strip_keeps_its_parts_apart_and_inside() {
        let area = R::new(0.0, 0.0, 1200.0, 38.0);
        let s = strip(area, 3, 7, false, true);
        // Three pinned tabs, then seven tabs, all inside the strip and in order.
        assert_eq!(s.pinned.len(), 3);
        assert_eq!(s.tabs.len(), 7);
        let mut last_right = 0.0;
        for r in s.pinned.iter().chain(s.tabs.iter()) {
            assert!(r.x >= last_right, "rects overlap or run backwards: {r:?} after {last_right}");
            assert!(r.right() <= s.wins.x, "a tab runs into the window buttons");
            assert!(r.y >= area.y && r.bottom() <= area.bottom());
            last_right = r.right();
        }
        // The pinned tabs are icon-sized, the + and the chip follow the tabs.
        assert!(s.pinned[0].w < s.tabs[0].w);
        assert!(s.plus.x >= last_right - 0.5);
        assert!(s.ws.is_some_and(|w| w.x < s.plus.x));
        assert!(s.plus.right() <= s.wins.x + 0.01);
        // The window buttons are the rightmost thing in the strip.
        assert!(s.wins.right() <= area.right());
        assert!(s.wins.x > s.plus.x);
        // Many tabs: the ones that fit stay inside; the rest are not laid out.
        let s = strip(area, 0, 60, false, false);
        assert!(s.tabs.len() < 60);
        assert!(s.tabs.iter().all(|r| r.right() <= s.wins.x));
        assert!(s.tabs.iter().all(|r| r.w >= 56.0 - 0.01), "a tab fell below the floor");
        // A narrow strip still yields clickable rectangles.
        let s = strip(R::new(0.0, 0.0, 300.0, 38.0), 0, 2, false, false);
        assert!(s.tabs.iter().all(|r| r.w > 0.0 && r.right() <= s.wins.x));
        assert!(s.plus.w > 0.0);
    }

    #[test]
    fn hit_areas_are_where_the_parts_are_drawn() {
        // A click at a tab's centre lands on that tab's rectangle and no other.
        let area = R::new(0.0, 0.0, 1000.0, 38.0);
        let s = strip(area, 1, 4, false, true);
        let mut all: Vec<R> = s.pinned.clone();
        all.extend(s.tabs.iter().copied());
        for (n, r) in all.iter().enumerate() {
            let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
            let inside = all.iter().filter(|o| o.contains(cx, cy)).count();
            assert_eq!(inside, 1, "rect {n} shares its centre with another");
        }
        // The + button and the chip do not overlap.
        if let Some(ws) = s.ws {
            assert!(ws.right() <= s.plus.x, "the workspace chip overlaps the + button");
        }
    }
}
