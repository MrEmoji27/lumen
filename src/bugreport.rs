//! A problem report the person reads in full before it goes anywhere.
//!
//! The report must never carry a URL, a host, a page title, a file path under
//! the person's folder or anything typed into a page. So the journal lines are
//! filtered down to the kinds and keys that are facts about Lumen, every message
//! is scrubbed of links and paths, and the crash log is cut to its last lines.
//!
//! Plain values and plain text. The window that shows the report, and whatever
//! sends it, live in main.rs.


use std::{fs, path::Path};

use serde_json::Value;
use windows::Win32::{
    Foundation::ERROR_SUCCESS,
    System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW},
};
use windows::core::HSTRING;

/// What the report knows about this run, as plain strings the window fills in.
#[derive(Debug, Clone)]
pub struct Facts {
    /// "0.6.3".
    pub version: String,
    /// "abc1234 2026-10-18": the commit and its date.
    pub build: String,
    /// "Windows 11 (build 26300)".
    pub os: String,
    /// The WebView2 runtime version, or "" when none has started.
    pub webview: String,
    /// The look's name: "signal", "glass", "precise".
    pub look: String,
    /// "vertical", "horizontal" or "hybrid".
    pub layout: String,
    /// "dark" or "light".
    pub theme: String,
    /// The ad and tracker blocker is on.
    pub blocker: bool,
    /// The agent door is open.
    pub agent_door: bool,
    /// Seconds the window has been up.
    pub uptime_s: u64,
    /// Tabs in the window.
    pub tabs: usize,
    /// The process tree's memory in MB, when it has been measured.
    pub mem_mb: Option<u64>,
}

/// The Windows version, from the registry: "Windows 11 Pro 24H2 (build 26300)".
/// Anything that will not read comes back as plain "Windows".
pub fn os_version() -> String {
    os_text(reg_sz("ProductName").trim(), reg_sz("DisplayVersion").trim(), reg_sz("CurrentBuild").trim())
}

/// The header's OS text, from the three registry values. Kept apart from the
/// read so the Windows 11 rule can be tested: ProductName says "Windows 10"
/// even on Windows 11, and the build (22000 and up) is the truth.
fn os_text(product: &str, display: &str, build: &str) -> String {
    let number: u32 = build.parse().unwrap_or(0);
    let mut text = if product.is_empty() { "Windows".to_string() } else { product.to_string() };
    if number >= 22_000 {
        text = text.replace("Windows 10", "Windows 11");
    }
    if !display.is_empty() {
        text.push(' ');
        text.push_str(display);
    }
    if !build.is_empty() {
        text.push_str(&format!(" (build {build})"));
    }
    text
}

