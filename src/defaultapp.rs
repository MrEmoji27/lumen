//! Lumen as a browser Windows knows about.

use windows::{
    Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{
            HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
        },
    },
    core::{HSTRING, PCWSTR},
};

pub const PROG_ID: &str = "LumenHTML";
const APP_NAME: &str = "Lumen";
const DESCRIPTION: &str = "A small, fast browser for Windows.";

const URL_SCHEMES: &[&str] = &["http", "https"];
const FILE_TYPES: &[&str] = &[".htm", ".html", ".shtml", ".xht", ".xhtml", ".svg", ".pdf", ".webp"];

fn choice_key(root: &str, scheme: &str) -> String {
    format!(r"{root}\Microsoft\Windows\Shell\Associations\UrlAssociations\{scheme}\UserChoice")
}

#[derive(Clone, Debug)]
pub struct Reg {
    root: String,
    exe: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Default,
    Listed,
    Absent,
}

impl Reg {
    pub fn current() -> Option<Reg> {
        let exe = std::env::current_exe().ok()?.to_string_lossy().into_owned();
        let root = if cfg!(debug_assertions) {
            std::env::var("LUMEN_REG_ROOT").ok().filter(|r| r.to_ascii_lowercase().starts_with(r"software\lumentest")).unwrap_or_else(|| "Software".into())
        } else {
            "Software".into()
        };
        Some(Reg { root, exe })
    }

    #[cfg(test)]
    fn scratch(root: &str, exe: &str) -> Reg {
        assert!(root.starts_with(r"Software\LumenTest"), "tests only write under Software\\LumenTest");
        Reg { root: root.into(), exe: exe.into() }
    }

    fn real(&self) -> bool {
        self.root == "Software"
    }

    fn progid_key(&self) -> String {
        format!(r"{}\Classes\{PROG_ID}", self.root)
    }

    fn client_key(&self) -> String {
        format!(r"{}\Clients\StartMenuInternet\{APP_NAME}", self.root)
    }

    fn caps_key(&self) -> String {
        format!(r"{}\Capabilities", self.client_key())
    }

    fn apps_key(&self) -> String {
        format!(r"{}\RegisteredApplications", self.root)
    }

    fn open_command(&self) -> String {
        format!("\"{}\" --single-argument %1", self.exe)
    }

    fn icon(&self) -> String {
        format!("\"{}\",0", self.exe)
    }

    pub fn write(&self) -> Result<(), String> {
        let (pid, client, caps) = (self.progid_key(), self.client_key(), self.caps_key());
        let icon = self.icon();
        let mut rows: Vec<(String, &str, String)> = vec![
            (pid.clone(), "", "Lumen HTML Document".into()),
            (format!(r"{pid}\DefaultIcon"), "", icon.clone()),
            (format!(r"{pid}\shell\open\command"), "", self.open_command()),
            (format!(r"{pid}\Application"), "ApplicationName", APP_NAME.into()),
            (format!(r"{pid}\Application"), "ApplicationDescription", DESCRIPTION.into()),
            (format!(r"{pid}\Application"), "ApplicationIcon", icon.clone()),
            (client.clone(), "", APP_NAME.into()),
            (format!(r"{client}\DefaultIcon"), "", icon.clone()),
            (format!(r"{client}\shell\open\command"), "", format!("\"{}\"", self.exe)),
            (caps.clone(), "ApplicationName", APP_NAME.into()),
            (caps.clone(), "ApplicationDescription", DESCRIPTION.into()),
            (caps.clone(), "ApplicationIcon", icon),
        ];
        for s in URL_SCHEMES {
            rows.push((format!(r"{caps}\URLAssociations"), s, PROG_ID.into()));
        }
        for t in FILE_TYPES {
            rows.push((format!(r"{caps}\FileAssociations"), t, PROG_ID.into()));
        }
        rows.push((self.apps_key(), APP_NAME, caps));
        for (key, name, value) in &rows {
            set_str(key, name, value).map_err(|e| format!("could not write {key}: error {e}"))?;
        }
        self.changed();
        Ok(())
    }

