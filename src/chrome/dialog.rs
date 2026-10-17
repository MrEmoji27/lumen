//! A small native form: a few labelled text boxes, OK and Cancel. For the short answers the
//! chrome asks for (a bookmark's title and address, a folder's name, a tile). Modal: the
//! owner is disabled until the form is answered.

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{COLOR_BTNFACE, HBRUSH, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DeleteObject, FF_DONTCARE, FW_NORMAL, OUT_DEFAULT_PRECIS},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::EM_SETSEL,
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus, VK_ESCAPE, VK_RETURN},
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

/// 1 and 2 are the ids Windows itself uses for Enter and Escape.
const ID_OK: usize = 1;
const ID_CANCEL: usize = 2;
/// The text boxes are 101, 102, ...
const ID_FIRST: usize = 101;

thread_local! {
    static GOT: std::cell::RefCell<Option<Vec<String>>> = const { std::cell::RefCell::new(None) };
    static COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = wp.0 & 0xFFFF;
                if id == ID_OK {
                    let all = (0..COUNT.get()).map(|k| super::wide_text(GetDlgItem(Some(hwnd), (ID_FIRST + k) as i32).unwrap_or_default())).collect();
                    GOT.with(|g| *g.borrow_mut() = Some(all));
                }
                if id == ID_OK || id == ID_CANCEL {
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

/// Ask for `fields` (label, starting text). The answers in the same order, or None when
/// cancelled. The first box has the focus and its text selected.
pub fn ask_fields(owner: HWND, title: &str, fields: &[(&str, &str)]) -> Option<Vec<String>> {
    GOT.with(|g| *g.borrow_mut() = None);
    COUNT.set(fields.len());
    let class = w!("LumenForm");
    let hinst: HINSTANCE = unsafe { GetModuleHandleW(None) }.map(|h| h.into()).unwrap_or_default();
    let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc), hInstance: hinst, lpszClassName: class, ..Default::default() };
    unsafe {
        // Registered already on a second use: the class is the same.
        let _ = RegisterClassExW(&wc);
        let mut owner_r = RECT::default();
        let _ = GetWindowRect(owner, &mut owner_r);
        let k = GetDpiForWindow(owner).max(96) as f32 / 96.0;
        let row = 52.0;
        let (wpx, hpx) = ((420.0 * k) as i32, ((fields.len() as f32 * row + 98.0) * k) as i32);
        let x = (owner_r.left + (owner_r.right - owner_r.left - wpx) / 2).max(0);
        let y = (owner_r.top + (owner_r.bottom - owner_r.top - hpx) / 2).max(0);
        let title_w: Vec<u16> = format!("{title}\0").encode_utf16().collect();
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            class,
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            x,
            y,
            wpx,
            hpx,
            Some(owner),
            None,
            Some(hinst),
            None,
        ) else {
            return None;
        };
        let face = HSTRING::from("Segoe UI");
        let font = CreateFontW(
            -(13.0 * k).round() as i32,
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
        let px = |v: f32| (v * k) as i32;
        let mut first = HWND::default();
        for (n, (label, text)) in fields.iter().enumerate() {
            let top = 12.0 + n as f32 * row;
            let lab: Vec<u16> = format!("{label}\0").encode_utf16().collect();
            let caption = CreateWindowExW(WS_EX_LEFT, w!("STATIC"), PCWSTR(lab.as_ptr()), WS_CHILD | WS_VISIBLE, px(12.0), px(top), wpx - px(24.0), px(18.0), Some(hwnd), None, Some(hinst), None)
                .unwrap_or_default();
            let edit = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                w!("EDIT"),
                &HSTRING::from(*text),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                px(12.0),
                px(top + 20.0),
                wpx - px(24.0),
                px(24.0),
                Some(hwnd),
                Some(HMENU((ID_FIRST + n) as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let _ = SendMessageW(caption, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            let _ = SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
            if n == 0 {
                first = edit;
            }
        }
        let ok: Vec<u16> = "OK\0".encode_utf16().collect();
        let cancel: Vec<u16> = "Cancel\0".encode_utf16().collect();
        let by = hpx - px(78.0);
        for (id, text, def) in [(ID_OK, ok.as_slice(), true), (ID_CANCEL, cancel.as_slice(), false)] {
            let b = CreateWindowExW(
                WS_EX_LEFT,
                w!("BUTTON"),
                PCWSTR(text.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE((if def { BS_DEFPUSHBUTTON } else { BS_PUSHBUTTON }) as u32),
                wpx - px(24.0) - px(196.0) + if def { px(100.0) } else { 0 },
                by,
                px(96.0),
                px(28.0),
                Some(hwnd),
                Some(HMENU(id as _)),
                Some(hinst),
                None,
            )
            .unwrap_or_default();
            let _ = SendMessageW(b, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        }
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(first));
        let _ = SendMessageW(first, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        let _ = EnableWindow(owner, false);
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            // Not a real dialog box: Enter and Escape are read here, before an edit box beeps.
            if m.message == WM_KEYDOWN {
                let key = m.wParam.0 as u16;
                if key == VK_RETURN.0 {
                    let all = (0..fields.len()).map(|n| super::wide_text(GetDlgItem(Some(hwnd), (ID_FIRST + n) as i32).unwrap_or_default())).collect();
                    GOT.with(|g| *g.borrow_mut() = Some(all));
                    let _ = DestroyWindow(hwnd);
                    continue;
                }
                if key == VK_ESCAPE.0 {
                    let _ = DestroyWindow(hwnd);
                    continue;
                }
            }
            if IsDialogMessageW(hwnd, &m).as_bool() {
                continue;
            }
            let _ = TranslateMessage(&m);
            let _ = DispatchMessageW(&m);
        }
        let _ = EnableWindow(owner, true);
        let _ = SetForegroundWindow(owner);
        if !font.is_invalid() {
            let _ = DeleteObject(font.into());
        }
    }
    GOT.with(|g| g.borrow_mut().take())
}

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
