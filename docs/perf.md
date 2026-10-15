# Perf

What runs the machine: one event loop, a wake deadline, a tick. When the
deadline passes and something waits, the tick runs. Nothing else spins.
This pass fixed the wake math, the memory clock and dead pages. Numbers below
are release builds, headless, throwaway profile under `%TEMP%`, measured by
`tools\measure.ps1`. Same machine, same script, before and after.

## What was broken

The wake loop lied. `next_wake` clamped its deadline to `now + 50ms`, so
`tick_due` could never fire on the thing that was actually due. The tab
ladder (`next_tab_wake`) had a dead filter and a hardcoded 86400-second hack.
The tick asked "what is due" with the wrong clock, and the memory readout
never ran: `next_mem` was uninitialized and the read had a borrow error.

Crashes were worse. A dead renderer became a white page forever. A dead
browser process took the whole session with it.

## What changed

- `tick_due = next_tick <= now && tick_waiting(now)`. Rate limited to one tick
  per TICK. No spin, no missed deadline.
- `next_wake` is exact: the earliest of its own deadlines, and if nothing
  waits, the next tab step, and IDLE_TICK only when nothing does at all.
- `next_tab_wake` walks the ladder for real: trim at 30s, freeze at
  `sleep_after_min`, discard at `discard_after_min`, done when pinned and
  frozen. Skips the active tab, audio, crashed, viewless.
- `next_mem` is an `Instant` on its own clock. The event loop runs
  `read_memory` every MEM_TICK. The tick does not touch it.
- Aria's poll thread blocks on a condvar instead of sleeping blind.
- `hook_crash` only reacts to the two deaths that mean something:
  renderer exit (mark the tab crashed) and browser exit (drop every view;
  the next click rebuilds the environment from the session).
- Clicking a crashed tab reloads it. Sidebar and control page show
  "Crashed". Session is remembered for the panic hook.
- TRIM_AFTER 60s → 30s. Hidden tabs give up their caches sooner.

## Numbers

Working-set-private MB and CPU % over a 20s window. `after2` is a repeat of
`three` to show spread.

| scenario | window | before ws_private | after ws_private | before cpu | after cpu |
|---|---|---|---|---|---|
| idle | 25s | 18.2 | 18.5 | 0.00 | 0.00 |
| idle | 95s | 17.9 | 18.4 | 0.08 | 0.00 |
| wikipedia | 25s | 115.9 | 113.0 | 0.31 | 0.31 |
| wikipedia | 95s | 112.9 | **91.0** | 0.47 | **0.16** |
| three | 25s | 210.2 | 204.7 / 208.8 | 0.62 | 1.48 / 1.88 |
| three | 95s | 206.8 | 200.3 / 207.4 | 0.23 | 0.23 |

- **wikipedia at 95s: 19% less private memory, 66% less CPU.** That is the
  wake fix and the trim sitting still instead of polling.
- **idle is flat.** Both sleep. That is the point.
- **three at 95s: same CPU, same memory.** Steady state did not regress.
- **three at 25s: more CPU.** Two runs, both higher. That window is page
  load finishing plus the trim hint firing on two fresh hidden tabs. The
  extra work is ~0.1–0.3 CPU-seconds over 20 seconds. Load-time noise and a
  one-time hint, not a spinning loop. Steady state proves it: same 0.23.

Procs are unchanged per scenario (1 / 5 / 7). Within 95s only the 30s trim
can fire; freeze waits 5 minutes, discard 30. That is by design.

## Crash smoke test

Headless, release profile, one Wikipedia tab, driven by `lumen-agent`:

1. Killed its renderer process. Lumen stayed up, tab still listed.
2. Clicked the tab. Page reloaded, article came back. ✓
3. Killed the browser process (the WebView2 parent). Lumen still up.
4. Clicked the tab. Environment rebuilt, page reloaded, article back. ✓

One Lumen per profile. Dead webviews are rebuilt from the session, not from
hope.

## Reproduce

```
pwsh -NoProfile -File tools\measure.ps1 -Tag after -Scenario idle -Stop
pwsh -NoProfile -File tools\measure.ps1 -Tag after -Scenario wikipedia -Stop
pwsh -NoProfile -File tools\measure.ps1 -Tag after -Scenario three -Stop
```

Rows land in `docs\perf-data.csv`. Write settings.json before starting
Lumen, or the agent door is shut and the tool hangs waiting.

## Downloads

aria2 got tuned. HTTP: 64 MB disk cache, `falloc` instead of `none`, gzip
when offered, async DNS, five concurrent downloads with aria2 sliding the
count toward whatever moves, 15s to connect and 30 to move, five tries three
seconds apart, and a 10K/s floor before a crawl is hung up on. The floor is
safe for torrents: aria2's own help says it never touches BitTorrent. Every
flag in `src\aria.rs` FLAGS was read in `aria2c.exe --help=#all` for 1.37.0,
because aria2 drops an option it does not know without a word and the tuning
would then be a lie.

Torrents: 128 peers (aria2's default is 55; 0 means uncapped and an uncapped
swarm is how a laptop cooks itself), more peers sought under 1M/s, a DHT
entry point to shake hands with, and 10s tracker timeouts so a dead tracker
cannot hold the swarm for a minute.

