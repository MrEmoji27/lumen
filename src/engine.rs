//! Engine layer. Everything that talks to WebView2 lives here.
//! main.rs only sees WebView handles and `Ev` messages.
//! To swap engines later (Servo), replace this file.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use tao::{event_loop::EventLoopProxy, window::Window};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, ContainsFullScreenElementChangedEventHandler,
    FaviconChangedEventHandler, GetFaviconCompletedHandler, HistoryChangedEventHandler,
    IsDocumentPlayingAudioChangedEventHandler, Microsoft::Web::WebView2::Win32::*,
    SourceChangedEventHandler, TrySuspendCompletedHandler, WindowCloseRequestedEventHandler,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, RECT},
        Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn},
        System::{
            Com::CoTaskMemFree,
            Threading::{GetCurrentProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::{
            Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT},
            WindowsAndMessaging::GetClientRect,
        },
    },
    core::{Interface, PWSTR},
};
use wry::{
    NewWindowResponse, PageLoadEvent, Rect, Theme, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

/// Messages from engine callbacks to the main loop.
/// Callbacks never touch app state. They post here. No re-entrancy bugs.
#[derive(Debug)]
pub enum Ev {
    Ui(String),
    Title(u64, String),
    Url(u64, String),
    Loading(u64, bool),
    Favicon(u64, String),
    Nav(u64, bool, bool),
    Audio(u64, bool),
    FullScreen(u64, bool),
    CloseMe(u64),
    Popup(u64),
    Suspended(u64, bool),
    Key(Action),
    Usage(Vec<crate::monitor::Proc>),
}

#[derive(Debug, Clone, Copy)]
pub enum Action {
    NewTab,
    CloseTab,
    Reopen,
    NextTab,
    PrevTab,
    Nth(usize),
    LastTab,
    Omnibox,
    Reload,
    Back,
    Forward,
    Sidebar,
    Fullscreen,
    Settings,
    Control,
    DevTools,
}

pub struct Ctx {
    pub window: Rc<Window>,
    pub proxy: EventLoopProxy<Ev>,
    pub env: RefCell<Option<ICoreWebView2Environment>>,
    pub next_id: Cell<u64>,
    /// Webviews born from window.open, waiting for the main loop to adopt them.
    pub popups: RefCell<Vec<(u64, WebView)>>,
    pub dark: Cell<bool>,
    /// Runs on every page right after it is built. Popups included.
    pub on_built: RefCell<Option<Box<dyn Fn(&WebView)>>>,
    /// The first page creates the WebView2 environment with these.
    pub web_context: RefCell<wry::WebContext>,
    pub args: String,
    pub tracking: RefCell<String>,
}

impl Ctx {
    pub fn alloc_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }
}

/// Drain a COM stream. Favicons are small; cap at 256 KB to be safe.
fn read_stream(stream: &windows::Win32::System::Com::IStream) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let mut n = 0u32;
        let hr = unsafe { stream.Read(buf.as_mut_ptr() as _, buf.len() as u32, Some(&mut n)) };
        if hr.is_err() || n == 0 || out.len() > 256 * 1024 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    out
}

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

pub fn theme(dark: bool) -> Theme {
    if dark { Theme::Dark } else { Theme::Light }
}

/// Page background shown before first paint. Kills the white flash.
fn page_bg(dark: bool) -> (u8, u8, u8, u8) {
    if dark { (22, 20, 19, 255) } else { (247, 244, 240, 255) }
}

// ---------------------------------------------------------------- building

/// Chromium switches for the whole browser. Set once, when the environment is born.
/// LUMEN_ARGS appends more, for measuring without a rebuild.
pub fn browser_args(gpu_in_process: bool, share_processes: bool) -> String {
    // wry's own defaults. Passing args replaces them, so keep them.
    let mut a = String::from("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection");
    if gpu_in_process {
        // Measured: about 100 MB less commit, 55 MB less working set, one process fewer.
        // Cost: a GPU driver crash takes the browser down with it.
        a.push_str(" --in-process-gpu --enable-features=NetworkServiceInProcess2");
    }
    if share_processes {
        // Measured on 40 tabs: 54 -> 23 renderers, 310 MB less.
        // Cost: one heavy tab can slow other tabs of the same site.
        a.push_str(" --process-per-site");
    }
    if let Ok(extra) = std::env::var("LUMEN_ARGS") {
        a.push(' ');
        a.push_str(&extra);
    }
    a
}

