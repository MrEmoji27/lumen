//! The agent stage: a second top-level window, off every monitor, that hosts every
//! agent tab's view. Agent views must be visible to Chromium or it drops their mouse
//! and keys; the person must never see this window. Views move between it and the
//! main window with wry's `reparent` — same page, same profile, same CDP session.

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
    hwnd: windows::Win32::Foundation::HWND,
    tiles: Tiles,
}

const CLASS: windows::core::PCWSTR = windows::core::w!("LumenStage");

impl Stage {
    /// Made the first time an agent tab needs it, not at start: a Lumen nobody's agent
    /// uses never pays for it. A plain Win32 window (no tao): it needs no event loop.
    pub fn new() -> Self {
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, RegisterClassExW, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
        };
        unsafe {
            let hinst: windows::Win32::Foundation::HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(stage_proc),
                hInstance: hinst,
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS,
                windows::core::w!("Lumen stage"),
                WS_POPUP,
                0,
                0,
                1000,
                700,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("stage window");
            place_offscreen(hwnd);
            Stage { hwnd, tiles: Tiles::new() }
        }
    }
    pub fn hwnd(&self) -> isize {
        self.hwnd.0 as isize
    }
    pub fn slot(&mut self, tab: u64, page: (f64, f64)) -> (f64, f64, f64, f64) {
        use windows::Win32::UI::HiDpi::GetDpiForWindow;
        use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowPos};
        let idx = self.tiles.slot(tab);
        // Enough room for every tile ever handed out; freed tiles keep their place.
        let n = self.tiles.slots.len().max(1);
        let scale = unsafe { GetDpiForWindow(self.hwnd) }.max(96) as f64 / 96.0;
        let (w, h) = ((page.0 * n as f64 * scale) as i32, (page.1 * scale) as i32);
        unsafe {
            let _ = SetWindowPos(self.hwnd, None, 0, 0, w, h, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER);
        }
        tile_rect(idx, page)
    }
    pub fn release(&mut self, tab: u64) {
        self.tiles.release(tab);
    }
}

/// Nothing to handle: Windows' own answers.
unsafe extern "system" fn stage_proc(h: windows::Win32::Foundation::HWND, m: u32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(h, m, w, l) }
}

/// No taskbar button, no Alt-Tab entry, never activates (the styles it was made with).
/// Shown without activation and placed past the virtual screen's right edge, so nothing
/// covers it (Chromium would mark a covered window's page `hidden`) and the person never
/// sees it.
fn place_offscreen(h: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, HWND_BOTTOM, SM_CXVIRTUALSCREEN, SM_XVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOSIZE, SW_SHOWNOACTIVATE, SetWindowPos,
        ShowWindow,
    };
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) + 200;
        let _ = SetWindowPos(h, Some(HWND_BOTTOM), x, 0, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE);
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
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
