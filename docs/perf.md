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

## The wake storm (2026-10-17)

The journal said: median 12 wakes a minute, worst minute 33,527, cpu near zero.

Cause. Not a timer. `next_wake` never hands the loop a deadline in the past.
tao registers for raw mouse and keyboard input while the window has focus. It
sends the loop one wake per mouse sample, 125 to 1000 a second, for events
Lumen never reads. The storms follow the pointer: settings and downloads
panels, github, youtube, chatgpt. The minutes with the most wakes have the
least cpu, because each wake does almost nothing.

Fixes, in `main.rs`, `engine.rs`, `pacing.rs`:

- `DeviceEventFilter::Always`. No raw input, no wakes.
- `pacing::clamp_wake`: the loop never waits for less than 50 ms. A deadline
  already past can cost 20 wakes a second, never a spin.
- Page titles reach the loop at most 4 a second, newest wins, same title twice
  is nothing (`engine::title_pacer`).

Measured, headless, debug build, one tab:

| page | wakes |
|---|---|
| title rewritten every 10 ms, before | 8,531 in 100 s |
| title rewritten every 10 ms, after | 254 in 60 s |
| about:blank, idle (headless ticks every 3 s) | 94 in 60 s |
| aria2 download running (polled every 700 ms) | 165 in 60 s |

Not measured live: the raw input fix itself. A headless window has no focus
and no mouse. Check the journal after a day of use: `wake` should sit near
the median everywhere.

## Video, in-process GPU against a separate GPU process (2026-10-17)

One YouTube video (Big Buck Bunny 60fps, pinned to 720p, muted), headless,
debug build, 62 s of playback after 28 s of warm up. `tools\measure-video.ps1`.
Frames are `getVideoPlaybackQuality()`. CPU is the whole process tree.

| run | gpu | frames | dropped | cpu (one core) | working set | private | processes |
|---|---|---|---|---|---|---|---|
| v1 | in-process | 3765 | 294 (7.8%) | 118% | 915 MB | 639 MB | 8 |
| v3 | in-process | 3736 | 218 (5.8%) | 127% | 922 MB | 639 MB | 8 |
| v2 | separate | 3728 | 0 | 80% | 1011 MB | 720 MB | 10 |
| v4 | separate | 3718 | 0 | 70% | 1023 MB | 733 MB | 10 |

Separate GPU process: no dropped frames, about 40% less cpu. It costs about
100 MB of working set and 80 to 95 MB private. That is past the 30 MB line, so
the default stays in-process. The switch is there: Settings, "Graphics in
browser process". Turn it off if tearing and lag bother you more than 100 MB.

Hardware decode: not confirmed. YouTube sent AV1 (`av01.0.08M.08`). The GPU's
VideoDecode engine read 0% in every run, in both modes. H.264 reports
`powerEfficient: true`, so the machine can do it. AV1 here was probably
decoded in software. Hardware acceleration was not touched.

The headless window is off-screen, so these numbers say nothing about
presentation. Tearing on a real window is not measured here.

## Lucide icons

Lucide icons (SVG rasterized once per size into alpha masks, tinted when drawn): idle private bytes of lumen.exe, debug build, headless, 25 s, 3 runs each: 31.5 MB before (31.4 to 31.8), 31.9 MB after (31.9 to 32.0). About +0.4 MB, mostly the masks and the SVG code; the Direct3D device used to rasterize is dropped after each frame that needed it.

## Motion and customization (2026-10-17)

The custom pass turned every colour, font, size and spacing into one `Look`
derived from the settings, and added `src/chrome/motion.rs`, a spring and
tween library every animation in the chrome runs on. The promise that matters
here: **motion costs nothing when nothing moves.** The frame timer is armed
only while `Motion::running()`, values nobody touches for a frame are dropped,
and an idle Lumen therefore repaints exactly as often as it did before.

Method: `tools`-style manual run, debug build (this pass is about a difference,
not absolutes), headless, fresh profile under `%TEMP%`, `settings.json` with
`{"agent": true, "agent_port": 476xx, "journal": true}`, `LUMEN_JOURNAL_SECS=10`
so the journal samples every ten seconds, one `about:blank` tab opened through
`lumen-agent`. 60 s window after 15 s of settling. Wakes are the journal's
`wake` lines; CPU is the journal's own `cpu` sample (the whole process tree,
percent of one core). Before is the branch point build; after is this branch.

