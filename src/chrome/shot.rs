//! Debug builds only: save what the chrome draws as a PNG, and seed it with sample data.
//!
//! `LUMEN_TEST_SHOT=<dir>` writes `<dir>/<name>.png` about 600 ms after the last paint
//! that left nothing moving. `LUMEN_TEST_NAME` names the file (default: the open panel,
//! or "chrome"). `LUMEN_TEST_CROP=x,y,w,h` (DIPs) cuts out the part worth looking at.
//! `LUMEN_TEST_FAKE=tabs,dl,hist` adds sample tabs, downloads and history rows so the
//! pages can be judged full as well as empty. None of this exists in a release build.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use windows::Win32::{
    Foundation::RECT,
    UI::WindowsAndMessaging::{GetClientRect, KillTimer, SetTimer},
};

use super::{Inner, TIMER_SHOT, UiDl, UiEssential, UiMark, UiState, UiTab, UiVisit};

fn dir() -> Option<PathBuf> {
    std::env::var_os("LUMEN_TEST_SHOT").map(PathBuf::from)
}

fn fake_has(what: &str) -> bool {
    std::env::var("LUMEN_TEST_FAKE").is_ok_and(|v| v.split(',').any(|x| x.trim() == what))
}

/// A count in the fake data: `LUMEN_TEST_FAKE=tabs:8` asks for eight plain tabs,
/// `pins:3` for three pinned ones. A layout can then be looked at with a known
/// number of tabs in it.
fn fake_count(what: &str) -> Option<usize> {
    let v = std::env::var("LUMEN_TEST_FAKE").ok()?;
    v.split(',')
        .filter_map(|x| x.trim().split_once(':'))
        .find(|(k, _)| *k == what)
        .and_then(|(_, n)| n.trim().parse().ok())
}

/// After a real paint: if nothing is moving, arm the one-shot timer. Every paint resets it,
/// so the file always holds the last settled frame.
pub fn after_paint(i: &Inner) {
    if dir().is_none() {
        return;
    }
    // Armed once, not reset by every paint: a spinner paints every 33 ms and would starve it.
    if i.animating || ARMED.swap(true, Ordering::Relaxed) {
        return;
    }
    unsafe {
        SetTimer(Some(i.hwnd), TIMER_SHOT, 600, None);
    }
}

static ARMED: AtomicBool = AtomicBool::new(false);

pub fn capture(i: &mut Inner) {
    ARMED.store(false, Ordering::Relaxed);
    unsafe {
        let _ = KillTimer(Some(i.hwnd), TIMER_SHOT);
    }
    let (Some(dir), Some(s)) = (dir(), i.state.clone()) else { return };
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(i.hwnd, &mut rc);
    }
    let (w, h) = (rc.right as u32, rc.bottom as u32);
    let name = NEXT_NAME
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .or_else(|| std::env::var("LUMEN_TEST_NAME").ok())
        .unwrap_or_else(|| if s.panel == "none" { "chrome".into() } else { s.panel.into() });
    let k = i.k();
    let crop = std::env::var("LUMEN_TEST_CROP").ok().and_then(|v| {
        let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (n.len() == 4).then(|| ((n[0] * k) as i32, (n[1] * k) as i32, (n[2] * k) as i32, (n[3] * k) as i32))
    });
    let _ = std::fs::create_dir_all(&dir);
    if !i.gfx.begin_shot(w, h) {
        return;
    }
    let bg = i.theme.bg;
    if i.gfx.begin(w, h, bg) {
        i.draw_all(&s);
    }
    let ok = i.gfx.end_shot(&dir.join(format!("{name}.png")), crop);
    eprintln!("lumen: shot {name} {w}x{h} dpi {} ok={ok}", i.gfx.dpi);
}

/// The name the next shot should carry, set by a `shot` script step.
static NEXT_NAME: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// WM_APP + 1: a script step asks the chrome for a shot of what it shows now (after a short wait).
pub const WM_SHOT: u32 = 0x8001;
/// WM_APP + 2: a script step puts the pointer at a point and leaves it there.
pub const WM_HOVER: u32 = 0x8002;

/// A script step wants a shot: remember its name and arm the timer.
pub fn requested(i: &Inner) {
    unsafe {
        ARMED.store(true, Ordering::Relaxed);
        SetTimer(Some(i.hwnd), TIMER_SHOT, 700, None);
    }
}

