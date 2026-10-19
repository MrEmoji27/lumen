# Changelog

One version number, in `Cargo.toml`. Every exe also carries the commit it was built from and
that commit's date: `lumen-agent --version`, the Settings page under its title, and the agent
door's server info all say it. `+dev` means the build had changes nobody committed.

The rule: a release build that goes to the person gets the next number. Small fixes bump the last
digit. A batch of new features bumps the middle one.

## 0.6.7.2 — 2026-10-19

Measured against Brave, on the same machine and the same pages, and made to win.

- **The page you are watching gives memory back.** Fifteen seconds after your last click or key,
  the page on screen is told to keep only what it needs (WebView2's low memory target); the next
  click or key gives it everything back within a second. Pages still load at full memory, video
  plays at full frame rate, and scrolling a trimmed page did not drop a frame in testing. Settings >
  Performance turns it off.
- **YouTube on hardware without AV1 decoding gets VP9.** This GPU decodes VP9 but not AV1, so AV1
  was decoded on the CPU, in the page's own memory. Lumen now tells YouTube AV1 is not on offer
  when the engine says it would not be efficient: about 50 MB less and half the CPU on a video.
- Against Brave (tools/vs_brave.py, fresh profiles, the whole process tree, private working set):
  nothing open 36 MB to Brave's 151; one Wikipedia article 49 to 192; one YouTube video 327 to
  600; ten sites 331 to 1169, and 124 to 1111 after six and a half minutes. Those are with you
  away from the keyboard for 15 s. While you scroll and click, a single heavy page costs about
  what it costs in Brave (YouTube 653 to 553, Wikipedia 218 to 193); the start page and many
  tabs still win (37 to 156, 706 to 1069, and 311 to 1057 after six and a half minutes).

## 0.6.7.1 — 2026-10-19

The first side-by-side test against Brave, on the same YouTube video, found Lumen using four
times the CPU. The cause was Lumen's own: one of its memory savings cost the graphics card's
video decoding.

- **Video decodes on the graphics card again.** "Graphics in the browser process" saved about
  55 MB, and it also sent every video to the CPU: on a 30-second YouTube test, 8.3% CPU and 93
  dropped frames with it, 4.3% and 11 without. It is off now, for everyone who had the old
  default, and its row in Settings says what it costs.
- **Menus and cards stay above a playing video.** A video repaints all the time and painted
  over the chrome's floating cards: the tab menu in the compact sidebar was cut off at the
  page's edge. Every page window clips its neighbours now.
- The close, sound and picture-in-picture buttons on the active tab no longer turn into a dark
  block under the pointer.
- Versions now have four numbers, 0.6.7.1 and on, while 0.6.7 is the one being shared.

## 0.6.7 — 2026-10-19

The one to hand to other people. Every piece of the interface is Lumen's own now, it installs
like a program, and it tells you when it cannot run.

- **All of it drawn by Lumen.** The colour picker, every form (add a tile, edit a bookmark, name
  a folder, save a look), every yes-or-no question, every right-click menu and every file picker
  are Lumen's own cards, in Lumen's look, light or dark. No grey Windows boxes left. A web page's
  right-click menu is Lumen's too, with WebView2's own items in it and two more: open the link in
  a private tab, search for what you selected.
- **Pages ask in Lumen's cards.** A page's alert, confirm, prompt and "leave this site?" come up
  over that page, naming the site. A page that keeps asking gets "Stop this page asking". A site
  that wants a user name and password gets a sign-in card. A page that cannot load gets Lumen's
  own error page that says why in plain words, with Try again. Where a link goes shows in a small
  pill at the foot of the page.
- **The address bar finishes the name.** Type `yout` and it fills in `youtube.com`, the site you
  go to most; Enter goes there. Backspace takes it away. The start page's box does the same.
- **Calls stay up.** A tab in a call (a live microphone, camera or screen share, or a call link
  that is up) is never put to sleep, talking or not. Discord froze when nobody spoke; it does not
  now. The tab shows a microphone.
- **Ads on YouTube's first page.** A YouTube tab that loaded the moment Lumen started (a restored
  tab, a link that opened Lumen) got its ads. The first page now waits for the blocker, at most
  1.5 s, about 0.1 s.
