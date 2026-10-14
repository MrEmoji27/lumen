#![windows_subsystem = "windows"]

//! Lumen. A small browser shell.
//! One window. A slim UI page for the sidebar. Pages beside it.
//! Rust decides who lives, who sleeps, and when the UI hears about it.

mod blocker;
mod engine;
mod monitor;
mod store;

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use base64::Engine as _;
use serde_json::{Value, json};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::windows::{IconExtWindows, WindowBuilderExtWindows, WindowExtWindows},
    window::{Fullscreen, Icon, ResizeDirection, Theme as WinTheme, WindowBuilder},
};
use wry::{MemoryUsageLevel, Rect, WebContext, WebView, WebViewExtWindows};

use engine::{Action, Ctx, Ev};
use store::{SavedTab, Session, Settings};

const TICK: Duration = Duration::from_secs(3);
/// A hidden tab keeps its caches this long, so flipping back is instant.
const TRIM_AFTER: Duration = Duration::from_secs(60);
const MAX_CLOSED: usize = 25;
const COMPACT_W: f64 = 56.0;
/// Control page refresh. Only runs while the page is open.
const SAMPLE: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    None,
    Settings,
    Control,
}

impl Panel {
    fn name(self) -> &'static str {
        match self {
            Panel::None => "none",
            Panel::Settings => "settings",
            Panel::Control => "control",
        }
    }
}

struct Tab {
    id: u64,
    url: String,
    title: String,
    favicon: String,
    loading: bool,
    back: bool,
    fwd: bool,
    pinned: bool,
    audio: bool,
    muted: bool,
    suspended: bool,
    trimmed: bool,
    view: Option<WebView>,
    hidden_at: Option<Instant>,
}

impl Tab {
    fn new(id: u64, url: String) -> Self {
        Self {
            id,
            url,
            title: String::new(),
            favicon: String::new(),
            loading: false,
            back: false,
            fwd: false,
            pinned: false,
            audio: false,
            muted: false,
            suspended: false,
            trimmed: false,
            view: None,
            hidden_at: None,
        }
    }

    fn status(&self) -> &'static str {
        match (&self.view, self.url.is_empty(), self.suspended) {
            (None, true, _) => "new",
            (None, false, _) => "off",
            (Some(_), _, true) => "asleep",
            (Some(_), _, false) => "live",
        }
    }
}

struct App {
    ctx: Rc<Ctx>,
    ui: WebView,
    _web_context: WebContext,
    blocker: Rc<blocker::Blocker>,
    tabs: Vec<Tab>,
    active: u64,
    closed: Vec<SavedTab>,
    settings: Settings,
    panel: Panel,
    peek: bool,
    page_fs: bool,
    user_fs: bool,
    ui_ready: bool,
    /// Something changed. The UI hears once per frame, not once per event.
    ui_dirty: bool,
    /// Favicons are big. They travel alone, only when they change.
    fav_queue: Vec<u64>,
    /// Sidebar color reported by the UI. Paints the gaps around the page.
    bg: (u8, u8, u8),
    /// Last page size that got rounded corners. Skip the work when nothing moved.
    rounded: Option<(u64, u32, u32)>,
    was_max: bool,
    shown: bool,
    dirty: bool,
    mem: Option<(u64, u32)>,
    born: Instant,
    next_tick: Instant,
    exit: bool,
    /// The browser tree's Job Object. The CPU cap lives here.
    job: Option<monitor::Job>,
    /// Control page bookkeeping.
    next_sample: Option<Instant>,
    prev_cpu: HashMap<u32, u64>,
    prev_at: Instant,
    net: monitor::NetBytes,
    watches: HashMap<u64, monitor::NetWatch>,
    cores: f64,
}

