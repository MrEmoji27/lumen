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

/// The gap that keeps the strip's fixed parts from touching each other.
const GAP: f32 = 4.0;

/// The band of chrome left at the edge of the window when the sidebar is
/// hidden until the mouse reaches the edge: enough for the mouse to find it.
pub const EDGE_BAND: f32 = 8.0;

/// What the strip's fixed parts take on the right, so the tabs know what is left:
/// the window buttons, the "+", the workspaces chip and the gap between them.
pub fn strip_fixed(compact: bool, ws: bool) -> f32 {
    let win = if compact { 26.0 } else { 30.0 };
    let plus = if compact { 30.0 } else { 34.0 };
    let chip = if ws { if compact { 34.0 } else { 40.0 } } else { 0.0 };
    3.0 * (win + 1.0) + plus + chip + GAP
}

/// The horizontal strip's rectangles: pinned tabs (icon only), the tabs, the
/// "+", the workspaces chip, then the window buttons, left to right.
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
    let pad = if compact { 2.0 } else { 4.0 };
    let win = if compact { 26.0 } else { 30.0 };
    let wins_w = 3.0 * (win + 1.0);
    let wins = R::new(area.right() - pad - wins_w, area.y, wins_w, area.h);
    let pin_w = if compact { 32.0 } else { 38.0 };
    let plus_w = if compact { 30.0 } else { 34.0 };
    let chip_w = if compact { 34.0 } else { 40.0 };
    // Pinned tabs first, as small icon cells. The ones that do not fit are left
    // out: the strip has no second row to give them.
    let mut x = area.x + pad;
    let mut pinned = Vec::new();
    for _ in 0..pins {
        if x + pin_w > wins.x {
            break;
        }
        pinned.push(R::new(x, area.y + 1.0, pin_w - 2.0, area.h - 2.0));
        x += pin_w;
    }
    // The workspaces chip stands at the far right, by the window buttons, and the
    // "+" follows the tabs. The tabs get the room between the pins and the "+",
    // so neither the chip nor the button can land on top of one of them.
    let plus_x = area.right() - pad - strip_fixed(compact, ws);
    let chip_x = plus_x + GAP + plus_w;
    let avail = (plus_x - x).max(0.0);
    let tw = tab_width(avail, tabs, compact);
    let mut tabs_r = Vec::new();
    for _ in 0..tabs {
        // A tab that does not fit whole is not drawn at all: half a tab is worse
        // than no tab.
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
        // The chip stands at the far right, by the window buttons: it never lands
        // on a tab.
        assert!(s.ws.is_some_and(|w| w.x >= s.plus.right()));
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
            assert!(ws.x >= s.plus.right(), "the workspace chip overlaps the + button");
            assert!(ws.right() <= s.wins.x + 0.01, "the workspace chip runs into the window buttons");
        }
    }

    #[test]
    fn the_strip_gives_the_tabs_the_room_they_need() {
        // Eight tabs in a 1280 wide window: they fill the room between the pins
        // and the "+", with no gap and no overlap.
        let area = R::new(0.0, 0.0, 1280.0, 38.0);
        let s = strip(area, 0, 8, false, true);
        assert_eq!(s.tabs.len(), 8);
        let last = s.tabs.last().expect("tabs");
        assert!((last.right() - s.plus.x).abs() < 0.5, "the tabs do not fill the strip: {last:?} plus {:?}", s.plus);
        // The chip is at the far right, by the window buttons, and clear of the "+".
        let ws = s.ws.expect("a chip");
        assert_eq!(ws.right(), s.wins.x, "the chip does not stand by the window buttons");
        assert!(ws.x >= s.plus.right(), "the chip sits on the + button");
        // The same strip without workspaces: the "+" takes the chip's place.
        let s = strip(area, 0, 8, false, false);
        assert!(s.ws.is_none());
        assert!(s.plus.right() <= s.wins.x);
        // Compact: the same shape, smaller.
        let s = strip(area, 0, 8, true, true);
        assert_eq!(s.tabs.len(), 8);
        assert!(s.ws.is_some_and(|w| w.x >= s.plus.right()));
    }

    #[test]
    fn a_tab_is_never_cut_in_half() {
        // Forty tabs in 1280 pixels do not fit: the ones that fit keep their
        // width, and the last one is not sliced.
        let area = R::new(0.0, 0.0, 1280.0, 38.0);
        let s = strip(area, 0, 40, false, false);
        assert!(s.tabs.len() < 40, "all forty fit? the room is wrong");
        assert!(s.tabs.iter().all(|r| r.w >= 56.0 - 0.01), "a tab fell below the floor");
        // Every tab is the same width: a strip of even cells, not a ragged end.
        let w = s.tabs[0].w;
        assert!(s.tabs.iter().all(|r| (r.w - w).abs() < 0.01), "the tabs are not even");
        // And the "+" and the chip are still clear of them.
        assert!(s.tabs.last().expect("tabs").right() <= s.plus.x + 0.01);
    }
}