    pub fn remove(&self) {
        delete_tree(&self.progid_key());
        delete_tree(&self.client_key());
        delete_value(&self.apps_key(), APP_NAME);
        self.changed();
    }

    fn changed(&self) {
        if !self.real() {
            return;
        }
        use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }

    pub fn registered_exe(&self) -> Option<String> {
        let cmd = get_str(&format!(r"{}\shell\open\command", self.progid_key()), "")?;
        exe_of_command(&cmd)
    }

    pub fn is_current(&self) -> bool {
        get_str(&format!(r"{}\shell\open\command", self.progid_key()), "").as_deref() == Some(self.open_command().as_str())
            && get_str(&self.apps_key(), APP_NAME).as_deref() == Some(self.caps_key().as_str())
    }

    pub fn ensure(&self) -> Result<bool, String> {
        if self.is_current() {
            return Ok(false);
        }
        self.write().map(|_| true)
    }

    pub fn follow_exe(&self) -> bool {
        match self.registered_exe() {
            Some(old) if !same_path(&old, &self.exe) => self.write().is_ok(),
            _ => false,
        }
    }

    pub fn default_progid(&self) -> Option<String> {
        get_str(&choice_key(&self.root, "https"), "ProgId")
    }

    pub fn status(&self) -> Status {
        if self.default_progid().as_deref() == Some(PROG_ID) {
            Status::Default
        } else if self.registered_exe().is_some() {
            Status::Listed
        } else {
            Status::Absent
        }
    }
}

fn exe_of_command(cmd: &str) -> Option<String> {
    let c = cmd.trim();
    let path = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => c.split_whitespace().next()?,
    };
    (!path.is_empty()).then(|| path.to_string())
}

fn same_path(a: &str, b: &str) -> bool {
    a.replace('/', "\\").eq_ignore_ascii_case(&b.replace('/', "\\"))
}

pub fn status() -> Status {
    use std::{cell::Cell, time::Instant};
    thread_local! {
        static CACHE: Cell<Option<(Instant, Status)>> = const { Cell::new(None) };
    }
    if let Some((at, s)) = CACHE.get()
        && at.elapsed().as_millis() < 800
    {
        return s;
    }
    let s = Reg::current().map_or(Status::Absent, |r| r.status());
    CACHE.set(Some((Instant::now(), s)));
    s
}

pub fn make_default() -> String {
    let Some(reg) = Reg::current() else { return "Could not find the Lumen exe".into() };
    if let Err(e) = reg.ensure() {
        return e;
    }
    if reg.real() {
        open_default_apps();
    }
    "Pick Lumen in the Windows list".into()
}

pub fn unregister() -> String {
    match Reg::current() {
        Some(reg) => {
            reg.remove();
            "Lumen's entries are removed".into()
        }
        None => "Could not find the Lumen exe".into(),
    }
}

pub fn open_default_apps() {
    use windows::{
        Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        core::w,
    };
    let uri = HSTRING::from(format!("ms-settings:defaultapps?registeredAppUser={APP_NAME}"));
    unsafe {
        let _ = ShellExecuteW(None, w!("open"), &uri, None, None, SW_SHOWNORMAL);
    }
}

pub fn startup_check() {
    if cfg!(debug_assertions) && std::env::var_os("LUMEN_REG_ROOT").is_none() {
        return;
    }
    if crate::headless() {
        return;
    }
    if let Some(reg) = Reg::current() {
        reg.follow_exe();
    }
}

fn wide(s: &str) -> HSTRING {
    HSTRING::from(s)
}

fn set_str(key: &str, name: &str, value: &str) -> Result<(), u32> {
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let name = (!name.is_empty()).then(|| wide(name));
    let r = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &wide(key),
            name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    if r == ERROR_SUCCESS { Ok(()) } else { Err(r.0) }
}

fn get_str(key: &str, name: &str) -> Option<String> {
    let (key, name) = (wide(key), (!name.is_empty()).then(|| wide(name)));
    let name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let mut bytes = 0u32;
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, &key, name, RRF_RT_REG_SZ, None, None, Some(&mut bytes)) != ERROR_SUCCESS || bytes < 2 {
            return None;
        }
        let mut buf = vec![0u16; bytes as usize / 2 + 1];
        let mut got = (buf.len() * 2) as u32;
        if RegGetValueW(HKEY_CURRENT_USER, &key, name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut got)) != ERROR_SUCCESS {
            return None;
        }
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..n]))
    }
}

