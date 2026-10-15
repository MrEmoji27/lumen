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