- **Less memory when it counts.** Hidden tabs get a low memory target the moment they hide. When
  Windows runs short of memory, the hidden tab you looked at longest ago is unloaded first, never
  one with sound, a call, a pin or a working agent. On battery, battery saver puts hidden tabs to
  sleep sooner and calms the motion; a leaf in the footer says so.
- **New:** a command palette on Ctrl+K; turning blocking off for one site from the lock; picture
  in picture, a now-playing card in the footer, and the media keys; userscripts (`*.user.js`);
  for agents, snapshots that send only what changed and `browser_extract`.
- **Shipping.** An installer (`lumen-0.6.7-setup.exe`, for your user, no admin; it fetches
  WebView2 if the PC has none) and a zip with a README. Without WebView2, Lumen says so in a
  window of its own and opens Microsoft's download page.

## 0.6.6 — 2026-10-18

The real YouTube fix.

- Hover previews would not play and videos froze on load: Lumen was refusing every request to
  google.com, including the script YouTube runs before it will hand over a video. The phishing
  list names single bad links on big sites (one Google Doc, one bit.ly link, `google.com/amp/...`),
  and Lumen read each such line as "this whole host is dangerous", then counted every subdomain
  under it too. That took down all of google.com, docs.google.com, accounts.google.com,
  sites.google.com, bit.ly, t.co and dropbox.com, ad blocking on or off. Now a host rule
  (`||evil.example^`) still blocks the host and what is under it, and a link rule blocks that
  link only (with or without its query), as URL hashes. It came in with the malware lists on
  18 October, the same day as the redraw.
- 0.6.5's list refresh stays (lists keep as long as they say), but it was not the cause.

## 0.6.5 — 2026-10-18

- YouTube videos failed with "An error occurred" and "Something went wrong" because the ad
  lists had gone stale. uBlock's quick-fixes list follows YouTube's player changes and says it
  keeps for 8 hours; Lumen refreshed every list every four days, and only when it started. Now
  each list keeps as long as its own `Expires:` line says (between 6 hours and 4 days), only the
  stale ones are fetched again, and a Lumen left running looks once an hour and rebuilds the
  matcher behind the one in use.

## 0.6.4 — 2026-10-18

Reports reach zemo.

- Report a problem has a box for what happened and a **Send to zemo** button. Send posts
  exactly the text on the page, with your words in it, to zemo's inbox (through Formspree).
  Nothing goes without the press; a report goes once; when it cannot go, the page says why and
  Copy still works.
- After a run that never closed properly (a crash, a kill, a power cut), the start page says so
  once and offers a report. Its x puts it away.
- The report's journal times are local time.

## 0.6.3 — 2026-10-18

For daily use.

- Every frame of the window drew in under a millisecond and then waited 16 to 40 ms for the
  screen's next refresh before Lumen could do anything else: pages, agent calls and your clicks
  queued behind it. It no longer waits (Windows composes the window, so nothing tears). A frame
  now costs about 1.5 ms all told.
- Settings > About: the version with **What's new**, **Report a problem**, and who made Lumen.
- What's new: the changelog is built into Lumen and shown as a page, a card per version. After
  an update the start page says so once, and opens it.
- Report a problem: a plain-text report you read in full first: version, Windows, WebView2,
  your look and layout, uptime, and the journal's recent starts, exits, crashes, errors and
  stalls. No addresses, no page titles, nothing typed into pages. Copy it, save it to
  Downloads, or open the page to send it to.