fn delete_tree(key: &str) {
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &wide(key));
    }
}

fn delete_value(key: &str, name: &str) {
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &wide(key), &wide(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{
        Foundation::ERROR_NO_MORE_ITEMS,
        System::Registry::{HKEY, KEY_READ, RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW},
    };

    const EXE: &str = r"C:\Program Files\Lumen\lumen.exe";

    struct Scratch(String);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let root = format!(r"Software\LumenTest\{name}-{}", std::process::id());
            delete_tree(&root);
            Scratch(root)
        }

        fn reg(&self, exe: &str) -> Reg {
            Reg::scratch(&self.0, exe)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            delete_tree(&self.0);
            unsafe {
                let _ = windows::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, &wide(r"Software\LumenTest"));
            }
        }
    }

    fn dump(key: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut h = HKEY::default();
        unsafe {
            if RegOpenKeyExW(HKEY_CURRENT_USER, &wide(key), None, KEY_READ, &mut h) != ERROR_SUCCESS {
                return out;
            }
            let mut i = 0;
            loop {
                let mut name = [0u16; 256];
                let mut n = name.len() as u32;
                if RegEnumKeyExW(h, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut n, None, None, None, None) == ERROR_NO_MORE_ITEMS {
                    break;
                }
                let sub = format!(r"{key}\{}", String::from_utf16_lossy(&name[..n as usize]));
                out.push(sub.clone());
                out.extend(dump(&sub));
                i += 1;
            }
            let mut i = 0;
            loop {
                let mut name = [0u16; 256];
                let mut n = name.len() as u32;
                if RegEnumValueW(h, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut n, None, None, None, None) == ERROR_NO_MORE_ITEMS {
                    break;
                }
                let v = String::from_utf16_lossy(&name[..n as usize]);
                out.push(format!(r"{key} = {}", if v.is_empty() { "(default)" } else { &v }));
                i += 1;
            }
            let _ = RegCloseKey(h);
        }
        out
    }

    #[test]
    fn writes_what_windows_needs_and_reads_it_back() {
        let s = Scratch::new("write");
        let r = s.reg(EXE);
        assert_eq!(r.status(), Status::Absent);
        r.write().unwrap();
        let root = &s.0;
        assert_eq!(
            get_str(&format!(r"{root}\Classes\LumenHTML\shell\open\command"), "").as_deref(),
            Some(r#""C:\Program Files\Lumen\lumen.exe" --single-argument %1"#)
        );
        assert_eq!(get_str(&format!(r"{root}\Classes\LumenHTML\DefaultIcon"), "").as_deref(), Some(r#""C:\Program Files\Lumen\lumen.exe",0"#));
        let caps = format!(r"{root}\Clients\StartMenuInternet\Lumen\Capabilities");
        assert_eq!(get_str(&caps, "ApplicationName").as_deref(), Some("Lumen"));
        assert!(get_str(&caps, "ApplicationDescription").is_some());
        assert_eq!(get_str(&caps, "ApplicationIcon").as_deref(), Some(r#""C:\Program Files\Lumen\lumen.exe",0"#));
        for scheme in ["http", "https"] {
            assert_eq!(get_str(&format!(r"{caps}\URLAssociations"), scheme).as_deref(), Some("LumenHTML"));
        }
        for ext in [".htm", ".html", ".shtml", ".xht", ".xhtml", ".svg", ".pdf", ".webp"] {
            assert_eq!(get_str(&format!(r"{caps}\FileAssociations"), ext).as_deref(), Some("LumenHTML"), "{ext}");
        }
        assert_eq!(get_str(&format!(r"{root}\RegisteredApplications"), "Lumen").as_deref(), Some(caps.as_str()));
        assert_eq!(r.registered_exe().as_deref(), Some(EXE));
        assert!(r.is_current());
        assert_eq!(r.status(), Status::Listed);
    }

    #[test]
    fn unregister_leaves_nothing_and_spares_the_neighbours() {
        let s = Scratch::new("remove");
        let r = s.reg(EXE);
        set_str(&format!(r"{}\RegisteredApplications", s.0), "Other", r"Software\Clients\StartMenuInternet\Other\Capabilities").unwrap();
        set_str(&format!(r"{}\Classes\OtherHTML", s.0), "", "Other").unwrap();
        set_str(&format!(r"{}\Clients\StartMenuInternet\Other", s.0), "", "Other").unwrap();
        let before = dump(&s.0);
        r.write().unwrap();
        assert!(dump(&s.0).len() > before.len());
        r.remove();
        assert_eq!(dump(&s.0), before, "unregister must leave exactly what was there before");
        assert_eq!(r.registered_exe(), None);
        assert_eq!(r.status(), Status::Absent);
    }

    #[test]
    fn unregister_on_a_clean_root_leaves_no_values() {
        let s = Scratch::new("clean");
        let r = s.reg(EXE);
        r.write().unwrap();
        r.remove();
        let left = dump(&s.0);
        assert!(left.iter().all(|l| !l.contains(" = ")), "values left behind: {left:?}");
        assert!(left.iter().all(|l| !l.contains("LumenHTML") && !l.contains(r"StartMenuInternet\Lumen")), "{left:?}");
    }

    #[test]
    fn writing_twice_is_the_same_as_once() {
        let s = Scratch::new("twice");
        let r = s.reg(EXE);
        assert_eq!(r.ensure(), Ok(true));
        let once = dump(&s.0);
        assert_eq!(r.ensure(), Ok(false));
        r.write().unwrap();
        assert_eq!(dump(&s.0), once);
    }

    #[test]
    fn ensure_rewrites_when_pointing_at_another_exe() {
        let s = Scratch::new("moved");
        s.reg(r"D:\old\lumen.exe").write().unwrap();
        let now = s.reg(EXE);
        assert!(!now.is_current());
        assert_eq!(now.ensure(), Ok(true));
        assert_eq!(now.registered_exe().as_deref(), Some(EXE));
    }

    #[test]
    fn follow_exe_only_follows_a_registration_that_exists() {
        let s = Scratch::new("follow");
        let now = s.reg(EXE);
        assert!(!now.follow_exe());
        assert_eq!(now.status(), Status::Absent);
        s.reg(r"D:\old\lumen.exe").write().unwrap();
        assert!(now.follow_exe());
        assert_eq!(now.registered_exe().as_deref(), Some(EXE));
        assert!(!s.reg(&EXE.to_uppercase()).follow_exe());
    }

    #[test]
    fn default_state_reads_the_users_choice() {
        let s = Scratch::new("choice");
        let r = s.reg(EXE);
        r.write().unwrap();
        assert_eq!(r.status(), Status::Listed);
        let key = choice_key(&s.0, "https");
        set_str(&key, "ProgId", "ChromeHTML").unwrap();
        assert_eq!(r.default_progid().as_deref(), Some("ChromeHTML"));
        assert_eq!(r.status(), Status::Listed);
        set_str(&key, "ProgId", "LumenHTML").unwrap();
        assert_eq!(r.status(), Status::Default);
        r.remove();
        assert_eq!(r.status(), Status::Default);
    }

    #[test]
    fn exe_paths_come_out_of_commands() {
        assert_eq!(exe_of_command(r#""C:\a b\lumen.exe" --single-argument %1"#).as_deref(), Some(r"C:\a b\lumen.exe"));
        assert_eq!(exe_of_command(r"C:\lumen.exe %1").as_deref(), Some(r"C:\lumen.exe"));
        assert_eq!(exe_of_command(""), None);
        assert!(same_path(r"C:\A\Lumen.exe", "c:/a/lumen.exe"));
    }

    #[test]
    fn scratch_roots_are_never_real() {
        let s = Scratch::new("real");
        assert!(!s.reg(EXE).real());
        assert!(Reg { root: "Software".into(), exe: EXE.into() }.real());
    }
}