| run | wakes (per 10 s sample, steady state) | cpu samples | lumen private |
|---|---|---|---|
| before | 5, 5 | 0.00, 0.00 % | 31.0 MB |
| after | 6, 5 | 0.01, 0.02 % | 20.7 MB |

- **Wakes are where they were: 5–6 per ten seconds, and they are the loop's own
  clocks** (the 3 s tick and the 5 s memory read with `show_memory` on), not the
  chrome. The chrome's new 16 ms animation timer only runs while something is
  moving; a settling entrance is over in under a third of a second.
- **CPU is the same to the noise floor.** Both runs are under 0.1 %, and the
  journal's own samples in the after run read 0.00 to 0.08 % across the whole
  minute (page load included).
- **Private bytes went down, not up, in this pair of runs:** 31.0 to 20.7 MB in
  `lumen.exe` itself. Not chased: a single run either side of a large diff is
  not evidence, and the after build does *more* (a full-window render target in
  every layout, an installed-font list, the wallpaper path). What it says is
  that nothing here blew memory up. Absolute numbers stay where `tools`-style
  release runs put them.

What is *not* measured here: the animation timer under a real window at 60 Hz,
and the underlay repaint (the chrome now covers the whole window in every
layout, with the page drawn over it). Both are visible in the journal's
`ui_slow` counter over a day of use; the idle numbers above are the claim the
brief asked to keep.

## Idle memory, 18 October

Lumen at idle had grown from 18.2 MB to 24.6 MB (private working set, headless, start page, 95 s).
A heap census found it in one allocation: the ad blocker's matcher, 8.3 MB on 15 October and
14.4 MB on 18 October. The privacy report had switched the matcher to debug mode so every rule
kept its text and its list, only to say "ad" or "tracker". That cost 6 MB for as long as Lumen
ran.

Now the matcher is built without debug info, and a sorted table of the tracker lists' host
hashes (47,559 hosts, about 380 KB, `filters/trackers4.bin`) answers ad or tracker. A blocked
request whose host, or a domain above it, is in the table is a tracker; the rest are ads.

| Build | Matcher on disk | Idle, warm start |
|---|---|---|
| 0.2.0 | 14.0 MB | 23.9 / 24.2 MB |
| fix | 8.1 MB | 18.5 / 18.5 MB |

The old matcher files (`engine.dat`, `engine2.dat`, `engine3.dat`, about 31 MB) are deleted on start.

## Frames, 18 October

