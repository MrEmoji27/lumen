# Lumen

What it is so far, and where it goes next. Version 0.2.1, 18 October 2026.

## What it is

Lumen is a web browser for Windows, written in Rust. It is built for people who want a browser
that does not eat the machine, and for AI agents that need a real browser to work in.

Be clear about one thing first. Lumen is not a new web engine. Pages are drawn by WebView2, the
Chromium that ships with every Windows 11 machine. A page costs about what it costs in Chrome or
Edge, because it is the same engine. What Lumen replaces is everything around the page: the
window, the tabs, the sidebar, the blocker, the downloads, the memory policy. That part is
Lumen's own code, about 40,000 lines, small enough to read.

The bet is simple. Most of a browser's weight is not the page you are looking at. It is the shell,
the tabs you are not looking at, and features that run whether you use them or not. Cut those and
the same Chromium gets light.

## What it weighs

All numbers are release builds, measured on this laptop.

| What | Lumen |
|---|---|
| Lumen itself, idle, start page | 18.5 MB (private working set) |
| An empty window, before and after dropping the HTML interface | 144 MB, then 18 MB, one process instead of five |
| One Wikipedia article | about 91 MB |
| 40 tabs, renderer processes | 23, down from 54 (one renderer per site, not per tab), 310 MB less |
| 40 heavy tabs, after they sleep | peaked at 2.7 GB, settled at 1.1 GB, 0.4% CPU |
| One agent tab | about 85 MB |

These compare Lumen with itself. A side-by-side run against Edge, Chrome and Brave on the same
pages is the next measurement, and it goes here when it is done. Until then, no claim about
other browsers.

How the weight stays off:

- **The interface is drawn natively** with Direct2D. No web page pretends to be a toolbar.
- **Hidden tabs sleep.** Their caches are trimmed after 30 seconds, they freeze after 5 minutes,
  and they are unloaded after 30.
- **GPU work runs in the browser process.** One process fewer and about 100 MB less.
- **The event loop sleeps** until something is due. Chromium features Lumen never uses are off.
- **Every feature pays rent.** On 18 October idle had crept to 24.6 MB. A heap census found one
  6 MB block that existed only to label a report. It was cut, and idle went back to 18.5 MB.
- **Caps you can set:** RAM, CPU and network.

## What you get

- Sidebar or tab strip, in vertical, horizontal or hybrid layouts. Colours, fonts, shapes,
  motion, clock and wallpaper can all be changed.
- An ad, tracker and cookie-banner blocker built in (Brave's adblock-rust, the EasyList and uBlock
  lists, uBlock's YouTube scriptlets). It is on by default.
- Pop-ups blocked unless you clicked for them. Malware and phishing sites get a warning page first.
- A privacy report: what was blocked, what it saved, and what was counted, with trends.
- Reader view, dark pages for sites without a dark mode, translation that runs on the PC, and
  read aloud.
- Downloads through aria2 with 16 connections, plus torrents. Save video from a page.
- History you can search by what a page said, not just its title.
- Pins, bookmarks with folders and import, a speed dial, workspaces, split view, tab search.
- KeePass logins, filled from a `.kdbx` file. Kept in memory only, locked after 10 minutes.
- A usage journal kept on the PC (memory, CPU, slow sites, crashes) that sends nothing anywhere.

## A browser for agents

Lumen has a door for AI agents built in. Any agent on the machine can drive it through MCP:
Claude Code, opencode, Codex, Cursor. One program, `lumen-agent`, connects them. It finds Lumen
and its token by itself, so an agent's config holds a path, never a secret.

- An agent sees a page as a snapshot: every heading and control, each with a short ref like `e5`.
  It clicks and types by ref, with real input events.
- 41 tools: reading, clicking, typing, forms, frames, downloads, screenshots, PDF, the console and
  network logs, saved logins, and record and replay.
- Every agent gets its own tabs and its own profile, with its own cookies and storage. One agent
  cannot see another's tabs, and none can see yours unless you lend one.
- When a login, captcha or payment needs a person, the agent hands the tab over and waits.
- `lumen.exe --headless` is a real Lumen off-screen. No focus, no taskbar button, no sound.

The first real test drive was on 18 October. Reading was fast: 80 to 130 ms a call. Clicking in a
background tab was broken: the click did nothing and still said "clicked". The fix on hand pulled
the agent's tab in front of the person, which is the one thing an agent browser must never do.
That is what the plan below is about.

## The plan

In order. Each step ships as its own version, with a line in `docs/changelog.md`.

1. **Honest replies and a smoke test.** A failed page says its HTTP status. A click says what it
   changed, or that nothing changed. Page text is marked as page text, so a page cannot pose as
   the person. Before every release, a real browser test clicks real buttons. *In progress.*
2. **The agent stage.** Agent tabs live in a window of their own, off every screen. Chromium sees
   them as visible, so clicks land, but nothing ever comes to the front. *In progress.*
3. **Watching.** Click an agent's tab and its live page shows in your window. The agent has its
   own cursor in Lumen's accent colour, and your mouse cannot knock it off.
4. **Talking to an agent.** A text field in Lumen to steer an agent now, queue the next task, or
   start a new one.
5. **Speed for agents.** Fewer calls and smaller replies: batches of steps in one call, snapshots
   that send only what changed, and higher-level tools (fill a form, read a table).
6. **Headless trims.** Measure first, then cut what a browser nobody sees still does.
7. **The comparison.** Lumen against Edge, Chrome and Brave, same pages, same machine, published
   with the method.
8. **Open source.** Lumen is meant to be open source. The license follows from what it bundles:
   uBlock's scriptlets are GPL-3.0. The public repo starts with clean history.
9. **A release people can install.** A signed installer, so Windows does not warn about an unknown
   publisher, and updates that arrive by themselves.

Later, and only if they earn their memory:

- Servo for light sites. It is already built and renders Hacker News in 38 MB. It waits until it
  can block ads, download files and open new tabs.
- A local AI model reading the page, once one is pulled into Ollama.
- Agent scripts in Luau: small local workflows an agent can run in one call.
- An optional PDF editor that starts only when you ask for it.

## What it is not

- **Not cross-platform.** Windows only, because WebView2 is the engine.
- **No Chrome extensions.** The things people install extensions for (ad blocking, dark mode,
  passwords through KeePass, translation) are built in. Anything else is not there.
- **Not finished.** It was built in five days by one person and Claude, with AI workers writing
  much of the code and every change reviewed before it reached master. It is in daily use, and it
  is early.

The rule does not change: if a feature costs memory while you are not using it, it had better be
worth it.
