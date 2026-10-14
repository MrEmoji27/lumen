#![windows_subsystem = "windows"]

//! Lumen. A small browser shell.
//! One window. One UI page. Pages on top. Rust decides who lives and who sleeps.

mod engine;
mod store;
mod blocker;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

use base64::Engine as _;
use serde_json::{Value, json};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::windows::{WindowBuilderExtWindows, WindowExtWindows},
    window::{Fullscreen, ResizeDirection, Theme as WinTheme, WindowBuilder},
};
use wry::{Rect, WebContext, WebView, WebViewExtWindows};

use engine::{Action, Ctx, Ev};
use store::{SavedTab, Session, Settings};

const TICK: Duration = Duration::from_secs(3);
const MAX_CLOSED: usize = 25;

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
    panel: bool,
    peek: bool,
    page_fs: bool,
    user_fs: bool,
    ui_ready: bool,
    shown: bool,
    dirty: bool,
    mem: Option<(u64, u32)>,
    born: Instant,
    next_tick: Instant,
    exit: bool,
}

fn main() {
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
        .with_decorations(false)
        .with_transparent(true)
        // DWM draws the backdrop through black client pixels of an extended frame.
        // Without this the client stays white and the glass never shows.
        .with_background_color((0, 0, 0, 0))
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
    let ui = engine::build_ui(&ctx, &mut web_context, ui_html(), full_rect(&ctx)).expect("ui webview");
    engine::set_tracking(&ui, &settings.tracking);

    let mut app = App {
        ctx,
        ui,
        _web_context: web_context,
        blocker,
        tabs: Vec::new(),
        active: 0,
        closed: Vec::new(),
        settings,
        panel: false,
        peek: false,
        page_fs: false,
        user_fs: false,
        ui_ready: false,
        shown: false,
        dirty: false,
        mem: None,
        born: Instant::now(),
        next_tick: Instant::now() + TICK,
        exit: false,
    };
    app.apply_backdrop();
    app.restore(session);

    event_loop.run(move |event, _, cf| {
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => app.tick(),
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::Resized(size) => {
                    if size.width == 0 || size.height == 0 {
                        // Minimized. Nobody is looking. Trim the active page too.
                        if let Some(v) = app.active_tab().and_then(|t| t.view.as_ref()) {
                            let _ = v.set_memory_usage_level(wry::MemoryUsageLevel::Low);
                        }
                    } else {
                        app.layout();
                        app.reveal();
                        app.push();
                    }
                }
                WindowEvent::ScaleFactorChanged { .. } => app.layout(),
                // Alt+Tab lands focus on the bare window. Hand it to the page, or keys go nowhere.
                WindowEvent::Focused(true) => app.focus_page(),
                WindowEvent::ThemeChanged(_) if app.settings.theme == "system" => {
                    app.retheme();
                    app.push();
                }
                WindowEvent::CloseRequested => app.exit = true,
                _ => {}
            },
            Event::UserEvent(ev) => app.on_event(ev),
            _ => {}
        }
        if app.exit {
            app.save_session();
            *cf = ControlFlow::Exit;
        } else {
            *cf = ControlFlow::WaitUntil(app.next_tick);
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
        if let Some((_, tld)) = host_only.rsplit_once('.') {
            if tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()) {
                return Some(format!("https://{s}"));
            }
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
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(store::data_dir().join("debug.log")) {
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

    /// The page rectangle. ui/app.js computes the same numbers. Keep them in step.
    fn stage(&self) -> Rect {
        let (w, h) = logical_size(&self.ctx);
        if self.fullscreen() {
            return logical_rect(0.0, 0.0, w, h);
        }
        let s = &self.settings;
        let side = if s.compact && !self.peek { 68.0 } else { s.sidebar_width as f64 };
        let g = s.gap as f64;
        let x = if s.sidebar_side == "right" { g } else { side };
        logical_rect(x, g, w - side - g, h - 2.0 * g)
    }

    fn layout(&self) {
        let _ = self.ui.set_bounds(full_rect(&self.ctx));
        if let Some(v) = self.active_tab().and_then(|t| t.view.as_ref()) {
            let _ = v.set_bounds(self.stage());
            let r = if self.fullscreen() { 0.0 } else { self.settings.radius as f64 };
            engine::round_corners(v, (r * self.ctx.window.scale_factor()).round() as i32);
        }
    }

    /// Show the active page unless the settings panel covers the stage.
    fn reveal(&self) {
        if let Some(v) = self.active_tab().and_then(|t| t.view.as_ref()) {
            engine::set_hidden(v, self.panel);
        }
    }

    fn focus_page(&self) {
        match self.active_tab().and_then(|t| t.view.as_ref()) {
            Some(v) if !self.panel => {
                let _ = v.focus();
            }
            _ => {
                let _ = self.ui.focus();
            }
        }
    }

    fn push(&self) {
        if !self.ui_ready {
            return;
        }
        let tabs: Vec<Value> = self
            .tabs
            .iter()
            .map(|t| {
                json!({
                    "id": t.id, "title": t.title, "url": t.url, "favicon": t.favicon,
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
            "panel": if self.panel { "settings" } else { "none" },
            "peek": self.peek,
            "mem": self.mem.map(|(b, p)| json!({"bytes": b, "procs": p})),
            "blocked": self.blocker.blocked(),
        });
        let _ = self.ui.evaluate_script(&format!("window.lumen&&lumen.state({state})"));
    }

    // ---------------------------------------------------------------- tabs

    fn restore(&mut self, session: Session) {
        for saved in session.tabs {
            let mut t = Tab::new(self.ctx.alloc_id(), saved.url);
            t.title = saved.title;
            t.favicon = saved.favicon;
            t.pinned = saved.pinned;
            self.tabs.push(t);
        }
        if self.tabs.is_empty() {
            self.tabs.push(Tab::new(self.ctx.alloc_id(), String::new()));
        }
        // Only the active tab gets a page. The rest stay "off" until clicked.
        let i = session.active.min(self.tabs.len() - 1);
        let id = self.tabs[i].id;
        self.activate(id);
    }

    fn ensure_view(&mut self, i: usize) {
        if self.tabs[i].view.is_some() || self.tabs[i].url.is_empty() {
            return;
        }
        let stage = self.stage();
        let t = &mut self.tabs[i];
        if let Ok(v) = engine::build_tab(&self.ctx, t.id, Some(&t.url), stage) {
            if t.muted {
                engine::set_muted(&v, true);
            }
            t.view = Some(v);
            t.suspended = false;
        }
    }

    fn activate(&mut self, id: u64) {
        let Some(i) = self.idx(id) else { return };
        if id != self.active {
            if let Some(prev) = self.idx(self.active) {
                let p = &mut self.tabs[prev];
                if let Some(v) = &p.view {
                    engine::set_hidden(v, true);
                }
                p.hidden_at = Some(Instant::now());
            }
            self.active = id;
        }
        self.panel = false;
        self.tabs[i].hidden_at = None;
        self.ensure_view(i);
        self.tabs[i].suspended = false;
        self.layout();
        self.reveal();
        self.focus_page();
        self.dirty = true;
        self.push();
    }

    fn new_tab(&mut self, url: Option<String>, after: Option<usize>) -> u64 {
        let id = self.ctx.alloc_id();
        let t = Tab::new(id, url.unwrap_or_default());
        match after {
            Some(i) if i < self.tabs.len() => self.tabs.insert(i + 1, t),
            _ => self.tabs.push(t),
        }
        self.activate(id);
        id
    }

    fn close(&mut self, id: u64) {
        let Some(i) = self.idx(id) else { return };
        let t = self.tabs.remove(i);
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
        t.view = None;
        t.suspended = false;
        t.loading = false;
        t.audio = false;
    }

    fn navigate(&mut self, raw: &str) {
        let Some(url) = resolve_input(raw, &self.settings.search) else { return };
        let i = self.active_idx();
        if let Some(v) = &self.tabs[i].view {
            let _ = v.load_url(&url);
            self.tabs[i].url = url;
        } else {
            self.tabs[i].url = url;
            self.ensure_view(i);
            self.layout();
        }
        self.panel = false;
        self.reveal();
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

    fn set_fullscreen(&mut self) {
        let on = self.fullscreen();
        self.ctx.window.set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        self.layout();
        self.push();
    }

    // ------------------------------------------------------------ settings

    fn apply_backdrop(&self) {
        let w = &*self.ctx.window;
        let dark = self.ctx.dark.get();
        let _ = window_vibrancy::clear_mica(w);
        let _ = window_vibrancy::clear_acrylic(w);
        let _ = window_vibrancy::clear_tabbed(w);
        let _ = match self.settings.backdrop.as_str() {
            "mica" => window_vibrancy::apply_mica(w, Some(dark)),
            "tabbed" => window_vibrancy::apply_tabbed(w, Some(dark)),
            "acrylic" => window_vibrancy::apply_acrylic(w, Some(if dark { (20, 18, 16, 120) } else { (240, 236, 230, 120) })),
            _ => Ok(()),
        };
        engine::extend_frame(w.hwnd(), self.settings.backdrop != "solid");
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
        self.blocker.set_enabled(self.settings.adblock);
        if old.tracking != self.settings.tracking {
            engine::set_tracking(&self.ui, &self.settings.tracking);
        }
        store::save_settings(&self.settings);
        self.layout();
        self.push();
    }

    // -------------------------------------------------------------- events

    fn on_event(&mut self, ev: Ev) {
        if cfg!(debug_assertions) {
            debug_log(&ev);
        }
        let tab = |app: &mut App, id: u64| app.idx(id);
        match ev {
            Ev::Ui(msg) => self.on_ui(&msg),
            Ev::Key(a) => self.on_key(a),
            Ev::Title(id, t) => {
                if let Some(i) = tab(self, id) {
                    self.tabs[i].title = t;
                    self.dirty = true;
                }
            }
            Ev::Url(id, u) => {
                if let Some(i) = tab(self, id) {
                    self.tabs[i].url = u;
                    self.dirty = true;
                }
            }
            Ev::Loading(id, on) => {
                if let Some(i) = tab(self, id) {
                    self.tabs[i].loading = on;
                }
            }
            Ev::Favicon(id, f) => {
                if let Some(i) = tab(self, id) {
                    self.tabs[i].favicon = f;
                    self.dirty = true;
                }
            }
            Ev::Nav(id, b, f) => {
                if let Some(i) = tab(self, id) {
                    self.tabs[i].back = b;
                    self.tabs[i].fwd = f;
                }
            }
            Ev::Audio(id, on) => {
                if let Some(i) = tab(self, id) {
                    self.tabs[i].audio = on;
                }
            }
            Ev::Suspended(id, ok) => {
                if let Some(i) = tab(self, id) {
                    // Only count it if the tab is still hidden. User may have clicked back.
                    self.tabs[i].suspended = ok && id != self.active;
                }
            }
            Ev::FullScreen(id, on) => {
                if id == self.active {
                    self.page_fs = on;
                    self.set_fullscreen();
                }
                return;
            }
            Ev::CloseMe(id) => return self.close(id),
            Ev::Popup(id) => {
                let found = {
                    let mut p = self.ctx.popups.borrow_mut();
                    p.iter().position(|(pid, _)| *pid == id).map(|k| p.remove(k).1)
                };
                if let Some(v) = found {
                    let mut t = Tab::new(id, String::new());
                    t.view = Some(v);
                    t.title = "Loading…".into();
                    let at = self.active_idx();
                    self.tabs.insert((at + 1).min(self.tabs.len()), t);
                    self.activate(id);
                }
                return;
            }
        }
        self.push();
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
                self.panel = !self.panel;
                self.reveal();
                self.focus_page();
                self.push();
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
        let w = &self.ctx.window;
        match m["t"].as_str().unwrap_or("") {
            "ready" => {
                self.ui_ready = true;
                self.push();
                if !self.shown {
                    self.shown = true;
                    w.set_visible(true);
                    self.layout();
                    self.focus_page();
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
                    self.tabs[i].pinned = !self.tabs[i].pinned;
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
                    let to = (m["to"].as_u64().unwrap_or(0) as usize).min(self.tabs.len() - 1);
                    let t = self.tabs.remove(i);
                    self.tabs.insert(to, t);
                    self.dirty = true;
                    self.push();
                }
            }
            "back" => self.on_key(Action::Back),
            "fwd" => self.on_key(Action::Forward),
            "reload" => self.on_key(Action::Reload),
            "reopen" => self.on_key(Action::Reopen),
            "devtools" => self.on_key(Action::DevTools),
            "win" => match m["a"].as_str().unwrap_or("") {
                "min" => w.set_minimized(true),
                "max" => w.set_maximized(!w.is_maximized()),
                "close" => self.exit = true,
                _ => {}
            },
            "drag" => {
                let _ = w.drag_window();
            }
            "resize" => {
                if let Some(d) = resize_dir(m["d"].as_str().unwrap_or("")) {
                    let _ = w.drag_resize_window(d);
                }
            }
            "settings" => {
                if let Ok(s) = serde_json::from_value::<Settings>(m["v"].clone()) {
                    self.apply_settings(s);
                }
            }
            "panel" => {
                self.panel = m["open"].as_bool().unwrap_or(false);
                self.reveal();
                if !self.panel {
                    self.focus_page();
                }
                self.push();
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

    /// Every few seconds: put hidden tabs to sleep, read memory, save.
    fn tick(&mut self) {
        let now = Instant::now();
        self.next_tick = now + TICK;

        let sleep = Duration::from_secs(self.settings.sleep_after_min as u64 * 60);
        let discard = Duration::from_secs(self.settings.discard_after_min as u64 * 60);
        let mut changed = false;
        for i in 0..self.tabs.len() {
            let t = &self.tabs[i];
            if t.id == self.active || t.view.is_none() || t.audio {
                continue;
            }
            let Some(at) = t.hidden_at else { continue };
            let idle = now - at;
            if idle >= discard && !t.pinned {
                self.discard(i);
                changed = true;
            } else if idle >= sleep && !t.suspended {
                if let Some(v) = &t.view {
                    engine::try_suspend(v, t.id, self.ctx.proxy.clone());
                }
            }
        }

        if self.settings.show_memory {
            if let Some(env) = self.ctx.env.borrow().as_ref() {
                let m = engine::memory(env);
                // Only redraw when it moved by more than 1 MB.
                if self.mem.is_none_or(|(b, _)| b.abs_diff(m.0) > 1 << 20) {
                    self.mem = Some(m);
                    changed = true;
                }
            }
        }

        // The UI never said ready. Show the window anyway so the user is not stuck.
        if !self.shown && now - self.born > Duration::from_secs(3) {
            self.shown = true;
            self.ctx.window.set_visible(true);
        }

        if self.dirty {
            self.save_session();
        }
        if changed {
            self.push();
        }
    }

    fn save_session(&mut self) {
        self.dirty = false;
        let (width, height) = logical_size(&self.ctx);
        let maximized = self.ctx.window.is_maximized();
        let s = Session {
            tabs: self
                .tabs
                .iter()
                .filter(|t| !t.url.is_empty() || t.id == self.active)
                .map(|t| SavedTab { url: t.url.clone(), title: t.title.clone(), favicon: t.favicon.clone(), pinned: t.pinned })
                .collect(),
            active: 0,
            width: if maximized || self.fullscreen() { 0.0 } else { width },
            height: if maximized || self.fullscreen() { 0.0 } else { height },
            maximized,
        };
        let active = s.tabs.iter().zip(self.tabs.iter().filter(|t| !t.url.is_empty() || t.id == self.active))
            .position(|(_, t)| t.id == self.active)
            .unwrap_or(0);
        store::save_session(&Session { active, ..s });
    }
}