fn main() {
    // Before WebView2 starts: its processes must be born inside the job.
    let job = monitor::Job::adopt_self();
    let settings = store::load_settings();
    let session = store::load_session();

    let event_loop = EventLoopBuilder::<Ev>::with_user_event().build();
    let (w, h) = if session.width >= 720.0 && session.height >= 480.0 {
        (session.width, session.height)
    } else {
        (1360.0, 860.0)
    };
    let window = WindowBuilder::new()
        .with_title("Lumen")
        // Icon resource 1, baked in by build.rs from assets/lumen.ico.
        .with_window_icon(Icon::from_resource(1, None).ok())
        .with_decorations(false)
        // Opaque window. Glass mode gets its backdrop from the extended DWM frame and
        // black client pixels, not from window transparency, which costs a composite.
        .with_transparent(false)
        .with_background_color((0, 0, 0, 255))
        .with_undecorated_shadow(true)
        .with_visible(false)
        .with_inner_size(LogicalSize::new(w, h))
        .with_min_inner_size(LogicalSize::new(720.0, 480.0))
        .with_maximized(session.maximized)
        .build(&event_loop)
        .expect("window");

    let dark = resolve_dark(&settings, window.theme());
    let ctx = Rc::new(Ctx {
        window: Rc::new(window),
        proxy: event_loop.create_proxy(),
        env: RefCell::new(None),
        next_id: Cell::new(1),
        popups: RefCell::new(Vec::new()),
        dark: Cell::new(dark),
        on_built: RefCell::new(None),
    });

    let blocker = blocker::Blocker::start(store::data_dir());
    blocker.set_enabled(settings.adblock);
    {
        let b = blocker.clone();
        ctx.on_built.replace(Some(Box::new(move |wv: &WebView| b.attach(wv))));
    }

    let mut web_context = WebContext::new(Some(store::data_dir().join("WebView2")));
    let ui = engine::build_ui(&ctx, &mut web_context, ui_html(), full_rect(&ctx), settings.gpu_in_process).expect("ui webview");
    engine::set_tracking(&ui, &settings.tracking);

    let bg = if dark { (24, 24, 26) } else { (243, 243, 244) };
    let mut app = App {
        ctx,
        ui,
        _web_context: web_context,
        blocker,
        tabs: Vec::new(),
        active: 0,
        closed: Vec::new(),
        settings,
        panel: Panel::None,
        peek: false,
        page_fs: false,
        user_fs: false,
        ui_ready: false,
        ui_dirty: false,
        fav_queue: Vec::new(),
        bg,
        rounded: None,
        was_max: session.maximized,
        shown: false,
        dirty: false,
        mem: None,
        born: Instant::now(),
        next_tick: Instant::now() + TICK,
        exit: false,
        job,
        next_sample: None,
        prev_cpu: HashMap::new(),
        prev_at: Instant::now(),
        net: Rc::new(RefCell::new(HashMap::new())),
        watches: HashMap::new(),
        cores: std::thread::available_parallelism().map_or(1.0, |n| n.get() as f64),
    };
    app.apply_backdrop();
    app.apply_limits(None);
    app.restore(session);

    event_loop.run(move |event, _, cf| {
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                let now = Instant::now();
                if app.next_sample.is_some_and(|t| t <= now) {
                    app.sample();
                }
                if app.next_tick <= now {
                    app.tick();
                }
            }
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::Resized(size) => {
                    if size.width == 0 || size.height == 0 {
                        // Minimized. Nobody is looking. Trim the active page too.
                        if let Some(v) = app.active_tab().and_then(|t| t.view.as_ref()) {
                            let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
                        }
                    } else {
                        app.layout();
                        let max = app.ctx.window.is_maximized();
                        if max != app.was_max {
                            app.was_max = max;
                            app.push();
                        }
                    }
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    app.rounded = None;
                    app.layout();
                }
                // Alt+Tab lands focus on the bare window. Hand it on, or keys go nowhere.
                WindowEvent::Focused(true) => app.focus_page(),
                WindowEvent::ThemeChanged(_) if app.settings.theme == "system" => {
                    app.retheme();
                    app.push();
                }
                WindowEvent::CloseRequested => app.exit = true,
                _ => {}
            },
            Event::UserEvent(ev) => app.on_event(ev),
            Event::MainEventsCleared => app.flush(),
            _ => {}
        }
        if app.exit {
            app.save_session();
            *cf = ControlFlow::Exit;
        } else {
            let wake = app.next_sample.map_or(app.next_tick, |t| t.min(app.next_tick));
            *cf = ControlFlow::WaitUntil(wake);
        }
    });
}

// ------------------------------------------------------------------- ui page

fn ui_html() -> String {
    let (html, css, js) = ui_sources();
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let face = |family: &str, style: &str, weight: &str, data: &[u8]| {
        format!(
            "@font-face{{font-family:'{family}';font-style:{style};font-weight:{weight};font-display:block;\
             src:url(data:font/woff2;base64,{}) format('woff2')}}",
            b64(data)
        )
    };
    let fonts = [
        face("Instrument Sans", "normal", "400 700", include_bytes!("../ui/fonts/sans.woff2")),
        face("Instrument Serif", "normal", "400", include_bytes!("../ui/fonts/serif.woff2")),
        face("Instrument Serif", "italic", "400", include_bytes!("../ui/fonts/serif-italic.woff2")),
    ]
    .concat();
    html.replace("/*__FONTS__*/", &fonts)
        .replace("/*__CSS__*/", &css)
        .replace("/*__JS__*/", &js)
}

/// Debug builds read ui/ from disk at start, so UI edits need no recompile.
#[cfg(debug_assertions)]
fn ui_sources() -> (String, String, String) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    (read("index.html"), read("app.css"), read("app.js"))
}

#[cfg(not(debug_assertions))]
fn ui_sources() -> (String, String, String) {
    (
        include_str!("../ui/index.html").into(),
        include_str!("../ui/app.css").into(),
        include_str!("../ui/app.js").into(),
    )
}

// ------------------------------------------------------------------- helpers

fn resolve_dark(s: &Settings, os: WinTheme) -> bool {
    match s.theme.as_str() {
        "light" => false,
        "system" => os == WinTheme::Dark,
        _ => true,
    }
}

fn logical_rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(x, y).into(),
        size: LogicalSize::new(w.max(1.0), h.max(1.0)).into(),
    }
}

fn logical_size(ctx: &Ctx) -> (f64, f64) {
    let s = ctx.window.inner_size();
    let f = ctx.window.scale_factor();
    (s.width as f64 / f, s.height as f64 / f)
}

fn full_rect(ctx: &Ctx) -> Rect {
    let (w, h) = logical_size(ctx);
    logical_rect(0.0, 0.0, w, h)
}

