//! The self-check: what is wrong with this Lumen, and what to do about it.

use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{aria, blocker, bugreport, defaultapp, engine, hello, store, video};

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Ok,
    Warn,
    Fail,
}

/// One check: what it is called, how it came out, what was found, and what to do.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub name: &'static str,
    pub mark: Mark,
    pub found: String,
    /// Empty on a row that is ok.
    pub fix: &'static str,
}

fn row(name: &'static str, mark: Mark, found: impl Into<String>, fix: &'static str) -> Row {
    Row { name, mark, found: found.into(), fix }
}

/// Every check, and the first line of the text.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub head: String,
    pub rows: Vec<Row>,
}

impl Report {
    /// True when the process should leave with 1.
    pub fn failed(&self) -> bool {
        self.rows.iter().any(|r| r.mark == Mark::Fail)
    }

    /// The report as plain text: one check a line, the fix indented under a row that needs
    /// one, and the count at the end. Every row is scrubbed, so no address or path leaves here.
    pub fn text(&self) -> String {
        let mut out = bugreport::scrub(&self.head);
        for r in &self.rows {
            let mark = match r.mark {
                Mark::Ok => "ok",
                Mark::Warn => "warn",
                Mark::Fail => "fail",
            };
            out.push_str(&format!("\n{mark:<5}{:<20} {}", r.name, bugreport::scrub(&r.found)));
            if r.mark != Mark::Ok && !r.fix.is_empty() {
                out.push_str("\n     -> ");
                out.push_str(&bugreport::scrub(r.fix));
            }
        }
        out.push_str("\n\n");
        out.push_str(&self.summary());
        out
    }

    /// "9 checks: 7 ok, 1 warning, 1 failed."
    pub fn summary(&self) -> String {
        let (mut ok, mut warn, mut fail) = (0, 0, 0);
        for r in &self.rows {
            match r.mark {
                Mark::Ok => ok += 1,
                Mark::Warn => warn += 1,
                Mark::Fail => fail += 1,
            }
        }
        let mut parts = vec![format!("{ok} ok")];
        if warn > 0 {
            parts.push(format!("{warn} warning{}", if warn == 1 { "" } else { "s" }));
        }
        if fail > 0 {
            parts.push(format!("{fail} failed"));
        }
        format!("{} checks: {}", self.rows.len(), parts.join(", "))
    }
}

/// What only the window knows, and a check outside it cannot.
#[derive(Clone, Copy, Debug, Default)]
pub struct Live {
    /// The report comes from the window, which has the door's listener and its tabs.
    pub in_app: bool,
    /// A lock is in use: "Lock Lumen", or a tab locked.
    pub locked: bool,
}

/// Whether `settings.json` or the session file is there and reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parsed {
    Absent,
    Fine,
    Broken,
}

/// Who holds the agent door's port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    Free,
    ThisLumen,
    Other,
}

/// Every check, run against this PC.
pub fn run(live: &Live) -> Report {
    let dir = store::data_dir();
    let settings = store::load_settings();
    let tools = dir.join("tools");
    let (present, total, stale) = blocker::list_state(&dir.join("filters"), SystemTime::now());
    let helpers = [
        ("aria2c", aria::pinned_state(&dir)),
        ("yt-dlp", video::ytdlp_state(&tools)),
        ("FFmpeg", video::ffmpeg_state(&tools)),
    ];
    Report {
        head: format!("Lumen {}", crate::VERSION),
        rows: vec![
            webview_row(engine::runtime_version().as_deref()),
            folder_row(dir.is_dir(), probe_write(&dir), free_bytes(&dir)),
            files_row(parsed::<store::Settings>(&dir.join("settings.json")), parsed::<store::Session>(&dir.join("session.json"))),
            lists_row(present, total, stale),
            tools_row(&helpers),
            hello_row(hello::available(), live.locked),
            default_row(defaultapp::status()),
            door_row(settings.agent, settings.agent_port, probe_door(settings.agent_port), live.in_app),
            crashes_row(crash_last(&dir).as_deref(), unclean_exits(&dir.join("journal"), now_ms(), 7), 7),
        ],
    }
}

// ------------------------------------------------------------------- the checks

/// The WebView2 Runtime is installed, and its version.
pub fn webview_row(version: Option<&str>) -> Row {
    match version {
        Some(v) => row("WebView2", Mark::Ok, v, ""),
        None => row("WebView2", Mark::Fail, "not installed", "install the WebView2 Runtime from Microsoft's page"),
    }
}

