use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowBuilder};
use tao::dpi::LogicalSize;
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};

pub struct Tiles {
    slots: Vec<Option<u64>>,
}

impl Tiles {
    pub fn new() -> Self { Tiles { slots: Vec::new() } }
    pub fn slot(&mut self, tab: u64, _page: (f64, f64)) -> usize {
        // reuse a free slot
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
    pub fn index_of(&self, tab: u64) -> Option<usize> {
        self.slots.iter().position(|s| *s == Some(tab))
    }
}

pub fn tile_rect(index: usize, page: (f64, f64)) -> (f64, f64, f64, f64) {
    (index as f64 * page.0, 0.0, page.0, page.1)
}

pub struct Stage {
    window: Window,
    tiles: Tiles,
    tile_size: (f64, f64),
}

impl Stage {
    pub fn new(target: &EventLoopWindowTarget<crate::engine::Ev>) -> Self {
        let window = WindowBuilder::new()
            .with_title("Lumen stage")
            .with_decorations(false)
            .with_visible(true)
            .with_skip_taskbar(true)
            .with_inner_size(LogicalSize::new(1000.0, 700.0))
            .build(target)
            .expect("stage window");
        // place off-screen: right edge + 200
        // best effort; exact placement left to builder
        Stage { window, tiles: Tiles::new(), tile_size: (1000.0, 700.0) }
    }
    pub fn hwnd(&self) -> isize {
        self.window.hwnd() as isize
    }
    pub fn set_visible(&self, on: bool) {
        let _ = self.window.set_visible(on);
    }
    pub fn slot(&mut self, tab: u64, page: (f64, f64)) -> (f64, f64, f64, f64) {
        self.tile_size = page;
        let idx = self.tiles.slot(tab, page);
        // resize window to fit all tiles
        let n = self.tiles.slots.len();
        let _ = self.window.set_inner_size(tao::dpi::LogicalSize::new(page.0 * n as f64, page.1));
        tile_rect(idx, page)
    }
    pub fn release(&mut self, tab: u64) {
        self.tiles.release(tab);
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
        let i0 = t.slot(1, (100.0, 100.0));
        let i1 = t.slot(2, (100.0, 100.0));
        assert_ne!(i0, i1);
        t.release(1);
        let i2 = t.slot(3, (100.0, 100.0));
        assert_eq!(i2, i0);
    }
}