/// One page. All pages share the UI's environment, so one browser process,
/// one GPU process, one network process for the whole app.
pub fn build_tab(ctx: &Rc<Ctx>, id: u64, url: Option<&str>, bounds: Rect) -> wry::Result<WebView> {
    let env = ctx.env.borrow().clone();
    build_tab_in(ctx, id, url, bounds, env)
}

fn build_tab_in(
    ctx: &Rc<Ctx>,
    id: u64,
    url: Option<&str>,
    bounds: Rect,
    env: Option<ICoreWebView2Environment>,
) -> wry::Result<WebView> {
    let dark = ctx.dark.get();
    let p_title = ctx.proxy.clone();
    let p_load = ctx.proxy.clone();
    let popup_ctx = ctx.clone();

    // No environment yet: this page is the first, and it makes one for everyone.
    let first = env.is_none();
    let mut wc = first.then(|| ctx.web_context.borrow_mut());
    let base = match (env, wc.as_mut()) {
        (Some(env), _) => WebViewBuilder::new().with_environment(env),
        (None, Some(wc)) => WebViewBuilder::new_with_web_context(wc).with_additional_browser_args(ctx.args.clone()),
        (None, None) => unreachable!(),
    };
    let mut b = base
        .with_bounds(bounds)
        .with_visible(false)
        .with_devtools(true)
        .with_hotkeys_zoom(true)
        .with_theme(theme(dark))
        .with_background_color(page_bg(dark))
        .with_document_title_changed_handler(move |t| {
            let _ = p_title.send_event(Ev::Title(id, t));
        })
        .with_on_page_load_handler(move |e, _| {
            let _ = p_load.send_event(Ev::Loading(id, matches!(e, PageLoadEvent::Started)));
        })
        .with_new_window_req_handler(move |_uri, features| {
            // window.open / target=_blank. Make a real tab and hand it to WebView2,
            // so window.opener works (OAuth popups need it).
            let ctx = &popup_ctx;
            let new_id = ctx.alloc_id();
            let bounds = content_bounds_now(ctx);
            match build_tab_in(ctx, new_id, None, bounds, Some(features.opener.environment.clone())) {
                Ok(wv) => {
                    let core = wv.webview();
                    ctx.popups.borrow_mut().push((new_id, wv));
                    let _ = ctx.proxy.send_event(Ev::Popup(new_id));
                    NewWindowResponse::Create { webview: core }
                }
                Err(_) => NewWindowResponse::Deny,
            }
        });
    if let Some(u) = url {
        b = b.with_url(u);
    }
    let wv = b.build_as_child(&*ctx.window)?;
    drop(wc);
    if first {
        ctx.env.replace(Some(wv.environment()));
        set_tracking(&wv, &ctx.tracking.borrow());
    }
    hook_page(&wv, id, ctx.proxy.clone());
    hook_keys(&wv, ctx.proxy.clone());
    if let Some(f) = ctx.on_built.borrow().as_ref() {
        f(&wv);
    }
    Ok(wv)
}

/// Popups are born inside a callback with no access to app state.
/// They start at the bounds of whatever page is showing; main loop fixes them on adopt.
fn content_bounds_now(ctx: &Ctx) -> Rect {
    let size = ctx.window.inner_size();
    Rect {
        position: wry::dpi::PhysicalPosition::new(0, 0).into(),
        size: wry::dpi::PhysicalSize::new(size.width, size.height).into(),
    }
}

// ------------------------------------------------------------------- hooks