The journal said `ui_slow` in the first minute of nearly every run: one frame of 100 to 160 ms.
`LUMEN_PROFILE_PAINT=1` prints each frame's three parts on stderr: begin (the render target),
draw (Lumen's own code), end (`EndDraw`, the present). A release build, headless, start page:

| | begin | draw | end |
|---|---|---|---|
| First frame | 108 ms | 45 ms | 48 ms |
| Every frame after, before | 0 | 0.7 ms | 26 to 40 ms |
| Every frame after, now | 0 | 0.7 ms | 1.2 to 2.7 ms |

The drawing was never the cost. The present was: the render target was made to wait for the
screen's vertical blank, so each frame held the UI thread, which also runs the pages' events, the
agent calls and the person's input, for a whole refresh or two. Now it presents and goes on
(`D2D1_PRESENT_OPTIONS_IMMEDIATELY`). Windows composes the window through DWM, so nothing
tears.

The first frame makes the GPU target, the fonts and the icon masks, and will stay slow. The
journal now writes it as `ui_first`, so `ui_slow` only counts real stutter.

## Agents, 18 October

`tests/agent_bench.rs` runs fixed tasks on local pages through the agent door, three times each in
one warm headless Lumen. No model: the number of calls stands in for the model's round trips, the
reply bytes for what it reads.

```
cargo test --release -j 2 --test agent_bench -- --ignored --nocapture
```

Where the time went, before: not the browser. A snapshot of a page is 2 ms of WebView2 work. But
every agent call redrew the whole sidebar, about 35 ms, and a tool makes several calls to the
window (the policy look-up, the script, the DevTools events). And the waits were clocks: 550 ms at
least for a page to load, 350 ms after every click in case it navigated, 400 ms of network quiet.

Before and after, both debug builds (a release build is within a few percent: the waits are the page's, not Rust's):

| Task | 0.3.0 | 0.4.0 | Calls | Reply bytes |
|---|---|---|---|---|
| read title and URL | 1147 ms | 749 ms | 2 | 204 |
| extract 100 links | 1094 ms | 736 ms | 2 | 8805 |
| fill and send a form | 5716 ms | 1684 ms | 9 | 1662 |
| table built by JS | 1320 ms | 764 ms | 3 | 1316 |
| ten-step workflow | 8772 ms | 2639 ms | 12 | 3963 |
| two tabs, two reads | 2135 ms | 1381 ms | 4 | 676 |

The same tasks with the new tools, 0.4.0 release:

| Task | Time | Calls | Reply bytes |
|---|---|---|---|
| form, `browser_fill` | 1721 ms | 3 (from 9) | 1038 |
| links, `browser_links` | 905 ms | 2 | 6956 |
| table, `browser_table` | 1164 ms | 3 | 1637 |
| workflow, a Luau script (it also reads a second result) | 3358 ms | 1 (from 12) | 580 |
| two tabs, `browser_batch` | 1380 ms | 1 (from 4) | 1029 |

Wall time barely moves with fewer calls: here the calls are local and the waits are the page's.
What moves is the model's side: one round trip instead of twelve, 580 bytes to read instead of
4 KB. With a model that thinks for a few seconds a turn, that is the whole game.

| Per call, 0.4.0 release | |
|---|---|
| HTTP alone (ping) | 0.5 ms |
| a round trip to the window | 0.7 ms (35 ms before) |
| snapshot | 2 ms |
| open, warm session | 756 ms: view 100 to 180 ms, load about 240 ms, 250 ms of network quiet |
| open, first in a new session | about 2.6 s: WebView2 builds the agent's own profile |
| click | 357 ms |
| type | 137 ms |

Memory: a warm idle Lumen is 19.2 MB, 19.1 MB at 0.3.0. Luau is in the exe (15.5 MB, from
14.0 MB) and costs nothing until a script runs: each run gets a fresh VM, capped at 32 MB, gone
when it ends.

## Group A, 18 October

Three changes, measured one at a time on this laptop: release builds into `target/perf-a`,
headless, a fresh throwaway profile under `%TEMP%` for every run, three tabs at most, muted
(headless is). "before" is the tree as that change found it; "after" is that tree plus the one
change, built the same way. A change that did not help was reverted; what follows says what each
one did.

### Hidden tabs: the low memory target, at once

WebView2's `SetMemoryUsageTargetLevel` was only ever set 30 seconds into hiding, by the sleep
policy's trim, and an agent's stage views never got it at all. Now the place that already decides
a page is on screen, `sync_panes`, tells WebView2 `Low` the moment one goes off screen, and
`Normal` before a page that comes back paints. The minimize path says the same on restore. An
agent's stage view goes `Low` when its agent has been idle for `STAGE_IDLE` (30 s) and `Normal`
when a call wakes it. The 30 s trim still runs; it is no longer the first time WebView2 hears
about a page that is gone.

Method: release, headless, a fresh profile per run under `%TEMP%`, warmed once first so the
filter lists are already on disk. Three tabs: a long Wikipedia article as the shown tab (the
restored active tab), a news front page and another Wikipedia article as agent tabs on the
off-screen stage. The number is the private working set of `lumen.exe` plus every WebView2 process
whose command line names this run's data folder, summed at 90 s. The journal's own `sample` line,
which walks the same tree through `engine::memory`, rides beside it. Three runs each.

| run | before at 90 s | before, journal 60 s / 120 s | after at 90 s | after, journal 60 s / 120 s |
|---|---|---|---|---|
| 1 | 215.0 MB | 219.0 / 213.0 MB | 182.0 MB | 180.1 / 180.7 MB |
| 2 | 214.3 MB | 220.3 / 212.4 MB | 182.1 MB | 185.3 / 185.9 MB |
| 3 | 214.9 MB | 220.4 / 212.4 MB | 154.9 MB | 158.0 / 154.7 MB |
| median | 214.9 MB | | 182.0 MB | |

**Kept: 214.9 MB to 182.0 MB, 33 MB less (15%)** for two hidden heavy pages, with the shown tab
costing what it always did. All three pages loaded in every run (`nav ... ok=True`, and the
journal's `tabs=3 live=3` at both samples). The after side spread 27 MB between its runs (154.9
to 182.1); how much Chromium lets go when it is told a page is gone depends on the run. Every
after run was below every before run; the median is the number to read.

### mimalloc as the allocator

`mimalloc` 0.1.52, default features off, as `#[global_allocator]` in `src/main.rs`. `lumen-agent`
was left alone: it is a one-shot bridge that lives for one call, and neither measure below is its.

Measured the way "Idle memory" says: a warm idle A/B, first run fetches the filter lists, second
run measured after 60 s, three samples. Release, headless, a fresh profile per pair under `%TEMP%`,
no page ever built, so the number is `lumen.exe`'s own private working set. The agent bench's p50
column rides beside it, before and after.

| build | idle at 60 s | 63 s | 66 s | bench, whole run | `browser_open` |
|---|---|---|---|---|---|
| before | 19.5 MB | 19.5 MB | 19.5 MB | 43 s | 718 ms a call |
| mimalloc | 80.5 MB | 80.5 MB | 80.5 MB | 39 s | 548 ms a call |
| mimalloc, a second A/B | 80.8 MB | 80.8 MB | 80.8 MB | | |
| after the revert | 19.7 MB | 19.7 MB | 19.7 MB | | |

The whole p50 column, one bench run each side:

| task | before | mimalloc |
|---|---|---|
| read title and URL | 604 ms | 600 ms |
| extract 100 links | 548 ms | 515 ms |
| fill and send a form | 1464 ms | 1356 ms |
| table built by JS | 584 ms | 534 ms |
| ten-step workflow | 2010 ms | 2050 ms |
| two tabs, two reads | 1177 ms | 1086 ms |
| form, `browser_fill` | 1410 ms | 1389 ms |
| links, `browser_links` | 564 ms | 571 ms |
| table, `browser_table` | 652 ms | 640 ms |
| workflow, Luau script | 2204 ms | 2201 ms |
| two tabs, `browser_batch` | 1203 ms | 1132 ms |
| floor, ping / help / tabs | 0.5 / 0.8 / 0.9 ms | 0.6 / 1.0 / 1.3 ms |

**Reverted: idle 19.5 MB to 80.5 MB.** The bench was not slower — a little faster, and every task
p50 within a few percent of before (the largest moves: 1464 to 1356 ms on the form, 1203 to
1132 ms on the batch). The criterion is idle, and it grew by 61 MB: mimalloc's own arena is the
process's private pages too, and the first segment stays resident even with nothing in it Lumen
asked for. A shell whose whole point is what it does not hold cannot spend 61 MB it is not using.
The idle number came back to 19.7 MB the moment the dependency left.

### Pre-warming an agent session's profile

At `initialize` the session's WebView2 profile is made ready off the request path: a hidden page of
that profile in the shared environment (dropped at once, the profile stays), or the whole
environment for `agent_strict_isolation`. An op whose answer nobody waits for; the door replies
without it.

Measured with the new bench line `first open, fresh session`: initialize, then one `browser_open`
of the bench's local page, timed from initialize's reply, five fresh sessions a run, three runs,
p50.

| run | before | after |
|---|---|---|
| 1 | 626 ms | 764 ms |
| 2 | 623 ms | 743 ms |
| 3 | 624 ms | 722 ms |

**Reverted: 624 ms to 743 ms, and that number is a finding.** In a Lumen that has already built a
page, a fresh session's profile costs about 70 ms — the bench says a fresh session's first open is
about 0.6 s, not the 2.6 s the Agents section names. Pre-making the profile spends a whole hidden
page instead (the docs' own `view` figure is 100 to 180 ms), on the UI thread, inside the same
window: the window grew by about 120 ms, which is the hidden page minus the profile it made, and
one session initialize stalled the UI (in that run the bench's own `browser_help` floor read
266 ms a call instead of 0.8). The 2.6 s belongs to the first session of a fresh Lumen, where the
environment is made as well as the profile; that case was not measured here, so nothing is claimed
for it and the pre-warm is not kept on faith. The bench line stays: it is the measurement, and it
is the first number this repo has for a fresh session's first open.
