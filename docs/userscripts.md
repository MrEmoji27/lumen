# Userscripts

Lumen has no extensions. A userscript covers most of what people install them for: a page
with a script of your own in it.

Put `*.user.js` files in `userscripts\` in Lumen's data folder:

    %APPDATA%\Lumen\userscripts

A file needs a header; the rest is plain JavaScript, run in every page it matches.

```js
// ==UserScript==
// @name Hide YouTube Shorts
// @match *://*.youtube.com/*
// ==/UserScript==
function hide() {
  for (const e of document.querySelectorAll('ytd-reel-shelf-renderer, ytd-rich-shelf-renderer[is-shorts]')) e.remove();
}
hide();
new MutationObserver(hide).observe(document.documentElement, { childList: true, subtree: true });
```

- `@name` is what the script is called. A script without one is called after its file.
- `@match` says which pages it runs on; repeat it for more. The pattern is Chrome's:
  `scheme://host/path`, where `*` as the scheme means http and https, `*.example.com`
  means example.com and its subdomains, and `*` inside the path matches any run, as in
  `https://a.b/path*`. `<all_urls>` means every page. No `@match` at all: it never runs.
- `@exclude` takes the same patterns; a page that matches one does not get the script.
- `@run-at` is when it runs: `document-start`, `document-end` (the default) or
  `document-idle`.

Scripts run in your own tabs, never in an agent's tab and never in a private one. Each
one runs on its own: a script that fails or is not even parseable cannot stop the others.
A page whose `Content-Security-Policy` forbids it will not let a script's text be
compiled: the page's console shows one error per script, and the page is untouched.
The folder is read when a tab is made, so a new script needs a new tab (or a reload,
after a restart). `userscripts` in settings.json (on by default) turns them all off; a
change reaches tabs opened after it.

`docs/scripts/hide-shorts.user.js` is the example above, ready to copy.
