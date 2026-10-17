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

use super::{Inner, TIMER_SHOT, UiDl, UiState, UiTab, UiVisit};

fn dir() -> Option<PathBuf> {
    std::env::var_os("LUMEN_TEST_SHOT").map(PathBuf::from)
}

fn fake_has(what: &str) -> bool {
    std::env::var("LUMEN_TEST_FAKE").is_ok_and(|v| v.split(',').any(|x| x.trim() == what))
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
                    }
                    "wheel" => {
                        let (x, y) = xy(&args);
                        let delta: isize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(-120);
                        let lp = LPARAM((y << 16) | (x & 0xffff));
                        let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), lp);
                        let _ = PostMessageW(Some(hwnd), WM_MOUSEWHEEL, WPARAM(((delta & 0xffff) << 16) as usize), lp);
                    }
                    "evk" => {
                        // A key into the address bar's edit box (ID 101), as typing there would.
                        let vk: usize = args.first().and_then(|v| v.parse().ok()).unwrap_or(0);
                        if let Ok(edit) = GetDlgItem(Some(hwnd), 101) {
                            let _ = PostMessageW(Some(edit), WM_KEYDOWN, WPARAM(vk), LPARAM(0));
                        }
                    }
                    "vk" => {
                        let vk: usize = args.first().and_then(|v| v.parse().ok()).unwrap_or(0);
                        let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(vk), LPARAM(0));
                    }
                    _ => {}
                }
            }
        }
    });
}

/// Sample data, appended to what the browser really has.
pub fn fake(s: &mut UiState) {
    if fake_has("tabs") {
        let t = |id: u64, title: &str, url: &str| UiTab {
            id,
            title: title.into(),
            url: url.into(),
            loading: false,
            back: false,
            fwd: false,
            pinned: false,
            pin_away: false,
            audio: false,
            muted: false,
            status: "live",
            agent: false,
            reader: false,
            private: false,
            split: false,
            translated: false,
            translating: false,
            servo: false,
        };
        let mut v = vec![
            UiTab { pinned: true, ..t(9001, "Mail", "https://mail.example.com/") },
            UiTab { pinned: true, audio: true, ..t(9002, "Music", "https://music.example.com/") },
            UiTab { pinned: true, status: "off", ..t(9003, "Calendar", "https://calendar.example.com/") },
            UiTab { pinned: true, status: "off", ..t(9011, "Docs", "https://docs.example.com/") },
            t(9004, "The Rust Programming Language - a very long page title that cannot fit in the sidebar", "https://doc.rust-lang.org/book/"),
            UiTab { loading: true, ..t(9005, "Loading something", "https://slow.example.com/") },
            UiTab { audio: true, ..t(9006, "Lo-fi beats to study to", "https://video.example.com/watch") },
            UiTab { status: "asleep", ..t(9007, "Frozen tab", "https://frozen.example.com/") },
            UiTab { status: "off", ..t(9008, "Unloaded tab", "https://off.example.com/") },
            UiTab { agent: true, ..t(9009, "Agent research", "https://agent.example.com/") },
            UiTab { private: true, ..t(9010, "Private page", "https://private.example.com/") },
        ];
        s.tabs.append(&mut v);
    }
    if fake_has("mem") {
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
}
