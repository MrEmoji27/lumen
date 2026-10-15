# Lumen for AI agents

Lumen is a real browser that agents can drive. Not a headless toy, not a test harness: the same
browser you use, with your downloads, your ad blocker, your history search. An agent gets its own
tabs, marked AI in the sidebar, and cannot touch yours unless you say so.

## Connect

One program does it: `lumen-agent.exe`, next to `lumen.exe`. It finds the running Lumen, reads the
secret token itself, and starts a headless Lumen if none is running. Your agent config holds a
path. Never a secret.

| Agent | How |
|---|---|
| Claude Code | `claude mcp add -s user lumen -- C:\Users\mremo\projects\browser\target\release\lumen-agent.exe mcp` |
| opencode | in `~/.config/opencode/opencode.json`: `"mcp": {"lumen": {"type": "local", "command": ["...\\lumen-agent.exe", "mcp"]}}` |
| Codex | in `~/.codex/config.toml`: `[mcp_servers.lumen]` `command = '...\lumen-agent.exe'` `args = ["mcp"]` |
| Cursor, Windsurf, anything MCP | a stdio server: command `lumen-agent.exe`, argument `mcp` |
| A worker with only a shell | `lumen-agent browser_open url=example.com` and friends, see below |

All three of the first rows are set up on this PC already.

The switch is Settings > AI agent. Off means the door is shut: every call gets an answer saying so.

## From a shell

Same tools, one call per command. Good for workers that have Bash but no MCP.

```
lumen-agent open url=news.ycombinator.com          # prints: tab 3, then the snapshot
lumen-agent click tab=3 ref=e12
lumen-agent type tab=3 ref=e2 "text=rust browsers" submit=true
lumen-agent article tab=3                          # just the text, cheapest to read
lumen-agent screenshot tab=3 full_page=true        # prints the path of the image
lumen-agent tools                                  # everything there is
```

`--session NAME` keeps a worker's calls in its own set of tabs. Exit code 1 means the tool said no
(the message says why), 2 means Lumen could not be reached.

## How an agent sees a page

Like Playwright MCP and agent-browser: a snapshot, not a screenshot. One line per heading and per
control, each control with a ref.

```
Title: Agent test
URL: http://127.0.0.1:8765/agent.html

- heading "Form" [h2]
- textbox "Search words" [ref=e1]
- combobox "s" [ref=e2] value="Apple" options: Apple | Banana | Cherry
- checkbox "Remember" [ref=e3]
- button "Go" [ref=e5]
```

Act by ref. Take a new snapshot after the page changes; refs are made fresh each time.

## The tools

| Read | Act | Watch | Other |
|---|---|---|---|
| snapshot | click (left, right, double, x/y) | console (logs, errors, dialogs) | open, navigate, back, forward, reload |
| read (snapshot + text) | type (real keyboard text) | network (method, status, URL) | tabs, show, close |
| article (Readability) | press (Enter, Control+A...) | wait (text, gone, selector, URL) | screenshot (viewport, full, element) |
| find | select, scroll, hover, drag | dialogs (accept or dismiss) | pdf, download, downloads |
| eval (awaits promises) | upload (files into a file input) | resize (mobile, tablet, any) | handoff (to the person) |

Clicks, keys and typing go through the DevTools Protocol, so the page sees trusted events, the
same as a person's. A synthetic `element.click()` is only used when something covers the element.

Alerts and confirms do not freeze anything: Lumen answers them (accept by default) and writes what
they said into the console log.

**handoff** is for what an agent must not do alone: a login, a captcha, a payment, a choice. The
tab comes to the front, the sidebar shows the agent's message and a Done button, and the agent
waits until the person presses it.

## Many agents at once

Every MCP session owns the tabs it opens. Agent A cannot list, read, click or close agent B's tabs.
When a client disconnects, its tabs close. Workers can browse side by side without stepping on each
other.

## Out of the way

`lumen.exe --headless` (or `LUMEN_HEADLESS=1`) runs a real Lumen whose window sits off-screen. It
never takes focus, has no taskbar button and makes no sound, and pages still paint at full speed,
so screenshots work. lumen-agent starts this kind when no Lumen is running. It leaves by itself
after 10 minutes with nothing to do.

Start `lumen.exe` normally while a headless one runs and the headless one becomes your window:
centred, in the taskbar, with sound. One Lumen per profile, always.

## Safety

- Localhost only. A token on every request. Host and Origin checked, so a web page cannot talk to
  the door by pretending to be local. Bodies capped at 1 MB.
- Agent tabs live in their own profile (own cookies) unless Settings shares yours.
- Agent tabs never go into history, never save passwords, get no site permissions.
- The "Agent working · Stop" chip in the sidebar shuts the door and closes every agent tab.

## What I'd still add

What I, an agent, would want next, in order:

1. **Network bodies.** `browser_network` shows URL and status; for debugging an API I want the
   response body of one request (DevTools `Network.getResponseBody`).
2. **Element screenshots in the snapshot.** A tiny image per ambiguous control (three buttons all
   called "Edit") would save a round of guessing.
3. **Recording.** Save a sequence of tool calls and replay it: a test, written by browsing once.
4. **Cookies and storage in and out.** Save a logged-in state and load it in another session
   (agent-browser's `state save`), so a worker does not need a handoff every time.
5. **Frames.** Snapshots stop at iframes. Payment forms and embeds live in them.
6. **Native drag and drop.** HTML5 drag events need DevTools' drag interception; mouse drags work today.
