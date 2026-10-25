//! Servo, for the light sites.

use std::{cell::RefCell, num::NonZeroIsize, rc::Rc};

use dpi::PhysicalSize;
use raw_window_handle::{
    DisplayHandle, RawDisplayHandle, RawWindowHandle, WindowsDisplayHandle, Win32WindowHandle, WindowHandle,
};
use servo::{
    Cursor, DevicePoint, InputEvent, Key, KeyState, LoadStatus, MouseButton, MouseButtonAction, MouseButtonEvent,
    MouseMoveEvent, NamedKey, RenderingContext, Servo, ServoBuilder, WebView, WebViewBuilder, WebViewDelegate, WheelDelta,
    WheelEvent, WheelMode, WindowRenderingContext,
};
use tao::event_loop::EventLoopProxy;
use url::Url;
use windows::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, ScreenToClient},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent},
        WindowsAndMessaging::*,
    },
};
use windows::core::{PCWSTR, w};

use crate::engine::Ev;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_search_1(first: *const u8, last: *const u8, needle: *const u8, n: usize) -> *const u8 {
    if n == 0 {
        return first;
    }
    let len = unsafe { last.offset_from(first) } as usize;
    if n > len {
        return last;
    }
    let hay = unsafe { std::slice::from_raw_parts(first, len) };
    let pat = unsafe { std::slice::from_raw_parts(needle, n) };
    match hay.windows(n).position(|w| w == pat) {
        Some(i) => unsafe { first.add(i) },
        None => last,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_find_first_of_trivial_pos_1(hay: *const u8, hay_len: usize, needle: *const u8, needle_len: usize) -> usize {
    if hay_len == 0 || needle_len == 0 {
        return usize::MAX;
    }
    let hay = unsafe { std::slice::from_raw_parts(hay, hay_len) };
    let set = unsafe { std::slice::from_raw_parts(needle, needle_len) };
    hay.iter().position(|b| set.contains(b)).unwrap_or(usize::MAX)
}

#[derive(Clone)]
struct Waker(EventLoopProxy<Ev>);

impl servo::EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn servo::EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {
        let _ = self.0.send_event(Ev::ServoWake);
    }
}

pub struct Engine {
    servo: Servo,
}

impl Engine {
    pub fn new(proxy: EventLoopProxy<Ev>) -> Engine {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let servo = ServoBuilder::default().event_loop_waker(Box::new(Waker(proxy))).build();
        servo.setup_logging();
        Engine { servo }
    }

    pub fn spin(&self) {
        self.servo.spin_event_loop();
    }

    pub fn page(&self, parent: HWND, id: u64, url: &str, proxy: EventLoopProxy<Ev>) -> Option<Page> {
        Page::new(&self.servo, parent, id, url, proxy)
    }
}

#[derive(Default)]
struct State {
    webview: Option<WebView>,
    rendering: Option<Rc<WindowRenderingContext>>,
    at: DevicePoint,
    holding: bool,
    cursor: Cursor,
}

pub struct Page {
    hwnd: HWND,
    state: Rc<RefCell<State>>,
}

impl Page {
    fn new(servo: &Servo, parent: HWND, id: u64, url: &str, proxy: EventLoopProxy<Ev>) -> Option<Page> {
        let parsed = Url::parse(url).ok()?;
        let state: Rc<RefCell<State>> = Rc::new(RefCell::new(State::default()));
        let hwnd = create_window(parent, Rc::as_ptr(&state))?;
        let (w, h) = client_size(hwnd);
        let rendering = match WindowRenderingContext::new(display_handle(), window_handle(hwnd), physical(w, h)) {
            Ok(r) => Rc::new(r),
            Err(e) => {
                eprintln!("lumen: servo: no rendering context: {e:?}");
                destroy(hwnd);
                return None;
            }
        };
        let _ = rendering.make_current();
        let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
        let webview = WebViewBuilder::new(servo, rendering.clone())
            .url(parsed)
            .hidpi_scale_factor(euclid::Scale::new(scale))
            .delegate(Rc::new(Delegate { id, hwnd, proxy }))
            .build();
        webview.show();
        webview.set_focused(true);
        {
            let mut s = state.borrow_mut();
            s.rendering = Some(rendering);
            s.webview = Some(webview);
        }
        Some(Page { hwnd, state })
    }

    pub fn set_bounds(&self, x: f64, y: f64, w: f64, h: f64) {
        unsafe {
            let k = GetDpiForWindow(self.hwnd) as f64 / 96.0;
            let _ = SetWindowPos(
                self.hwnd,
                None,
                (x * k).round() as i32,
                (y * k).round() as i32,
                (w * k).round().max(1.0) as i32,
                (h * k).round().max(1.0) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNA);
        }
        if let Some(v) = &self.state.borrow().webview {
            v.show();
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        if let Some(v) = &self.state.borrow().webview {
            v.hide();
        }
    }

    pub fn load(&self, url: &str) {
        if let (Ok(u), Some(v)) = (Url::parse(url), self.state.borrow().webview.as_ref()) {
            v.load(u);
        }
    }

    pub fn reload(&self) {
        if let Some(v) = &self.state.borrow().webview {
            v.reload();
        }
    }

    pub fn back(&self) {
        if let Some(v) = &self.state.borrow().webview {
            if v.can_go_back() {
                v.go_back(1);
            }
        }
    }

    pub fn forward(&self) {
        if let Some(v) = &self.state.borrow().webview {
            if v.can_go_forward() {
                v.go_forward(1);
            }
        }
    }

    pub fn focus(&self) {
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
        }
        if let Some(v) = &self.state.borrow().webview {
            v.set_focused(true);
        }
    }

    #[cfg(debug_assertions)]
    pub fn proof_of_paint(&self) {
        let view = self.state.borrow().webview.clone();
        let Some(view) = view else { return };
        view.take_screenshot(None, |shot| match shot {
            Ok(img) => {
                let (w, h) = img.dimensions();
                let ink: u64 = img.pixels().take(4096).map(|p| p.0[0] as u64 + p.0[1] as u64 + p.0[2] as u64).sum();
                eprintln!("lumen: servo painted {w}x{h}, ink {ink}");
            }
            Err(e) => eprintln!("lumen: servo screenshot: {e:?}"),
        });
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        {
            let mut s = self.state.borrow_mut();
            s.webview = None;
            s.rendering = None;
        }
        destroy(self.hwnd);
    }
}

struct Delegate {
    id: u64,
    hwnd: HWND,
    proxy: EventLoopProxy<Ev>,
}

impl WebViewDelegate for Delegate {
    fn notify_new_frame_ready(&self, _: WebView) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn notify_url_changed(&self, _: WebView, url: Url) {
        let _ = self.proxy.send_event(Ev::ServoUrl(self.id, url.to_string()));
    }

    fn notify_page_title_changed(&self, _: WebView, title: Option<String>) {
        let _ = self.proxy.send_event(Ev::ServoTitle(self.id, title.unwrap_or_default()));
    }

    fn notify_load_status_changed(&self, _: WebView, status: LoadStatus) {
        let _ = self.proxy.send_event(Ev::ServoLoad(self.id, matches!(status, LoadStatus::Started)));
    }

    fn notify_cursor_changed(&self, _: WebView, cursor: Cursor) {
        if let Some(cell) = state_of(self.hwnd) {
            if let Ok(mut s) = cell.try_borrow_mut() {
                s.cursor = cursor;
            }
        }
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_SETCURSOR, WPARAM(0), LPARAM(0));
        }
    }

    fn notify_closed(&self, _: WebView) {
        let _ = self.proxy.send_event(Ev::CloseMe(self.id));
    }

    fn notify_crashed(&self, _: WebView, reason: String, _: Option<String>) {
        eprintln!("lumen: servo: page {} crashed: {reason}", self.id);
        let _ = self.proxy.send_event(Ev::ServoGone(self.id));
    }
}