- Made by zemo, in Settings, linked to [the portfolio](https://portfolio.mremoji47.workers.dev/).
- The journal reports the first frame of a run as `ui_first`, on its own: it builds the GPU
  target and the fonts and was always "slow", which hid the real stalls in `ui_slow`.
- An MCP client talking to the door straight over HTTP was told "lumen-agent is older than
  Lumen: reconnect" at the top of every reply. Only lumen-agent says which build it is; a
  client that says nothing is not lumen-agent and is now told nothing.
- The live agent checklist passes all nine points. It had failed on that line since it was
  written, and a failed run left its Lumen running; it now stops it whatever happens.
- While a card floats over a page, a tab opened meanwhile no longer covers it.
- The start page's Ctrl L chip in Glass no longer wraps its L away.

## 0.6.2 — 2026-10-18

- Closing the start page no longer wakes a pinned tab. Pins restored at launch were never
  visited, so "the tab you were on before" had nothing to say, and the old fallback took the
  neighbour in the row: a pin, which then loaded. Now: the tab you were on before, else the
  nearest plain tab, else a pin that already has its page, else a fresh start page. A pin
  without its page is never where you land.
- Lumen's memory shows before any page has opened. It was measured through WebView2, which
  does not exist until the first page does, so a start page alone showed nothing.
- Every layout gets every card. Horizontal had no tab search, no message card, no "needs
  you" card, no site prompt and no address suggestions: they hung off a sidebar it does not
  have. Hybrid had no address suggestions. A compact sidebar squeezed the cards to its icon
  column; they now keep a readable width and grow over the page. The page is a window above
  the chrome, so while a card reaches over it the chrome rises above the page, cut down to
  its own strips and the cards, and sinks back when they close.
- The horizontal tab strip draws like the sidebar: pins as small cards, the current tab as
  the ink pill, quiet letter tiles.
- Drive scrolls under its task card instead of not at all; a step's time lands on its own
  step when an agent has several calls in the air; the settings search box hides under the
  font list.

## 0.6.1 — 2026-10-18

The rest of the redraw.

- Drive, as drawn: the task in a card with Run, Stop and "Pause and take over" by the title,
  the steps numbered in plain words ("going to amazon.com", "typing ...") with how long each
  took, the one going now marked NOW, and what the agent says in a card beside them.
- Settings has a search box. Sections keep only what matches; the menu dims the rest.
- Bookmarks and Site settings: cards, site icons, spaced labels, and Allow / Block / Ask as one
  segmented control.
- The floating cards (tab search, the site card under the lock, address suggestions, "needs
  you", a site asking for the camera, the message card) are one kind of card: soft shadow, pill
  fields and buttons, ink for the main button. Opaque in Glass, so nothing shows through.
- Start page tiles show the site's real icon. A site with none is asked for its own icon once
  per run, straight from the site; no icon service sees the address.
- Gone: Title weight, Tile size and Display font did nothing since 0.6.0; the looks own them.
  Clock face now does what it says: the look's clock, or the interface font at its size.

## 0.6.0 — 2026-10-18

Lumen, redrawn, to docs/design/lumen-ui.html.

- Three looks, each a whole set of colours, type, corners and motion: **Signal** (the default:
  warm paper and ink, monospace labels, one red signal; light and dark), **Glass** (a frosted
  sidebar over a colour wallpaper, the page a floating card) and **Precise** (near-black,
  hairlines, square-ish corners, one violet accent). Settings > Appearance picks one.
- First run: "Pick a look", Signal chosen. Anyone who already had Lumen never sees it.
- One accent, one meaning: something live (a working agent, a running Drive, the download
  badge, the clock's colon). Buttons, chosen options and the active tab are ink.
- One sans for everything; titles bold; labels, figures and addresses in monospace. The display
  serif is gone from the chrome.
- Sidebar: a pill address field, pinned tabs as cards, a TABS label with +, the active tab as a
  solid pill that slides, AI and PRIVATE as quiet tags, agent rows with Message and Stop, the
  footer's buttons on the left and the memory on the right.
- Start page: a big monospace clock, "blocked today" and Lumen's own memory as cards, an ink
  search bar, site cards with icon, name and address.
- Settings: an icon menu, every section a card. History, Downloads, Dashboard (one row per agent),
  Control and the Privacy report: cards, spaced labels, ink bars.
- Closing a tab goes back to the tab you were on before it, not to a pinned tab.
- Site icons are kept on disk by site, so tiles and History show them.

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