/// One REG_SZ under HKLM's CurrentVersion key. "" when it will not read; the
/// report is not worth a failure.
fn reg_sz(name: &str) -> String {
    let key = HSTRING::from(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let name = HSTRING::from(name);
    unsafe {
        let mut bytes = 0u32;
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
}

/// The kinds a report keeps. A visit is not one of them: the report says what
/// Lumen did, never where the person went.
const KINDS: &[&str] = &["start", "exit", "unclean_exit", "crash", "panic", "error", "ui_first", "ui_slow"];

/// The keys a kept line keeps, in this order, when present. Everything else a
/// line carries — a host, a title, a query — is dropped.
const KEYS: &[&str] = &["k", "v", "clean", "uptime", "n", "worst", "ms", "f", "m", "headless", "reload"];

/// The newest `days` day files of the journal, oldest first, cut to the facts:
/// "10-18 16:11:09 start v=0.6.2 headless=false", in local time. The last
/// `max_lines` lines are kept; a folder that is not there says "".
pub fn journal_digest(dir: &Path, days: usize, max_lines: usize) -> String {
    let Ok(entries) = fs::read_dir(dir) else { return String::new() };
    let mut names: Vec<String> =
        entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|name| day_file(name)).collect();
    names.sort();
    let mut lines: Vec<String> = Vec::new();
    for name in names.iter().skip(names.len().saturating_sub(days)) {
        let Ok(text) = fs::read_to_string(dir.join(name)) else { continue };
        for raw in text.lines() {
            let Ok(value) = serde_json::from_str::<Value>(raw) else { continue };
            let Some(kind) = value.get("k").and_then(Value::as_str).filter(|k| KINDS.contains(k)) else { continue };
            lines.push(line_text(&value, kind));
        }
    }
    if lines.len() > max_lines {
        lines.drain(..lines.len() - max_lines);
    }
    lines.join("\n")
}

/// The last `lines` lines of a crash log, each scrubbed the same way, blank
/// lines left out. A file that is not there says "".
pub fn crash_tail(path: &Path, lines: usize) -> String {
    let Ok(text) = fs::read_to_string(path) else { return String::new() };
    let all: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    all[all.len().saturating_sub(lines)..].iter().map(|line| scrub(line)).collect::<Vec<_>>().join("\n")
}

/// The whole report, as the person reads it before it goes anywhere. Sections
/// with nothing in them say "none".
pub fn build(f: &Facts, journal: &str, crash: &str) -> String {
    let mut out = String::from("Lumen problem report\nVersion ");
    out.push_str(&f.version);
    if !f.build.trim().is_empty() {
        out.push_str(&format!(" ({})", f.build.trim()));
    }
    if !f.os.trim().is_empty() {
        out.push_str(&format!(" · {}", f.os.trim()));
    }
    if !f.webview.trim().is_empty() {
        out.push_str(&format!(" · WebView2 {}", f.webview.trim()));
    }
    out.push_str("\n\nWhat happened:\n");
    out.push_str("(say what you did, what you expected, and what Lumen did instead)\n\n");
    out.push_str(&format!(
        "Setup: look {} · layout {} · theme {} · blocker {} · agent door {}\n",
        f.look,
        f.layout,
        f.theme,
        on(f.blocker),
        on(f.agent_door)
    ));
    out.push_str(&format!("Running: {} {}", f.tabs, if f.tabs == 1 { "tab" } else { "tabs" }));
    if let Some(mb) = f.mem_mb {
        out.push_str(&format!(" · {mb} MB"));
    }
    out.push_str(&format!(" · up {}\n\n", uptime(f.uptime_s)));
    out.push_str("Recent journal (no addresses, no page content, local time):\n");
    out.push_str(section(journal));
    out.push_str("\n\nLast crash log lines:\n");
    out.push_str(section(crash));
    out.push('\n');
    out
}

/// A section with nothing in it says "none".
fn section(text: &str) -> &str {
    if text.trim().is_empty() { "none" } else { text }
}

fn on(yes: bool) -> &'static str {
    if yes { "on" } else { "off" }
}

/// "1 h 12 min", "12 min", "40 s": how long the window has been up.
fn uptime(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 { format!("{h} h {m} min") } else if m > 0 { format!("{m} min") } else { format!("{s} s") }
}

/// One line of a day file as the report writes it: the time, the kind, then the
/// kept keys in order. `m` is scrubbed like everything else that leaves here.
fn line_text(value: &Value, kind: &str) -> String {
    let mut out = String::new();
    if let Some(t) = value.get("t").and_then(Value::as_u64) {
        out.push_str(&stamp(t));
        out.push(' ');
    }
    out.push_str(kind);
    for key in KEYS {
        if *key == "k" {
            continue;
        }
        let Some(field) = value.get(*key).filter(|field| !field.is_null()) else { continue };
        out.push(' ');
        out.push_str(key);
        out.push('=');
        match field {
            Value::String(text) if *key == "m" => out.push_str(&scrub(text)),
            Value::String(text) => out.push_str(text),
            other => out.push_str(&other.to_string()),
        }
    }
    out
}