/// The data folder exists, can be written, and has room.
pub fn folder_row(exists: bool, writable: bool, free: Option<u64>) -> Row {
    if !exists {
        return row("Data folder", Mark::Fail, "missing", "Lumen makes it at the next start; if it stays missing, check where the app data folder lives");
    }
    if !writable {
        return row("Data folder", Mark::Fail, "cannot be written", "check the folder's permissions and free space, then start Lumen again");
    }
    match free {
        Some(b) if b < GIB => row("Data folder", Mark::Warn, format!("writable, only {} free", size(b)), "free some space on the drive"),
        Some(b) => row("Data folder", Mark::Ok, format!("writable, {} free", size(b)), ""),
        None => row("Data folder", Mark::Warn, "writable, free space unknown", "check the drive"),
    }
}

/// `settings.json` and the session file parse. A file that is not there is a first run.
pub fn files_row(settings: Parsed, session: Parsed) -> Row {
    let broken = |p: Parsed| p == Parsed::Broken;
    match (broken(settings), broken(session)) {
        (true, true) => row("Settings and session", Mark::Fail, "settings.json and session.json do not parse", "Lumen starts with defaults; fix or delete the two files"),
        (true, false) => row("Settings and session", Mark::Fail, "settings.json does not parse", "Lumen starts with defaults; fix or delete settings.json"),
        (false, true) => row("Settings and session", Mark::Fail, "session.json does not parse", "Lumen opens with one start page; fix or delete session.json"),
        (false, false) if settings == Parsed::Absent && session == Parsed::Absent => {
            row("Settings and session", Mark::Ok, "not there yet (a first run)", "")
        }
        (false, false) => row("Settings and session", Mark::Ok, "both parse", ""),
    }
}

/// The filter lists are on disk and inside their own `Expires:`.
pub fn lists_row(present: usize, total: usize, stale: usize) -> Row {
    if present == 0 {
        return row("Filter lists", Mark::Fail, "none on disk", "open a page with the blocker on; Lumen fetches them");
    }
    if stale > 0 {
        let theirs = if stale == 1 { "its expiry" } else { "their expiry" };
        return row(
            "Filter lists",
            Mark::Warn,
            format!("{present} of {total} present, {stale} past {theirs}"),
            "they refresh themselves when a page needs them; check the network if they stay stale",
        );
    }
    row("Filter lists", Mark::Ok, format!("{present} of {total} present, all current"), "")
}

/// The helpers: absent is fine, present must match the pinned bytes. `None` is absent,
/// `Some(true)` matches, `Some(false)` does not.
pub fn tools_row(items: &[(&str, Option<bool>)]) -> Row {
    let wrong: Vec<&str> = items.iter().filter(|(_, state)| *state == Some(false)).map(|(name, _)| *name).collect();
    if !wrong.is_empty() {
        return row(
            "Helpers",
            Mark::Fail,
            format!("{} does not match the pinned SHA-256", wrong.join(", ")),
            "delete it from the tools folder; Lumen fetches a fresh copy when it next needs one",
        );
    }
    let have = items.iter().filter(|(_, state)| state.is_some()).count();
    if have == 0 {
        return row("Helpers", Mark::Ok, "none fetched yet", "");
    }
    row("Helpers", Mark::Ok, format!("{have} of {} present, all match the pinned SHA-256", items.len()), "")
}

/// Windows Hello is only needed when something is locked.
pub fn hello_row(available: bool, lock_in_use: bool) -> Row {
    match (available, lock_in_use) {
        (true, _) => row("Windows Hello", Mark::Ok, "set up", ""),
        (false, true) => row("Windows Hello", Mark::Warn, "not set up, and a lock is in use", "set up Windows Hello in Windows Settings, or turn the lock off"),
        (false, false) => row("Windows Hello", Mark::Ok, "not set up; nothing needs it", ""),
    }
}

/// Whether Windows hands links to Lumen. News, not a fault: Windows will not let a program
/// make itself the default.
pub fn default_row(status: defaultapp::Status) -> Row {
    let found = match status {
        defaultapp::Status::Default => "Lumen is the default browser",
        defaultapp::Status::Listed => "registered; not picked in Windows yet",
        defaultapp::Status::Absent => "not registered",
    };
    row("Default browser", Mark::Ok, found, "")
}

