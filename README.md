# Lumen

A browser for one person. It does two things well: it looks good, and it does not eat your RAM.

## Run it

```
cargo build --release
target\release\lumen.exe
```

Debug builds read `ui/` from disk at start, so UI edits need no recompile:

```
cargo run
```

Data lives in `%APPDATA%\Lumen`: `settings.json`, `session.json`, `filters\`, and the WebView2 profile.
Debug builds also write `debug.log` there.

## Keys

| Keys | Does |
|---|---|
| Ctrl+T | New tab, cursor in address bar |
| Ctrl+Shift+T | Reopen closed tab |
| Ctrl+W | Close tab |
| Ctrl+L, Ctrl+K, F6 | Address bar |
| Ctrl+Tab, Ctrl+Shift+Tab | Next, previous tab |
| Ctrl+1..8, Ctrl+9 | Jump to tab, last tab |
| Alt+Left, Alt+Right | Back, forward |
| Ctrl+R, F5 | Reload |
| Ctrl+B | Compact sidebar |
| F11 | Fullscreen |
| Ctrl+, | Settings |
| Shift+Esc | Control: memory, CPU, network per tab, and limits |
| F12, Ctrl+Shift+I | DevTools |
| Ctrl+F | Find in page |

## How it works

One window. One UI page. Web pages sit on top of it.

- `src/main.rs` owns tabs, layout, the session, and who sleeps.
- `src/engine.rs` is the only file that talks to WebView2. Swap it to change engines.
- `src/blocker.rs` kills ads and trackers at the network layer. EasyList, EasyPrivacy,
  Peter Lowe, uBlock filters. About 2 microseconds a request. Matcher cached to disk.
- `src/monitor.rs` measures each tab and holds the limiters.
- `src/store.rs` is settings and session, JSON, written atomically.
- `ui/` is the sidebar, address bar, new tab page and settings. Plain HTML, CSS, JS.

Memory rules:

1. All pages share one browser process, one GPU process, one network process.
2. On start, only the active tab loads. The rest wait until clicked.
3. Hidden tab: no paint, low priority, caches trimmed.
4. Hidden for 5 minutes: frozen. Wakes instantly.
5. Hidden for 30 minutes: discarded. Its process dies. Click reloads it.
6. Tabs playing sound never sleep. Pinned tabs freeze but are never discarded.
7. Ads never load at all.
8. Graphics run inside the browser process. Measured: a third less memory at idle.

All the timers are in Settings > Performance. Control (Shift+Esc, or click the memory
readout) shows each tab's memory, CPU and network, and holds three limits: a soft RAM
limit (unloads the oldest hidden tab), a hard CPU cap (Windows Job Object over the whole
browser), and a per-tab bandwidth cap. The footer shows live memory for the
whole app, every process counted, the same way Task Manager counts it.

## Engine

WebView2, the Chromium engine that ships with Windows 11. Every site works and
Microsoft patches it. Servo is the plan for later; `engine.rs` is the seam.
