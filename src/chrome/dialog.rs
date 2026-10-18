//! The one native box still in use: a question with a few buttons (the privacy report's
//! reset, an external link asking). Everything else is drawn by the chrome.

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{COLOR_BTNFACE, HBRUSH, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DeleteObject, FF_DONTCARE, FW_NORMAL, OUT_DEFAULT_PRECIS},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE},
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

// ------------------------------------------------------------------- a choice

const ID_PICK: usize = 201;

thread_local! {
    static PICKED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

unsafe extern "system" fn choice_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = wp.0 & 0xFFFF;
                if id >= ID_PICK {
                    PICKED.set(Some(id - ID_PICK));
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_CLOSE => {
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

/// A question with a few buttons: which one was pressed (the first is the default), or None
/// for Escape or the close box. Blocks in a message loop of its own, so run it on a thread of
/// its own. It is not owned by the browser window: that one keeps working while it is open.
/// `near` is a window to sit in the middle of (raw handle, 0 for the screen).
pub fn choose(title: &str, text: &str, buttons: &[&str], near: isize) -> Option<usize> {
    PICKED.set(None);
    let class = w!("LumenChoice");
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None) }.map(|h| h.into()).unwrap_or_default();
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(choice_proc),
        hInstance: hinst,
        lpszClassName: class,
        hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as _),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
        ..Default::default()
    };
    unsafe {
        let _ = RegisterClassExW(&wc);
        let anchor = HWND(near as _);
        let mut at = RECT::default();
        let have = near != 0 && GetWindowRect(anchor, &mut at).is_ok();
        if !have {
            at = RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) };
        }
        let k = if have { GetDpiForWindow(anchor).max(96) } else { 96 } as f32 / 96.0;
        let px = |v: f32| (v * k) as i32;
        let bw: Vec<i32> = buttons.iter().map(|b| px(28.0 + b.chars().count() as f32 * 7.0).max(px(84.0))).collect();
        let row: i32 = bw.iter().sum::<i32>() + px(8.0) * (buttons.len() as i32 - 1);
        let (wpx, hpx) = ((px(36.0) + row.max(px(380.0))).min(px(640.0)), px(168.0));
        let x = (at.left + (at.right - at.left - wpx) / 2).max(0);
        let y = (at.top + (at.bottom - at.top - hpx) / 2).max(0);
        let title_w: Vec<u16> = format!("{title}\0").encode_utf16().collect();
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT | WS_EX_TOPMOST,
            class,
            PCWSTR(title_w.as_ptr()),
            WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            x,
            y,
            wpx,
            hpx,
            None,
            None,
            Some(hinst),
            None,
        ) else {
            return None;
        };
        let face = HSTRING::from("Segoe UI");
        let font = CreateFontW(
            -(14.0 * k).round() as i32,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            &face,
        );
        let say = CreateWindowExW(WS_EX_LEFT, w!("STATIC"), &HSTRING::from(text), WS_CHILD | WS_VISIBLE, px(18.0), px(18.0), wpx - px(36.0), px(48.0), Some(hwnd), None, Some(hinst), None)
            .unwrap_or_default();
        let _ = SendMessageW(say, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        // The buttons sit at the right edge, the first (the default) leftmost.
        let mut bx = wpx - px(18.0) - px(16.0) - row;
        let by = px(86.0);
        for (n, label) in buttons.iter().enumerate() {
            let b = CreateWindowExW(
                WS_EX_LEFT,
                w!("BUTTON"),
                &HSTRING::from(*label),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE((if n == 0 { BS_DEFPUSHBUTTON } else { BS_PUSHBUTTON }) as u32),
                bx,
                by,
                bw[n],
                px(30.0),
                Some(hwnd),
                Some(HMENU((ID_PICK + n) as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let _ = SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            bx += bw[n] + px(8.0);
        }
        let _ = SetForegroundWindow(hwnd);
        if let Ok(first) = GetDlgItem(Some(hwnd), ID_PICK as i32) {
            let _ = SetFocus(Some(first));
        }
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            if m.message == WM_KEYDOWN && m.wParam.0 as u16 == VK_ESCAPE.0 {
                let _ = DestroyWindow(hwnd);
                continue;
            }
            if IsDialogMessageW(hwnd, &m).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&m);
            let _ = DispatchMessageW(&m);
        }
        if !font.is_invalid() {
            let _ = DeleteObject(font.into());
        }
    }
    PICKED.take()
}
