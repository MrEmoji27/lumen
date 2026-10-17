# Changelog

One version number, in `Cargo.toml`. Every exe also carries the commit it was built from and
that commit's date: `lumen-agent --version`, the Settings page under its title, and the agent
door's server info all say it. `+dev` means the build had changes nobody committed.

The rule: a release build that goes to the person gets the next number. Small fixes bump the last
digit. A batch of new features bumps the middle one.

## 0.3.0 — 2026-10-18

- Agents: the stage. Every agent tab's view lives in a second, off-screen tool window where
  Chromium keeps it visible, so clicks and keys land and no agent action can bring anything to
  the front. An agent tab idle for 30 s stops painting and wakes on the next call.
- Agents: the person can watch any agent tab. Click it in the sidebar: the same view moves into
  the window live (no reload) while the agent keeps working; leave it and the view goes back to
  the stage.
- Agents: `open` and `navigate` say a failed page's status on the first line (`HTTP 429 Too Many
  Requests`), and return only the controls in view, not the whole page.
- Agents: a click says what it changed — the page went somewhere, a new tab opened, the target is
  now checked — or that nothing on the page changed.
- Agents: page text is fenced and marked as the page's own writing; a page cannot pose as the
  person. The bridge and Lumen compare build stamps, and a stale bridge is told to reconnect.
- Agents: a smoke suite drives a real headless Lumen against local pages (open, type, click,
  select, submit, the stage, watching), and `tools/release.ps1` runs it before every release
  build.

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
