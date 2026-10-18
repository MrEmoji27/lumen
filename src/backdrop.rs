//! Windows' own frost behind the window, for the Glass look.
//!
//! Mica and Acrylic are drawn by DWM, from the desktop behind the window: the
//! blur happens in the compositor, so Lumen pays no blur pass, no second
//! surface and no memory of its own for it. Both need Windows 11 22H2
//! (build 22621), where DWM grew `DWMWA_SYSTEMBACKDROP_TYPE`. Older Windows
//! has no frost to put on, and `set` says so with `false`.
//!
//! What shows through is the window's own client area: an opaque background
//! hides the frost completely. Making the sidebar let it through is the
//! integrator's call, not this file's.

use std::{mem::size_of_val, ptr::from_ref, sync::OnceLock};
use windows::Win32::{
    Foundation::{ERROR_SUCCESS, HWND},
    Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMSBT_MAINWINDOW, DWMSBT_NONE,
        DWMSBT_TRANSIENTWINDOW,
    },
    System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW},
};
use windows::core::HSTRING;

/// What DWM puts behind the window's own content.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backdrop {
    None,
    Mica,
    Acrylic,
}

/// The first build of Windows 11 (22H2) that knows the backdrop attribute.
const FIRST_BUILD: u32 = 22621;

/// Put DWM's frost on a window, or take it off with `Backdrop::None`.
/// The tint is dark or light to match `dark`. True when Windows took it: on a
/// Windows older than 22H2 there is no frost to put and none to clear, so only
/// `Backdrop::None` counts as done.
pub fn set(hwnd: isize, b: Backdrop, dark: bool) -> bool {
    if hwnd == 0 {
        return false;
    }
    if !supported() {
        return b == Backdrop::None;
    }
    let hwnd = HWND(hwnd as _);
    unsafe {
        // The tint follows the window's own dark or light choice. Losing it
        // changes the colour of the frost, not the frost, so it never decides
        // the answer.
        if b != Backdrop::None {
            let on = dark as u32;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, from_ref(&on).cast(), size_of_val(&on) as u32);
        }
        let kind = match b {
            Backdrop::None => DWMSBT_NONE,
            Backdrop::Mica => DWMSBT_MAINWINDOW,
            Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
        };
        DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, from_ref(&kind).cast(), size_of_val(&kind) as u32).is_ok()
    }
}

/// Whether this machine's Windows knows DWM's backdrop attribute at all:
/// Windows 11 22H2 (22621) or newer. The build is read once.
pub fn supported() -> bool {
    ok_build(build_number())
}

/// The version parse behind `supported()`, kept apart so it can be tested.
/// A build number on its own, as the registry writes it. Anything else is
/// "not supported": an unreadable version must never pass for Windows 11.
fn ok_build(s: &str) -> bool {
    s.trim().parse::<u32>().is_ok_and(|b| b >= FIRST_BUILD)
}

/// The OS build number ("26300"), read once from the registry. Empty when the
/// read fails, which `ok_build` reads as no frost rather than as a yes.
fn build_number() -> &'static str {
    static BUILD: OnceLock<String> = OnceLock::new();
    BUILD.get_or_init(|| {
        let key = HSTRING::from(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        let name = HSTRING::from("CurrentBuildNumber");
        let mut bytes = 0u32;
        unsafe {
            if RegGetValueW(HKEY_LOCAL_MACHINE, &key, &name, RRF_RT_REG_SZ, None, None, Some(&mut bytes)) != ERROR_SUCCESS
                || bytes < 2
            {
                return String::new();
            }
            let mut buf = vec![0u16; bytes as usize / 2];
            let mut got = bytes;
            if RegGetValueW(HKEY_LOCAL_MACHINE, &key, &name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut got))
                != ERROR_SUCCESS
            {
                return String::new();
            }
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..n])
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_22h2_and_newer_have_the_backdrop() {
        assert!(ok_build("22621"));
        assert!(ok_build("26300"));
        // The registry hands over exactly the digits, but a stray newline or
        // space from whoever read it must not decide the answer.
        assert!(ok_build(" 26300\r\n"));
        // The last build without it, and Windows 10 before it.
        assert!(!ok_build("22620"));
        assert!(!ok_build("19045"));
        // Unreadable is never a yes.
        assert!(!ok_build(""));
        assert!(!ok_build("   "));
        assert!(!ok_build("build 26300"));
        assert!(!ok_build("10.0.22621.2428"));
        assert!(!ok_build("-1"));
        assert!(!ok_build("99999999999999999999"));
    }

    #[test]
    fn the_build_number_comes_from_the_registry() {
        // A failure reads as "" and would silently turn the frost off for the
        // whole run, so the read itself is checked on a real machine.
        assert!(!build_number().is_empty(), "no build number read from the registry");
        assert_eq!(supported(), ok_build(build_number()));
    }

    #[test]
    fn no_window_gets_no_frost() {
        assert!(!set(0, Backdrop::Acrylic, true));
        assert!(!set(0, Backdrop::None, false));
    }
}
