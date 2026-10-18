# Changelog

One version number, in `Cargo.toml`. Every exe also carries the commit it was built from and
that commit's date: `lumen-agent --version`, the Settings page under its title, and the agent
door's server info all say it. `+dev` means the build had changes nobody committed.

The rule: a release build that goes to the person gets the next number. Small fixes bump the last
digit. A batch of new features bumps the middle one.

## 0.5.1 — 2026-10-18

- The message box is a card in Lumen's own look, over the sidebar: who it is for, the text,
  Steer now / Queue / New task as one segmented control (Tab switches), and Send (Enter). It
  replaces a stock Windows dialog that looked twenty years old. The watch pill's **Message** opens
  it too.
- The agent's cursor and the pill are drawn with Direct2D in windows with real transparency:
  smooth edges, a soft shadow, a ring that grows and fades on a click, text in Segoe UI Variable.
  They were GDI with a colour key: jagged edges and Windows XP buttons.
- The pill says what the agent is doing in words: `typing "Ada Lovelace"`, `clicking e13`,
  `going to httpbin.org`.

## 0.5.0 — 2026-10-18

- Watching: click an agent's tab and a shield goes over its page, so a stray click or scroll
  cannot knock the agent off. The agent's cursor is drawn over it in the agent's colour, with its
  name: it glides to every point the agent clicks, hovers or types at, and rings on a click. A
  pill at the bottom says the agent's last step and has **Take over**: the shield goes, the page
  is yours, the agent may only ask for it back, and **Give back** returns it.
- Talking to an agent: the send button on an agent's row in the sidebar opens a message.
  **Steer now** puts `[The person says, now: "..."]` at the top of that agent's next reply;
  **Queue** says it is for after the current task; **New task** starts a Drive run. A queued
  message to a Drive run starts the next run when this one ends. The row says what became of the
  message: waiting for its next call, delivered with which call, or dropped because the agent
  left. The MCP instructions tell every agent what those lines are.

## 0.4.2 — 2026-10-18

From a test drive on real pages (httpbin, Wikipedia):

- `browser_fill` sets time, date, colour and range inputs; typed keys never reached them.
- `browser_table` reads tables with header rows that span: each column gets one name
  (`Urban area / Population`), cells that span rows fill each row, footnote marks are dropped.
- A batch's step line says which tab an open made: `1 ok browser_open 812 ms: tab 3`.
- A snapshot cut at its line cap says so and points at `browser_snapshot`.

## 0.4.1 — 2026-10-18

- The version shows where people look: the Dashboard's header says `Lumen 0.4.1 (commit date)`,
  and Settings puts the number on a line of its own under its title, the commit and date under
  that. Before, it was one faint line that a long build stamp clipped.

## 0.4.0 — 2026-10-18

- Agents: every call is faster. The sidebar was redrawn on every agent call, about 35 ms each, and
  a tool makes several; now only a call that changes what the sidebar shows redraws it. A round
  trip to the window went from 35 ms to under 2 ms. Waiting for a page waits on WebView2's own
  start and finish of the navigation, not on fixed sleeps. On the agent bench, filling a form went
  from 5.7 s to 1.7 s and a ten-step workflow from 8.8 s to 2.6 s.
- Agents: `browser_batch` (up to 20 calls in one), `browser_fill` (a whole form, read back),
  `browser_table` and `browser_links`. Clicks and other actions answer with the controls in view.
- Scripts: Luau. The person puts `.luau` files in the scripts folder; agents list and run them with
  `browser_scripts` and `browser_script_run`. Each call a script makes is checked as if the agent
  made it. Sandboxed, 60 s, 32 MB, 200 calls. The ten-step workflow as a script: one call, 580
  bytes back, against twelve calls and 4 KB.
- The agent stage window is made with the first agent tab, not at start.
- `tests/agent_bench.rs`: fixed tasks on local pages, measured in calls, bytes and time.

## 0.3.0 — 2026-10-18

- Agents: the stage. Every agent tab's view lives in a second, off-screen tool window where
  Chromium keeps it visible, so clicks and keys land and no agent action can bring anything to
  the front: `browser_open` always opens in the background, and `browser_show` no longer
  activates anything. An agent tab idle for 30 s stops painting and wakes on the next call.
- Agents: the person can watch any agent tab. Click it in the sidebar: the same view moves into
  the window live (no reload) while the agent keeps working; leave it and the view goes back to
  the stage.
- Agents: `open` and `navigate` say a failed page's status on the first line (`HTTP 429 Too Many
  Requests`), and return only the controls in view, not the whole page.
- Agents: a click says what it changed — the page went somewhere, a new tab opened, the target is
  now checked, or how many changes the page made — or that nothing on the page changed.
- Agents: page text is fenced and marked as the page's own writing; a page cannot pose as the
  person. The bridge and Lumen compare build stamps, and a stale bridge is told to reconnect.
- Agents: a smoke suite drives a real headless Lumen against local pages (open, type, click,
  select, submit, the stage, watching), and `tools/release.ps1` runs it before every release
  build.

## 0.2.1 — 2026-10-18

- Memory: idle back from 24.6 MB to 18.5 MB. The ad blocker's matcher had been built in debug
  mode for the privacy report; a 380 KB tracker host table answers "ad or tracker" instead.
  Old matcher files (about 31 MB on disk) are deleted.
- Privacy report: three sections (blocked, saved, privacy stats) with trends against the period
  before. Pop-ups without a click are blocked, and so are hosts that `$popup` rules name.
  Malware and phishing sites (URLhaus and phishing lists, weekly, checked by SHA-256) get a
  warning page with Back and Continue. WebRTC no longer shows local IP addresses, and leaks,
  wrong KeePass passwords and https upgrades are counted. Those lists cost 0.5 MB: their 81,000
  hosts are kept as hashes.

## 0.2.0 — 2026-10-18

- Agents: per-agent profiles, frames, response bodies, saved state, record and replay, the Drive
  panel (type a task, an agent CLI runs it in Lumen).
- Agents: every click, type, hover and select works again. A missing brace in the element
  resolver had broken all of them. A test now checks those scripts parse.
- Look: customize everything (colours, fonts, shape, motion, clock), and vertical, horizontal and
  hybrid layouts.
- Sidebar: agent tabs grouped per agent, with a colour dot and a Stop for each agent.
- Footer: dashboard, Drive, downloads, settings. Bookmarks and the privacy report moved to the
  dashboard. The memory readout measures its room and shows whole or not at all.
- Versions: this file, and a build stamp in every exe.

## 0.1.0

Everything before numbering started.