/// "MM-DD HH:MM:SS" in the PC's own time zone, as the person's clock read then. Windows
/// converts; should it refuse, the UTC reading is used.
fn stamp(ms: u64) -> String {
    use windows::Win32::{
        Foundation::{FILETIME, SYSTEMTIME},
        System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
    };
    let ft100 = (ms / 1000 + 11_644_473_600) * 10_000_000 + (ms % 1000) * 10_000;
    let ft = FILETIME { dwLowDateTime: ft100 as u32, dwHighDateTime: (ft100 >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    let ok = unsafe { FileTimeToSystemTime(&ft, &mut utc).is_ok() && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok() };
    if !ok {
        return stamp_utc(ms);
    }
    format!("{:02}-{:02} {:02}:{:02}:{:02}", local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond)
}

/// "MM-DD HH:MM:SS", UTC, computed by hand.
fn stamp_utc(ms: u64) -> String {
    let secs = ms / 1000;
    let (_, m, d) = civil_from_days((secs / 86_400) as i64);
    let tod = secs % 86_400;
    format!("{:02}-{:02} {:02}:{:02}:{:02}", m, d, tod / 3600, tod % 3600 / 60, tod % 60)
}

/// The civil date of days since 1970-01-01. Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A URL or a Windows path becomes "…". The report may not carry an address, a
/// host or a place on disk, and the only way to be sure of that is to keep none
/// of it. Newlines become spaces: one line stays one line.
fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((at, len)) = next_private(rest) {
        out.push_str(&rest[..at]);
        out.push('…');
        rest = &rest[at + len..];
    }
    out.push_str(rest);
    out.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// The start and length of the next URL or Windows path in `s`, if any. A URL
/// runs to the whitespace or the closing mark after it; so does a path. Nothing
/// that looks like one is ever kept, host included.
fn next_private(s: &str) -> Option<(usize, usize)> {
    let bytes = s.as_bytes();
    for (i, _) in s.char_indices() {
        let rest = &s[i..];
        let link = rest.starts_with("http://") || rest.starts_with("https://");
        // "C:\dir", "C:/dir" and "\\server\share". A letter before the colon is
        // not enough on its own: "http://" has one too, hence the boundary.
        let drive = i + 2 < bytes.len()
            && bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && matches!(bytes[i + 2], b'\\' | b'/')
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric());
        let share = rest.starts_with("\\\\");
        if link || drive || share {
            let len = rest.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '>')).unwrap_or(rest.len());
            return Some((i, len));
        }
    }
    None
}