/// `LUMEN_TEST_SCRIPT="1500 key sidebar;2500 shot compact;3000 click 40 300;3500 wheel 800 400 -120"`:
/// steps at milliseconds after start, run on a thread, delivered the way a person's input is
/// (messages to the main loop or to the chrome window). So the paths under test are the real ones.
/// Steps: `key <action>`, `ui <json>`, `shot <name>`, `move x y`, `click x y`, `rclick x y`,
/// `wheel x y delta`, `vk <code>` (x, y in chrome pixels).
pub fn script(proxy: tao::event_loop::EventLoopProxy<crate::engine::Ev>, hwnd: windows::Win32::Foundation::HWND) {
    use crate::engine::{Action, Ev};
    use windows::Win32::{
        Foundation::{LPARAM, WPARAM},
        UI::WindowsAndMessaging::*,
    };
    let Ok(text) = std::env::var("LUMEN_TEST_SCRIPT") else { return };
    let hwnd_raw = hwnd.0 as isize;
    let steps: Vec<(u64, String)> = text
        .split(';')
        .filter_map(|s| {
            let s = s.trim();
            let (ms, rest) = s.split_once(' ')?;
            Some((ms.parse().ok()?, rest.trim().to_string()))
        })
        .collect();
    std::thread::spawn(move || {
        let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as _);
        let start = std::time::Instant::now();
        let xy = |a: &[&str]| -> (isize, isize) { (a.first().and_then(|v| v.parse().ok()).unwrap_or(0), a.get(1).and_then(|v| v.parse().ok()).unwrap_or(0)) };
        for (ms, step) in steps {
            let due = std::time::Duration::from_millis(ms);
            if let Some(wait) = due.checked_sub(start.elapsed()) {
                std::thread::sleep(wait);
            }
            let (cmd, rest) = step.split_once(' ').unwrap_or((step.as_str(), ""));
            let args: Vec<&str> = rest.split_whitespace().collect();
            unsafe {
                match cmd {
                    "key" => {
                        let a = match rest.trim() {
                            "sidebar" => Action::Sidebar,
                            "omnibox" => Action::Omnibox,
                            "control" => Action::Control,
                            "downloads" => Action::Downloads,
                            "history" => Action::History,
                            "settings" => Action::Settings,
                            "newtab" => Action::NewTab,
                            "fullscreen" => Action::Fullscreen,
                            "tabsearch" => Action::TabSearch,
                            "palette" => Action::Palette,
                            _ => continue,
                        };
                        let _ = proxy.send_event(Ev::Key(a));
                    }
                    "ui" => {
                        let _ = proxy.send_event(Ev::Ui(rest.to_string()));
                    }
                    "shot" => {
                        *NEXT_NAME.lock().unwrap_or_else(|e| e.into_inner()) = Some(rest.trim().to_string());
                        let _ = PostMessageW(Some(hwnd), WM_SHOT, WPARAM(0), LPARAM(0));
                    }
                    "hover" => {
                        // A mouse move that stays: the real one asks Windows to say when the
                        // pointer leaves, and a pointer that is not really there leaves at once.
                        let (x, y) = xy(&args);
                        let _ = PostMessageW(Some(hwnd), WM_HOVER, WPARAM(0), LPARAM((y << 16) | (x & 0xffff)));
                    }
                    "move" | "click" | "rclick" => {
                        let (x, y) = xy(&args);
                        let lp = LPARAM((y << 16) | (x & 0xffff));
                        let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), lp);
                        if cmd == "click" {
                            let _ = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(1), lp);
                            std::thread::sleep(std::time::Duration::from_millis(40));
                            let _ = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), lp);
                        }
                        if cmd == "rclick" {
                            let _ = PostMessageW(Some(hwnd), WM_RBUTTONDOWN, WPARAM(0), lp);
                            std::thread::sleep(std::time::Duration::from_millis(40));
                            let _ = PostMessageW(Some(hwnd), WM_RBUTTONUP, WPARAM(0), lp);
                        }
                    }
                    "wheel" => {
                        let (x, y) = xy(&args);
                        let delta: isize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(-120);
                        let lp = LPARAM((y << 16) | (x & 0xffff));
                        let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), lp);
                        let _ = PostMessageW(Some(hwnd), WM_MOUSEWHEEL, WPARAM(((delta & 0xffff) << 16) as usize), lp);
                    }
                    "evk" => {
                        // A key into an edit box by its id, as typing there would:
                        // "evk 9" is the address bar (101), "evk 110 9" the first form box.
                        let (id, vk) = match args.len() {
                            1 => (101, args[0].parse().unwrap_or(0)),
                            _ => (args[0].parse().unwrap_or(101), args.get(1).and_then(|v| v.parse().ok()).unwrap_or(0)),
                        };
                        if let Ok(edit) = GetDlgItem(Some(hwnd), id) {
                            let _ = PostMessageW(Some(edit), WM_KEYDOWN, WPARAM(vk), LPARAM(0));
                        }
                    }
                    "killfocus" => {
                        // The keyboard left the app, as Alt-Tab takes it: a menu must close.
                        let _ = PostMessageW(Some(hwnd), WM_KILLFOCUS, WPARAM(0), LPARAM(0));
                    }
                    "focus" => {
                        // Say which window has the keyboard: proof that Tab walked the form.
                        // The chrome's own thread, not the foreground one: a headless run
                        // is nobody's foreground.
                        let tid = GetWindowThreadProcessId(hwnd, None);
                        let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
                        if GetGUIThreadInfo(tid, &mut info).is_ok() {
                            let id = if info.hwndFocus.is_invalid() { -1 } else { GetDlgCtrlID(info.hwndFocus) };
                            eprintln!("lumen: focus {id}");
                        }
                    }
                    "drag" => {
                        // Press at (x1, y1), walk to (x2, y2) in a few steps, let go there.
                        let n: Vec<isize> = args.iter().filter_map(|v| v.parse().ok()).collect();
                        if n.len() == 4 {
                            let at = |x: isize, y: isize| LPARAM((y << 16) | (x & 0xffff));
                            let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), at(n[0], n[1]));
                            let _ = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(1), at(n[0], n[1]));
                            for s in 1..=8 {
                                std::thread::sleep(std::time::Duration::from_millis(40));
                                let (x, y) = (n[0] + (n[2] - n[0]) * s / 8, n[1] + (n[3] - n[1]) * s / 8);
                                let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(1), at(x, y));
                            }
                            std::thread::sleep(std::time::Duration::from_millis(60));
                            let _ = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), at(n[2], n[3]));
                        }
                    }
                    "bmq" => {
                        // Words into the Bookmarks search box (ID 103), as typing there would.
                        if let Ok(edit) = GetDlgItem(Some(hwnd), 103) {
                            let _ = SetWindowTextW(edit, &windows::core::HSTRING::from(rest.trim()));
                        }
                    }
                    "etext" => {
                        // Words into any edit box by its ID, as typing there would: "etext 101 rust".
                        let (id, words) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
                        if let Some(id) = id.parse::<i32>().ok()
                            && let Ok(edit) = GetDlgItem(Some(hwnd), id)
                        {
                            let _ = SetWindowTextW(edit, &windows::core::HSTRING::from(words));
                        }
                    }
                    "ekey" => {
                        // A key into an edit box by its ID: "ekey 102 13" is Enter in the start
                        // page's box.
                        let id: i32 = args.first().and_then(|v| v.parse().ok()).unwrap_or(0);
                        let vk: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                        if let Ok(edit) = GetDlgItem(Some(hwnd), id) {
                            let _ = PostMessageW(Some(edit), WM_KEYDOWN, WPARAM(vk), LPARAM(0));
                        }
                    }
                    "etype" => {
                        // Letters into an edit box by its ID, one keypress each, as typing
                        // there would: "etype 101 yout".
                        let (id, words) = rest.trim().split_once(' ').unwrap_or((rest.trim(), ""));
                        if let Some(id) = id.parse::<i32>().ok()
                            && let Ok(edit) = GetDlgItem(Some(hwnd), id)
                        {
                            for c in words.encode_utf16() {
                                let _ = PostMessageW(Some(edit), WM_CHAR, WPARAM(c as usize), LPARAM(0));
                            }
                        }
                    }
                    "setq" => {
                        // Words into the Settings search box (ID 107).
                        if let Ok(edit) = GetDlgItem(Some(hwnd), 107) {
                            let _ = SetWindowTextW(edit, &windows::core::HSTRING::from(rest.trim()));
                        }
                    }
                    "vk" => {
                        let vk: usize = args.first().and_then(|v| v.parse().ok()).unwrap_or(0);
                        let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(vk), LPARAM(0));
                    }
                    "char" => {
                        // A character to the chrome window, as typing would send it.
                        let ch: usize = args.first().and_then(|v| v.parse().ok()).unwrap_or(0);
                        let _ = PostMessageW(Some(hwnd), WM_CHAR, WPARAM(ch), LPARAM(0));
                    }
                    _ => {}
                }
            }
        }
    });
}

