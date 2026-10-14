/* ============================================================ Lumen chrome
   All logic. Rust inlines this file into index.html at build time.
   ========================================================================= */
(function () {
'use strict';

// ------------------------------------------------------------ tiny helpers
const root = document.documentElement;
const $ = (s, r) => (r || document).querySelector(s);
const $$ = (s, r) => Array.prototype.slice.call((r || document).querySelectorAll(s));
const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const esc = s => String(s == null ? '' : s).replace(/[&<>"']/g,
  c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

const EL = {};
// resolved by real ids below, kept explicit so the wiring stays readable
EL.sidebar = document.getElementById('sidebar');
EL.stage = document.getElementById('stage');
EL.rz = document.getElementById('rz');
EL.bBack = document.getElementById('b-back');
EL.bFwd = document.getElementById('b-fwd');
EL.bReload = document.getElementById('b-reload');
EL.wmax = document.querySelector('[data-win="max"]');
EL.omni = document.getElementById('omni');
EL.omniIco = document.getElementById('omni-ico');
EL.omniIn = document.getElementById('omni-in');
EL.omniList = document.getElementById('omni-list');
EL.pins = document.getElementById('pins');
EL.tabs = document.getElementById('tabs');
EL.bNew = document.getElementById('b-new');
EL.mem = document.getElementById('mem');
EL.bSet = document.getElementById('b-set');
EL.menu = document.getElementById('menu');
EL.ntp = document.getElementById('ntp');
EL.ntpBg = document.getElementById('ntp-bg');
EL.ntpIn = document.getElementById('ntp-in');
EL.clock = document.getElementById('clock');
EL.greet = document.getElementById('greet');
EL.date = document.getElementById('date');
EL.scs = document.getElementById('scs');
EL.settings = document.getElementById('settings');
EL.setNav = document.getElementById('set-nav');
EL.setTitle = document.getElementById('set-title');
EL.setScroll = document.getElementById('set-scroll');
EL.setClose = document.getElementById('set-close');

function hostOf(u) {
  if (!u) return '';
  let s = String(u).trim();
  try { return new URL(s).host.replace(/^www\./, ''); }
  catch (e) {
    return s.replace(/^[a-z][\w+.-]*:\/\//i, '').split(/[\/?#]/)[0].replace(/^www\./, '');
  }
}
function hash(s) { let h = 0; for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0; return Math.abs(h); }
function fmtMem(b) {
  if (!b && b !== 0) return '';
  if (b >= 1073741824) return (b / 1073741824).toFixed(1) + ' GB';
  return Math.round(b / 1048576) + ' MB';
}

// ------------------------------------------------------------------- icons
const ICONS = {
  back: '<path d="M7.6 3.4 4 7l3.6 3.6"/><path d="M4 7h8.6"/>',
  fwd: '<path d="M8.4 3.4 12 7l-3.6 3.6"/><path d="M12 7H3.4"/>',
  reload: '<path d="M13.1 8a5.1 5.1 0 1 1-1.6-3.68"/><path d="M13.3 2.5v2.8h-2.8"/>',
  stop: '<rect x="4.4" y="4.4" width="7.2" height="7.2" rx="1.6"/>',
  min: '<path d="M3.6 8h8.8"/>',
  max: '<rect x="3.8" y="3.8" width="8.4" height="8.4" rx="1.8"/>',
  restore: '<path d="M5.2 10.4H3.9A1.4 1.4 0 0 1 2.5 9V3.9A1.4 1.4 0 0 1 3.9 2.5H9A1.4 1.4 0 0 1 10.4 3.9v.7"/>' +
          '<rect x="5.6" y="5.6" width="7.9" height="7.9" rx="1.8"/>',
  close: '<path d="M4.2 4.2l7.6 7.6M11.8 4.2l-7.6 7.6"/>',
  lock: '<rect x="3.6" y="7" width="8.8" height="6.4" rx="1.8"/><path d="M5.9 7V5.3a2.1 2.1 0 0 1 4.2 0V7"/>',
  globe: '<circle cx="8" cy="8" r="5.2"/><path d="M2.8 8h10.4"/>' +
         '<path d="M8 2.8c1.55 1.45 2.33 3.2 2.33 5.2S9.55 11.75 8 13.2C6.45 11.75 5.67 9.98 5.67 8S6.45 4.25 8 2.8Z"/>',
  search: '<circle cx="7.1" cy="7.1" r="4.05"/><path d="M10.1 10.1l3.15 3.15"/>',
  plus: '<path d="M8 3.4v9.2M3.4 8h9.2"/>',
  gear: '<circle cx="8" cy="8" r="2.5"/><path d="M8 1.6v1.7M8 12.7v1.7M1.6 8h1.7M12.7 8h1.7' +
        'M3.5 3.5l1.2 1.2M11.3 11.3l1.2 1.2M12.5 3.5l-1.2 1.2M4.7 11.3l-1.2 1.2"/>',
  moon: '<path d="M13.2 9.9A5.6 5.6 0 0 1 6.1 2.8a5.72 5.72 0 1 0 7.1 7.1Z"/>',
  audio: '<path d="M9.4 3.4 6 6H3.8v4H6l3.4 2.6V3.4Z"/><path d="M11.4 5.7a3.1 3.1 0 0 1 0 4.6"/>',
  mute: '<path d="M9.4 3.4 6 6H3.8v4H6l3.4 2.6V3.4Z"/><path d="M11.2 6.4l3 3.2M14.2 6.4l-3 3.2"/>',
  pin: '<path d="M6.1 2.4h3.8l-.55 4.1 2 2.05v1.2H4.65v-1.2l2-2.05L6.1 2.4Z"/><path d="M8 9.75v3.85"/>',
  copy: '<rect x="5.6" y="5.6" width="7.9" height="7.9" rx="1.8"/>' +
        '<path d="M10.4 5.6V3.9A1.4 1.4 0 0 0 9 2.5H3.9A1.4 1.4 0 0 0 2.5 3.9V9a1.4 1.4 0 0 0 1.4 1.4h.7"/>',
  zzz: '<path d="M3.4 2h4.3l-4.3 4.3h4.3"/><path d="M8.6 9.7h3.4l-3.4 3.4h3.4"/>',
  layerX: '<rect x="2.4" y="2.4" width="8.4" height="8.4" rx="1.7"/>' +
          '<path d="M6.2 13.6h5.4A2.2 2.2 0 0 0 13.8 11.4V6.1"/><path d="M4.7 4.7l3 3M7.7 4.7l-3 3"/>',
  trash: '<path d="M3.4 5h9.2"/><path d="M6.4 5V3.8A1.3 1.3 0 0 1 7.7 2.5h.6a1.3 1.3 0 0 1 1.3 1.3V5"/>' +
         '<path d="M4.9 5l.5 7.1A1.5 1.5 0 0 0 6.9 13.5h2.2a1.5 1.5 0 0 0 1.5-1.4l.5-7.1"/>',
  check: '<path d="M3.4 8.4 6.4 11.4l6.2-6.8"/>',
  right: '<path d="M6 3.4 10.6 8 6 12.6"/>'
};
const svg = p => '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"' +
  ' stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + p + '</svg>';
function ic(n, cls) {
  return '<span class="i' + (cls ? ' ' + cls : '') + '" data-icon="' + n + '">' +
    svg(ICONS[n] || '') + '</span>';
}
function paintIcons(r) {
  $$('[data-icon]', r || document).forEach(el => {
    const n = el.getAttribute('data-icon');
    if (ICONS[n]) el.innerHTML = svg(ICONS[n]);
  });
}
// one global handler for every favicon: load shows it, error drops it back to the monogram
document.addEventListener('load', e => {
  if (e.target && e.target.tagName === 'IMG') e.target.classList.add('ok');
}, true);
window.addEventListener('error', e => {
  if (e.target && e.target.tagName === 'IMG' && e.target.parentNode) e.target.parentNode.removeChild(e.target);
}, true);

// Favicon or monogram chip. Hue leans on the palette accent so chips feel related.
let AH = 50;
function monoBg(host) {
  const h = (hash(host || 'x') % 60) - 30 + AH;
  return 'color-mix(in oklab, oklch(0.72 0.12 ' + h + ') 72%, var(--panel))';
}
function favHtml(t) {
  const host = hostOf(t.url);
  return '<span class="fav">' +
    (t.favicon ? '<img src="' + esc(t.favicon) + '" alt="" referrerpolicy="no-referrer">' : '') +
    '<span class="mono" style="--mono-bg:' + monoBg(host) + '">' + esc((host[0] || '·').toUpperCase()) + '</span>' +
    '<i class="st" aria-hidden="true"></i></span>';
}

// --------------------------------------------------------------- ipc + state
function send(m) {
  try { if (window.ipc && window.ipc.postMessage) window.ipc.postMessage(JSON.stringify(m)); }
  catch (e) { /* preview without a shell */ }
}

const PALETTES = ['ember', 'paper', 'moss', 'ink', 'dune'];
const BACKDROPS = ['mica', 'acrylic', 'tabbed', 'solid'];
const DEF_SET = {
  palette: 'ember', accent: '', backdrop: 'mica', theme: 'dark', sidebar_side: 'left',
  sidebar_width: 264, compact: false, gap: 8, radius: 10, animations: true,
  search: 'https://duckduckgo.com/?q=%s', wallpaper: '', sleep_after_min: 5,
  discard_after_min: 30, tracking: 'balanced', show_memory: true, adblock: true,
  shortcuts: [
    { title: 'YouTube', url: 'https://www.youtube.com' },
    { title: 'GitHub', url: 'https://github.com' },
    { title: 'Reddit', url: 'https://www.reddit.com' },
    { title: 'Gmail', url: 'https://mail.google.com' },
    { title: 'Wikipedia', url: 'https://en.wikipedia.org' }
  ]
};

const S = {
  tabs: [], active: 0, settings: Object.assign({}, DEF_SET), dark: true,
  maximized: false, fullscreen: false, panel: 'none', peek: false, mem: null
};
let localPeek = false;
const tabById = id => S.tabs.find(t => t.id === id);
const activeTab = () => tabById(S.active) || S.tabs[0] || null;
const peekOn = () => localPeek || !!S.peek;
const compacted = () => !!S.settings.compact && !peekOn();

// ------------------------------------------------------- geometry + theming
// Mirrors the Rust layout exactly:
//   S = compact && !peek ? 68 : sidebar_width
//   stage = x S|g, y g, w W-S-g, h H-2g   (radius = settings.radius)
function geom() {
  const st = S.settings;
  const W = Math.max(320, window.innerWidth), H = Math.max(240, window.innerHeight);
  const sw = compacted() ? 68 : clamp(+st.sidebar_width || 264, 200, 420);
  return { W, H, sw, right: st.sidebar_side === 'right', gap: clamp(+st.gap || 8, 6, 32) };
}
function setStyle(el, p, v) {
  const c = el._st || (el._st = {});
  if (c[p] !== v) { c[p] = v; el.style[p] = v; }
}
function flag(name, on) {
  const has = root.hasAttribute(name);
  if (on && !has) root.setAttribute(name, '');
  else if (!on && has) root.removeAttribute(name);
}
function layout() {
  const g = geom(), st = S.settings, fs = !!S.fullscreen, sb = EL.sidebar, stg = EL.stage;
  sb.style.display = fs ? 'none' : '';
  if (!fs) {
    setStyle(sb, 'left', g.right ? (g.W - g.sw) + 'px' : '0px');
    setStyle(sb, 'width', g.sw + 'px');
  }
  const x = fs ? 0 : (g.right ? g.gap : g.sw);
  const w = fs ? g.W : Math.max(0, g.W - g.sw - g.gap);
  const h = fs ? g.H : Math.max(0, g.H - 2 * g.gap);
  setStyle(stg, 'left', x + 'px');
  setStyle(stg, 'top', (fs ? 0 : g.gap) + 'px');
  setStyle(stg, 'width', w + 'px');
  setStyle(stg, 'height', h + 'px');
  flag('data-fs', fs);
  EL.rz.toggleAttribute('data-off', fs || !!S.maximized);
  EL.rz.style.pointerEvents = (fs || S.maximized) ? 'none' : '';
}
function syncAttrs() {
  const st = S.settings;
  root.setAttribute('data-palette', PALETTES.indexOf(st.palette) >= 0 ? st.palette : 'ember');
  root.setAttribute('data-theme', S.dark ? 'dark' : 'light');
  root.setAttribute('data-side', st.sidebar_side === 'right' ? 'right' : 'left');
  root.setAttribute('data-backdrop', BACKDROPS.indexOf(st.backdrop) >= 0 ? st.backdrop : 'mica');
  root.style.setProperty('--sb-w', geom().sw + 'px');
  root.style.setProperty('--gap', geom().gap + 'px');
  root.style.setProperty('--radius', clamp(+st.radius || 0, 0, 24) + 'px');
  flag('data-anim', 'animations' in st ? !!st.animations : true);
  flag('data-compact', compacted());
  flag('data-peek', peekOn());
  if (st.accent) root.style.setProperty('--accent', st.accent);
  else root.style.removeProperty('--accent');
  AH = parseFloat(getComputedStyle(root).getPropertyValue('--ah')) || 50;
}

// ------------------------------------------------- keyed, incremental list
function syncList(parent, items, make, update) {
  if (drag.active) return;
  const map = parent._m || (parent._m = new Map());
  const keep = new Set(items.map(t => t.id));
  for (const el of Array.prototype.slice.call(parent.children)) {
    if (!keep.has(+el.dataset.id)) { el.parentNode.removeChild(el); map.delete(+el.dataset.id); }
  }
  items.forEach((t, i) => {
    let el = map.get(t.id);
    if (!el) { el = make(t); map.set(t.id, el); }
    update(el, t);
    const ref = parent.children[i];
    if (ref !== el) parent.insertBefore(el, ref || null);
  });
}

// ---------------------------------------------------------------- tab chrome
function makeTab(t) {
  const el = document.createElement('div');
  el.className = 'tab';
  el.dataset.id = t.id;
  el.setAttribute('role', 'tab');
  el.tabIndex = -1;
  el.innerHTML = favHtml(t) +
    '<span class="t-body"><span class="t-title"></span></span>' +
    '<span class="t-trail">' +
    '<button class="ib t-aud" aria-label="Mute tab"></button>' +
    '<button class="ib t-x" aria-label="Close tab"></button>' +
    '</span><i class="t-load"></i>';
  return el;
}
function makePin(t) {
  const el = makeTab(t);
  el.className = 'pin';
  return el;
}
function paintTrail(el, t, v) {
  const aud = $('.t-aud', el), on = !!t.audio, mu = !!t.muted;
  const n = mu ? 'mute' : 'audio';
  aud.dataset.on = on ? '1' : '0';
  aud.dataset.muted = mu ? '1' : '0';
  aud.setAttribute('aria-label', mu ? 'Unmute tab' : 'Mute tab');
  if (v._aud !== n) { aud.innerHTML = svg(ICONS[n]); v._aud = n; }
  if (!v._x) { $('.t-x', el).innerHTML = svg(ICONS.close); v._x = 1; }
  v.audio = on; v.muted = mu;
}
function updateTab(el, t) {
  const v = el._v || (el._v = {});
  const st = t.status || 'live';
  const title = t.title || hostOf(t.url) || 'New tab';
  if (v.st !== st) {
    el.dataset.st = st; v.st = st;
    const b = $('.st', el);
    if (b) b.innerHTML = (st === 'asleep' || st === 'off') ? svg(ICONS.moon) : '';
    paintTrail(el, t, v);
  }
  if (v.title !== title) {
    $('.t-title', el).textContent = title;
    el.title = title;
    v.title = title;
  }
  if (v.fav !== t.favicon) {
    const fav = $('.fav', el);
    if (fav) { fav.innerHTML = favHtml(t).replace(/^<span class="fav">|<\/span>$/g, ''); }
    v.fav = t.favicon;
  }
  const ld = t.loading ? 1 : 0;
  if (v.loading !== ld) { el.dataset.loading = ld; v.loading = ld; }
  if (v.audio !== !!t.audio || v.muted !== !!t.muted) paintTrail(el, t, v);
  const sel = t.id === S.active ? 'true' : 'false';
  if (v.sel !== sel) { el.setAttribute('aria-selected', sel); v.sel = sel; }
}
function renderTabs() {
  const pins = S.tabs.filter(t => t.pinned);
  const rest = S.tabs.filter(t => !t.pinned);
  EL.pins.hidden = pins.length === 0;
  if (pins.length) syncList(EL.pins, pins, makePin, updateTab);
  syncList(EL.tabs, rest, makeTab, updateTab);
}

// ------------------------------------------------------------- window chrome
function renderTop() {
  const t = activeTab();
  EL.bBack.disabled = !(t && t.canBack);
  EL.bFwd.disabled = !(t && t.canFwd);
  const ri = (t && t.loading) ? 'stop' : 'reload';
  if (EL.bReload._i !== ri) {
    EL.bReload._i = ri;
    EL.bReload.innerHTML = svg(ICONS[ri]);
    EL.bReload.setAttribute('aria-label', ri === 'stop' ? 'Stop loading' : 'Reload');
  }
}
function renderFoot() {
  const on = !!S.settings.show_memory && !!S.mem;
  EL.mem.hidden = !on;
  if (on) {
    const n = S.mem.procs || 1;
    EL.mem.textContent = fmtMem(S.mem.bytes) + ' · ' + n + ' process' + (n === 1 ? '' : 'es');
  } else EL.mem.textContent = '';
}

// --------------------------------------------------------------- the omnibox
const omni = { open: false, items: [], idx: -1, focused: false, val: '' };
function looksUrl(s) {
  if (!s) return false;
  if (/^[a-z][\w+.-]*:\/\//i.test(s)) return true;
  if (/\s/.test(s)) return false;
  return /^[^\s/?]+(\.[^\s/?]+)+(\/\S*)?$/.test(s);
}
function renderOmni() {
  if (omni.focused) return;
  const t = activeTab();
  const url = t ? (t.url || '') : '';
  const val = hostOf(url);
  if (omni.val !== val) { omni.val = val; EL.omniIn.value = val; }
  const icn = !url ? 'search' : (/^https:/i.test(url) ? 'lock' : 'globe');
  if (EL.omniIco._i !== icn) {
    EL.omniIco._i = icn;
    EL.omniIco.setAttribute('data-icon', icn);
    EL.omniIco.innerHTML = svg(ICONS[icn]);
  }
  EL.omni.classList.toggle('insecure', /^http:/i.test(url));
}
function suggest(q) {
  const ql = q.trim().toLowerCase(), out = [];
  if (!ql) return out;
  for (const t of S.tabs) {
    if (!t.url || t.status === 'new') continue;
    const host = hostOf(t.url), title = (t.title || '').toLowerCase();
    if (title.indexOf(ql) >= 0 || host.indexOf(ql) >= 0) out.push({ k: 'tab', t });
  }
  out.push({ k: 'search', q: q.trim() });
  if (looksUrl(q.trim())) out.push({ k: 'url', q: q.trim() });
  return out.slice(0, 8);
}
function paintList() {
  const list = omni.items;
  if (!omni.open || !list.length) {
    EL.omniList.hidden = true;
    EL.omniIn.setAttribute('aria-expanded', 'false');
    if (!list.length) EL.omniList.innerHTML = '';
    return;
  }
  EL.omniList.hidden = false;
  EL.omniIn.setAttribute('aria-expanded', 'true');
  EL.omniList.innerHTML = list.map((it, i) => {
    const sel = i === omni.idx ? 'true' : 'false';
    if (it.k === 'tab') {
      return '<div class="ol-item" role="option" aria-selected="' + sel + '" data-i="' + i + '">' +
        favHtml(it.t) + '<span class="ol-main"><span class="ol-t">' + esc(it.t.title || hostOf(it.t.url)) +
        '</span><span class="ol-s">Switch to tab · ' + esc(hostOf(it.t.url)) + '</span></span></div>';
    }
    if (it.k === 'url') {
      return '<div class="ol-item" role="option" aria-selected="' + sel + '" data-i="' + i + '">' +
        ic('globe') + '<span class="ol-main"><span class="ol-t">' + esc(it.q) +
        '</span><span class="ol-s">Open address</span></span></div>';
    }
    return '<div class="ol-item" role="option" aria-selected="' + sel + '" data-i="' + i + '">' +
      ic('search') + '<span class="ol-main"><span class="ol-t">Search for “' + esc(it.q) +
      '”</span><span class="ol-s">' + esc(hostOf(S.settings.search) || 'search') + '</span></span></div>';
  }).join('');
}
function openList() {
  omni.open = true;
  omni.items = suggest(EL.omniIn.value);
  omni.idx = -1;
  paintList();
}
function closeList() {
  omni.open = false; omni.items = []; omni.idx = -1;
  paintList();
}
function pick(it) {
  if (!it) return;
  if (it.k === 'tab') send({ t: 'activate', id: it.t.id });
  else send({ t: 'nav', url: it.q });
  EL.omniIn.blur();
}
function submitOmni() {
  const q = EL.omniIn.value.trim();
  if (q) send({ t: 'nav', url: q });
  EL.omniIn.blur();
}

// --------------------------------------------------------------- context menu
let menuId = 0;
function openMenu(id, cx, cy) {
  const t = tabById(id);
  if (!t) return;
  const items = [
    { a: 'pin', l: t.pinned ? 'Unpin' : 'Pin', i: 'pin', chk: t.pinned },
    { a: 'duplicate', l: 'Duplicate', i: 'copy' },
    { a: 'mute', l: t.muted ? 'Unmute' : 'Mute', i: t.muted ? 'mute' : 'audio', chk: t.muted, sep: 1 },
    { a: 'sleep', l: 'Sleep now', i: 'zzz' },
    { a: 'closeOthers', l: 'Close others', i: 'layerX' },
    { a: 'close', l: 'Close', i: 'trash', sep: 1 }
  ];
  EL.menu.innerHTML = items.map((it, i) =>
    (it.sep ? '<div class="menu-sep"></div>' : '') +
    '<button class="menu-item" role="menuitem" data-act="' + it.a + '" data-i="' + i +
    '" aria-checked="' + (it.chk ? 'true' : 'false') + '">' + ic(it.i) + '<span>' + it.l +
    '</span>' + ic('check', 'chk') + '</button>').join('');
  EL.menu.hidden = false;
  menuId = id;
  const sb = EL.sidebar.getBoundingClientRect();
  const mw = EL.menu.offsetWidth, mh = EL.menu.offsetHeight;
  const x = clamp(cx - sb.left, 8, Math.max(8, sb.width - mw - 8));
  const y = clamp(cy - sb.top, 8, Math.max(8, sb.height - mh - 8));
  EL.menu.style.left = x + 'px';
  EL.menu.style.top = y + 'px';
  const first = $('.menu-item', EL.menu);
  if (first) first.focus({ preventScroll: true });
}
function closeMenu() { if (!EL.menu.hidden) { EL.menu.hidden = true; EL.menu.innerHTML = ''; } }
function menuDo(act, id) {
  const m = { pin: { t: 'pin', id }, duplicate: { t: 'duplicate', id }, mute: { t: 'mute', id },
    sleep: { t: 'sleep', id }, closeOthers: { t: 'closeOthers', id }, close: { t: 'close', id } }[act];
  if (m) send(m);
  closeMenu();
}
EL.menu.addEventListener('click', e => {
  const b = e.target.closest('[data-act]');
  if (b) menuDo(b.dataset.act, menuId);
});

// ------------------------------------------------------- tab drag to reorder
const drag = { active: false, moved: false, y0: 0 };
let dragMoved = false;
function startDrag(el, e) {
  if (e.button !== 0) return;
  const list = el.parentElement;
  drag.active = true; drag.moved = false; drag.y0 = e.clientY;
  el.classList.add('dragging');
  try { el.setPointerCapture(e.pointerId); } catch (err) { /* ignore */ }
  const move = ev => {
    if (!drag.moved && Math.abs(ev.clientY - drag.y0) < 5) return;
    drag.moved = true;
    const rows = Array.prototype.slice.call(list.children).filter(r => r !== el);
    const r = el.getBoundingClientRect();
    const mid = r.top + r.height / 2;
    let after = null;
    for (const row of rows) {
      const b = row.getBoundingClientRect();
      if (mid > b.top + b.height / 2) after = row;
    }
    if (after) { if (after.nextSibling !== el) list.insertBefore(el, after.nextSibling); }
    else if (list.firstChild !== el) list.insertBefore(el, list.firstChild);
  };
  const up = ev => {
    list.removeEventListener('pointermove', move);
    list.removeEventListener('pointerup', up);
    list.removeEventListener('pointercancel', up);
    try { el.releasePointerCapture(ev.pointerId); } catch (err) { /* ignore */ }
    el.classList.remove('dragging');
    const moved = drag.moved;
    drag.active = false; drag.moved = false;
    if (moved) { dragMoved = true; commitDrag(el, list); }
  };
  list.addEventListener('pointermove', move);
  list.addEventListener('pointerup', up);
  list.addEventListener('pointercancel', up);
}
function commitDrag(el, list) {
  const id = +el.dataset.id, t = tabById(id);
  if (!t) return;
  const pinned = !!t.pinned;
  const group = S.tabs.filter(x => !!x.pinned === pinned && x.id !== id);
  const rows = Array.prototype.slice.call(list.children);
  const at = clamp(rows.indexOf(el), 0, group.length);
  let to;
  if (!group.length) to = S.tabs.length;
  else if (at === 0) to = S.tabs.indexOf(group[0]);
  else if (at >= group.length) {
    to = S.tabs.indexOf(group[group.length - 1]) + 1;
    if (!pinned) { while (to < S.tabs.length && S.tabs[to].pinned) to++; }
  } else to = S.tabs.indexOf(group[at]);
  send({ t: 'move', id, to });
}

// ============================================================== new tab page
const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July',
  'August', 'September', 'October', 'November', 'December'];
const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

function greetText(h) {
  if (h < 5) return 'Still up';
  if (h < 12) return 'Good morning';
  if (h < 18) return 'Good afternoon';
  if (h < 22) return 'Good evening';
  return 'Good night';
}
function tickClock() {
  const d = new Date();
  let h = d.getHours();
  const ap = h < 12 ? 'AM' : 'PM';
  h = h % 12 || 12;
  const m = String(d.getMinutes()).padStart(2, '0');
  EL.clock.innerHTML = '<span class="ck-h">' + h + ':</span><em class="ck-m">' + m +
    '</em><span class="ck-ap">' + ap + '</span>';
  EL.greet.textContent = greetText(d.getHours());
  EL.date.textContent = DAYS[d.getDay()] + ' · ' + MONTHS[d.getMonth()] + ' ' + d.getDate();
}
function renderScs() {
  const list = (S.settings.shortcuts || []).filter(s => s && s.url);
  let html = list.map((s, i) =>
    '<div class="sc anim" style="--d:' + (150 + i * 45) + 'ms" data-sci="' + i + '">' +
    '<a class="sc-main" href="' + esc(s.url) + '" data-nav="' + esc(s.url) + '">' +
    '<span class="n">' + esc(s.title || hostOf(s.url)) + '</span>' +
    '<span class="u">' + esc(hostOf(s.url)) + '</span></a>' +
    '<button class="ib sc-x" data-del="' + i + '" aria-label="Remove ' + esc(s.title || hostOf(s.url)) + '">' +
    svg(ICONS.close) + '</button></div>').join('');
  if (!list.length) html = '<p class="empty anim" style="--d:150ms">No shortcuts yet.</p>';
  html += '<button class="sc-add anim" style="--d:' + (150 + list.length * 45) + 'ms" data-add="1">' +
    ic('plus') + '<span>Add shortcut</span></button>';
  EL.scs.innerHTML = html;
}
function saveShortcuts(list) {
  S.settings.shortcuts = list;
  send({ t: 'settings', v: S.settings });
  renderScs();
  if (!EL.settings.hidden) syncSettings();
}
function ntpWallpaper() {
  const u = S.settings.wallpaper;
  EL.ntpBg.style.backgroundImage = u ? 'url("' + u.replace(/"/g, '%22') + '")' : 'none';
  EL.ntpBg.classList.toggle('on', !!u);
}
function showNtp(on) {
  ntpOn = on;
  EL.ntp.hidden = !on;
  if (on) {
    EL.ntp.classList.remove('show');
    void EL.ntp.offsetWidth;
    EL.ntp.classList.add('show');
    tickClock();
    renderScs();
    ntpWallpaper();
    if (EL.settings.hidden) EL.ntpIn.focus({ preventScroll: true });
  }
}
EL.scs.addEventListener('click', e => {
  const del = e.target.closest('[data-del]');
  if (del) {
    const i = +del.dataset.del;
    const list = (S.settings.shortcuts || []).slice();
    list.splice(i, 1);
    saveShortcuts(list);
    return;
  }
  if (e.target.closest('[data-add]')) { scForm(); return; }
  const a = e.target.closest('[data-nav]');
  if (a) { e.preventDefault(); send({ t: 'nav', url: a.dataset.nav }); }
});
function scForm() {
  const btn = EL.scs.querySelector('[data-add]');
  if (!btn || EL.scs.querySelector('.sc-form')) return;
  const f = document.createElement('div');
  f.className = 'sc-form anim';
  f.innerHTML = '<input class="f-t" type="text" placeholder="Title" aria-label="Shortcut title" maxlength="40">' +
    '<input class="f-u" type="text" placeholder="example.com" aria-label="Shortcut address" maxlength="200">' +
    '<button class="go" data-save="1">Add</button><button class="no" data-cancel="1">Cancel</button>';
  btn.replaceWith(f);
  paintIcons(f);
  const t = $('.f-t', f), u = $('.f-u', f);
  t.focus();
  const done = save => {
    const v = u.value.trim();
    if (save && v) {
      const list = (S.settings.shortcuts || []).slice();
      list.push({ title: t.value.trim() || hostOf(v) || v, url: /^[a-z][\w+.-]*:\/\//i.test(v) ? v : 'https://' + v });
      saveShortcuts(list);
    } else renderScs();
  };
  $('[data-save]', f).addEventListener('click', () => done(true));
  $('[data-cancel]', f).addEventListener('click', () => done(false));
  f.addEventListener('keydown', e => {
    if (e.key === 'Enter') { e.preventDefault(); done(true); }
    else if (e.key === 'Escape') { e.preventDefault(); done(false); }
  });
}
EL.ntpIn.addEventListener('keydown', e => {
  if (e.key === 'Enter') {
    e.preventDefault();
    const q = EL.ntpIn.value.trim();
    if (q) { send({ t: 'nav', url: q }); EL.ntpIn.blur(); }
  } else if (e.key === 'Escape') { EL.ntpIn.blur(); send({ t: 'focusPage' }); }
});

// ============================================================ settings panel
const SECTIONS = ['Appearance', 'Search', 'Performance', 'New tab', 'Keys'];
let sec = 'Appearance';
const SEARCHES = [
  ['https://duckduckgo.com/?q=%s', 'DuckDuckGo', 'No profile, no filter bubble. The default.'],
  ['https://www.google.com/search?q=%s', 'Google', 'Best results, most tracking.'],
  ['https://search.brave.com/search?q=%s', 'Brave', 'Independent index.'],
  ['https://www.bing.com/search?q=%s', 'Bing', 'Microsoft index.'],
  ['https://www.startpage.com/sp/search?query=%s', 'Startpage', 'Google results, proxied.']
];
const BACK_INFO = [
  ['mica', 'Mica', 'Desktop wallpaper, tinted. The signature Lumen look.'],
  ['acrylic', 'Acrylic', 'Heavier blur, brighter, more see-through behind the sidebar.'],
  ['tabbed', 'Tabbed', 'Mica pulled toward the window background. Calmer, darker.'],
  ['solid', 'Solid', 'Opaque surface. Nothing shows through.']
];
const TRACK_INFO = [
  ['strict', 'Strict', 'Blocks everything it can find. Some sites break.'],
  ['balanced', 'Balanced', 'Blocks known trackers. The default.'],
  ['basic', 'Basic', 'Only obvious cross-site trackers.'],
  ['off', 'Off', 'No prevention. Not recommended.']
];
const KEYS = [
  ['Ctrl+T', 'New tab'], ['Ctrl+Shift+T', 'Reopen last closed'],
  ['Ctrl+W', 'Close tab'], ['Ctrl+L · Ctrl+K', 'Address bar'],
  ['Ctrl+Tab · Ctrl+Shift+Tab', 'Next · previous tab'],
  ['Ctrl+1…8', 'Jump to tab'], ['Ctrl+9', 'Last tab'],
  ['Alt+← · Alt+→', 'Back · forward'], ['Ctrl+R · F5', 'Reload'],
  ['Ctrl+B', 'Compact sidebar'], ['F11', 'Fullscreen'],
  ['Ctrl+,', 'Settings'], ['F12', 'DevTools'], ['Ctrl+F', 'Find on page']
];

let di = 0;
const D = () => ' style="--d:' + (di++ * 34) + 'ms"';
const yn = v => v ? 'true' : 'false';
const row = (ctrl, k, hint) =>
  '<div class="row anim"' + D() + '><div class="row-l">' +
  (k ? '<span class="k">' + esc(k) + '</span>' : '') +
  (hint ? '<span class="h">' + hint + '</span>' : '') + '</div>' +
  '<div class="row-c">' + ctrl + '</div></div>';
const grp = (t, inner) => '<div class="grp anim"' + D() + '>' +
  (t ? '<div class="grp-t">' + esc(t) + '</div>' : '') + inner + '</div>';
// every control renders the CURRENT value, so the panel is right on first paint
// and syncSettings only has to keep up with later changes
const now = k => S.settings[k];
// forgiving compare: the shell owns these strings, so never fail on case or spacing
const same = (a, b) => String(a == null ? '' : a).trim().toLowerCase() ===
  String(b == null ? '' : b).trim().toLowerCase();
const seg = (k, opts) => '<div class="seg" role="radiogroup" aria-label="' + esc(k) + '">' +
  opts.map(o => '<button role="radio" data-set="' + k + '" data-v="' + esc(o[0]) +
    '" aria-checked="' + yn(same(now(k), o[0])) + '">' + esc(o[1]) + '</button>').join('') + '</div>';
const tog = (k, label) => '<button class="sw2" role="switch" data-tog="' + k +
  '" aria-checked="' + yn(!!now(k)) + '" aria-label="' + esc(label) + '"><i></i></button>';
const rng = (k, min, max, step, label) =>
  '<div class="rng"><input type="range" data-rng="' + k + '" min="' + min + '" max="' + max +
  '" step="' + step + '" value="' + now(k) + '" aria-label="' + esc(label) + '">' +
  '<output data-out="' + k + '">' + outFmt(k, now(k)) + '</output></div>';
const txt = (k, ph, wide) => '<input class="txt' + (wide ? ' w' : '') + '" type="text" data-txt="' + k +
  '" value="' + esc(now(k) || '') + '" spellcheck="false" placeholder="' + esc(ph) +
  '" aria-label="' + esc(k) + '">';
const radios = (k, opts) => '<div class="radios" role="radiogroup" aria-label="' + esc(k) + '">' +
  opts.map(o => '<button class="radio" role="radio" data-set="' + k + '" data-v="' + esc(o[0]) +
    '" aria-checked="' + yn(same(now(k), o[0])) + '"><span class="dot"></span>' +
    '<span class="t"><b>' + esc(o[1]) + '</b><span>' + o[2] + '</span></span></button>').join('') + '</div>';

// swatch fills come from the live palette tokens. A hidden probe inherits them,
// so the root never has to be mutated to read them.
const SW_PROBE = document.createElement('div');
SW_PROBE.setAttribute('aria-hidden', 'true');
SW_PROBE.style.cssText = 'position:absolute;width:0;height:0;overflow:hidden;visibility:hidden';
document.body.appendChild(SW_PROBE);
let SWF = null, SWF_KEY = '';
function swFills(dark) {
  const key = (dark ? 'd' : 'l') + root.getAttribute('data-palette');
  if (SWF && SWF_KEY === key) return SWF;
  const prev = root.getAttribute('data-palette');
  const out = {};
  for (const p of PALETTES) {
    SW_PROBE.setAttribute('data-palette', p);
    const cs = getComputedStyle(SW_PROBE);
    const bg = cs.getPropertyValue('--bg').trim() || 'var(--panel)';
    const ac = cs.getPropertyValue('--accent').trim() || 'var(--accent)';
    out[p] = 'linear-gradient(135deg,' + bg + ' 0 50%,' + ac + ' 50% 100%)';
  }
  root.setAttribute('data-palette', prev || 'ember');
  SWF = out; SWF_KEY = key;
  return out;
}
function swsHtml() {
  const f = swFills(S.dark), cur = now('palette');
  return '<div class="sws" role="radiogroup" aria-label="Palette">' + PALETTES.map(p =>
    '<button class="sw" role="radio" data-set="palette" data-v="' + p +
    '" aria-checked="' + yn(p === cur) + '" aria-label="' + p + '">' +
    '<i style="background:' + f[p] + '"></i></button>').join('') + '</div>';
}

function outFmt(k, v) {
  if (k === 'sidebar_width' || k === 'gap' || k === 'radius') return v + ' px';
  if (k === 'sleep_after_min') return v + ' min';
  if (k === 'discard_after_min') return v + ' min';
  return String(v);
}function secHtml() {
  const st = S.settings;
  di = 0;
  if (sec === 'Appearance') return (
    grp('Palette', swsHtml()) +
    row('<div class="acc"><input type="color" data-col="accent" value="' +
      toHex(now('accent') || getComputedStyle(root).getPropertyValue('--accent')) +
      '" aria-label="Accent color">' +
      '<button class="rst" data-set="accent" data-v=""' + (now('accent') ? '' : ' disabled') +
      '>Reset</button></div>',
      'Accent', 'Overrides the palette accent.') +
    grp('Theme', row(seg('theme', [['dark', 'Dark'], ['light', 'Light'], ['system', 'System']]),
      'Mode', 'System follows the Windows setting.') +
      row(radios('backdrop', BACK_INFO), 'Backdrop', '')) +
    grp('Sidebar', row(seg('sidebar_side', [['left', 'Left'], ['right', 'Right']]), 'Position', '') +
      row(rng('sidebar_width', 200, 420, 1, 'Sidebar width'), 'Width', 'Compact mode uses a 68 px rail.') +
      row(tog('compact', 'Compact sidebar'), 'Compact', 'Favicons only. The address bar peeks when focused.')) +
    grp('Page', row(rng('gap', 6, 32, 1, 'Page gap'), 'Gap', 'Air between the page and the window edge.') +
      row(rng('radius', 0, 24, 1, 'Corner radius'), 'Corner radius', '0 is square.')) +
    grp('Motion', row(tog('animations', 'Animations'), 'Animations',
      'Transform and opacity only. Always off if Windows asks for reduced motion.'))
  );
  if (sec === 'Search') return (
    grp('Engine', radios('search', SEARCHES.map(s =>
      [s[0], s[1], '<code>' + esc(s[0]) + '</code>']))) +
    grp('Custom', row(txt('search', 'https://example.com/search?q=%s', true), 'Search URL',
      'Use %s where the query goes. Anything else falls back to DuckDuckGo.'))
  );
  if (sec === 'Performance') return (
    grp('Memory', row(rng('sleep_after_min', 1, 240, 1, 'Sleep after'), 'Sleep after',
      'Minutes a hidden tab waits before it is frozen in memory.') +
      row(rng('discard_after_min', 1, 1440, 1, 'Discard after'), 'Discard after',
        'Minutes before the memory is freed entirely. <b>Asleep</b> = frozen in memory, instant wake. ' +
        '<b>Off</b> = memory freed, the page reloads on click.') +
      row(tog('show_memory', 'Show memory meter'), 'Memory meter', 'Readout in the sidebar footer.')) +
    grp('Blocking', row(tog('adblock', 'Block ads and trackers'), 'Block ads and trackers',
      'One hold switch for everything below. Turning it off leaves tracking prevention alone.') +
      row(radios('tracking', TRACK_INFO), 'Tracking prevention', ''))
  );
  if (sec === 'New tab') return (
    grp('Wallpaper', row(txt('wallpaper', 'https://example.com/wall.jpg', true), 'Image URL',
      'Empty for the plain background. A scrim keeps the text readable.')) +
    grp('Shortcuts', '<div class="sce" id="sce"></div>' +
      '<button class="sce-add" data-sceadd="1">' + ic('plus') + '<span>Add shortcut</span></button>')
  );
  return grp('Keyboard', '<div class="keys anim"' + D() + '>' + KEYS.map(k =>
    '<div class="key"><span class="d">' + esc(k[1]) + '</span><span class="c">' +
    k[0].split(' · ').map(part => '<kbd>' + esc(part) + '</kbd>').join('<kbd>·</kbd>') +
    '</span></div>').join('') + '</div>' +
    '<p class="h anim"' + D() + ' style="color:var(--text-3);font-size:11.5px;margin-top:10px">' +
    'Global shortcuts are handled by the shell and work while a page has focus.</p>');
}
// Let the browser resolve any color syntax (oklch, hex, rgb…) into #rrggbb.
function toHex(c) {
  const cv = document.createElement('canvas');
  cv.width = cv.height = 1;
  const ctx = cv.getContext('2d');
  if (!ctx) return '#c8833a';
  try {
    ctx.fillStyle = '#c8833a';
    ctx.fillStyle = String(c || '');
    ctx.fillRect(0, 0, 1, 1);
    const d = ctx.getImageData(0, 0, 1, 1).data;
    return '#' + [d[0], d[1], d[2]].map(n => n.toString(16).padStart(2, '0')).join('');
  } catch (e) { return '#c8833a'; }
}
function sceHtml() {
  const list = S.settings.shortcuts || [];
  if (!list.length) return '<p class="empty">No shortcuts. Add one below.</p>';
  return list.map((s, i) =>
    '<div class="sce-row" data-scei="' + i + '">' +
    '<input class="txt n" type="text" value="' + esc(s.title || '') + '" placeholder="Title" ' +
    'data-scet="' + i + '" aria-label="Shortcut title" maxlength="40">' +
    '<input class="txt" type="text" value="' + esc(s.url || '') + '" placeholder="https://" ' +
    'data-sceu="' + i + '" aria-label="Shortcut address" maxlength="200" spellcheck="false">' +
    '<button class="ib" data-scedel="' + i + '" aria-label="Remove shortcut">' +
    svg(ICONS.trash) + '</button></div>').join('');
}

function buildNav() {
  EL.setNav.innerHTML = '<h3>Lumen</h3>' + SECTIONS.map(s =>
    '<button role="tab" data-sec="' + esc(s) + '" aria-selected="' + (s === sec) + '">' +
    esc(s) + '</button>').join('');
}
function setSec(name) {
  sec = SECTIONS.indexOf(name) >= 0 ? name : 'Appearance';
  $$('[data-sec]', EL.setNav).forEach(b =>
    b.setAttribute('aria-selected', String(b.dataset.sec === sec)));
  EL.setTitle.textContent = sec;
  EL.setScroll.innerHTML = secHtml();
  EL.setScroll.scrollTop = 0;
  paintIcons(EL.setScroll);
  syncSettings();
}
function set(k, v) {
  S.settings[k] = v;
  if (k === 'discard_after_min') S.settings.discard_after_min =
    Math.max(+v || 0, +S.settings.sleep_after_min || 0);
  if (k === 'sleep_after_min' && +S.settings.discard_after_min < +v) S.settings.discard_after_min = +v;
  applySettings();
  send({ t: 'settings', v: S.settings });
}
function applySettings() {
  syncAttrs();
  layout();
  renderFoot();
  ntpWallpaper();
}
let sendT = 0;
function sendSoon() {
  clearTimeout(sendT);
  sendT = setTimeout(() => send({ t: 'settings', v: S.settings }), 120);
}
let lastSce = '';
function syncSettings() {
  const st = S.settings, sc = EL.setScroll;
  $$('[data-set]', sc).forEach(b => b.setAttribute('aria-checked',
    yn(same(st[b.dataset.set], b.dataset.v))));
  $$('[data-tog]', sc).forEach(b => b.setAttribute('aria-checked', yn(!!st[b.dataset.tog])));
  $$('[data-rng]', sc).forEach(r => { if (r !== document.activeElement) r.value = st[r.dataset.rng]; });
  $$('[data-out]', sc).forEach(o => { o.textContent = outFmt(o.dataset.out, st[o.dataset.out]); });
  $$('[data-txt]', sc).forEach(t => { if (t !== document.activeElement) t.value = st[t.dataset.txt] || ''; });
  $$('[data-col]', sc).forEach(inp => {
    if (inp !== document.activeElement) inp.value = toHex(st.accent || getComputedStyle(root).getPropertyValue('--accent'));
  });
  const rst = $('.rst', sc);
  if (rst) rst.disabled = !st.accent;
  // refresh swatch fills when the theme flips; the values are baked in at build time
  const f = swFills(S.dark);
  $$('.sw', sc).forEach(b => {
    const i = b.firstElementChild;
    if (i && i.style.background !== f[b.dataset.v]) i.style.background = f[b.dataset.v];
  });
  const ser = JSON.stringify(st.shortcuts || []);
  if (ser !== lastSce) {
    lastSce = ser;
    const host = document.getElementById('sce');
    if (host && !host.contains(document.activeElement)) host.innerHTML = sceHtml();
  }
}
EL.setScroll.addEventListener('click', e => {
  const b = e.target.closest('[data-set]');
  if (b) { set(b.dataset.set, b.dataset.v); return; }
  const t = e.target.closest('[data-tog]');
  if (t) { set(t.dataset.tog, t.getAttribute('aria-checked') !== 'true'); return; }
  const del = e.target.closest('[data-scedel]');
  if (del) {
    const list = (S.settings.shortcuts || []).slice();
    list.splice(+del.dataset.scedel, 1);
    set('shortcuts', list);
    const host = document.getElementById('sce');
    if (host) { host.innerHTML = sceHtml(); lastSce = JSON.stringify(list); }
    return;
  }
  if (e.target.closest('[data-sceadd]')) {
    const list = (S.settings.shortcuts || []).slice();
    list.push({ title: '', url: '' });
    set('shortcuts', list);
    const host = document.getElementById('sce');
    if (host) {
      host.innerHTML = sceHtml();
      lastSce = JSON.stringify(list);
      const rows = host.children;
      const last = rows[rows.length - 1];
      if (last) { const i = last.querySelector('[data-scet]'); i && i.focus(); }
    }
  }
});
EL.setScroll.addEventListener('input', e => {
  const r = e.target.closest('[data-rng]');
  if (r) {
    const k = r.dataset.rng;
    S.settings[k] = +r.value;
    if (k === 'discard_after_min' && S.settings.discard_after_min < S.settings.sleep_after_min) {
      S.settings.discard_after_min = S.settings.sleep_after_min;
      r.value = S.settings.discard_after_min;
    }
    if (k === 'sleep_after_min' && S.settings.discard_after_min < S.settings.sleep_after_min) {
      S.settings.discard_after_min = S.settings.sleep_after_min;
      $$('[data-rng="discard_after_min"]', EL.setScroll).forEach(x => { x.value = S.settings.discard_after_min; });
    }
    $$('[data-out]', EL.setScroll).forEach(o => { o.textContent = outFmt(o.dataset.out, S.settings[o.dataset.out]); });
    applySettings();
    sendSoon();
    return;
  }
  const t = e.target.closest('[data-txt]');
  if (t) { S.settings[t.dataset.txt] = t.value; applySettings(); sendSoon(); return; }
  const col = e.target.closest('[data-col]');
  if (col) { set('accent', col.value); }
});
EL.setScroll.addEventListener('change', e => {
  if (e.target.closest('[data-txt]')) { clearTimeout(sendT); send({ t: 'settings', v: S.settings }); }
  const s = e.target.closest('[data-scet],[data-sceu]');
  if (s) {
    const i = +s.dataset.scet >= 0 ? +s.dataset.scet : +s.dataset.sceu;
    const list = (S.settings.shortcuts || []).slice();
    if (!list[i]) list[i] = { title: '', url: '' };
    if ('scet' in s.dataset) list[i].title = s.value;
    else list[i].url = s.value;
    lastSce = JSON.stringify(list);
    S.settings.shortcuts = list;
    applySettings();
    send({ t: 'settings', v: S.settings });
  }
});

// ==================================================================== render
let ntpOn = false, setOn = false;
function render() {
  syncAttrs();
  layout();
  renderTop();
  renderOmni();
  renderTabs();
  renderFoot();
  const t = activeTab();
  const wantSet = S.panel === 'settings';
  const wantNtp = !!(t && t.status === 'new') && !wantSet;
  if (wantNtp !== ntpOn) showNtp(wantNtp);
  else EL.ntp.hidden = !wantNtp;
  if (wantSet !== setOn) {
    setOn = wantSet;
    EL.settings.hidden = !wantSet;
    if (wantSet) {
      EL.ntpIn.blur();
      setSec(sec);
    } else lastSce = JSON.stringify(S.settings.shortcuts || []);
  } else if (wantSet) syncSettings();
  EL.stage.style.visibility = (wantNtp || wantSet) ? '' : 'hidden';
}

// --------------------------------------------------------------- tab events
function rowEvents(list) {
  const activate = id => send({ t: 'activate', id });
  list.addEventListener('click', e => {
    const el = e.target.closest('.tab,.pin');
    if (!el) return;
    if (dragMoved) { dragMoved = false; return; }
    const id = +el.dataset.id;
    if (e.target.closest('.t-x')) { send({ t: 'close', id }); return; }
    if (e.target.closest('.t-aud')) { send({ t: 'mute', id }); return; }
    activate(id);
  });
  list.addEventListener('auxclick', e => {
    if (e.button !== 1) return;
    const el = e.target.closest('.tab,.pin');
    if (el) { e.preventDefault(); send({ t: 'close', id: +el.dataset.id }); }
  });
  list.addEventListener('pointerdown', e => {
    const el = e.target.closest('.tab,.pin');
    if (!el || e.target.closest('button')) return;
    startDrag(el, e);
  });
  list.addEventListener('keydown', e => {
    const el = e.target.closest('.tab,.pin');
    if (!el) return;
    const rows = $$('.tab', EL.tabs).concat($$('.pin', EL.pins));
    let i = rows.indexOf(el);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      i = clamp(i + (e.key === 'ArrowDown' ? 1 : -1), 0, rows.length - 1);
      rows[i] && rows[i].focus();
    } else if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault(); activate(+el.dataset.id);
    } else if (e.key === 'Delete') {
      e.preventDefault(); send({ t: 'close', id: +el.dataset.id });
    }
  });
}
rowEvents(EL.tabs);
rowEvents(EL.pins);

// ------------------------------------------------------------ omnibox events
EL.omniIn.addEventListener('focus', () => {
  omni.focused = true;
  const t = activeTab();
  EL.omniIn.value = t ? (t.url || '') : '';
  omni.val = EL.omniIn.value;
  EL.omniIn.select();
  if (S.settings.compact) {
    localPeek = true; syncAttrs(); layout();
    send({ t: 'peek', on: true });
  }
  openList();
});
EL.omniIn.addEventListener('blur', () => {
  omni.focused = false;
  closeList();
  if (localPeek) {
    localPeek = false; syncAttrs(); layout();
    if (S.settings.compact) send({ t: 'peek', on: false });
  }
  renderOmni();
});
EL.omniIn.addEventListener('input', () => { omni.focused = true; openList(); });
EL.omniIn.addEventListener('keydown', e => {
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    const n = omni.items.length;
    if (!n) return;
    e.preventDefault();
    omni.idx = clamp(omni.idx + (e.key === 'ArrowDown' ? 1 : -1), -1, n - 1);
    paintList();
  } else if (e.key === 'Enter') {
    e.preventDefault();
    if (omni.items.length) pick(omni.items[omni.idx >= 0 ? omni.idx : 0]);
    else submitOmni();
  } else if (e.key === 'Escape') {
    e.preventDefault();
    EL.omniIn.blur();
    send({ t: 'focusPage' });
  }
});
EL.omniList.addEventListener('mousedown', e => {
  const it = e.target.closest('[data-i]');
  if (!it) return;
  e.preventDefault();
  pick(omni.items[+it.dataset.i]);
});

// --------------------------------------------------------- window + edges
let lastDrag = 0;
EL.rz.addEventListener('pointerdown', e => {
  const b = e.target.closest('b[data-d]');
  if (!b || S.maximized || S.fullscreen) return;
  send({ t: 'resize', d: b.dataset.d });
});
document.addEventListener('pointerdown', e => {
  if (e.target.closest('#rz')) return;
  const zone = e.target.closest('[data-drag]');
  if (!zone || e.target.closest('button,input,a,select,textarea,[role="button"],[role="switch"]')) return;
  if (Date.now() - lastDrag < 260) return;
  lastDrag = Date.now();
  send({ t: 'drag' });
}, true);
document.addEventListener('dblclick', e => {
  const zone = e.target.closest('[data-drag]');
  if (zone && !e.target.closest('button,input,a,[role="button"]')) send({ t: 'win', a: 'max' });
});
EL.sidebar.addEventListener('click', e => {
  const w = e.target.closest('[data-win]');
  if (w) { send({ t: 'win', a: w.dataset.win }); return; }
  if (e.target.closest('#b-back')) send({ t: 'back' });
  else if (e.target.closest('#b-fwd')) send({ t: 'fwd' });
  else if (e.target.closest('#b-reload')) send({ t: 'reload' });
  else if (e.target.closest('#b-set')) {
    const open = S.panel !== 'settings';
    S.panel = open ? 'settings' : 'none';
    send({ t: 'panel', open });
    render();
  } else if (e.target.closest('#b-new')) send({ t: 'newtab' });
});
EL.setClose.addEventListener('click', () => {
  S.panel = 'none';
  send({ t: 'panel', open: false });
  render();
  send({ t: 'focusPage' });
});
EL.setNav.addEventListener('click', e => {
  const b = e.target.closest('[data-sec]');
  if (b) setSec(b.dataset.sec);
});
EL.sidebar.addEventListener('contextmenu', e => {
  const el = e.target.closest('.tab,.pin');
  if (!el) return;
  e.preventDefault();
  openMenu(+el.dataset.id, e.clientX, e.clientY);
});
document.addEventListener('pointerdown', e => {
  if (!EL.menu.hidden && !e.target.closest('#menu')) closeMenu();
}, true);
window.addEventListener('blur', closeMenu);
document.addEventListener('keydown', e => {
  if (!EL.menu.hidden) {
    const items = $$('.menu-item', EL.menu);
    const i = items.indexOf(document.activeElement);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      items[clamp(i + (e.key === 'ArrowDown' ? 1 : -1), 0, items.length - 1)].focus();
    } else if (e.key === 'Escape') { e.preventDefault(); closeMenu(); }
    return;
  }
  if (S.panel === 'settings' && e.key === 'Escape') {
    const a = document.activeElement;
    if (a && (a.tagName === 'INPUT' || a.tagName === 'TEXTAREA')) { a.blur(); return; }
    e.preventDefault();
    EL.setClose.click();
  }
});

// ------------------------------------------------------------------- boot
window.addEventListener('resize', () => { layout(); });
setInterval(() => { if (ntpOn) tickClock(); }, 1000);

paintIcons(document);
buildNav();
setSec('Appearance');
EL.settings.hidden = true;
syncAttrs();
layout();
render();
send({ t: 'ready' });

window.lumen = {
  state(s) {
    if (!s || typeof s !== 'object') return;
    if (s.settings && typeof s.settings === 'object') S.settings = Object.assign({}, DEF_SET, s.settings);
    if (Array.isArray(s.tabs)) S.tabs = s.tabs;
    if (s.active != null) S.active = s.active;
    if (typeof s.dark === 'boolean') S.dark = s.dark;
    if (typeof s.maximized === 'boolean') S.maximized = s.maximized;
    if (typeof s.fullscreen === 'boolean') S.fullscreen = s.fullscreen;
    if (typeof s.peek === 'boolean') S.peek = s.peek;
    S.panel = s.panel === 'settings' ? 'settings' : 'none';
    if (s.mem && typeof s.mem === 'object') S.mem = s.mem;
    if (!Array.isArray(S.settings.shortcuts)) S.settings.shortcuts = [];
    const ser = JSON.stringify(S.settings.shortcuts);
    if (ser !== lastSce) lastSce = ser;
    render();
  },
  focusOmnibox() {
    if (compacted()) { localPeek = true; syncAttrs(); layout(); send({ t: 'peek', on: true }); }
    EL.omniIn.focus();
    EL.omniIn.select();
  }
};
})();