/// The agent door: off, open at its port, or open with nothing of Lumen's behind it.
pub fn door_row(on: bool, port: u32, state: Port, in_app: bool) -> Row {
    if !on {
        return row("Agent door", Mark::Ok, "off", "");
    }
    match state {
        Port::ThisLumen => row("Agent door", Mark::Ok, format!("on, at port {port}"), ""),
        Port::Other => row("Agent door", Mark::Warn, format!("on, but port {port} is held by something else"), "pick another port in Settings > AI agent"),
        Port::Free if in_app => {
            row("Agent door", Mark::Fail, format!("on, but nothing answers at port {port}"), "turn the agent door off and on again in Settings > AI agent")
        }
        Port::Free => row("Agent door", Mark::Warn, format!("on, but nothing answers at port {port} right now"), "start Lumen, or turn the door off"),
    }
}

/// The last line of `crash.log`, and how many runs ended without saying goodbye.
pub fn crashes_row(crash_last: Option<&str>, unclean: usize, days: u32) -> Row {
    let window = format!("the last {days} days");
    let exits = |n: usize| format!("{n} unclean exit{}", if n == 1 { "" } else { "s" });
    match (crash_last.filter(|line| !line.trim().is_empty()), unclean) {
        (Some(line), n) if n > 0 => {
            row("Crashes", Mark::Warn, format!("last crash: {line}; {} in {window}", exits(n)), "send a problem report from Settings > About")
        }
        (Some(line), _) => row("Crashes", Mark::Warn, format!("last crash: {line}"), "send a problem report from Settings > About"),
        (None, n) if n > 0 => row("Crashes", Mark::Warn, format!("{} in {window}", exits(n)), "send a problem report from Settings > About"),
        (None, _) => row("Crashes", Mark::Ok, format!("no crash log, no unclean exit in {window}"), ""),
    }
}

// -------------------------------------------------------------------- the reads

const GIB: u64 = 1 << 30;

/// "221 GB", "720 MB": the free space on a drive.
fn size(bytes: u64) -> String {
    let gb = bytes as f64 / GIB as f64;
    if gb >= 1.0 { format!("{gb:.0} GB") } else { format!("{} MB", bytes / (1 << 20)) }
}

/// The file's own bytes, as `T` reads them.
fn parsed<T: for<'de> serde::Deserialize<'de>>(path: &Path) -> Parsed {
    match fs::read_to_string(path) {
        Err(_) => Parsed::Absent,
        Ok(text) => {
            if serde_json::from_str::<T>(&text).is_ok() {
                Parsed::Fine
            } else {
                Parsed::Broken
            }
        }
    }
}

/// Write a small file into the folder and take it away. False if either fails.
fn probe_write(dir: &Path) -> bool {
    let path = dir.join(format!(".doctor-{}", std::process::id()));
    let wrote = fs::write(&path, b"ok").is_ok();
    let _ = fs::remove_file(&path);
    wrote
}

/// Free bytes on the drive the folder is on.
fn free_bytes(dir: &Path) -> Option<u64> {
    use windows::{Win32::Storage::FileSystem::GetDiskFreeSpaceExW, core::HSTRING};
    let path = HSTRING::from(dir);
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(&path, Some(&mut free), None, None) }.ok()?;
    Some(free)
}

/// The last line of the crash log, already scrubbed, or None when there is none.
fn crash_last(dir: &Path) -> Option<String> {
    let line = bugreport::crash_tail(&dir.join("crash.log"), 1);
    (!line.trim().is_empty()).then_some(line)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The unclean exits the journal holds within `days` of `now_ms`.
fn unclean_exits(dir: &Path, now_ms: u64, days: u32) -> usize {
    let since = now_ms.saturating_sub(days as u64 * 86_400_000);
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| bugreport::day_file(&e.file_name().to_string_lossy()))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .map(|text| unclean_in(&text, since))
        .sum()
}

/// Unclean exits in one day file, from the window on.
fn unclean_in(text: &str, since_ms: u64) -> usize {
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("k").and_then(|k| k.as_str()) == Some("unclean_exit"))
        .filter(|v| v.get("t").and_then(|t| t.as_u64()).unwrap_or(0) >= since_ms)
        .count()
}

