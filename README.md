# lumen

a browser for windows that tries to stay out of your RAM.

rust around webview2. every bit of interface (sidebar, tabs, menus, settings, file pickers) is drawn by hand with direct2d, no html.

<p align="center">
  <img src="docs/img/readme-signal-dark.png" width="860" alt="lumen, signal dark">
</p>

> [!WARNING]
> the ui is unfinished. some screens are rough, some settings are missing, and things will move around between versions. it's my daily driver, but expect sharp edges.

## why

idle, nothing open, whole process tree, same machine:

| | lumen | brave |
|---|---|---|
| idle | 36 MB | 151 MB |
| one wikipedia tab | 49 MB | 192 MB |
| 20 tabs, 6 min later | 124 MB | 1111 MB |

hidden tabs freeze and then unload, the chrome is one process, and nothing runs that you didn't ask for. numbers from `tools/vs_brave.py`, measured on 0.6.7.2.

## what's in it

- ads and trackers blocked (adblock-rust, easylist, easyprivacy, ublock lists), cookie banners hidden, pop-ups and sneaky redirects stopped
- a privacy report that only shows things it actually counted
- tabs that sleep, show what they cost on hover, and leave sooner when they get heavy
- downloads through aria2: many connections, resume, torrents
- lock a tab, or all of lumen, with windows hello. a secret downloads folder too
- offline translation, reader view, read aloud, split view, workspaces, pinned tabs
- three looks: signal, glass, precise

<p align="center">
  <img src="docs/img/readme-glass.png" width="420" alt="glass">
  <img src="docs/img/readme-precise.png" width="420" alt="precise">
</p>

## agents

lumen speaks MCP. `lumen-agent.exe` is the bridge; each agent gets its own tabs and its own profile, and anything like paying, sending or logging in gets handed back to you.

```
claude mcp add -s user lumen -- %LOCALAPPDATA%\Programs\Lumen\lumen-agent.exe mcp
```

more in [docs/agents.md](docs/agents.md).

## install

grab the installer from [releases](../../releases). windows 10 or 11, 64-bit. webview2 is already on every windows 11 machine.

it isn't signed yet, so smartscreen may complain the first time.

if something's off, `lumen.exe --doctor` checks the install and tells you what to fix.

## build

```
cargo build --release
```

rust 1.98+, windows only. `cargo test` runs the unit tests; the live suites in `tests/` drive a headless lumen.

## license

GPL-3.0. third-party pieces (ublock scriptlets, dark reader, readability, dompurify, bergamot, lucide) keep their own licenses, next to them in `assets/`.

built with a lot of help from claude and a few other ai agents.