fn hook_page(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>) {
    let core = wv.webview();
    let mut token = 0i64;
    unsafe {
        let p = proxy.clone();
        let _ = core.add_SourceChanged(
            &SourceChangedEventHandler::create(Box::new(move |w, _| {
                if let Some(w) = w {
                    let mut s = PWSTR::null();
                    w.Source(&mut s)?;
                    let _ = p.send_event(Ev::Url(id, take(s)));
                }
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let _ = core.add_HistoryChanged(
            &HistoryChangedEventHandler::create(Box::new(move |w, _| {
                if let Some(w) = w {
                    let (mut b, mut f) = (Default::default(), Default::default());
                    w.CanGoBack(&mut b)?;
                    w.CanGoForward(&mut f)?;
                    let _ = p.send_event(Ev::Nav(id, b.as_bool(), f.as_bool()));
                }
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let _ = core.add_ContainsFullScreenElementChanged(
            &ContainsFullScreenElementChangedEventHandler::create(Box::new(move |w, _| {
                if let Some(w) = w {
                    let mut fs = Default::default();
                    w.ContainsFullScreenElement(&mut fs)?;
                    let _ = p.send_event(Ev::FullScreen(id, fs.as_bool()));
                }
                Ok(())
            })),
            &mut token,
        );

        let p = proxy.clone();
        let _ = core.add_WindowCloseRequested(
            &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                let _ = p.send_event(Ev::CloseMe(id));
                Ok(())
            })),
            &mut token,
        );

        if let Ok(c15) = core.cast::<ICoreWebView2_15>() {
            // Ask WebView2 for the icon it already downloaded, as PNG bytes.
            // No second fetch, works offline, survives session restore.
            let p = proxy.clone();
            let _ = c15.add_FaviconChanged(
                &FaviconChangedEventHandler::create(Box::new(move |w, _| {
                    let Some(w) = w.and_then(|w| w.cast::<ICoreWebView2_15>().ok()) else { return Ok(()) };
                    let p = p.clone();
                    w.GetFavicon(
                        COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
                        &GetFaviconCompletedHandler::create(Box::new(move |hr, stream| {
                            if hr.is_ok()
                                && let Some(stream) = stream
                            {
                                let png = read_stream(&stream);
                                if !png.is_empty() {
                                    use base64::Engine as _;
                                    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
                                    let _ = p.send_event(Ev::Favicon(id, format!("data:image/png;base64,{b64}")));
                                }
                            }
                            Ok(())
                        })),
                    )?;
                    Ok(())
                })),
                &mut token,
            );
        }

        if let Ok(c8) = core.cast::<ICoreWebView2_8>() {
            let p = proxy.clone();
            let _ = c8.add_IsDocumentPlayingAudioChanged(
                &IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |w, _| {
                    if let Some(w) = w.and_then(|w| w.cast::<ICoreWebView2_8>().ok()) {
                        let mut on = Default::default();
                        w.IsDocumentPlayingAudio(&mut on)?;
                        let _ = p.send_event(Ev::Audio(id, on.as_bool()));
                    }
                    Ok(())
                })),
                &mut token,
            );
        }
    }
}

/// Shortcuts must work while a page has focus. Pages swallow keys,
/// so catch them at the controller, before the page sees them.
fn hook_keys(wv: &WebView, proxy: EventLoopProxy<Ev>) {
    let controller = wv.controller();
    let mut token = 0i64;
    unsafe {
        let _ = controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                let mut vk = 0u32;
                args.VirtualKey(&mut vk)?;
                // A lone Alt or F10 drops a Win32 window into menu mode, which eats the
                // next keys. Lumen has no menu bar. Swallow them.
                const ALT: u32 = 0x12;
                const F10: u32 = 0x79;
                if vk == ALT || vk == F10 {
                    args.SetHandled(true)?;
                    return Ok(());
                }
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }
                let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
                args.PhysicalKeyStatus(&mut status)?;
                // Async state: the key went to the WebView2 process, so this thread never saw it.
                let down = |k: u16| GetAsyncKeyState(k as i32) < 0;
                let (ctrl, shift, alt) = (down(VK_CONTROL.0), down(VK_SHIFT.0), down(VK_MENU.0));
                if let Some(a) = map_key(vk, ctrl, shift, alt) {
                    args.SetHandled(true)?;
                    // Holding a key repeats. Only tab cycling should repeat.
                    let repeat = status.WasKeyDown.as_bool();
                    if !repeat || matches!(a, Action::NextTab | Action::PrevTab) {
                        let _ = proxy.send_event(Ev::Key(a));
                    }
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

pub fn map_key(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<Action> {
    use Action::*;
    const TAB: u32 = 0x09;
    const LEFT: u32 = 0x25;
    const RIGHT: u32 = 0x27;
    const F5: u32 = 0x74;
    const F6: u32 = 0x75;
    const F11: u32 = 0x7A;
    const F12: u32 = 0x7B;
    const ESC: u32 = 0x1B;
    const COMMA: u32 = 0xBC;
    let k = |c: char| c as u32;
    Some(match (ctrl, shift, alt, vk) {
        (true, false, false, v) if v == k('T') => NewTab,
        (true, true, false, v) if v == k('T') => Reopen,
        (true, false, false, v) if v == k('W') => CloseTab,
        (true, false, false, v) if v == k('L') || v == k('K') => Omnibox,
        (false, false, false, F6) => Omnibox,
        (true, false, false, TAB) => NextTab,
        (true, true, false, TAB) => PrevTab,
        (true, false, false, v) if (k('1')..=k('8')).contains(&v) => Nth((v - k('1')) as usize),
        (true, false, false, v) if v == k('9') => LastTab,
        (true, false, false, v) if v == k('R') => Reload,
        (false, false, false, F5) => Reload,
        (false, false, true, LEFT) => Back,
        (false, false, true, RIGHT) => Forward,
        (true, false, false, v) if v == k('B') => Sidebar,
        (false, false, false, F11) => Fullscreen,
        (true, false, false, COMMA) => Settings,
        (false, false, false, F12) => DevTools,
        (false, true, false, ESC) => Control,
        (true, true, false, v) if v == k('I') => DevTools,
        _ => return None,
    })
}

// ------------------------------------------------------------ memory knobs

/// Freeze a hidden page. Scripts stop, memory is trimmed hard.
/// WebView2 resumes it by itself when shown again.
pub fn try_suspend(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>) {
    if let Ok(c3) = wv.webview().cast::<ICoreWebView2_3>() {
        unsafe {
            let _ = c3.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(move |hr, ok| {
                let _ = proxy.send_event(Ev::Suspended(id, hr.is_ok() && ok));
                Ok(())
            })));
        }
    }
}

pub fn set_muted(wv: &WebView, muted: bool) {
    if let Ok(c8) = wv.webview().cast::<ICoreWebView2_8>() {
        unsafe {
            let _ = c8.SetIsMuted(muted);
        }
    }
}

pub fn set_tracking(wv: &WebView, level: &str) {
    let lvl = match level {
        "strict" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_STRICT,
        "basic" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BASIC,
        "off" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_NONE,
        _ => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BALANCED,
    };
    unsafe {
        if let Ok(p3) = wv
            .webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|c| c.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile3>())
        {
            let _ = p3.SetPreferredTrackingPreventionLevel(lvl);
        }
    }
}

/// Glass on: DWM backdrop through black client pixels, frame stretched over the client.
/// Glass off: a plain opaque window. Cheapest thing Windows can draw.
pub fn set_glass(hwnd: isize, on: bool) {
    use windows::Win32::{
        Graphics::{
            Dwm::DwmExtendFrameIntoClientArea,
            Gdi::{RDW_ERASE, RDW_ERASENOW, RDW_INVALIDATE, RedrawWindow},
        },
        UI::Controls::MARGINS,
    };
    let hwnd = HWND(hwnd as _);
    let m = if on { -1 } else { 0 };
    let margins = MARGINS { cxLeftWidth: m, cxRightWidth: m, cyTopHeight: m, cyBottomHeight: m };
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        // Erase now: tao repaints the background color only on erase, never on its own.
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_ERASENOW);
    }
}

/// Clip the page to rounded corners. `r` in physical pixels.
pub fn round_corners(wv: &WebView, r: i32) {
    unsafe {
        let mut hwnd = HWND::default();
        if wv.controller().ParentWindow(&mut hwnd).is_err() || hwnd.is_invalid() {
            return;
        }
        if r <= 0 {
            SetWindowRgn(hwnd, None, true);
            return;
        }
        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_err() {
            return;
        }
        let rgn = CreateRoundRectRgn(0, 0, rc.right + 1, rc.bottom + 1, r * 2, r * 2);
        // The window owns the region after this call. Do not free it.
        SetWindowRgn(hwnd, Some(rgn), true);
    }
}

/// Private working set of this app plus every WebView2 process it owns.
/// Same number Task Manager shows as "Memory".
pub fn memory(env: &ICoreWebView2Environment) -> (u64, u32) {
    use crate::monitor::private_ws;
    let mut total = private_ws(unsafe { GetCurrentProcess() });
    let mut count = 1u32;
    unsafe {
        let Ok(list) = env.cast::<ICoreWebView2Environment8>().and_then(|e| e.GetProcessInfos()) else {
            return (total, count);
        };
        let mut n = 0u32;
        let _ = list.Count(&mut n);
        for i in 0..n {
            let Ok(info) = list.GetValueAtIndex(i) else { continue };
            let mut pid = 0i32;
            if info.ProcessId(&mut pid).is_err() {
                continue;
            }
            if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32) {
                total += private_ws(h);
                count += 1;
                let _ = CloseHandle(h);
            }
        }
    }
    (total, count)
}
