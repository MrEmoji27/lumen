//! No WebView2 on this PC. Lumen cannot show a single page without it, so instead of a
//! message box it opens one small window of its own, drawn like the rest of the chrome: what
//! is missing, a button to Microsoft's download page, and Close. Then it leaves.

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT},
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
            LibraryLoader::GetModuleHandleW,
            Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        },
        UI::{
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN},
            Shell::ShellExecuteW,
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};

use super::gfx::{Align, Font, Gfx, R};
use super::theme::Theme;

/// Microsoft's page for the WebView2 Runtime; its Evergreen bootstrapper is the first download.
const GET_IT: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
const W: f32 = 520.0;
const H: f32 = 236.0;
const PAD: f32 = 28.0;

struct State {
    gfx: Option<Gfx>,
    theme: Theme,
    hover: Option<usize>,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

/// The two buttons, right-aligned at the foot: "Get WebView2" and "Close".
fn buttons() -> [R; 2] {
    let close = R::new(W - PAD - 84.0, H - PAD - 34.0, 84.0, 34.0);
    let get = R::new(close.x - 10.0 - 132.0, close.y, 132.0, 34.0);
    [get, close]
}

/// Dark when Windows' apps are dark. A debug build's shot can ask with LUMEN_TEST_DARK=0 or 1.
fn windows_dark() -> bool {
    if cfg!(debug_assertions)
        && let Some(v) = std::env::var_os("LUMEN_TEST_DARK")
    {
        return v == "1";
    }
    let mut v = 1u32;
    let mut n = 4u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as _),
            Some(&mut n),
        )
    };
    ok.is_ok() && v == 0
}

fn draw(g: &mut Gfx, th: &Theme, hover: Option<usize>, w: u32, h: u32) -> bool {
    if !g.begin(w, h, th.side) {
        return false;
    }
    g.text("LUMEN", R::new(PAD, PAD - 4.0, W - 2.0 * PAD, 14.0), Font::ui(10.5, 600), th.faint);
    g.text("Lumen needs WebView2", R::new(PAD, PAD + 16.0, W - 2.0 * PAD, 30.0), Font::ui(20.0, 650), th.text);
    g.text_wrap(
        "WebView2 is the part of Windows that draws web pages, and this PC does not have it. \
         Microsoft gives it away; the download takes a minute. Install it, then start Lumen again.",
        R::new(PAD, PAD + 56.0, W - 2.0 * PAD, 60.0),
        Font::ui(13.0, 450),
        th.dim,
    );
    let [get, close] = buttons();
    g.round(get, 17.0, if hover == Some(0) { th.pill.alpha(0.88) } else { th.pill });
    g.text("Get WebView2", get, Font::ui(13.0, 600).align(Align::Center), th.pill_ink);
    g.round(close, 17.0, if hover == Some(1) { th.hover } else { th.bg });
    g.round_stroke(close, 17.0, th.line, 1.0);
    g.text("Close", close, Font::ui(13.0, 500).align(Align::Center), th.text);
    true
}

fn get_it(hwnd: HWND) {
    unsafe {
        let url: Vec<u16> = GET_IT.encode_utf16().chain(Some(0)).collect();
        ShellExecuteW(None, w!("open"), PCWSTR(url.as_ptr()), None, None, SW_SHOWNORMAL);
        let _ = DestroyWindow(hwnd);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let k = GetDpiForWindow(hwnd) as f32 / 96.0;
        let at = |lp: LPARAM| ((lp.0 & 0xffff) as i16 as f32 / k, ((lp.0 >> 16) & 0xffff) as i16 as f32 / k);
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                STATE.with(|s| {
                    if let Some(st) = s.borrow_mut().as_mut()
                        && let Some(g) = st.gfx.as_mut()
                    {
                        if draw(g, &st.theme, st.hover, rc.right as u32, rc.bottom as u32) {
                            g.end();
                        }
                    }
                });
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_NCHITTEST => {
                // The whole window drags, except its buttons.
                let mut p = windows::Win32::Foundation::POINT { x: (lp.0 & 0xffff) as i16 as i32, y: ((lp.0 >> 16) & 0xffff) as i16 as i32 };
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
                let (x, y) = (p.x as f32 / k, p.y as f32 / k);
                if buttons().iter().any(|b| b.contains(x, y)) { LRESULT(HTCLIENT as isize) } else { LRESULT(HTCAPTION as isize) }
            }
            WM_MOUSEMOVE => {
                let (x, y) = at(lp);
                let hover = buttons().iter().position(|b| b.contains(x, y));
                STATE.with(|s| {
                    if let Some(st) = s.borrow_mut().as_mut()
                        && st.hover != hover
                    {
                        st.hover = hover;
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                });
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = at(lp);
                match buttons().iter().position(|b| b.contains(x, y)) {
                    Some(0) => get_it(hwnd),
                    Some(_) => {
                        let _ = DestroyWindow(hwnd);
                    }
                    None => {}
                }
                LRESULT(0)
            }
            WM_KEYDOWN if wp.0 as u16 == VK_RETURN.0 => {
                get_it(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN if wp.0 as u16 == VK_ESCAPE.0 => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Show the window and run it until it is closed. A debug build with `LUMEN_TEST_SHOT` set
/// writes `nowebview.png` there instead and returns.
pub fn show() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(hinst) = GetModuleHandleW(None) else { return };
        let class = w!("LumenNoWebView2");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(1 as _)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let k = windows::Win32::UI::HiDpi::GetDpiForSystem() as f32 / 96.0;
        let (w, h) = ((W * k) as i32, (H * k) as i32);
        let x = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - h) / 2;
        let shot = cfg!(debug_assertions).then(|| std::env::var_os("LUMEN_TEST_SHOT")).flatten();
        let style = if shot.is_some() { WS_POPUP } else { WS_POPUP | WS_VISIBLE };
        let Ok(hwnd) = CreateWindowExW(WS_EX_APPWINDOW, class, w!("Lumen"), style, x, y, w, h, None, None, Some(hinst.into()), None) else { return };
        let dark = windows_dark();
        let gfx = Gfx::new(hwnd, GetDpiForWindow(hwnd) as f32).ok();
        STATE.with(|s| *s.borrow_mut() = Some(State { gfx, theme: Theme::new("signal", dark, ""), hover: None }));
        #[cfg(debug_assertions)]
        if let Some(dir) = shot {
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut()
                    && let Some(g) = st.gfx.as_mut()
                    && g.begin_shot(w as u32, h as u32)
                {
                    let _ = draw(g, &st.theme, None, w as u32, h as u32);
                    let dir = std::path::PathBuf::from(dir);
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = g.end_shot(&dir.join("nowebview.png"), None);
                }
            });
            let _ = DestroyWindow(hwnd);
            return;
        }
        let _ = SetForegroundWindow(hwnd);
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}
