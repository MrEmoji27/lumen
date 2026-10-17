//! The agent stage: a second top-level window, off every monitor, that hosts every
//! agent tab's view. Agent views must be visible to Chromium or it drops their mouse
//! and keys; the person must never see this window. Views move between it and the
//! main window with wry's `reparent` — same page, same profile, same CDP session.

use tao::dpi::LogicalSize;
use tao::event_loop::EventLoopWindowTarget;
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::{Window, WindowBuilder};

/// Which tile of the stage row each agent tab sits in. A released tile is reused;
/// tiles never move, so a tab keeps its place across watching and returning.
pub struct Tiles {
    slots: Vec<Option<u64>>,
}

impl Tiles {
    pub fn new() -> Self {
        Tiles { slots: Vec::new() }
    }
    pub fn slot(&mut self, tab: u64) -> usize {
        // A tab that already has a tile keeps it.
        if let Some(i) = self.slots.iter().position(|s| *s == Some(tab)) {
            return i;
        }
        for (i, s) in self.slots.iter_mut().enumerate() {
            if s.is_none() {
                *s = Some(tab);
                return i;
            }
        }
        self.slots.push(Some(tab));
        self.slots.len() - 1
    }
    pub fn release(&mut self, tab: u64) {
        for s in self.slots.iter_mut() {
            if *s == Some(tab) {
                *s = None;
            }
        }
    }
}

/// One tile: page-sized, side by side in a single row, never overlapping. Chromium
/// marks an occluded window's page hidden, so tiles must not cover each other.
pub fn tile_rect(index: usize, page: (f64, f64)) -> (f64, f64, f64, f64) {
    (index as f64 * page.0, 0.0, page.0, page.1)
}

pub struct Stage {
    window: Window,
    tiles: Tiles,
}

impl Stage {
    pub fn new(target: &EventLoopWindowTarget<crate::engine::Ev>) -> Self {
        let window = WindowBuilder::new()
            .with_title("Lumen stage")
            .with_decorations(false)
            .with_visible(false)
            .with_skip_taskbar(true)
            .with_inner_size(LogicalSize::new(1000.0, 700.0))
            .build(target)
            .expect("stage window");
        place_offscreen(&window);
        Stage { window, tiles: Tiles::new() }
    }
    pub fn hwnd(&self) -> isize {
        self.window.hwnd() as isize
    }
    /// The stage window itself stays shown: its views are what idle, on the tick.
    pub fn set_visible(&self, on: bool) {
        let _ = self.window.set_visible(on);
    }
    pub fn slot(&mut self, tab: u64, page: (f64, f64)) -> (f64, f64, f64, f64) {
        let idx = self.tiles.slot(tab);
        // Enough room for every tile ever handed out; freed tiles keep their place.
        let n = self.tiles.slots.len().max(1);
        let _ = self.window.set_inner_size(LogicalSize::new(page.0 * n as f64, page.1));
        tile_rect(idx, page)
    }
    pub fn release(&mut self, tab: u64) {
        self.tiles.release(tab);
    }
}

/// Tool window: no taskbar button, no Alt-Tab entry, never activates. Shown without
/// activation and placed past the virtual screen's right edge, so nothing covers it
/// (Chromium would mark a covered window's page `hidden`) and the person never sees it.
fn place_offscreen(window: &Window) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, GetWindowLongPtrW, HWND_BOTTOM, SetWindowLongPtrW, SetWindowPos, ShowWindow,
        GWL_EXSTYLE, SM_CXVIRTUALSCREEN, SM_XVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOSIZE, SW_SHOWNOACTIVATE,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    let h = HWND(window.hwnd() as _);
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let _ = SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_TOOLWINDOW.0 as isize | WS_EX_NOACTIVATE.0 as isize);
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) + 200;
        // Same trick as a headless main window: show first (showing puts a window
        // back at its last place), then move it out of sight, bottom of the z-order.
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(h, Some(HWND_BOTTOM), x, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tiles_never_overlap() {
        assert_eq!(tile_rect(0, (1000.0, 700.0)), (0.0, 0.0, 1000.0, 700.0));
        assert_eq!(tile_rect(2, (1000.0, 700.0)).0, 2000.0);
        let mut t = Tiles::new();
        let i0 = t.slot(1);
        let i1 = t.slot(2);
        assert_ne!(i0, i1);
        // Asking again for the same tab returns the same tile; it never moves.
        assert_eq!(t.slot(1), i0);
        // A released tile is the one reused.
        t.release(1);
        let i2 = t.slot(3);
        assert_eq!(i2, i0);
        assert_eq!(t.slot(3), i0);
    }
}