/// Omnibox text in, URL out. Looks like an address: go there. Else: search.
fn resolve_input(raw: &str, search: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    const SCHEMES: [&str; 7] = ["http://", "https://", "file:", "about:", "data:", "view-source:", "edge://"];
    if SCHEMES.iter().any(|p| lower.starts_with(p)) {
        return Some(s.to_string());
    }
    if !s.contains(char::is_whitespace) {
        let host = lower.split(['/', '?', '#']).next().unwrap_or("");
        let host_only = host.rsplit_once(':').map_or(host, |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) { h } else { host }
        });
        if host_only == "localhost" || host_only.parse::<std::net::Ipv4Addr>().is_ok() {
            return Some(format!("http://{s}"));
        }
        if let Some((_, tld)) = host_only.rsplit_once('.')
            && tld.len() >= 2
            && tld.chars().all(|c| c.is_ascii_alphabetic())
        {
            return Some(format!("https://{s}"));
        }
    }
    Some(search.replace("%s", &url_encode(s)))
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Debug builds only: append every engine event to %APPDATA%\Lumen\debug.log.
fn debug_log(ev: &Ev) {
    use std::io::Write;
    let mut line = format!("{ev:?}");
    if line.len() > 160 {
        let cut = (0..=160).rev().find(|&i| line.is_char_boundary(i)).unwrap_or(0);
        line.truncate(cut);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(store::data_dir().join("debug.log"))
    {
        let _ = writeln!(f, "{line}");
    }
}

fn resize_dir(d: &str) -> Option<ResizeDirection> {
    Some(match d {
        "n" => ResizeDirection::North,
        "s" => ResizeDirection::South,
        "e" => ResizeDirection::East,
        "w" => ResizeDirection::West,
        "ne" => ResizeDirection::NorthEast,
        "nw" => ResizeDirection::NorthWest,
        "se" => ResizeDirection::SouthEast,
        "sw" => ResizeDirection::SouthWest,
        _ => return None,
    })
}

// ----------------------------------------------------------------------- app

impl App {
    fn idx(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn active_idx(&self) -> usize {
        self.idx(self.active).unwrap_or(0)
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.idx(self.active).map(|i| &self.tabs[i])
    }

    fn fullscreen(&self) -> bool {
        self.page_fs || self.user_fs
    }

    fn pinned_count(&self) -> usize {
        self.tabs.iter().filter(|t| t.pinned).count()
    }

    fn sidebar_w(&self) -> f64 {
        if self.settings.compact && !self.peek { COMPACT_W } else { self.settings.sidebar_width as f64 }
    }

    /// The UI needs the whole window only for the new tab page and settings.
    /// The rest of the time it is a strip. A small surface is a fast surface.
    fn ui_full(&self) -> bool {
        self.panel != Panel::None || self.active_tab().is_some_and(|t| t.status() == "new")
    }

    /// The page rectangle, in window coordinates.
    fn stage(&self) -> (f64, f64, f64, f64) {
        let (w, h) = logical_size(&self.ctx);
        if self.fullscreen() {
            return (0.0, 0.0, w, h);
        }
        let side = self.sidebar_w();
        let g = self.settings.gap as f64;
        let x = if self.settings.sidebar_side == "right" { g } else { side };
        (x, g, (w - side - g).max(1.0), (h - 2.0 * g).max(1.0))
    }

    fn layout(&mut self) {
        let (w, h) = logical_size(&self.ctx);
        let fs = self.fullscreen();
        let _ = self.ui.set_visible(!fs);
        if !fs {
            let side = self.sidebar_w();
            let r = if self.ui_full() {
                logical_rect(0.0, 0.0, w, h)
            } else if self.settings.sidebar_side == "right" {
                logical_rect(w - side, 0.0, side, h)
            } else {
                logical_rect(0.0, 0.0, side, h)
            };
            let _ = self.ui.set_bounds(r);
        }
        let (x, y, sw, sh) = self.stage();
        let scale = self.ctx.window.scale_factor();
        let radius = if fs { 0 } else { (self.settings.radius as f64 * scale).round() as i32 };
        let i = self.active_idx();
        if let Some(t) = self.tabs.get(i)
            && let Some(v) = &t.view
        {
            let _ = v.set_bounds(logical_rect(x, y, sw, sh));
            // The corner clip is a region rebuild and a repaint. Only when size changed.
            let key = (t.id, (sw * scale) as u32, (sh * scale) as u32 + radius as u32);
            if self.rounded != Some(key) {
                self.rounded = Some(key);
                engine::round_corners(v, radius);
            }
        }
    }

    fn focus_page(&self) {
        match self.active_tab().and_then(|t| t.view.as_ref()) {
            Some(v) if self.panel == Panel::None => {
                let _ = v.focus();
            }
            _ => {
                let _ = self.ui.focus();
            }
        }
    }

    /// Mark the UI stale. The real send happens once, after the event burst.
    fn push(&mut self) {
        self.ui_dirty = true;
    }

    fn flush(&mut self) {
        if !self.ui_ready {
            return;
        }
        if self.ui_dirty {
            self.ui_dirty = false;
            let (w, h) = logical_size(&self.ctx);
            let (sx, sy, sw, sh) = self.stage();
            let tabs: Vec<Value> = self
                .tabs
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id, "title": t.title, "url": t.url,
                        "loading": t.loading && t.view.is_some(), "canBack": t.back, "canFwd": t.fwd,
                        "pinned": t.pinned, "audio": t.audio, "muted": t.muted, "status": t.status(),
                    })
                })
                .collect();
            let state = json!({
                "tabs": tabs,
                "active": self.active,
                "settings": self.settings,
                "dark": self.ctx.dark.get(),
                "maximized": self.ctx.window.is_maximized(),
                "fullscreen": self.fullscreen(),
                "panel": self.panel.name(),
                "cpuCap": self.job.is_some(),
                "peek": self.peek,
                "frame": if self.ui_full() { "full" } else { "strip" },
                "win": [w, h],
                "stage": [sx, sy, sw, sh],
                "mem": self.mem.map(|(b, p)| json!({"bytes": b, "procs": p})),
                "blocked": self.blocker.blocked(),
            });
            let _ = self.ui.evaluate_script(&format!("window.lumen&&lumen.state({state})"));
        }
        if !self.fav_queue.is_empty() {
            let mut js = String::new();
            for id in std::mem::take(&mut self.fav_queue) {
                if let Some(i) = self.idx(id) {
                    js.push_str(&format!("lumen.fav({id},{});", Value::String(self.tabs[i].favicon.clone())));
                }
            }
            let _ = self.ui.evaluate_script(&format!("if(window.lumen){{{js}}}"));
        }
    }

    // ---------------------------------------------------------------- tabs

    fn restore(&mut self, session: Session) {
        let mut active = None;
        for (i, saved) in session.tabs.into_iter().enumerate() {
            let mut t = Tab::new(self.ctx.alloc_id(), saved.url);
            t.title = saved.title;
            t.favicon = saved.favicon;
            t.pinned = saved.pinned;
            if i == session.active {
                active = Some(t.id);
            }
            self.tabs.push(t);
        }
        // Pinned tabs live at the front. The UI groups them; the order must agree.
        self.tabs.sort_by_key(|t| !t.pinned);
        if self.tabs.is_empty() {
            self.tabs.push(Tab::new(self.ctx.alloc_id(), String::new()));
        }
        // Only the active tab gets a page. The rest stay "off" until clicked.
        let id = active.filter(|id| self.idx(*id).is_some()).unwrap_or(self.tabs[0].id);
        self.activate(id);
    }

    fn ensure_view(&mut self, i: usize) {
        if self.tabs[i].view.is_some() || self.tabs[i].url.is_empty() {
            return;
        }
        let (x, y, w, h) = self.stage();
        let t = &mut self.tabs[i];
        if let Ok(v) = engine::build_tab(&self.ctx, t.id, Some(&t.url), logical_rect(x, y, w, h)) {
            if t.muted {
                engine::set_muted(&v, true);
            }
            t.view = Some(v);
            t.suspended = false;
            t.trimmed = false;
        }
        self.limit_tab(i);
    }

    /// New page: give it the bandwidth cap and, if the Control page is open, a byte counter.
    fn limit_tab(&mut self, i: usize) {
        let t = &self.tabs[i];
        let Some(v) = &t.view else { return };
        if self.settings.net_limit {
            monitor::throttle(v, Some(self.settings.net_limit_kbps));
        }
        if self.panel == Panel::Control
            && !self.watches.contains_key(&t.id)
            && let Some(w) = monitor::watch(v, t.id, self.net.clone())
        {
            self.watches.insert(t.id, w);
        }
    }

    fn activate(&mut self, id: u64) {
        let Some(i) = self.idx(id) else { return };
        let prev = (id != self.active).then(|| self.idx(self.active)).flatten();
        self.active = id;
        self.leave_panel();
        self.ensure_view(i);
        // Show the new page first, then hide the old. No flash of empty stage between.
        self.layout();
        let t = &mut self.tabs[i];
        t.hidden_at = None;
        t.suspended = false;
        if let Some(v) = &t.view {
            if t.trimmed {
                let _ = v.set_memory_usage_level(MemoryUsageLevel::Normal);
                t.trimmed = false;
            }
            let _ = v.set_visible(true);
        }
        if let Some(p) = prev {
            let p = &mut self.tabs[p];
            if let Some(v) = &p.view {
                let _ = v.set_visible(false);
            }
            p.hidden_at = Some(Instant::now());
        }
        self.focus_page();
        self.dirty = true;
        self.push();
    }

    fn new_tab(&mut self, url: Option<String>, after: Option<usize>) -> u64 {
        let id = self.ctx.alloc_id();
        let t = Tab::new(id, url.unwrap_or_default());
        let floor = self.pinned_count();
        match after {
            Some(i) if i < self.tabs.len() => self.tabs.insert((i + 1).max(floor), t),
            _ => self.tabs.push(t),
        }
        self.activate(id);
        id
    }

    fn close(&mut self, id: u64) {
        let Some(i) = self.idx(id) else { return };
        let t = self.tabs.remove(i);
        if let Some(w) = self.watches.remove(&id) {
            monitor::unwatch(None, w, true);
        }
        if !t.url.is_empty() {
            self.closed.push(SavedTab { url: t.url, title: t.title, favicon: t.favicon, pinned: t.pinned });
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
        drop(t.view);
        if self.tabs.is_empty() {
            self.new_tab(None, None);
            return;
        }
        if id == self.active {
            let next = self.tabs[i.min(self.tabs.len() - 1)].id;
            self.active = 0;
            self.activate(next);
        } else {
            self.dirty = true;
            self.push();
        }
    }

    /// Drop the page, keep the tab. Frees the renderer process.
    fn discard(&mut self, i: usize) {
        let t = &mut self.tabs[i];
        if t.id == self.active || t.view.is_none() {
            return;
        }
        if let Some(w) = self.watches.remove(&t.id) {
            monitor::unwatch(None, w, true);
        }
        t.view = None;
        t.suspended = false;
        t.trimmed = false;
        t.loading = false;
        t.audio = false;
    }

    fn navigate(&mut self, raw: &str) {
        let Some(url) = resolve_input(raw, &self.settings.search) else { return };
        let i = self.active_idx();
        self.leave_panel();
        if let Some(v) = &self.tabs[i].view {
            let _ = v.load_url(&url);
            self.tabs[i].url = url;
        } else {
            self.tabs[i].url = url;
            self.ensure_view(i);
            if let Some(v) = &self.tabs[i].view {
                let _ = v.set_visible(true);
            }
        }
        self.layout();
        self.focus_page();
        self.dirty = true;
        self.push();
    }

    fn cycle(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        let i = (self.active_idx() as isize + step).rem_euclid(n) as usize;
        let id = self.tabs[i].id;
        self.activate(id);
    }

    fn set_panel(&mut self, p: Panel) {
        let was = self.panel;
        self.panel = p;
        if was == Panel::Control && p != Panel::Control {
            self.stop_monitor();
        }
        if p == Panel::Control && was != Panel::Control {
            self.start_monitor();
        }
        if let Some(v) = self.active_tab().and_then(|t| t.view.as_ref()) {
            let _ = v.set_visible(p == Panel::None);
        }
        self.layout();
        self.focus_page();
        self.push();
    }

    /// Switching tabs or navigating closes any panel, quietly.
    fn leave_panel(&mut self) {
        if self.panel == Panel::Control {
            self.stop_monitor();
        }
        self.panel = Panel::None;
    }

    fn start_monitor(&mut self) {
        self.net.borrow_mut().clear();
        self.prev_cpu.clear();
        for i in 0..self.tabs.len() {
            self.limit_tab(i);
        }
        self.sample();
    }

    fn stop_monitor(&mut self) {
        self.next_sample = None;
        let keep = self.settings.net_limit;
        for (id, w) in std::mem::take(&mut self.watches) {
            let v = self.idx(id).and_then(|i| self.tabs[i].view.as_ref());
            monitor::unwatch(v, w, keep);
        }
    }

    fn sample(&mut self) {
        self.next_sample = Some(Instant::now() + SAMPLE);
        if let Some(env) = self.ctx.env.borrow().as_ref() {
            monitor::sample(env, self.ctx.proxy.clone());
        }
    }

    /// Raw process numbers in; per-tab memory, CPU and network out, straight to the UI.
    fn on_usage(&mut self, procs: Vec<monitor::Proc>) {
        if self.panel != Panel::Control {
            return;
        }
        let now = Instant::now();
        let secs = (now - self.prev_at).as_secs_f64().max(0.001);
        self.prev_at = now;
        let first = self.prev_cpu.is_empty();
        // Main-frame id -> tab.
        let mut owner: HashMap<u32, u64> = HashMap::new();
        for t in &self.tabs {
            if let Some(f) = t.view.as_ref().and_then(monitor::frame_id) {
                owner.insert(f, t.id);
            }
        }
        let ui_frame = monitor::frame_id(&self.ui);
        let mut tabs: HashMap<u64, (f64, f64)> = HashMap::new();
        let (mut ui, mut core, mut total) = ((0.0, 0.0), (0.0, 0.0), (0.0, 0.0));
        let mut next_cpu = HashMap::new();
        let mut parts: Vec<Value> = Vec::new();
        for p in &procs {
            let prev = self.prev_cpu.get(&p.pid).copied().unwrap_or(p.cpu);
            next_cpu.insert(p.pid, p.cpu);
            // 100 ns ticks to percent of the whole machine.
            let cpu = if first { 0.0 } else { (p.cpu.saturating_sub(prev) as f64 / 1e7) / secs / self.cores * 100.0 };
            let mem = p.mem as f64;
            total.0 += mem;
            total.1 += cpu;
            let mine: Vec<u64> = p.frames.iter().filter_map(|f| owner.get(f).copied()).collect();
            if !mine.is_empty() {
                // A process shared by several tabs: split it evenly. Honest enough.
                let n = mine.len() as f64;
                for id in mine {
                    let e = tabs.entry(id).or_default();
                    e.0 += mem / n;
                    e.1 += cpu / n;
                }
            } else if ui_frame.is_some_and(|f| p.frames.contains(&f)) {
                ui = (ui.0 + mem, ui.1 + cpu);
            } else {
                core = (core.0 + mem, core.1 + cpu);
                parts.push(json!({"kind": p.kind, "mem": mem, "cpu": cpu}));
            }
        }
        self.prev_cpu = next_cpu;
        let net: HashMap<u64, u64> = std::mem::take(&mut *self.net.borrow_mut());
        let net_total: u64 = net.values().sum();
        let per_tab: serde_json::Map<String, Value> = self
            .tabs
            .iter()
            .map(|t| {
                let (m, c) = tabs.get(&t.id).copied().unwrap_or_default();
                let n = net.get(&t.id).copied().unwrap_or(0) as f64 / secs;
                (t.id.to_string(), json!({"mem": m, "cpu": c, "net": n}))
            })
            .collect();
        let report = json!({
            "tabs": per_tab,
            "ui": {"mem": ui.0, "cpu": ui.1},
            "core": {"mem": core.0, "cpu": core.1, "parts": parts},
            "total": {"mem": total.0, "cpu": total.1, "net": net_total as f64 / secs, "procs": procs.len()},
        });
        let _ = self.ui.evaluate_script(&format!("window.lumen&&lumen.usage&&lumen.usage({report})"));
    }

    /// CPU cap and bandwidth cap. The RAM limit is enforced in tick().
    fn apply_limits(&mut self, old: Option<&Settings>) {
        let s = &self.settings;
        let cpu_changed = old.is_none_or(|o| (o.cpu_limit, o.cpu_limit_pct) != (s.cpu_limit, s.cpu_limit_pct));
        if cpu_changed && let Some(job) = &self.job {
            job.cap_cpu(s.cpu_limit.then_some(s.cpu_limit_pct));
        }
        let net_changed = old.is_some_and(|o| (o.net_limit, o.net_limit_kbps) != (s.net_limit, s.net_limit_kbps));
        if net_changed {
            let cap = s.net_limit.then_some(s.net_limit_kbps);
            for v in self.tabs.iter().filter_map(|t| t.view.as_ref()) {
                monitor::throttle(v, cap);
            }
        }
    }

    fn set_fullscreen(&mut self) {
        let on = self.fullscreen();
        self.ctx.window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        self.rounded = None;
        self.layout();
        self.push();
    }

    // ------------------------------------------------------------ settings

    /// Solid: an ordinary opaque window, the cheapest thing Windows can draw.
    /// Glass: DWM backdrop through black client pixels and a transparent UI.
    fn apply_backdrop(&self) {
        let w = &*self.ctx.window;
        let dark = self.ctx.dark.get();
        let _ = window_vibrancy::clear_mica(w);
        let _ = window_vibrancy::clear_acrylic(w);
        let _ = window_vibrancy::clear_tabbed(w);
        let glass = match self.settings.backdrop.as_str() {
            "mica" => window_vibrancy::apply_mica(w, Some(dark)).is_ok(),
            "tabbed" => window_vibrancy::apply_tabbed(w, Some(dark)).is_ok(),
            "acrylic" => window_vibrancy::apply_acrylic(w, Some((0, 0, 0, 0))).is_ok(),
            _ => false,
        };
        let (r, g, b) = self.bg;
        if glass {
            w.set_background_color(Some((0, 0, 0, 255)));
            let _ = self.ui.set_background_color((0, 0, 0, 0));
        } else {
            w.set_background_color(Some((r, g, b, 255)));
            let _ = self.ui.set_background_color((r, g, b, 255));
        }
        engine::set_glass(w.hwnd(), glass);
    }

    fn retheme(&mut self) {
        let dark = resolve_dark(&self.settings, self.ctx.window.theme());
        self.ctx.dark.set(dark);
        let theme = engine::theme(dark);
        let _ = self.ui.set_theme(theme);
        for v in self.tabs.iter().filter_map(|t| t.view.as_ref()) {
            let _ = v.set_theme(theme);
        }
        self.apply_backdrop();
    }

    fn apply_settings(&mut self, mut new: Settings) {
        new.clamp();
        let old = std::mem::replace(&mut self.settings, new);
        if old.theme != self.settings.theme {
            self.retheme();
        } else if old.backdrop != self.settings.backdrop {
            self.apply_backdrop();
        }
        if old.tracking != self.settings.tracking {
            engine::set_tracking(&self.ui, &self.settings.tracking);
        }
        if old.radius != self.settings.radius {
            self.rounded = None;
        }
        self.blocker.set_enabled(self.settings.adblock);
        self.apply_limits(Some(&old));
        store::save_settings(&self.settings);
        self.layout();
        self.push();
    }

    // -------------------------------------------------------------- events

    fn on_event(&mut self, ev: Ev) {
        if cfg!(debug_assertions) {
            debug_log(&ev);
        }
        match ev {
            Ev::Ui(msg) => self.on_ui(&msg),
            Ev::Key(a) => self.on_key(a),
            Ev::Title(id, t) => {
                if let Some(i) = self.idx(id) {
                    self.tabs[i].title = t;
                    self.dirty = true;
                    self.push();
                }
            }
            Ev::Url(id, u) => {
                if let Some(i) = self.idx(id) {
                    self.tabs[i].url = u;
                    self.dirty = true;
                    self.push();
                }
            }
            Ev::Loading(id, on) => {
                if let Some(i) = self.idx(id)
                    && self.tabs[i].loading != on
                {
                    self.tabs[i].loading = on;
                    self.push();
                }
            }
            Ev::Favicon(id, f) => {
                if let Some(i) = self.idx(id)
                    && self.tabs[i].favicon != f
                {
                    self.tabs[i].favicon = f;
                    self.fav_queue.push(id);
                    self.dirty = true;
                }
            }
            Ev::Nav(id, b, f) => {
                if let Some(i) = self.idx(id)
                    && (self.tabs[i].back, self.tabs[i].fwd) != (b, f)
                {
                    self.tabs[i].back = b;
                    self.tabs[i].fwd = f;
                    self.push();
                }
            }
            Ev::Audio(id, on) => {
                if let Some(i) = self.idx(id) {
                    self.tabs[i].audio = on;
                    self.push();
                }
            }
            Ev::Suspended(id, ok) => {
                if let Some(i) = self.idx(id) {
                    // Only count it if the tab is still hidden. User may have clicked back.
                    self.tabs[i].suspended = ok && id != self.active;
                    self.push();
                }
            }
            Ev::FullScreen(id, on) => {
                if id == self.active {
                    self.page_fs = on;
                    self.set_fullscreen();
                }
            }
            Ev::CloseMe(id) => self.close(id),
            Ev::Usage(procs) => self.on_usage(procs),
            Ev::Popup(id) => {
                let found = {
                    let mut p = self.ctx.popups.borrow_mut();
                    p.iter().position(|(pid, _)| *pid == id).map(|k| p.remove(k).1)
                };
                if let Some(v) = found {
                    let mut t = Tab::new(id, String::new());
                    t.view = Some(v);
                    t.title = "Loading…".into();
                    let at = (self.active_idx() + 1).max(self.pinned_count()).min(self.tabs.len());
                    self.tabs.insert(at, t);
                    self.limit_tab(at);
                    self.activate(id);
                }
            }
        }
    }

    fn on_key(&mut self, a: Action) {
        let i = self.active_idx();
        match a {
            Action::NewTab => {
                // Like Chrome: a new tab means "I want to type an address".
                self.new_tab(None, None);
                self.on_key(Action::Omnibox);
            }
            Action::CloseTab => self.close(self.active),
            Action::Reopen => {
                if let Some(t) = self.closed.pop() {
                    self.new_tab(Some(t.url), Some(i));
                }
            }
            Action::NextTab => self.cycle(1),
            Action::PrevTab => self.cycle(-1),
            Action::Nth(n) => {
                if let Some(t) = self.tabs.get(n) {
                    let id = t.id;
                    self.activate(id);
                }
            }
            Action::LastTab => {
                let id = self.tabs[self.tabs.len() - 1].id;
                self.activate(id);
            }
            Action::Omnibox => {
                let _ = self.ui.focus();
                let _ = self.ui.evaluate_script("window.lumen&&lumen.focusOmnibox()");
            }
            Action::Reload => match &self.tabs[i].view {
                Some(v) => {
                    let _ = v.reload();
                }
                None => self.activate(self.active),
            },
            Action::Back => {
                if let Some(v) = &self.tabs[i].view {
                    let _ = v.go_back();
                }
            }
            Action::Forward => {
                if let Some(v) = &self.tabs[i].view {
                    let _ = v.go_forward();
                }
            }
            Action::Sidebar => {
                let mut s = self.settings.clone();
                s.compact = !s.compact;
                self.apply_settings(s);
            }
            Action::Fullscreen => {
                self.user_fs = !self.user_fs;
                self.set_fullscreen();
            }
            Action::Settings => {
                let p = if self.panel == Panel::Settings { Panel::None } else { Panel::Settings };
                self.set_panel(p);
            }
            Action::Control => {
                let p = if self.panel == Panel::Control { Panel::None } else { Panel::Control };
                self.set_panel(p);
            }
            Action::DevTools => match &self.tabs[i].view {
                Some(v) => v.open_devtools(),
                None => self.ui.open_devtools(),
            },
        }
    }

    fn on_ui(&mut self, raw: &str) {
        let Ok(m) = serde_json::from_str::<Value>(raw) else { return };
        let id = m["id"].as_u64().unwrap_or(0);
        match m["t"].as_str().unwrap_or("") {
            "ready" => {
                self.ui_ready = true;
                self.fav_queue = self.tabs.iter().filter(|t| !t.favicon.is_empty()).map(|t| t.id).collect();
                self.push();
                if !self.shown {
                    self.shown = true;
                    self.ctx.window.set_visible(true);
                    self.layout();
                    self.focus_page();
                }
            }
            "bg" => {
                let c = &m["rgb"];
                let px = |k: usize| c[k].as_u64().unwrap_or(0).min(255) as u8;
                let bg = (px(0), px(1), px(2));
                if bg != self.bg {
                    self.bg = bg;
                    self.apply_backdrop();
                }
            }
            "nav" => self.navigate(m["url"].as_str().unwrap_or("")),
            "newtab" => {
                self.new_tab(None, None);
            }
            "activate" => self.activate(id),
            "close" => self.close(id),
            "pin" => {
                if let Some(i) = self.idx(id) {
                    let mut t = self.tabs.remove(i);
                    t.pinned = !t.pinned;
                    // Pinned: end of the pinned block. Unpinned: first after it.
                    let at = self.pinned_count();
                    self.tabs.insert(at, t);
                    self.dirty = true;
                    self.push();
                }
            }
            "mute" => {
                if let Some(i) = self.idx(id) {
                    let t = &mut self.tabs[i];
                    t.muted = !t.muted;
                    if let Some(v) = &t.view {
                        engine::set_muted(v, t.muted);
                    }
                    self.push();
                }
            }
            "sleep" => {
                if let Some(i) = self.idx(id) {
                    self.discard(i);
                    self.push();
                }
            }
            "duplicate" => {
                if let Some(i) = self.idx(id) {
                    let url = self.tabs[i].url.clone();
                    if !url.is_empty() {
                        self.new_tab(Some(url), Some(i));
                    }
                }
            }
            "closeOthers" => {
                let others: Vec<u64> =
                    self.tabs.iter().filter(|t| t.id != id && !t.pinned).map(|t| t.id).collect();
                if self.idx(id).is_some() {
                    self.activate(id);
                }
                for o in others {
                    self.close(o);
                }
            }
            "move" => {
                if let Some(i) = self.idx(id) {
                    let t = self.tabs.remove(i);
                    // Stay inside your own group: pinned with pinned, the rest with the rest.
                    let pins = self.pinned_count();
                    let (lo, hi) = if t.pinned { (0, pins) } else { (pins, self.tabs.len()) };
                    let to = (m["to"].as_u64().unwrap_or(0) as usize).clamp(lo, hi);
                    self.tabs.insert(to, t);
                    self.dirty = true;
                    self.push();
                }
            }
            "back" => self.on_key(Action::Back),
            "fwd" => self.on_key(Action::Forward),
            "reload" => self.on_key(Action::Reload),
            "stop" => {
                if let Some(v) = self.active_tab().and_then(|t| t.view.as_ref()) {
                    let _ = v.evaluate_script("window.stop()");
                }
            }
            "reopen" => self.on_key(Action::Reopen),
            "devtools" => self.on_key(Action::DevTools),
            "win" => {
                let w = &self.ctx.window;
                match m["a"].as_str().unwrap_or("") {
                    "min" => w.set_minimized(true),
                    "max" => w.set_maximized(!w.is_maximized()),
                    "close" => self.exit = true,
                    _ => {}
                }
            }
            "drag" => {
                let _ = self.ctx.window.drag_window();
            }
            "resize" => {
                if let Some(d) = resize_dir(m["d"].as_str().unwrap_or("")) {
                    let _ = self.ctx.window.drag_resize_window(d);
                }
            }
            "settings" => {
                if let Ok(s) = serde_json::from_value::<Settings>(m["v"].clone()) {
                    self.apply_settings(s);
                }
            }
            "panel" => {
                let p = match (m["open"].as_bool().unwrap_or(false), m["which"].as_str()) {
                    (false, _) => Panel::None,
                    (true, Some("control")) => Panel::Control,
                    (true, _) => Panel::Settings,
                };
                self.set_panel(p);
            }
            "peek" => {
                self.peek = m["on"].as_bool().unwrap_or(false);
                self.layout();
                self.push();
            }
            "focusPage" => self.focus_page(),
            _ => {}
        }
    }

    // ---------------------------------------------------------------- tick

    /// Every few seconds: trim, freeze and discard hidden tabs, read memory, save.
    fn tick(&mut self) {
        let now = Instant::now();
        self.next_tick = now + TICK;

        let sleep = Duration::from_secs(self.settings.sleep_after_min as u64 * 60);
        let discard = Duration::from_secs(self.settings.discard_after_min as u64 * 60);
        for i in 0..self.tabs.len() {
            let t = &mut self.tabs[i];
            if t.id == self.active || t.view.is_none() || t.audio {
                continue;
            }
            let Some(at) = t.hidden_at else { continue };
            let idle = now - at;
            if idle >= discard && !t.pinned {
                self.discard(i);
                self.push();
            } else if idle >= sleep && !t.suspended {
                if let Some(v) = &t.view {
                    engine::try_suspend(v, t.id, self.ctx.proxy.clone());
                }
            } else if idle >= TRIM_AFTER && !t.trimmed {
                if let Some(v) = &t.view {
                    let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
                }
                t.trimmed = true;
            }
        }

        if self.settings.show_memory
            && let Some(env) = self.ctx.env.borrow().as_ref()
        {
            let m = engine::memory(env);
            // Only redraw when it moved by more than 1 MB.
            if self.mem.is_none_or(|(b, _)| b.abs_diff(m.0) > 1 << 20) {
                self.mem = Some(m);
                self.ui_dirty = true;
            }
        }
        self.enforce_ram(now);

        // The UI never said ready. Show the window anyway so the user is not stuck.
        if !self.shown && now - self.born > Duration::from_secs(3) {
            self.shown = true;
            self.ctx.window.set_visible(true);
        }

        if self.dirty {
            self.save_session();
        }
    }

    /// Soft RAM limit. Over it: unload the hidden tab looked at longest ago.
    /// One per tick, so a spike never empties the window.
    fn enforce_ram(&mut self, now: Instant) {
        if !self.settings.ram_limit {
            return;
        }
        let used = if self.settings.show_memory {
            self.mem.map_or(0, |m| m.0)
        } else {
            self.ctx.env.borrow().as_ref().map_or(0, |e| engine::memory(e).0)
        };
        if used <= self.settings.ram_limit_mb as u64 * 1024 * 1024 {
            return;
        }
        let victim = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.id != self.active && t.view.is_some() && !t.pinned && !t.audio)
            .min_by_key(|(_, t)| t.hidden_at.unwrap_or(now))
            .map(|(i, _)| i);
        if let Some(i) = victim {
            self.discard(i);
            self.push();
        }
    }

    fn save_session(&mut self) {
        self.dirty = false;
        let (width, height) = logical_size(&self.ctx);
        let maximized = self.ctx.window.is_maximized();
        let keep: Vec<&Tab> = self.tabs.iter().filter(|t| !t.url.is_empty() || t.id == self.active).collect();
        let s = Session {
            tabs: keep
                .iter()
                .map(|t| SavedTab { url: t.url.clone(), title: t.title.clone(), favicon: t.favicon.clone(), pinned: t.pinned })
                .collect(),
            active: keep.iter().position(|t| t.id == self.active).unwrap_or(0),
            width: if maximized || self.fullscreen() { 0.0 } else { width },
            height: if maximized || self.fullscreen() { 0.0 } else { height },
            maximized,
        };
        store::save_session(&s);
    }
}
