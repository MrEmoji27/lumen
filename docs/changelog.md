# Changelog

One version number, in `Cargo.toml`. Every exe also carries the commit it was built from and
that commit's date: `lumen-agent --version`, the Settings page under its title, and the agent
door's server info all say it. `+dev` means the build had changes nobody committed.

The rule: a release build that goes to the person gets the next number. Small fixes bump the last
digit. A batch of new features bumps the middle one.

## 0.2.1 — 2026-10-18

- Memory: idle back from 24.6 MB to 18.5 MB. The ad blocker's matcher had been built in debug
  mode for the privacy report; a 380 KB tracker host table answers "ad or tracker" instead.
  Old matcher files (about 31 MB on disk) are deleted.

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
