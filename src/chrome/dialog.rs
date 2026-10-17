//! A small native form: a few labelled text boxes, OK and Cancel. For the short answers the
//! chrome asks for (a bookmark's title and address, a folder's name, a tile). Modal: the
//! owner is disabled until the form is answered.

use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, DeleteObject, FF_DONTCARE, FW_NORMAL, OUT_DEFAULT_PRECIS},
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
