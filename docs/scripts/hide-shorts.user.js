// ==UserScript==
// @name Hide YouTube Shorts
// @match *://*.youtube.com/*
// ==/UserScript==
// The shelf of Shorts on the home page and in search, and the Shorts button in the sidebar.
function hide() {
  for (const e of document.querySelectorAll(
    'ytd-reel-shelf-renderer, ytd-rich-shelf-renderer[is-shorts], ytd-guide-entry-renderer:has(a[title="Shorts"])'
  )) e.remove();
}
hide();
// YouTube builds the shelves after the first paint, so look again as they come.
new MutationObserver(hide).observe(document.documentElement, { childList: true, subtree: true });