/// Who answers at the agent door's port: a head request, and the code it comes back with.
fn probe_door(port: u32) -> Port {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port as u16));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return if TcpListener::bind(addr).is_ok() { Port::Free } else { Port::Other };
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let ask = format!("GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(ask.as_bytes()).is_err() {
        return Port::Other;
    }
    let mut buf = [0u8; 512];
    let n = stream.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    // A shut door says 503, an open one without the token says 401. Either is Lumen's.
    if head.starts_with("HTTP/1.1 401") || head.starts_with("HTTP/1.1 503") { Port::ThisLumen } else { Port::Other }
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
        let dir = std::env::temp_dir().join(format!("lumen-doctor-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    #[test]
    fn a_missing_webview_is_a_failure() {
        assert_eq!(webview_row(Some("141.0.3537.71")).mark, Mark::Ok);
        assert_eq!(webview_row(Some("141.0.3537.71")).found, "141.0.3537.71");
        assert_eq!(webview_row(None).mark, Mark::Fail);
        assert!(webview_row(None).found.contains("not installed"));
    }

    #[test]
    fn the_data_folder_must_exist_be_writable_and_have_room() {
        assert_eq!(folder_row(false, true, Some(GIB * 100)).mark, Mark::Fail);
        assert_eq!(folder_row(true, false, Some(GIB * 100)).mark, Mark::Fail);
        assert_eq!(folder_row(true, true, Some(GIB * 100)).mark, Mark::Ok);
        assert!(folder_row(true, true, Some(GIB * 100)).found.contains("100 GB"));
        // Under a gigabyte, and a drive that will not say: both worth a look, neither a failure.
        assert_eq!(folder_row(true, true, Some(500 << 20)).mark, Mark::Warn);
        assert_eq!(folder_row(true, true, Some(500 << 20)).found, "writable, only 500 MB free");
        assert_eq!(folder_row(true, true, None).mark, Mark::Warn);
    }

    #[test]
    fn only_a_broken_file_fails() {
        assert_eq!(files_row(Parsed::Fine, Parsed::Fine).mark, Mark::Ok);
        assert_eq!(files_row(Parsed::Fine, Parsed::Fine).found, "both parse");
        assert_eq!(files_row(Parsed::Absent, Parsed::Absent).mark, Mark::Ok, "a first run is not a fault");
        assert_eq!(files_row(Parsed::Absent, Parsed::Fine).mark, Mark::Ok);
        let settings = files_row(Parsed::Broken, Parsed::Fine);
        assert_eq!(settings.mark, Mark::Fail);
        assert!(settings.found.contains("settings.json"));
        let both = files_row(Parsed::Broken, Parsed::Broken);
        assert_eq!(both.mark, Mark::Fail);
        assert!(both.found.contains("settings.json") && both.found.contains("session.json"));
    }

    #[test]
    fn the_lists_fail_when_none_are_there_and_warn_when_stale() {
        assert_eq!(lists_row(0, 9, 9).mark, Mark::Fail);
        assert_eq!(lists_row(9, 9, 0).mark, Mark::Ok);
        assert_eq!(lists_row(9, 9, 0).found, "9 of 9 present, all current");
        let stale = lists_row(8, 9, 1);
        assert_eq!(stale.mark, Mark::Warn);
        assert_eq!(stale.found, "8 of 9 present, 1 past its expiry");
        assert_eq!(lists_row(2, 9, 7).found, "2 of 9 present, 7 past their expiry");
    }

    #[test]
    fn a_helper_with_the_wrong_bytes_fails() {
        let all = [("aria2c", Some(true)), ("yt-dlp", None), ("FFmpeg", Some(true))];
        assert_eq!(tools_row(&all).mark, Mark::Ok);
        assert_eq!(tools_row(&all).found, "2 of 3 present, all match the pinned SHA-256");
        let none = [("aria2c", None), ("yt-dlp", None), ("FFmpeg", None)];
        assert_eq!(tools_row(&none).mark, Mark::Ok);
        assert_eq!(tools_row(&none).found, "none fetched yet");
        let wrong = [("aria2c", Some(true)), ("yt-dlp", Some(false)), ("FFmpeg", Some(false))];
        let r = tools_row(&wrong);
        assert_eq!(r.mark, Mark::Fail);
        assert_eq!(r.found, "yt-dlp, FFmpeg does not match the pinned SHA-256");
    }

    #[test]
    fn hello_only_warns_when_a_lock_needs_it() {
        assert_eq!(hello_row(true, false).mark, Mark::Ok);
        assert_eq!(hello_row(true, true).mark, Mark::Ok);
        assert_eq!(hello_row(false, false).mark, Mark::Ok, "nothing locked, nothing needed");
        assert_eq!(hello_row(false, true).mark, Mark::Warn);
    }

    #[test]
    fn the_default_browser_is_only_news() {
        for status in [defaultapp::Status::Default, defaultapp::Status::Listed, defaultapp::Status::Absent] {
            assert_eq!(default_row(status).mark, Mark::Ok);
        }
        assert_eq!(default_row(defaultapp::Status::Absent).found, "not registered");
    }

    #[test]
    fn the_door_is_told_by_what_answers() {
        assert_eq!(door_row(false, 47615, Port::Other, true).found, "off");
        assert_eq!(door_row(true, 47615, Port::ThisLumen, false).mark, Mark::Ok);
        // Nothing behind it: a failure inside the window, a warning from a shell.
        assert_eq!(door_row(true, 47615, Port::Free, true).mark, Mark::Fail);
        assert_eq!(door_row(true, 47615, Port::Free, false).mark, Mark::Warn);
        // Someone else's program holds it.
        assert_eq!(door_row(true, 47615, Port::Other, true).mark, Mark::Warn);
        assert!(door_row(true, 47615, Port::Other, true).found.contains("something else"));
    }

    #[test]
    fn crashes_warn_and_a_quiet_week_does_not() {
        assert_eq!(crashes_row(None, 0, 7).mark, Mark::Ok);
        assert_eq!(crashes_row(None, 0, 7).found, "no crash log, no unclean exit in the last 7 days");
        assert_eq!(crashes_row(Some("last line at …"), 0, 7).mark, Mark::Warn);
        assert_eq!(crashes_row(None, 1, 7).found, "1 unclean exit in the last 7 days");
        let both = crashes_row(Some("panic at …"), 3, 7);
        assert_eq!(both.mark, Mark::Warn);
        assert_eq!(both.found, "last crash: panic at …; 3 unclean exits in the last 7 days");
        // A blank line is no crash.
        assert_eq!(crashes_row(Some("  "), 0, 7).mark, Mark::Ok);
    }

    #[test]
    fn only_the_window_counts_unclean_exits() {
        let now = 1_800_000_000_000u64;
        let text = concat!(
            "{\"t\":1799983000000,\"k\":\"unclean_exit\",\"from\":\"2026-10-24\"}\n",
            "{\"t\":1700000000000,\"k\":\"unclean_exit\",\"from\":\"2023-11-14\"}\n",
            "{\"t\":1799983000000,\"k\":\"exit\",\"clean\":true}\n",
            "not json\n",
        );
        assert_eq!(unclean_in(text, now - 7 * 86_400_000), 1, "the old one is outside the window");
        assert_eq!(unclean_in(text, 0), 2);
    }

    #[test]
    fn the_journal_is_read_by_day_file() {
        let dir = tmp("journal");
        fs::write(
            dir.0.join("2026-10-25.jsonl"),
            format!("{{\"t\":{},\"k\":\"unclean_exit\"}}\n{{\"t\":{},\"k\":\"unclean_exit\"}}\n", now_ms() - 1000, now_ms() - 2 * 86_400_000),
        )
        .unwrap();
        fs::write(dir.0.join("notes.jsonl"), format!("{{\"t\":{},\"k\":\"unclean_exit\"}}\n", now_ms())).unwrap();
        assert_eq!(unclean_exits(&dir.0, now_ms(), 7), 2, "the window is seven days; notes.jsonl is not a day");
        assert_eq!(unclean_exits(&dir.0, now_ms(), 1), 1);
        assert_eq!(unclean_exits(&dir.0.join("gone"), now_ms(), 7), 0);
    }

    #[test]
    fn a_report_carries_no_address() {
        let report = Report {
            head: "Lumen 0.6.7.7 (abc1234 2026-10-25)".into(),
            rows: vec![
                row("Crashes", Mark::Warn, "last crash: at https://example.com/a?token=1", "look at C:\\Users\\someone\\lumen.txt"),
                row("WebView2", Mark::Fail, "not installed", ""),
            ],
        };
        let text = report.text();
        assert!(!text.contains("http"), "{text}");
        assert!(!text.contains("example"), "{text}");
        assert!(!text.contains("Users"), "{text}");
        assert!(text.contains("last crash: at …"), "{text}");
        assert!(text.contains("-> look at …"), "{text}");
        // The first line is the version and build stamp, the last is the count.
        assert!(text.starts_with("Lumen 0.6.7.7 (abc1234 2026-10-25)\n"), "{text}");
        assert!(text.ends_with("2 checks: 0 ok, 1 warning, 1 failed"), "{text}");
        assert!(report.failed());
        assert!(!Report { head: String::new(), rows: vec![row("WebView2", Mark::Ok, "1", "")] }.failed());
    }
}