Trackers: `src\trackers.rs` fetches ngosang's best list once a week on its
own thread, keeps it in `data\aria2\trackers.txt`, validates every line
(udp/http/https/wss, under 300 chars, 50 max) and keeps the old file when
the fetch fails. aria2 gets the cache at startup and a fresh list pushed in
with `aria2.changeGlobalOption` while Lumen runs.

### Numbers

Headless Lumen, throwaway profile under `%TEMP%`, one 100 MiB pull of
`https://ash-speed.hetzner.com/100MB.bin`, queued with
`lumen-agent download url=... wait=true`, timed from the call to its return.
Before is the master build (a copy of the master checkout's debug binary;
its `src\` is byte-identical to master). After is this branch. proof.ovh was
the first choice per the brief and rate-limited 16 connections to 429, so
the file moved.

| run | before | after |
|---|---|---|
| 1 | 21.1 s (4.7 MB/s) | 14.7 s (6.8 MB/s) |
| 2 | 14.4 s (7.0 MB/s) | 16.2 s (6.2 MB/s) |
| mean | 17.8 s (5.6 MB/s) | 15.5 s (6.5 MB/s) |

Read that table knowing that two runs of the *same* binary swung by 46%.
The link is the bottleneck, not the flags. What this tuning buys is not
peak MB/s on a warm connection: `falloc` lays a big file down at once
instead of growing it in pieces and paying a seek per write, the disk cache
turns many small writes into a few large ones, and the timeouts decide how
fast a dead server is dropped. Those are disk churn and stuck downloads,
which one healthy 100 MiB pull will never show.

### What was checked live

- `aria::live::every_flag_is_one_aria2_knows`: every flag is in aria2
  1.37.0's own help. `--ignored`, needs the internet.
- `aria::live::a_fresh_tracker_list_reaches_a_running_aria2`: on a fresh
  profile with no cache, the background thread fetched the list, pushed it,
  and `aria2.getGlobalOption` read back the same 20 trackers that were
  written to disk.
- The two runs above: the first start of each fresh profile passed
  `--bt-tracker=` from the cache, and the one after it got the list at
  runtime instead.
- `cargo test` over the whole tree: 80 green, 0 failed, and the four
  ignored live tests all pass with `cargo test -- --ignored`.

Reproduce the speed check by hand: start `target\debug\lumen.exe --headless`
with `LUMEN_DATA` and `LUMEN_DOWNLOADS` set to fresh folders under `%TEMP%`,
a `settings.json` in the data folder with `{"agent": true, "agent_port":
476xx}`, then `lumen-agent download url=... wait=true` with the same
`LUMEN_DATA`. Kill the Lumen you started, delete the folders. The speed
script this used lived in `%TEMP%` and was not checked in.

## Journal

The usage journal (`docs/briefs/journal.md`) has to cost nothing: no thread,
no timer, no deadline of its own, no second process walk. It rides on the
memory read the sidebar already makes, buffers lines in one 64 KB string, and
touches the disk at most once a minute. When Lumen is idle (window hidden or
minimized and nothing loading) it does nothing at all.

What is measured here is that promise, not a feature: journal on against
journal off, same binary, same machine. Debug builds, the same way the
numbers above were taken; `tools\measure.ps1` wanted release and this pass
was about a difference, not an absolute.

| scenario | window | journal off | journal on |
|---|---|---|---|
| idle, new tab | 2 min | 18.2 MB, 0.03% cpu | 18.2 MB, 0.01% cpu |
| one Wikipedia tab | 2 min | 109.9–113.5 MB, 0.43–0.74% | 110.6–119.4 MB, 0.43–0.62% |
| one Wikipedia tab, lumen.exe alone | 2 min | 20.8–21.1 MB | 21.1 MB |

- **Idle: the difference is zero.** With no page ever built there is no
  WebView2 environment, so the journal never even samples: the day file holds
  `start` and `exit` and nothing else.
- **Wikipedia: the spread is the noise, not the journal.** Four runs each:
  off median 111.8 MB / 0.58% cpu, on median 111.2 MB / 0.54% cpu. Single
  runs of the *same* configuration swung by 8.8 MB (110.6 to 119.4), so the
  one-off 119.4 above is page-load timing, not journal cost. Where the
  journal's memory actually lives, lumen.exe's own private working set, the
  two are the same 21 MB to a tenth.
- **Both are inside the brief's bar**: cpu under 0.1 percent apart, private
  memory under 1 MB apart.

The day file after the wikipedia run (2.5 minutes, one page) was **710 bytes**:
one `start`, one `nav`, two `sample`, two `wake`, one `ui_slow`, one
`feature`. An idle day (start and exit) is 200 bytes. A browsing day lands
around a few hundred KB; the 20 MB cap and the 14-day window are there for
the day something goes wrong, not for normal use.

Reproduce: set `LUMEN_DATA` to a fresh folder under `%TEMP%` with
`{"agent": true, "agent_port": 476xx, "journal": true|false}`, start
`target\debug\lumen.exe --headless`, open
`https://en.wikipedia.org/wiki/Lighthouse` through `lumen-agent`, wait two
minutes, and read ws_private and CPU the `windows\measure.ps1` way. The
script this pass used lived in the session scratchpad and was not checked in.