/// "YYYY-MM-DD.jsonl" and nothing else.
fn day_file(name: &str) -> bool {
    let Some(day) = name.strip_suffix(".jsonl") else { return false };
    let bytes = day.as_bytes();
    bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A folder of its own under the temp dir, gone when the test is done.
    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("lumen-report-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    /// A journal with a visit in it: the report keeps the run and the trouble,
    /// never the address and never the page.
    #[test]
    fn a_digest_keeps_the_run_and_not_the_visit() {
        let dir = tmp("digest");
        fs::write(
            dir.0.join("2026-10-17.jsonl"),
            concat!(
                "{\"t\":1792227600000,\"k\":\"start\",\"v\":\"0.6.1\",\"headless\":false,\"theme\":\"dark\"}\n",
                "{\"t\":1792227660000,\"k\":\"nav\",\"host\":\"secret.example.com\",\"ms\":812}\n",
                "{\"t\":1792227720000,\"k\":\"sample\",\"lumen\":81.4}\n",
            ),
        )
        .unwrap();
        fs::write(
            dir.0.join("2026-10-18.jsonl"),
            concat!(
                "{\"t\":1792339869000,\"k\":\"error\",\"f\":\"download\",\"m\":\"yt-dlp gave up on https://videos.example.com/watch?v=secret123\",\"dropped\":\"x\"}\n",
                "{\"t\":1792339930000,\"k\":\"exit\",\"clean\":true,\"uptime\":3600}\n",
            ),
        )
        .unwrap();
        // Not a day file: not read at all.
        fs::write(dir.0.join("notes.jsonl"), "{\"t\":1,\"k\":\"error\",\"m\":\"https://other.example.com\"}\n").unwrap();

        let digest = journal_digest(&dir.0, 7, 100);
        assert!(!digest.contains("http"), "{digest}");
        assert!(!digest.contains(".com"), "{digest}");
        assert!(!digest.contains("nav"), "{digest}");
        assert!(!digest.contains("secret") && !digest.contains("dark"), "{digest}");
        // Oldest file first, kept kinds only, kept keys only, "m" scrubbed.
        // The time is local, so the test reads past it (15 characters: "MM-DD HH:MM:SS ").
        assert_eq!(
            digest.lines().map(|l| &l[15..]).collect::<Vec<_>>(),
            vec!["start v=0.6.1 headless=false", "error f=download m=yt-dlp gave up on …", "exit clean=true uptime=3600"],
        );
        // The newest days only, and the last lines only.
        assert_eq!(journal_digest(&dir.0, 1, 100).lines().count(), 2);
        assert!(journal_digest(&dir.0, 7, 1).ends_with(" exit clean=true uptime=3600"));
        // No folder: nothing to say.
        assert_eq!(journal_digest(&dir.0.join("gone"), 7, 100), "");
    }

    #[test]
    fn a_crash_tail_keeps_no_address() {
        let dir = tmp("crash");
        let path = dir.0.join("crash.log");
        fs::write(&path, "first line\npanic at https://example.com/a?token=1\n\nlast line at C:\\Users\\someone\\AppData\\lumen\\x.txt\n")
            .unwrap();
        let tail = crash_tail(&path, 3);
        assert_eq!(tail.lines().count(), 3, "{tail}");
        assert!(tail.contains("panic at …"), "{tail}");
        assert!(tail.contains("last line at …"), "{tail}");
        assert!(!tail.contains("http") && !tail.contains("example") && !tail.contains("Users"), "{tail}");
        // The last lines only, and a file that is not there says nothing.
        assert_eq!(crash_tail(&path, 1), "last line at …");
        assert_eq!(crash_tail(&dir.0.join("gone.log"), 3), "");
    }

    #[test]
    fn the_report_has_every_section() {
        let facts = Facts {
            version: "0.6.3".into(),
            build: "abc1234 2026-10-18".into(),
            os: "Windows 11 (build 26300)".into(),
            webview: "141.0.3537.71".into(),
            look: "signal".into(),
            layout: "vertical".into(),
            theme: "dark".into(),
            blocker: true,
            agent_door: true,
            uptime_s: 4_320,
            tabs: 4,
            mem_mb: Some(412),
        };
        let text = build(&facts, "10-18 16:11:09 start v=0.6.2 headless=false", "");
        assert!(text.starts_with("Lumen problem report\nVersion 0.6.3 (abc1234 2026-10-18)"), "{text}");
        assert!(text.contains("Windows 11 (build 26300) · WebView2 141.0.3537.71"), "{text}");
        assert!(text.contains("What happened:\n(say what you did, what you expected, and what Lumen did instead)"), "{text}");
        assert!(text.contains("Setup: look signal · layout vertical · theme dark · blocker on · agent door on"), "{text}");
        assert!(text.contains("Running: 4 tabs · 412 MB · up 1 h 12 min"), "{text}");
        assert!(text.contains("Recent journal (no addresses, no page content, local time):\n10-18 16:11:09 start v=0.6.2 headless=false"), "{text}");
        assert!(text.contains("Last crash log lines:\nnone"), "{text}");

        // Nothing measured, nothing to tail: the sections still stand, saying none.
        let bare = Facts { build: String::new(), os: String::new(), webview: String::new(), mem_mb: None, ..facts };
        let text = build(&bare, "", "");
        assert!(text.contains("Version 0.6.3\n"), "{text}");
        assert!(!text.contains("WebView2"), "{text}");
        assert!(text.contains("Recent journal (no addresses, no page content, local time):\nnone"), "{text}");
        assert!(text.contains("Last crash log lines:\nnone"), "{text}");
    }

    #[test]
    fn the_os_line_names_windows_and_the_build_names_the_version() {
        let os = os_version();
        assert!(os.starts_with("Windows"), "{os}");
        // ProductName still says Windows 10 on Windows 11: the build decides.
        assert_eq!(os_text("Windows 10 Pro", "24H2", "26300"), "Windows 11 Pro 24H2 (build 26300)");
        assert_eq!(os_text("Windows 10 Pro", "22H2", "19045"), "Windows 10 Pro 22H2 (build 19045)");
        assert_eq!(os_text("", "", ""), "Windows");
    }

    #[test]
    fn a_stamp_is_short_and_the_utc_reading_is_right() {
        assert_eq!(stamp_utc(1_700_000_000_000), "11-14 22:13:20");
        assert_eq!(stamp_utc(0), "01-01 00:00:00");
        // Local time: the same shape, whatever the zone.
        let s = stamp(1_700_000_000_000);
        assert_eq!(s.len(), 14, "{s}");
        assert_eq!(&s[2..3], "-");
    }
}