/// Sample data, appended to what the browser really has.
pub fn fake(s: &mut UiState) {
    let t = |id: u64, title: &str, url: &str| UiTab {
        id,
        title: title.into(),
        url: url.into(),
        loading: false,
        back: false,
        fwd: false,
        pinned: false,
        pin_away: false,
        essential: false,
        audio: false,
        muted: false,
        call: false,
        status: "live",
        agent: false,
        agent_ok: false,
        reader: false,
        private: false,
        split: false,
        translated: false,
        translating: false,
        servo: false,
        owner: String::new(),
        owner_name: String::new(),
    };
    // A known number of plain tabs, for a layout that has to hold them.
    if let Some(n) = fake_count("tabs") {
        for i in 0..n {
            s.tabs.push(t(9500 + i as u64, &format!("Tab {}", i + 1), &format!("https://site{}.example/", i + 1)));
        }
    }
    // A known number of essentials: the sites kept open in every workspace.
    if let Some(n) = fake_count("essentials") {
        for i in 0..n {
            s.essentials.push(UiEssential { url: format!("https://site{}.example/", i + 1), title: format!("Site {}", i + 1), away: false, tab: 0 });
        }
    }
    // And a known number of pinned ones.
    if let Some(n) = fake_count("pins") {
        for i in 0..n {
            s.tabs.push(UiTab { pinned: true, ..t(9600 + i as u64, &format!("Pin {}", i + 1), &format!("https://pin{}.example/", i + 1)) });
        }
    }
    // Pins that wandered from the page they were made on.
    if let Some(n) = fake_count("pinaway") {
        for i in 0..n {
            s.tabs.push(UiTab { pinned: true, pin_away: true, ..t(9700 + i as u64, &format!("Away {}", i + 1), &format!("https://elsewhere{}.example/", i + 1)) });
        }
    }
    if fake_has("tabs") {
        let mut v = vec![
            UiTab { pinned: true, ..t(9001, "Mail", "https://mail.example.com/") },
            UiTab { pinned: true, audio: true, ..t(9002, "Music", "https://music.example.com/") },
            UiTab { pinned: true, status: "off", ..t(9003, "Calendar", "https://calendar.example.com/") },
            UiTab { pinned: true, status: "off", ..t(9011, "Docs", "https://docs.example.com/") },
            t(9004, "The Rust Programming Language - a very long page title that cannot fit in the sidebar", "https://doc.rust-lang.org/book/"),
            UiTab { loading: true, ..t(9005, "Loading something", "https://slow.example.com/") },
            UiTab { audio: true, ..t(9006, "Lo-fi beats to study to", "https://video.example.com/watch") },
            UiTab { call: true, ..t(9012, "Voice: general", "https://discord.com/channels/1/2") },
            UiTab { status: "asleep", ..t(9007, "Frozen tab", "https://frozen.example.com/") },
            UiTab { status: "off", ..t(9008, "Unloaded tab", "https://off.example.com/") },
            UiTab { agent: true, ..t(9009, "Agent research", "https://agent.example.com/") },
            UiTab { private: true, ..t(9010, "Private page", "https://private.example.com/") },
            UiTab { split: true, muted: true, ..t(9021, "Split left page", "https://split.example.com/left") },
            UiTab { split: true, translated: true, ..t(9022, "Split right page", "https://split.example.com/right") },
        ];
        s.tabs.append(&mut v);
        // The Lo-fi tab has sound: the footer's now-playing card follows it.
        s.media = Some(9006);
    }
    if fake_has("battery") {
        s.battery = true;
    }
    if fake_has("page") {
        // The active tab is a secure, bookmarked page: lock, star and the offer chips show.
        let id = s.active;
        if let Some(t) = s.tabs.iter_mut().find(|t| t.id == id) {
            t.title = "The Rust Programming Language".into();
            t.url = "https://doc.rust-lang.org/book/".into();
            t.status = "live";
            t.back = true;
            t.fwd = true;
        }
        s.bookmarked = true;
        s.listen = "playing";
        s.kdbx = vec!["doc.rust-lang.org".into()];
    }
    if fake_has("marks") && (s.panel == "bookmarks" || s.settings.bm_bar) {
        let m = |id: u64, title: &str, url: &str, folder: &str| UiMark { id, title: title.into(), url: url.into(), folder: folder.into() };
        s.bm_folders = vec!["Work".into(), "Chrome".into(), "Reading".into()];
        s.bookmarks = vec![
            m(9201, "The Rust Programming Language", "https://doc.rust-lang.org/book/", ""),
            m(9202, "Hacker News", "https://news.ycombinator.com/", ""),
            m(9203, "Linear: issues for the team, with a title long enough that it has to be cut off", "https://linear.app/team/issues", "Work"),
            m(9204, "Figma", "https://www.figma.com/files", "Work"),
            m(9205, "Direct2D - Wikipedia", "https://en.wikipedia.org/wiki/Direct2D", "Chrome"),
            m(9206, "crates.io: Rust Package Registry", "https://crates.io/", "Chrome"),
        ];
    }
    if fake_has("drive") && s.panel == "drive" {
        let st = |w: &str, ms: Option<u64>| (w.to_string(), ms);
        s.drive = Some(super::DriveView {
            name: "drive-3".into(),
            running: true,
            paused: false,
            task: "Find the cheapest kettle that lasts and put it in the cart. Stop before paying.".into(),
            elapsed: "2m".into(),
            steps: vec![
                st("going to reddit.com", Some(812)),
                st("looking at the page", Some(2)),
                st("reading", Some(19)),
                st("going to amazon.com", Some(1104)),
                st("typing \u{201C}stainless kettle\u{201D}", Some(140)),
                st("clicking e12", None),
            ],
            log: vec![
                "The thread agrees on two: the Stagg EKG and a plain Bodum.".into(),
                "Checking prices.".into(),
                "Bodum 1.7 L is 39.99; the Stagg is 165.".into(),
                "Adding the Bodum to the cart, then I stop and hand over.".into(),
            ],
            exit: None,
        });
    }
    if fake_has("handoff") {
        s.handoff = Some(("claude-code".into(), "Log in to the shop with your account, then press Done and I carry on.".into()));
    }
    if fake_has("cardcolor") && s.settings.recent_colors.is_empty() {
        // A few colours picked before, so the recents row is full. Only once: the
        // real list, as the picker updates it, must not be overwritten on a state push.
        s.settings.recent_colors = vec!["#2f6bff".into(), "#13a36b".into(), "#f0a070".into(), "#5a43e6".into(), "#c4241c".into(), "#3c6e71".into()];
    }
    if fake_has("prompt") {
        s.prompt = Some((1, "https://meet.example.com".into(), "camera"));
    }
    if fake_has("sites") && s.panel == "sites" {
        s.rules = vec![
            ("https://meet.example.com".into(), "camera".into(), true),
            ("https://meet.example.com".into(), "microphone".into(), true),
            ("https://news.example.com".into(), "notifications".into(), false),
        ];
        s.zooms = vec![("doc.rust-lang.org".into(), 125), ("news.ycombinator.com".into(), 110)];
    }
    if let Some(mb) = fake_count("mem") {
        // A given load, in megabytes: "mem:1500" is 1.5 GB.
        s.mem = Some((mb as u64 * 1024 * 1024, 9));
        s.settings.show_memory = true;
    } else if fake_has("mem") {
        s.mem = Some((412 * 1024 * 1024, 9));
        s.blocked = 1280;
        s.settings.show_memory = true;
    }
    if fake_has("dl") {
        let d = |id: u64, name: &str, state: &'static str, kind: &'static str, got: i64, total: i64| UiDl {
            id,
            name: name.into(),
            url: "https://download.example.com/files/x".into(),
            received: got,
            total,
            state,
            speed: if state == "progress" { 4_200_000 } else { 0 },
            note: String::new(),
            kind,
            up: 0,
            peers: 0,
            seeds: 0,
            ratio: 0.0,
        };
        s.downloads.extend([
            d(9101, "ubuntu-24.04.1-desktop-amd64-with-a-really-long-file-name-that-will-not-fit.iso", "progress", "file", 2_100_000_000, 6_000_000_000),
            d(9102, "Big Buck Bunny.mp4", "progress", "video", 40_000_000, 0),
            d(9103, "dataset.zip", "paused", "file", 300_000_000, 900_000_000),
            UiDl { up: 310_000, peers: 12, ratio: 0.42, ..d(9104, "archlinux-2024.10.01-x86_64.iso", "seeding", "torrent", 1_000_000_000, 1_000_000_000) },
            d(9105, "report-final-v2.pdf", "done", "file", 2_400_000, 2_400_000),
            d(9106, "setup.exe", "done", "file", 85_000_000, 85_000_000),
            UiDl { note: "Server said no".into(), ..d(9107, "broken-link.bin", "failed", "file", 0, 0) },
        ]);
    }
    if fake_has("hist") {
        let v = |url: &str, title: &str, day: &str, time: &str| UiVisit { url: url.into(), title: title.into(), day: day.into(), time: time.into() };
        s.history.extend([
            v("https://doc.rust-lang.org/book/", "The Rust Programming Language", "Today", "14:02"),
            v("https://news.ycombinator.com/", "Hacker News", "Today", "13:40"),
            v("https://github.com/anthropics", "Anthropic on GitHub", "Today", "11:12"),
            v("https://example.com/a/very/long/path/that/goes/on/and/on", "A page with a title long enough to be cut off before it ever reaches the host name on the right", "Yesterday", "22:30"),
            v("https://en.wikipedia.org/wiki/Direct2D", "Direct2D - Wikipedia", "Yesterday", "21:05"),
            v("https://example.org/", "", "Yesterday", "09:00"),
        ]);
    }
    // Only while the page has nothing: after a real reset the numbers must be the
    // real, empty ones.
    if fake_has("report") && s.report.is_none() {
        use crate::privacy_stats::{Bar, Counts, SiteRow, Snapshot};
        let day = |n: u64, blocked: u64, bytes: u64| Bar { day: format!("2026-10-{n:02}"), blocked, bytes };
        let c = Counts {
            ads: 412,
            trackers: 268,
            other: 54,
            cosmetic: 190,
            cookies: 23,
            removeparam: 41,
            scriptlets: 18,
            sponsor: 6,
            popups: 14,
            webrtc: 9,
            signals: 22,
            wrong_password: 2,
            malicious: 3,
            danger: 1,
            https_up: 27,
            bytes: 48_200_000,
            ms: 96_000,
        };
        let prev = Counts { ads: 350, trackers: 240, other: 40, popups: 6, webrtc: 5, signals: 15, wrong_password: 0, malicious: 1, danger: 0, https_up: 12, bytes: 39_100_000, ms: 78_000, ..Default::default() };
        let row = |host: &str, share: f32, counts: Counts| SiteRow { host: host.into(), counts, share, last: "2026-10-18".into() };
        s.report = Some(Snapshot {
            all: c,
            range: "7".into(),
            totals: c,
            prev,
            bars: vec![day(12, 80, 4_000_000), day(13, 120, 6_000_000), day(14, 95, 5_200_000), day(15, 140, 7_100_000), day(16, 110, 5_800_000), day(17, 160, 8_400_000), day(18, 213, 11_700_000)],
            sites: vec![
                row("news.ycombinator.com", 0.32, Counts { ads: 90, trackers: 70, other: 20, ..c }),
                row("doc.rust-lang.org", 0.21, Counts { ads: 50, trackers: 55, other: 15, popups: 2, ..c }),
                row("example.com", 0.14, Counts { ads: 40, trackers: 30, other: 10, ..c }),
            ],
            domains: vec![("doubleclick.net".into(), 220), ("google-analytics.com".into(), 180), ("facebook.com".into(), 95), ("amazon-adsystem.com".into(), 60)],
            site: None,
            ..Default::default()
        });
    }
}