const CLASS: PCWSTR = w!("LumenServo");

fn create_window(parent: HWND, state: *const RefCell<State>) -> Option<HWND> {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).map(|h| h.into()).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
            0,
            0,
            640,
            480,
            Some(parent),
            None,
            Some(hinst),
            None,
        )
        .ok()?;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        Some(hwnd)
    }
}

fn destroy(hwnd: HWND) {
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let _ = DestroyWindow(hwnd);
    }
}

fn state_of(hwnd: HWND) -> Option<&'static RefCell<State>> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<State>;
    if p.is_null() { None } else { Some(unsafe { &*p }) }
}

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rc = windows::Win32::Foundation::RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    (rc.right.max(1), rc.bottom.max(1))
}

fn physical(w: i32, h: i32) -> PhysicalSize<u32> {
    PhysicalSize::new(w.max(1) as u32, h.max(1) as u32)
}

fn display_handle() -> DisplayHandle<'static> {
    unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Windows(WindowsDisplayHandle::new())) }
}

fn window_handle(hwnd: HWND) -> WindowHandle<'static> {
    let mut h = Win32WindowHandle::new(NonZeroIsize::new(hwnd.0 as isize).unwrap_or(NonZeroIsize::MIN));
    h.hinstance = NonZeroIsize::new(unsafe { GetModuleHandleW(None).map(|m| m.0 as isize).unwrap_or(0) });
    unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(h)) }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(cell) = state_of(hwnd) else { return unsafe { DefWindowProcW(hwnd, msg, wp, lp) } };
    let Ok(mut s) = cell.try_borrow_mut() else { return unsafe { DefWindowProcW(hwnd, msg, wp, lp) } };
    let (px, py) = ((lp.0 & 0xffff) as i16 as i32, ((lp.0 >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_PAINT => {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let _ = BeginPaint(hwnd, &mut ps);
                if let (Some(rendering), Some(view)) = (&s.rendering, &s.webview) {
                    let _ = rendering.make_current();
                    view.paint();
                    rendering.present();
                }
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            let (w, h) = ((lp.0 & 0xffff) as u32, ((lp.0 >> 16) & 0xffff) as u32);
            if w > 0 && h > 0 {
                if let Some(view) = &s.webview {
                    view.resize(physical(w as i32, h as i32));
                }
                if let Some(rendering) = &s.rendering {
                    rendering.resize(physical(w as i32, h as i32));
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            s.at = DevicePoint::new(px as f32, py as f32);
            let at = s.at;
            if let Some(view) = &s.webview {
                view.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(at.into())));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            let button = match msg {
                WM_LBUTTONDOWN => MouseButton::Primary,
                WM_RBUTTONDOWN => MouseButton::Secondary,
                _ => MouseButton::Auxiliary,
            };
            s.at = DevicePoint::new(px as f32, py as f32);
            s.holding = true;
            unsafe {
                SetCapture(hwnd);
                let _ = SetFocus(Some(hwnd));
            }
            let at = s.at;
            if let Some(view) = &s.webview {
                view.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(MouseButtonAction::Down, button, at.into())));
            }
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let button = match msg {
                WM_LBUTTONUP => MouseButton::Primary,
                WM_RBUTTONUP => MouseButton::Secondary,
                _ => MouseButton::Auxiliary,
            };
            s.at = DevicePoint::new(px as f32, py as f32);
            s.holding = false;
            unsafe {
                let _ = ReleaseCapture();
            }
            let at = s.at;
            if let Some(view) = &s.webview {
                view.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(MouseButtonAction::Up, button, at.into())));
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let mut pt = POINT { x: px, y: py };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let at = DevicePoint::new(pt.x as f32, pt.y as f32);
            let steps = ((wp.0 >> 16) & 0xffff) as i16 as f64 / 120.0;
            let view = s.webview.clone();
            if let Some(view) = view {
                view.notify_input_event(InputEvent::Wheel(WheelEvent::new(
                    WheelDelta { x: 0.0, y: steps * 76.0, z: 0.0, mode: WheelMode::DeltaLine },
                    at.into(),
                )));
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let vk = wp.0 as u16;
            if let Some(key) = named_key(vk) {
                let state = if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN { KeyState::Down } else { KeyState::Up };
                let view = s.webview.clone();
                if let Some(view) = view {
                    view.notify_input_event(InputEvent::Keyboard(servo::KeyboardEvent::from_state_and_key(state, key)));
                }
            }
            LRESULT(0)
        }
        WM_CHAR => {
            if let Some(c) = char::from_u32(wp.0 as u32)
                && !c.is_control()
            {
                let view = s.webview.clone();
                if let Some(view) = view {
                    for state in [KeyState::Down, KeyState::Up] {
                        view.notify_input_event(InputEvent::Keyboard(servo::KeyboardEvent::from_state_and_key(
                            state,
                            Key::Character(c.to_string()),
                        )));
                    }
                }
            }
            LRESULT(0)
        }
        WM_SETFOCUS => {
            let view = s.webview.clone();
            if let Some(view) = view {
                view.set_focused(true);
            }
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            let view = s.webview.clone();
            if let Some(view) = view {
                view.set_focused(false);
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            unsafe {
                let _ = SetCursor(cursor_of(s.cursor));
            }
            LRESULT(1)
        }
        WM_CAPTURECHANGED => {
            s.holding = false;
            LRESULT(0)
        }
        WM_NCHITTEST => LRESULT(HTCLIENT as isize),
        _ => {
            drop(s);
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
    }
}

fn named_key(vk: u16) -> Option<Key> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let named = match VIRTUAL_KEY(vk) {
        VK_RETURN => NamedKey::Enter,
        VK_BACK => NamedKey::Backspace,
        VK_TAB => NamedKey::Tab,
        VK_ESCAPE => NamedKey::Escape,
        VK_DELETE => NamedKey::Delete,
        VK_LEFT => NamedKey::ArrowLeft,
        VK_RIGHT => NamedKey::ArrowRight,
        VK_UP => NamedKey::ArrowUp,
        VK_DOWN => NamedKey::ArrowDown,
        VK_HOME => NamedKey::Home,
        VK_END => NamedKey::End,
        VK_PRIOR => NamedKey::PageUp,
        VK_NEXT => NamedKey::PageDown,
        VK_SHIFT => NamedKey::Shift,
        VK_CONTROL => NamedKey::Control,
        VK_MENU => NamedKey::Alt,
        _ => return None,
    };
    Some(Key::Named(named))
}

fn cursor_of(cursor: Cursor) -> Option<HCURSOR> {
    if cursor == Cursor::None {
        return None;
    }
    let id = match cursor {
        Cursor::Text => IDC_IBEAM,
        Cursor::Pointer => IDC_HAND,
        Cursor::Wait | Cursor::Progress => IDC_WAIT,
        Cursor::Crosshair => IDC_CROSS,
        Cursor::Move => IDC_SIZEALL,
        Cursor::EResize | Cursor::WResize | Cursor::EwResize | Cursor::ColResize => IDC_SIZEWE,
        Cursor::NResize | Cursor::SResize | Cursor::NsResize | Cursor::RowResize => IDC_SIZENS,
        Cursor::NoDrop | Cursor::NotAllowed => IDC_NO,
        Cursor::Help => IDC_HELP,
        _ => IDC_ARROW,
    };
    unsafe { LoadCursorW(None, id).ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_cover_the_page_editing_ones() {
        assert!(matches!(named_key(0x0D), Some(Key::Named(NamedKey::Enter))));
        assert!(matches!(named_key(0x08), Some(Key::Named(NamedKey::Backspace))));
        assert!(matches!(named_key(0x25), Some(Key::Named(NamedKey::ArrowLeft))));
        assert!(named_key(0x41).is_none());
    }

    #[test]
    fn cursors_come_out_of_the_table() {
        assert!(cursor_of(Cursor::Text).is_some());
        assert!(cursor_of(Cursor::Pointer).is_some());
        assert!(cursor_of(Cursor::None).is_none());
    }
}
