//! Windows' own frost behind the window, for the Glass look.

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backdrop {
    None,
    Mica,
    Acrylic,
}

const FIRST_BUILD: u32 = 22621;

pub fn set(hwnd: isize, b: Backdrop, dark: bool) -> bool {
    if hwnd == 0 {
        return false;
    }
    if !supported() {
        return b == Backdrop::None;
    }
    let hwnd = HWND(hwnd as _);
    unsafe {
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

pub fn supported() -> bool {
    ok_build(build_number())
}

fn ok_build(s: &str) -> bool {
    s.trim().parse::<u32>().is_ok_and(|b| b >= FIRST_BUILD)
}

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
        assert!(ok_build(" 26300\r\n"));
        assert!(!ok_build("22620"));
        assert!(!ok_build("19045"));
        assert!(!ok_build(""));
        assert!(!ok_build("   "));
        assert!(!ok_build("build 26300"));
        assert!(!ok_build("10.0.22621.2428"));
        assert!(!ok_build("-1"));
        assert!(!ok_build("99999999999999999999"));
    }

    #[test]
    fn the_build_number_comes_from_the_registry() {
        assert!(!build_number().is_empty(), "no build number read from the registry");
        assert_eq!(supported(), ok_build(build_number()));
    }

    #[test]
    fn no_window_gets_no_frost() {
        assert!(!set(0, Backdrop::Acrylic, true));
        assert!(!set(0, Backdrop::None, false));
    }
}
