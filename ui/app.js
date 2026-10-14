// Lumen UI. Rust owns the truth and calls lumen.state() at most once a frame.
// This file keeps the DOM in step with it, touching only what changed.
'use strict';
(() => {
const $ = (s, r = document) => r.querySelector(s);
const root = document.documentElement;
const send = m => { try { window.ipc.postMessage(JSON.stringify(m)); } catch (_) {} };
const esc = s => String(s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const tog = (el, c, on) => { if (el.classList.contains(c) !== on) el.classList.toggle(c, on); };
const attr = (k, v) => { if (v == null || v === false) root.removeAttribute(k); else if (root.getAttribute(k) !== String(v)) root.setAttribute(k, v === true ? '' : v); };
const host = u => { try { return new URL(u).hostname.replace(/^www\./, ''); } catch (_) { return ''; } };

// ------------------------------------------------------------------ icons
const I = {
  back: '<path d="M10 3.5 5.5 8l4.5 4.5"/>',
  fwd: '<path d="M6 3.5 10.5 8 6 12.5"/>',
  reload: '<path d="M12.8 8.6A4.8 4.8 0 1 1 11.3 4.4"/><path d="M12.6 2.8v2.6H10"/>',
  stop: '<path d="m4.5 4.5 7 7m0-7-7 7"/>',
  min: '<path d="M4 8.5h8"/>',
  max: '<rect x="4" y="4" width="8" height="8" rx="1.6"/>',
  restore: '<rect x="3.5" y="5.5" width="7" height="7" rx="1.4"/><path d="M6 5.5V5a1.5 1.5 0 0 1 1.5-1.5H11A1.5 1.5 0 0 1 12.5 5v3.5A1.5 1.5 0 0 1 11 10h-.5"/>',
  close: '<path d="m4.5 4.5 7 7m0-7-7 7"/>',
  plus: '<path d="M8 3.5v9M3.5 8h9"/>',
  search: '<circle cx="7.2" cy="7.2" r="4.2"/><path d="m10.4 10.4 2.8 2.8"/>',
  lock: '<rect x="4" y="7" width="8" height="6" rx="1.6"/><path d="M5.8 7V5.6a2.2 2.2 0 0 1 4.4 0V7"/>',
  globe: '<circle cx="8" cy="8" r="5.2"/><path d="M2.8 8h10.4M8 2.8c1.6 1.6 2.2 3.3 2.2 5.2S9.6 11.6 8 13.2C6.4 11.6 5.8 9.9 5.8 8S6.4 4.4 8 2.8"/>',
  tab: '<rect x="2.8" y="3.5" width="10.4" height="9" rx="2"/><path d="M2.8 6.5h10.4"/>',
  sliders: '<path d="M3 5h6.5M12.5 5H13M3 11h.5M6.5 11H13"/><circle cx="11" cy="5" r="1.5"/><circle cx="5" cy="11" r="1.5"/>',
  sound: '<path d="M3 6.4h2.1L8 4v8L5.1 9.6H3z"/><path d="M10.6 6.1a2.8 2.8 0 0 1 0 3.8"/>',
  muted: '<path d="M3 6.4h2.1L8 4v8L5.1 9.6H3z"/><path d="m10.5 6.5 3 3m0-3-3 3"/>',
};
const svg = n => `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">${I[n]}</svg>`;
document.querySelectorAll('[data-i]').forEach(el => { el.innerHTML = svg(el.dataset.i); });

// ------------------------------------------------------------------ state
let S = null;
const favs = new Map();
const rows = new Map();
const active = () => S && S.tabs.find(t => t.id === S.active);

window.lumen = {
  state(s) { S = s; render(); },
  fav(id, uri) {
    favs.set(id, uri);
    const r = rows.get(id);
    if (r) setFav(r, uri);
  },
  focusOmnibox() { addr.focus(); addr.select(); },
  usage(u) { paintUsage(u); },
};

function render() {
  look();
  frame();
  chrome();
  renderTabs();
  omniIdle();
  footer();
  pages();
}

// ------------------------------------------------------------------- look
let lookKey = '';
function look() {
  const st = S.settings;
  const compact = st.compact && !S.peek;
  const key = [st.palette, st.accent, st.backdrop, S.dark, st.sidebar_side, st.sidebar_width, compact, st.radius, st.animations].join('|');
  if (key === lookKey) return;
  lookKey = key;
  attr('data-theme', S.dark ? 'dark' : 'light');
  attr('data-palette', ['graphite', 'sand', 'slate', 'moss', 'rose'].includes(st.palette) ? st.palette : 'graphite');
  attr('data-side', st.sidebar_side === 'right' ? 'right' : 'left');
  attr('data-compact', compact);
  attr('data-glass', st.backdrop !== 'solid');
  attr('data-anim', st.animations);
  root.style.setProperty('--sw', (compact ? 56 : st.sidebar_width) + 'px');
  root.style.setProperty('--radius', st.radius + 'px');
  if (st.accent) root.style.setProperty('--user-acc', st.accent); else root.style.removeProperty('--user-acc');
  // Tell Rust the sidebar color, so the window behind the page gaps matches.
  requestAnimationFrame(() => send({ t: 'bg', rgb: rgbOf(getComputedStyle(root).getPropertyValue('--bg')) }));
}

const probe = document.createElement('canvas').getContext('2d', { willReadFrequently: true });
function rgbOf(color) {
  probe.clearRect(0, 0, 1, 1);
  probe.fillStyle = color.trim();
  probe.fillRect(0, 0, 1, 1);
  const d = probe.getImageData(0, 0, 1, 1).data;
  return [d[0], d[1], d[2]];
}

function frame() {
  attr('data-frame', S.frame);
  attr('data-max', S.maximized);
  if (S.frame !== 'full') return;
  const [x, y, w, h] = S.stage;
  const st = stage.style;
  const px = v => Math.round(v) + 'px';
  if (st.left !== px(x)) st.left = px(x);
  if (st.top !== px(y)) st.top = px(y);
  if (st.width !== px(w)) st.width = px(w);
  if (st.height !== px(h)) st.height = px(h);
}

// ----------------------------------------------------------------- chrome
const back = $('#back'), fwd = $('#fwd'), reload = $('#reload'), maxBtn = $('#max');
let reloadIcon = 'reload', maxIcon = 'max';
function chrome() {
  const t = active();
  back.disabled = !(t && t.canBack);
  fwd.disabled = !(t && t.canFwd);
  const ri = t && t.loading ? 'stop' : 'reload';
  if (ri !== reloadIcon) { reloadIcon = ri; reload.innerHTML = svg(ri); reload.setAttribute('aria-label', ri === 'stop' ? 'Stop' : 'Reload'); }
  const mi = S.maximized ? 'restore' : 'max';
  if (mi !== maxIcon) { maxIcon = mi; maxBtn.innerHTML = svg(mi); }
}
back.onclick = () => send({ t: 'back' });
fwd.onclick = () => send({ t: 'fwd' });
reload.onclick = () => send({ t: reloadIcon === 'stop' ? 'stop' : 'reload' });
$('#min').onclick = () => send({ t: 'win', a: 'min' });
maxBtn.onclick = () => send({ t: 'win', a: 'max' });
$('#close').onclick = () => send({ t: 'win', a: 'close' });
$('#gear').onclick = () => send({ t: 'panel', open: S.panel !== 'settings', which: 'settings' });
$('#mem').onclick = () => send({ t: 'panel', open: S.panel !== 'control', which: 'control' });
$('#newtab').onclick = () => send({ t: 'newtab' });

// Drag the window from any empty bit of the top bar. Resize from the top edge.
$('#top').addEventListener('pointerdown', e => { if (e.button === 0 && !e.target.closest('button')) send({ t: 'drag' }); });
$('#top').addEventListener('dblclick', e => { if (!e.target.closest('button')) send({ t: 'win', a: 'max' }); });
$('#rz').addEventListener('pointerdown', e => { if (e.button === 0) send({ t: 'resize', d: 'n' }); });

// ------------------------------------------------------------------- tabs
const pinsEl = $('#pins'), tabsEl = $('#tabs'), list = $('#list');

function makeRow(t) {
  const el = document.createElement('div');
  el.className = 'tab';
  el.dataset.id = t.id;
  el.setAttribute('role', 'tab');
  el.innerHTML = '<span class="fav"><img alt=""><i class="mono"></i><i class="spin"></i></span>'
    + '<span class="title"></span><button class="snd" tabindex="-1"></button>'
    + `<button class="x" tabindex="-1" aria-label="Close tab">${svg('close')}</button>`;
  const r = { el, img: el.querySelector('img'), mono: el.querySelector('.mono'), title: el.querySelector('.title'), snd: el.querySelector('.snd'), k: {} };
  r.img.onload = () => el.classList.add('has-fav');
  r.img.onerror = () => el.classList.remove('has-fav');
  const f = favs.get(t.id);
  if (f) setFav(r, f);
  return r;
}

function setFav(r, uri) {
  if (r.k.fav === uri) return;
  r.k.fav = uri;
  r.el.classList.remove('has-fav');
  if (uri) r.img.src = uri; else r.img.removeAttribute('src');
}

function updRow(r, t) {
  const k = r.k;
  const title = t.title || (t.status === 'new' ? 'New tab' : host(t.url) || t.url || 'Untitled');
  if (k.title !== title) { k.title = title; r.title.textContent = title; r.el.title = title; }
  const m = t.status === 'new' ? '+' : (host(t.url) || '·')[0].toUpperCase();
  if (k.mono !== m) { k.mono = m; r.mono.textContent = m; }
  tog(r.el, 'active', t.id === S.active);
  tog(r.el, 'pin', t.pinned);
  tog(r.el, 'sleep', t.status === 'asleep' || t.status === 'off');
  tog(r.el, 'loading', t.loading);
  r.el.setAttribute('aria-selected', t.id === S.active);
  const snd = t.muted ? 'muted' : t.audio ? 'sound' : '';
  if (k.snd !== snd) {
    k.snd = snd;
    r.snd.innerHTML = snd ? svg(snd) : '';
    r.snd.setAttribute('aria-label', snd === 'muted' ? 'Unmute' : 'Mute');
    tog(r.el, 'has-snd', !!snd);
  }
}

function renderTabs() {
  const seen = new Set();
  let pi = 0, ti = 0;
  for (const t of S.tabs) {
    seen.add(t.id);
    let r = rows.get(t.id);
    if (!r) { r = makeRow(t); rows.set(t.id, r); }
    updRow(r, t);
    const parent = t.pinned ? pinsEl : tabsEl;
    const want = parent.children[t.pinned ? pi++ : ti++];
    if (want !== r.el) parent.insertBefore(r.el, want || null);
  }
  for (const [id, r] of rows) if (!seen.has(id)) { r.el.remove(); rows.delete(id); favs.delete(id); }
}

// Optimistic: move the highlight now, before Rust answers. Clicks feel instant.
function activate(id) {
  if (!S || id === S.active) return;
  S.active = id;
  for (const [rid, r] of rows) tog(r.el, 'active', rid === id);
  send({ t: 'activate', id });
}

let drag = null, eatClick = false;
list.addEventListener('pointerdown', e => {
  const el = e.target.closest('.tab');
  if (!el) return;
  if (e.button === 1) { e.preventDefault(); return; }
  if (e.button !== 0 || e.target.closest('button')) return;
  drag = { el, id: +el.dataset.id, x0: e.clientX, y0: e.clientY, moved: false, pid: e.pointerId };
});
list.addEventListener('pointermove', e => {
  if (!drag) return;
  const dx = e.clientX - drag.x0, dy = e.clientY - drag.y0;
  if (!drag.moved) {
    if (Math.hypot(dx, dy) < 5) return;
    drag.moved = true;
    drag.el.setPointerCapture(drag.pid);
    drag.el.classList.add('dragging');
    drag.sibs = [...drag.el.parentNode.children];
    drag.rects = drag.sibs.map(s => s.getBoundingClientRect());
  }
  drag.el.style.transform = drag.el.classList.contains('pin') ? `translate(${dx}px,${dy}px)` : `translateY(${dy}px)`;
});
list.addEventListener('pointerup', e => {
  const d = drag;
  drag = null;
  if (!d || !d.moved) return;
  d.el.classList.remove('dragging');
  d.el.style.transform = '';
  const pinned = d.el.classList.contains('pin');
  let idx = 0;
  d.rects.forEach((r, i) => {
    if (d.sibs[i] === d.el) return;
    const after = pinned
      ? e.clientY > r.bottom || (e.clientY >= r.top && e.clientX > (r.left + r.right) / 2)
      : e.clientY > (r.top + r.bottom) / 2;
    if (after) idx++;
  });
  const offset = pinned ? 0 : S.tabs.filter(t => t.pinned).length;
  send({ t: 'move', id: d.id, to: offset + idx });
  eatClick = true;
  setTimeout(() => { eatClick = false; }, 0);
});
list.addEventListener('click', e => {
  if (eatClick) return;
  const el = e.target.closest('.tab');
  if (!el) return;
  const id = +el.dataset.id;
  if (e.target.closest('.x')) send({ t: 'close', id });
  else if (e.target.closest('.snd')) send({ t: 'mute', id });
  else activate(id);
});
list.addEventListener('auxclick', e => {
  const el = e.target.closest('.tab');
  if (el && e.button === 1) send({ t: 'close', id: +el.dataset.id });
});

// ----------------------------------------------------------- context menu
const menu = $('#menu');
let menuPeek = false;
list.addEventListener('contextmenu', e => {
  e.preventDefault();
  const el = e.target.closest('.tab');
  if (el) openMenu(+el.dataset.id, e.clientY);
});
function openMenu(id, y) {
  const t = S.tabs.find(t => t.id === id);
  if (!t) return;
  if (S.settings.compact && !S.peek) { menuPeek = true; send({ t: 'peek', on: true }); }
  const items = [
    ['pin', t.pinned ? 'Unpin' : 'Pin'],
    t.url && ['duplicate', 'Duplicate'],
    (t.audio || t.muted) && ['mute', t.muted ? 'Unmute' : 'Mute'],
    t.id !== S.active && (t.status === 'live' || t.status === 'asleep') && ['sleep', 'Unload now'],
    '-',
    ['closeOthers', 'Close other tabs'],
    ['close', 'Close', 'Ctrl+W', 'danger'],
  ].filter(Boolean);
  menu.innerHTML = items.map(i => i === '-' ? '<hr>'
    : `<button data-a="${i[0]}" class="${i[3] || ''}">${esc(i[1])}${i[2] ? `<kbd>${i[2]}</kbd>` : ''}</button>`).join('');
  menu.dataset.id = id;
  menu.hidden = false;
  const h = menu.offsetHeight;
  menu.style.top = Math.max(8, Math.min(y, innerHeight - h - 8)) + 'px';
}
function closeMenu() {
  if (menu.hidden) return;
  menu.hidden = true;
  if (menuPeek) { menuPeek = false; send({ t: 'peek', on: false }); }
}
menu.addEventListener('click', e => {
  const b = e.target.closest('button');
  if (!b) return;
  send({ t: b.dataset.a, id: +menu.dataset.id });
  closeMenu();
});
document.addEventListener('pointerdown', e => { if (!menu.hidden && !menu.contains(e.target)) closeMenu(); }, true);
window.addEventListener('blur', () => { closeMenu(); if (editing) addr.blur(); });

// ---------------------------------------------------------------- omnibox
const addr = $('#addr'), sugg = $('#sugg');
let editing = false, picks = [], sel = 0, freshFocus = false;

function omniIdle() {
  const t = active();
  const url = t ? t.url : '';
  attr('data-secure', url.startsWith('https://'));
  // Focus went to the page (tab closed, Ctrl+Tab...) while we were editing: stop editing.
  if (editing && !document.hasFocus()) addr.blur();
  if (editing) return;
  const v = !t || t.status === 'new' ? '' : host(url) || url;
  if (addr.value !== v) addr.value = v;
}
addr.addEventListener('focus', () => {
  editing = true;
  freshFocus = true;
  const t = active();
  addr.value = t && t.status !== 'new' ? t.url : '';
  addr.select();
  if (S && S.settings.compact) send({ t: 'peek', on: true });
});
// A click into a fresh focus would undo the select-all. Keep it.
addr.addEventListener('mouseup', e => { if (freshFocus) { e.preventDefault(); freshFocus = false; } });
addr.addEventListener('blur', () => {
  editing = false;
  sugg.hidden = true;
  if (S) omniIdle();
  if (S && S.settings.compact) send({ t: 'peek', on: false });
});
addr.addEventListener('input', () => { freshFocus = false; suggest(); });
addr.addEventListener('keydown', e => {
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    if (!picks.length) return;
    e.preventDefault();
    sel = (sel + (e.key === 'ArrowDown' ? 1 : -1) + picks.length) % picks.length;
    paintSugg();
  } else if (e.key === 'Enter') {
    e.preventDefault();
    const q = addr.value.trim();
    if (!q) return;
    go(picks.length ? picks[sel] : { kind: 'go', q });
  } else if (e.key === 'Escape') {
    e.preventDefault();
    addr.blur();
    send({ t: 'focusPage' });
  }
});
const looksUrl = q => !/\s/.test(q) && (/^[a-z]+:\/\//i.test(q) || /^localhost(:\d+)?(\/|$)/i.test(q) || /^[^/?#]+\.[a-z]{2,}(:\d+)?([/?#]|$)/i.test(q));
function suggest() {
  const q = addr.value.trim();
  if (!q) { picks = []; sugg.hidden = true; return; }
  const lq = q.toLowerCase();
  const tabs = S.tabs.filter(t => t.id !== S.active && t.url && (t.title.toLowerCase().includes(lq) || t.url.toLowerCase().includes(lq))).slice(0, 3);
  picks = [];
  if (looksUrl(q)) picks.push({ kind: 'go', q, icon: 'globe', label: q, hint: 'Open' });
  picks.push({ kind: 'go', q, icon: 'search', label: q, hint: 'Search' });
  for (const t of tabs) picks.push({ kind: 'tab', id: t.id, icon: 'tab', label: t.title || host(t.url), hint: 'Switch to tab' });
  sel = 0;
  paintSugg();
}
function paintSugg() {
  sugg.innerHTML = picks.map((p, i) => `<div class="sg${i === sel ? ' on' : ''}" data-i="${i}" role="option">${svg(p.icon)}<b>${esc(p.label)}</b><small>${p.hint}</small></div>`).join('');
  sugg.hidden = false;
}
// pointerdown, not click: keeps focus in the box so blur does not race the pick.
sugg.addEventListener('pointerdown', e => {
  e.preventDefault();
  const el = e.target.closest('.sg');
  if (el) go(picks[+el.dataset.i]);
});
function go(p) {
  sugg.hidden = true;
  if (p.kind === 'tab') activate(p.id);
  else send({ t: 'nav', url: p.q });
  addr.blur();
}

// ----------------------------------------------------------------- footer
const mem = $('#mem');
let memText = '';
function footer() {
  const st = S.settings;
  let s = '';
  if (st.show_memory && S.mem) {
    const mb = S.mem.bytes / 1048576;
    s = (mb >= 1024 ? (mb / 1024).toFixed(2) + ' GB' : Math.round(mb) + ' MB');
    if (st.adblock && S.blocked) s += ' · ' + S.blocked.toLocaleString() + ' blocked';
  }
  if (s !== memText) { memText = s; mem.textContent = s; mem.title = S.mem ? S.mem.procs + ' processes' : ''; }
}

// ------------------------------------------------------------------ pages
const stage = $('#stage'), ntp = $('#ntp'), setEl = $('#set'), ctlEl = $('#ctl');
function pages() {
  const t = active();
  const showSet = S.panel === 'settings';
  const showCtl = S.panel === 'control';
  const showNtp = S.panel === 'none' && !!t && t.status === 'new';
  stage.hidden = !(showSet || showCtl || showNtp);
  if (ctlEl.hidden === showCtl) {
    ctlEl.hidden = !showCtl;
    if (showCtl) openControl();
  }
  if (showCtl) syncControl();
  if (ntp.hidden === showNtp) {
    ntp.hidden = !showNtp;
    if (showNtp) openNtp();
  }
  if (setEl.hidden === showSet) {
    setEl.hidden = !showSet;
    if (showSet) openSettings();
  }
  if (showSet) syncSettings();
}

// New tab page
const hh = $('#hh'), mm = $('#mm'), dateEl = $('#date'), ntpQ = $('#ntp-q'), sc = $('#sc');
let clockTimer = 0, scKey = '';
function tick() {
  const d = new Date();
  hh.textContent = String(d.getHours()).padStart(2, '0');
  mm.textContent = String(d.getMinutes()).padStart(2, '0');
  dateEl.textContent = d.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' });
}
function openNtp() {
  tick();
  clearInterval(clockTimer);
  clockTimer = setInterval(() => { if (ntp.hidden) clearInterval(clockTimer); else tick(); }, 10000);
  ntpQ.value = '';
  const wall = S.settings.wallpaper;
  ntp.classList.toggle('wall', !!wall);
  ntp.style.backgroundImage = wall ? `url("${wall.replace(/"/g, '%22')}")` : '';
  paintShortcuts();
  // Do not steal the caret from the address bar (Ctrl+T puts it there).
  if (document.activeElement === document.body || !document.activeElement) ntpQ.focus();
}
$('#ntp-form').addEventListener('submit', e => {
  e.preventDefault();
  const q = ntpQ.value.trim();
  if (q) send({ t: 'nav', url: q });
});
function paintShortcuts(force) {
  const list = S.settings.shortcuts || [];
  const key = JSON.stringify(list);
  if (key === scKey && !force) return;
  scKey = key;
  sc.innerHTML = list.map((s, i) => `<a class="chip" data-url="${esc(s.url)}" href="#"><i class="mono">${esc((s.title || host(s.url) || '?')[0].toUpperCase())}</i>${esc(s.title || host(s.url))}<button class="rm" data-rm="${i}" aria-label="Remove">${svg('close')}</button></a>`).join('')
    + `<button class="chip add" id="sc-add">${svg('plus')}Add</button>`;
}
sc.addEventListener('click', e => {
  e.preventDefault();
  const rm = e.target.closest('[data-rm]');
  if (rm) {
    const list = S.settings.shortcuts.slice();
    list.splice(+rm.dataset.rm, 1);
    setOpt('shortcuts', list, true);
    paintShortcuts(true);
    return;
  }
  if (e.target.closest('#sc-add')) return addShortcutForm();
  const chip = e.target.closest('.chip[data-url]');
  if (chip) send({ t: 'nav', url: chip.dataset.url });
});
function addShortcutForm() {
  sc.innerHTML = '<form class="sc-form"><input name="title" placeholder="Name"><input name="url" placeholder="Address"></form>';
  const f = sc.firstChild;
  f.title.focus();
  f.addEventListener('keydown', e => { if (e.key === 'Escape') paintShortcuts(true); });
  f.addEventListener('submit', e => {
    e.preventDefault();
    let url = f.url.value.trim();
    if (!url) return;
    if (!/^[a-z]+:\/\//i.test(url)) url = 'https://' + url;
    setOpt('shortcuts', [...S.settings.shortcuts, { title: f.title.value.trim() || host(url), url }], true);
    paintShortcuts(true);
  });
  f.url.addEventListener('keydown', e => { if (e.key === 'Enter') f.requestSubmit(); });
}

// --------------------------------------------------------------- settings
const PALETTES = [['graphite', 'Graphite'], ['sand', 'Sand'], ['slate', 'Slate'], ['moss', 'Moss'], ['rose', 'Rose']];
const ENGINES = [
  ['DuckDuckGo', 'https://duckduckgo.com/?q=%s'],
  ['Google', 'https://www.google.com/search?q=%s'],
  ['Brave', 'https://search.brave.com/search?q=%s'],
  ['Bing', 'https://www.bing.com/search?q=%s'],
  ['Startpage', 'https://www.startpage.com/do/search?q=%s'],
];
const SCHEMA = [
  ['Appearance', [
    { k: 'palette', type: 'swatch', label: 'Palette' },
    { k: 'accent', type: 'color', label: 'Accent', hint: 'Focus rings, toggles, the loading ring.' },
    { k: 'theme', type: 'seg', label: 'Theme', opts: [['dark', 'Dark'], ['light', 'Light'], ['system', 'System']] },
    { k: 'backdrop', type: 'seg', label: 'Backdrop', hint: 'Solid is fastest. Mica and Acrylic let the desktop through.', opts: [['solid', 'Solid'], ['mica', 'Mica'], ['acrylic', 'Acrylic']] },
    { k: 'sidebar_side', type: 'seg', label: 'Sidebar', opts: [['left', 'Left'], ['right', 'Right']] },
    { k: 'sidebar_width', type: 'range', label: 'Sidebar width', min: 180, max: 400, step: 4, unit: 'px' },
    { k: 'compact', type: 'toggle', label: 'Compact sidebar', hint: 'Icons only. Ctrl+B toggles it.' },
    { k: 'gap', type: 'range', label: 'Page margin', min: 0, max: 24, step: 1, unit: 'px' },
    { k: 'radius', type: 'range', label: 'Page corners', min: 0, max: 20, step: 1, unit: 'px' },
    { k: 'animations', type: 'toggle', label: 'Animations' },
  ]],
  ['Search', [
    { k: 'search', type: 'engine', label: 'Search engine', hint: 'Custom: any URL with %s where the words go.' },
  ]],
  ['Performance', [
    { k: 'adblock', type: 'toggle', label: 'Block ads and trackers', hint: 'Stops them before they load. Saves memory and data.' },
    { k: 'tracking', type: 'seg', label: 'Tracking prevention', opts: [['off', 'Off'], ['basic', 'Basic'], ['balanced', 'Balanced'], ['strict', 'Strict']] },
    { k: 'sleep_after_min', type: 'range', label: 'Freeze hidden tabs after', min: 1, max: 60, step: 1, unit: 'min', hint: 'Frozen tabs stop running and wake instantly.' },
    { k: 'discard_after_min', type: 'range', label: 'Unload hidden tabs after', min: 5, max: 240, step: 5, unit: 'min', hint: 'Unloaded tabs free all memory and reload when opened.' },
    { k: 'show_memory', type: 'toggle', label: 'Memory in sidebar' },
  ]],
  ['New tab', [
    { k: 'wallpaper', type: 'text', label: 'Wallpaper', hint: 'Image URL. Empty for none.', placeholder: 'https://…' },
  ]],
  ['Keys', 'keys'],
];
const KEYS = [
  ['New tab', 'Ctrl+T'], ['Reopen closed tab', 'Ctrl+Shift+T'], ['Close tab', 'Ctrl+W'],
  ['Address bar', 'Ctrl+L'], ['Next / previous tab', 'Ctrl+Tab / Ctrl+Shift+Tab'], ['Jump to tab', 'Ctrl+1…9'],
  ['Back / forward', 'Alt+← / Alt+→'], ['Reload', 'Ctrl+R'], ['Compact sidebar', 'Ctrl+B'],
  ['Find in page', 'Ctrl+F'], ['Fullscreen', 'F11'], ['Settings', 'Ctrl+,'], ['Control (memory, CPU, network)', 'Shift+Esc'], ['Developer tools', 'F12'],
];

let built = false;
const ctl = {};
function openSettings() {
  if (!built) buildSettings();
  setEl.querySelector('.set-body').scrollTop = 0;
}
function buildSettings() {
  built = true;
  const nav = SCHEMA.map(([name], i) => `<button data-sec="${i}"${i ? '' : ' class="on"'}>${name}</button>`).join('');
  const body = SCHEMA.map(([name, items], i) => `<section class="set-sec" data-sec="${i}"><h2>${name}</h2>`
    + (items === 'keys'
      ? `<div class="keys">${KEYS.map(([a, b]) => `<div>${a}</div><kbd>${b}</kbd>`).join('')}</div>`
      : items.map(optHtml).join(''))
    + '</section>').join('');
  setEl.innerHTML = `<nav class="set-nav"><h1>Settings</h1>${nav}<button class="close">Done <span class="link">Esc</span></button></nav><div class="set-body">${body}</div>`;
  const bodyEl = setEl.querySelector('.set-body');
  setEl.querySelector('.set-nav').addEventListener('click', e => {
    const b = e.target.closest('button');
    if (!b) return;
    if (b.classList.contains('close')) return send({ t: 'panel', open: false });
    bodyEl.querySelector(`.set-sec[data-sec="${b.dataset.sec}"]`).scrollIntoView({ block: 'start' });
  });
  bodyEl.addEventListener('scroll', () => {
    let cur = 0;
    bodyEl.querySelectorAll('.set-sec').forEach((s, i) => { if (s.offsetTop - bodyEl.offsetTop <= bodyEl.scrollTop + 40) cur = i; });
    setEl.querySelectorAll('.set-nav [data-sec]').forEach((b, i) => tog(b, 'on', i === cur));
  }, { passive: true });
  for (const [, items] of SCHEMA) if (items !== 'keys') for (const o of items) wire(o);
}
function optHtml(o) {
  let c = '';
  if (o.type === 'seg') c = `<div class="seg" data-k="${o.k}">${o.opts.map(([v, l]) => `<button data-v="${v}">${l}</button>`).join('')}</div>`;
  else if (o.type === 'toggle') c = `<button class="tog" data-k="${o.k}" role="switch" aria-label="${o.label}"></button>`;
  else if (o.type === 'range') c = `<input type="range" data-k="${o.k}" min="${o.min}" max="${o.max}" step="${o.step}"><span class="val" data-val="${o.k}"></span>`;
  else if (o.type === 'swatch') c = `<div class="sw" data-k="${o.k}">${PALETTES.map(([v, l]) => `<button data-v="${v}" title="${l}" aria-label="${l}" data-palette="${v}"></button>`).join('')}</div>`;
  else if (o.type === 'color') c = `<button class="link" data-reset="${o.k}">Reset</button><label class="color"><input type="color" data-k="${o.k}"></label>`;
  else if (o.type === 'text') c = `<input class="txt" data-k="${o.k}" placeholder="${o.placeholder || ''}" spellcheck="false">`;
  else if (o.type === 'engine') c = `<div class="seg" data-engine>${ENGINES.map(([l]) => `<button data-v="${l}">${l}</button>`).join('')}<button data-v="custom">Custom</button></div>`;
  const extra = o.type === 'engine' ? `<div class="opt" data-custom hidden><div class="lab"><b>Custom search URL</b></div><div class="ctl"><input class="txt" data-k="search" spellcheck="false"></div></div>` : '';
  return `<div class="opt"><div class="lab"><b>${o.label}</b>${o.hint ? `<small>${o.hint}</small>` : ''}</div><div class="ctl">${c}</div></div>${extra}`;
}
function wire(o) {
  const q = s => setEl.querySelector(s);
  if (o.type === 'seg' || o.type === 'swatch') {
    q(`[data-k="${o.k}"]`).addEventListener('click', e => { const b = e.target.closest('button'); if (b) setOpt(o.k, b.dataset.v); });
  } else if (o.type === 'toggle') {
    q(`.tog[data-k="${o.k}"]`).addEventListener('click', () => setOpt(o.k, !S.settings[o.k]));
  } else if (o.type === 'range') {
    q(`input[data-k="${o.k}"]`).addEventListener('input', e => setOpt(o.k, +e.target.value, false, o.k === 'sidebar_width' ? 60 : 30));
  } else if (o.type === 'color') {
    q(`input[data-k="${o.k}"]`).addEventListener('input', e => setOpt(o.k, e.target.value, false, 40));
    q(`[data-reset="${o.k}"]`).addEventListener('click', () => setOpt(o.k, ''));
  } else if (o.type === 'text') {
    q(`input[data-k="${o.k}"]`).addEventListener('change', e => setOpt(o.k, e.target.value.trim()));
  } else if (o.type === 'engine') {
    q('[data-engine]').addEventListener('click', e => {
      const b = e.target.closest('button');
      if (!b) return;
      if (b.dataset.v === 'custom') { q('[data-custom]').hidden = false; q('[data-custom] input').focus(); return; }
      setOpt('search', ENGINES.find(([l]) => l === b.dataset.v)[1]);
    });
    q('[data-custom] input').addEventListener('change', e => { const v = e.target.value.trim(); if (v.includes('%s')) setOpt('search', v); });
  }
}
function syncSettings() {
  if (!built) return;
  const st = S.settings;
  setEl.querySelectorAll('.seg[data-k], .sw[data-k]').forEach(g => g.querySelectorAll('button').forEach(b => tog(b, 'on', b.dataset.v === String(st[g.dataset.k]))));
  setEl.querySelectorAll('.tog[data-k]').forEach(b => { tog(b, 'on', !!st[b.dataset.k]); b.setAttribute('aria-checked', !!st[b.dataset.k]); });
  setEl.querySelectorAll('input[type="range"][data-k]').forEach(r => {
    if (document.activeElement !== r && +r.value !== st[r.dataset.k]) r.value = st[r.dataset.k];
    r.style.setProperty('--p', ((r.value - r.min) / (r.max - r.min) * 100) + '%');
    const o = SCHEMA.flatMap(([, it]) => it === 'keys' ? [] : it).find(x => x.k === r.dataset.k);
    setEl.querySelector(`[data-val="${r.dataset.k}"]`).textContent = r.value + ' ' + o.unit;
  });
  const color = setEl.querySelector('input[type="color"]');
  if (color && document.activeElement !== color) color.value = st.accent || rgbHex(getComputedStyle(root).getPropertyValue('--accent'));
  setEl.querySelectorAll('input.txt[data-k]').forEach(i => { if (document.activeElement !== i && i.value !== (st[i.dataset.k] || '')) i.value = st[i.dataset.k] || ''; });
  const eng = ENGINES.find(([, u]) => u === st.search);
  setEl.querySelectorAll('[data-engine] button').forEach(b => tog(b, 'on', eng ? b.dataset.v === eng[0] : b.dataset.v === 'custom'));
  const custom = setEl.querySelector('[data-custom]');
  if (custom && !eng) custom.hidden = false;
}
const rgbHex = c => '#' + rgbOf(c).map(v => v.toString(16).padStart(2, '0')).join('');

let sendTimer = 0;
function setOpt(k, v, now, delay) {
  S.settings = { ...S.settings, [k]: v };
  look();
  syncSettings();
  syncControl();
  clearTimeout(sendTimer);
  const go = () => send({ t: 'settings', v: S.settings });
  if (now || !delay) go(); else sendTimer = setTimeout(go, delay);
}

document.addEventListener('keydown', e => {
  if (e.key === 'Escape' && S && S.panel !== 'none' && !editing) { e.preventDefault(); send({ t: 'panel', open: false }); }
  if (e.key === 'Escape') closeMenu();
});

// ---------------------------------------------------------------- control
// Live use per tab, and the three limiters. Numbers arrive once a second
// while this page is open; nothing runs when it is closed.
const RAM_STEPS = [512, 768, 1024, 1536, 2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768];
const NET_STEPS = [32, 64, 128, 256, 512, 1024, 2048, 5120, 10240, 25600, 51200, 102400];
const nearest = (steps, v) => steps.reduce((bi, s, i) => Math.abs(s - v) < Math.abs(steps[bi] - v) ? i : bi, 0);
const fmtMem = b => { const mb = b / 1048576; return mb >= 1024 ? (mb / 1024).toFixed(2) + ' GB' : mb >= 10 ? Math.round(mb) + ' MB' : mb.toFixed(1) + ' MB'; };
const fmtMB = mb => mb >= 1024 ? (mb / 1024).toFixed(mb % 1024 ? 1 : 0) + ' GB' : mb + ' MB';
const fmtCpu = c => c >= 10 ? Math.round(c) + '%' : c.toFixed(1) + '%';
const fmtNet = bps => bps < 1 ? '—' : bps >= 1048576 ? (bps / 1048576).toFixed(1) + ' MB/s' : Math.max(1, Math.round(bps / 1024)) + ' KB/s';
const fmtKbps = k => k >= 1024 ? (k / 1024).toFixed(k % 1024 ? 1 : 0) + ' MB/s' : k + ' KB/s';
const KIND = { browser: 'Browser', gpu: 'Graphics', utility: 'Services', host: 'Lumen', renderer: 'Spare renderer', other: 'Other' };

let ctlBuilt = false, U = null;
function openControl() {
  if (!ctlBuilt) buildControl();
  syncControl();
}

function buildControl() {
  ctlBuilt = true;
  ctlEl.innerHTML = `
    <div class="ctl-in">
      <header class="ctl-head">
        <h1>Control</h1>
        <button class="link" id="ctl-done">Done <span>Esc</span></button>
      </header>
      <div class="stats">
        <div><b id="st-mem">—</b><small>Memory</small></div>
        <div><b id="st-cpu">—</b><small>CPU</small></div>
        <div><b id="st-net">—</b><small>Network</small></div>
        <div><b id="st-procs">—</b><small>Processes</small></div>
      </div>
      <div class="use">
        <div class="use-h"><span>Tab</span><span>Memory</span><span>CPU</span><span>Network</span><span></span></div>
        <div id="use-rows"></div>
      </div>
      <h2>Limits</h2>
      <div class="opt"><div class="lab"><b>RAM limit</b><small>Over the limit, the hidden tab you looked at longest ago unloads.</small></div>
        <div class="ctl"><input type="range" id="lim-ram" min="0" max="${RAM_STEPS.length - 1}" step="1"><span class="val" id="lim-ram-v"></span><button class="tog" data-lim="ram_limit" role="switch" aria-label="RAM limit"></button></div></div>
      <div class="opt"><div class="lab"><b>CPU limit</b><small id="lim-cpu-hint">Hard cap for the whole browser, share of all cores.</small></div>
        <div class="ctl"><input type="range" id="lim-cpu" min="5" max="100" step="5"><span class="val" id="lim-cpu-v"></span><button class="tog" data-lim="cpu_limit" role="switch" aria-label="CPU limit"></button></div></div>
      <div class="opt"><div class="lab"><b>Network limit</b><small>Bandwidth cap for each tab.</small></div>
        <div class="ctl"><input type="range" id="lim-net" min="0" max="${NET_STEPS.length - 1}" step="1"><span class="val" id="lim-net-v"></span><button class="tog" data-lim="net_limit" role="switch" aria-label="Network limit"></button></div></div>
      <div class="opt"><div class="lab"><b>Graphics in browser process</b><small>About 100 MB less memory. A graphics driver crash then restarts the whole browser. Applies after restart.</small></div>
        <div class="ctl"><button class="tog" data-lim="gpu_in_process" role="switch" aria-label="Graphics in browser process"></button></div></div>
      <div class="opt"><div class="lab"><b>Share processes within a site</b><small>Tabs of the same site share one process. Far fewer processes, less memory; one heavy tab can slow its siblings. Applies after restart.</small></div>
        <div class="ctl"><button class="tog" data-lim="share_processes" role="switch" aria-label="Share processes within a site"></button></div></div>
    </div>`;
  $('#ctl-done').onclick = () => send({ t: 'panel', open: false });
  ctlEl.querySelectorAll('[data-lim]').forEach(b => b.addEventListener('click', () => setOpt(b.dataset.lim, !S.settings[b.dataset.lim])));
  $('#lim-ram').addEventListener('input', e => setOpt('ram_limit_mb', RAM_STEPS[+e.target.value], false, 60));
  $('#lim-cpu').addEventListener('input', e => setOpt('cpu_limit_pct', +e.target.value, false, 60));
  $('#lim-net').addEventListener('input', e => setOpt('net_limit_kbps', NET_STEPS[+e.target.value], false, 60));
  $('#use-rows').addEventListener('click', e => {
    const b = e.target.closest('button[data-a]');
    if (b) send({ t: b.dataset.a, id: +b.dataset.id });
  });
}

function syncControl() {
  if (!ctlBuilt || !S) return;
  const st = S.settings;
  const range = (id, idx, label) => {
    const r = $(id);
    if (document.activeElement !== r && +r.value !== idx) r.value = idx;
    r.style.setProperty('--p', ((r.value - r.min) / (r.max - r.min) * 100) + '%');
    $(id + '-v').textContent = label;
  };
  range('#lim-ram', nearest(RAM_STEPS, st.ram_limit_mb), fmtMB(st.ram_limit_mb));
  range('#lim-cpu', st.cpu_limit_pct, st.cpu_limit_pct + '%');
  range('#lim-net', nearest(NET_STEPS, st.net_limit_kbps), fmtKbps(st.net_limit_kbps));
  ctlEl.querySelectorAll('[data-lim]').forEach(b => { tog(b, 'on', !!st[b.dataset.lim]); b.setAttribute('aria-checked', !!st[b.dataset.lim]); });
  const cpuTog = ctlEl.querySelector('[data-lim="cpu_limit"]');
  cpuTog.disabled = !S.cpuCap;
  $('#lim-cpu-hint').textContent = S.cpuCap ? 'Hard cap for the whole browser, share of all cores.' : 'Not available: Windows did not let Lumen manage its processes.';
}

function paintUsage(u) {
  if (!ctlBuilt || ctlEl.hidden) return;
  U = u;
  $('#st-mem').textContent = fmtMem(u.total.mem);
  $('#st-cpu').textContent = fmtCpu(u.total.cpu);
  $('#st-net').textContent = fmtNet(u.total.net);
  $('#st-procs').textContent = u.total.procs;
  const max = Math.max(1, u.total.mem);
  const rowsHtml = [];
  const tabs = S.tabs.map(t => ({ t, x: u.tabs[t.id] || { mem: 0, cpu: 0, net: 0 } }))
    .sort((a, b) => b.x.mem - a.x.mem);
  for (const { t, x } of tabs) {
    const f = favs.get(t.id);
    const name = t.title || (t.status === 'new' ? 'New tab' : host(t.url) || 'Untitled');
    const state = t.status === 'off' ? 'Unloaded' : t.status === 'asleep' ? 'Frozen' : t.status === 'new' ? '' : t.id === S.active ? 'Active' : '';
    const live = t.status === 'live' || t.status === 'asleep';
    rowsHtml.push(`<div class="use-r${live ? '' : ' off'}">
      <span class="use-name"><span class="fav">${f ? `<img src="${esc(f)}" alt="">` : `<i class="mono">${esc((host(t.url) || '+')[0].toUpperCase())}</i>`}</span><span class="nm">${esc(name)}</span>${state ? `<em>${state}</em>` : ''}</span>
      <span class="num">${live ? fmtMem(x.mem) : '—'}<i class="bar" style="--w:${(x.mem / max * 100).toFixed(1)}%"></i></span>
      <span class="num">${live ? fmtCpu(x.cpu) : '—'}</span>
      <span class="num">${live ? fmtNet(x.net) : '—'}</span>
      <span class="acts">${live && t.id !== S.active ? `<button class="link" data-a="sleep" data-id="${t.id}">Unload</button>` : ''}</span>
    </div>`);
  }
  const sys = (name, x, note) => `<div class="use-r sys"><span class="use-name"><span class="fav"><i class="mono sys">·</i></span><span class="nm">${name}</span>${note ? `<em>${note}</em>` : ''}</span>
    <span class="num">${fmtMem(x.mem)}<i class="bar" style="--w:${(x.mem / max * 100).toFixed(1)}%"></i></span><span class="num">${fmtCpu(x.cpu)}</span><span class="num">—</span><span class="acts"></span></div>`;
  rowsHtml.push(sys('Lumen interface', u.ui));
  const parts = (u.core.parts || []).filter(p => p.mem > 0).sort((a, b) => b.mem - a.mem).map(p => KIND[p.kind] || p.kind).filter((n, i, a) => a.indexOf(n) === i).join(', ');
  rowsHtml.push(sys('Browser core', u.core, parts));
  $('#use-rows').innerHTML = rowsHtml.join('');
}

send({ t: 'ready' });
})();
