//! Servo inside a Lumen window. The spike: one tab, Servo engine, tao window.
//!
//! cargo run --example servo_tab --features servo -- https://example.com
//!
//! Prints the title and load status as they change, so a script can watch it.

use std::{cell::RefCell, rc::Rc};

use euclid::Scale;
use servo::{
    DevicePoint, EventLoopWaker, InputEvent, LoadStatus, MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent,
    RenderingContext, Servo, ServoBuilder, WebView, WebViewBuilder, WheelDelta, WheelEvent, WheelMode, WindowRenderingContext,
};
use tao::{
    event::{ElementState, Event, MouseButton as TaoButton, MouseScrollDelta, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::WindowBuilder,
};
use url::Url;

// The prebuilt SpiderMonkey was compiled with a newer MSVC STL (VS 17.12+) than this
// machine's toolset (14.41), and calls two of its vectorized helpers. Same contract, in Rust,
// until the toolset is updated. Semantics from MSVC STL vector_algorithms.cpp.

/// First occurrence of [needle, needle+n) in [first, last); `last` if none; `first` if n == 0.
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

/// Index of the first haystack byte that appears in needle; usize::MAX if none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __std_find_first_of_trivial_pos_1(hay: *const u8, hay_len: usize, needle: *const u8, needle_len: usize) -> usize {
    if hay_len == 0 || needle_len == 0 {
        return usize::MAX;
    }
    let hay = unsafe { std::slice::from_raw_parts(hay, hay_len) };
    let set = unsafe { std::slice::from_raw_parts(needle, needle_len) };
    hay.iter().position(|b| set.contains(b)).unwrap_or(usize::MAX)
}

/// Servo wakes the event loop through this when it has work.
#[derive(Clone)]
struct Waker(EventLoopProxy<()>);

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
    fn wake(&self) {
        let _ = self.0.send_event(());
    }
}

struct State {
    window: tao::window::Window,
    rendering: Rc<WindowRenderingContext>,
    webview: RefCell<Option<WebView>>,
}

impl servo::WebViewDelegate for State {
    fn notify_new_frame_ready(&self, _: WebView) {
        self.window.request_redraw();
    }
    fn notify_page_title_changed(&self, _: WebView, title: Option<String>) {
        println!("TITLE {}", title.unwrap_or_default());
    }
    fn notify_url_changed(&self, _: WebView, url: Url) {
        println!("URL {url}");
    }
    fn notify_load_status_changed(&self, _: WebView, status: LoadStatus) {
        println!("LOAD {status:?}");
    }
}

fn main() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let target = std::env::args().nth(1).unwrap_or_else(|| "https://example.com".into());
    let url = Url::parse(&target).expect("a full URL, like https://example.com");

    let event_loop = EventLoopBuilder::<()>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Lumen · Servo")
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 800.0))
        .build(&event_loop)
        .expect("window");

    use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
    let size = window.inner_size();
    let rendering = Rc::new(
        WindowRenderingContext::new(
            window.display_handle().expect("display handle"),
            window.window_handle().expect("window handle"),
            dpi::PhysicalSize::new(size.width, size.height),
        )
        .expect("rendering context"),
    );
    let _ = rendering.make_current();

    let servo: Servo = ServoBuilder::default().event_loop_waker(Box::new(Waker(event_loop.create_proxy()))).build();
    let scale = window.scale_factor() as f32;
    let state = Rc::new(State { window, rendering: rendering.clone(), webview: RefCell::new(None) });
    let webview = WebViewBuilder::new(&servo, rendering.clone())
        .url(url)
        .hidpi_scale_factor(Scale::new(scale))
        .delegate(state.clone())
        .build();
    state.webview.replace(Some(webview));

    let mut cursor = DevicePoint::new(0.0, 0.0);
    event_loop.run(move |event, _, cf| {
        *cf = ControlFlow::Wait;
        servo.spin_event_loop();
        let wv = state.webview.borrow().clone();
        let Some(wv) = wv else { return };
        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => *cf = ControlFlow::Exit,
                WindowEvent::Resized(s) => {
                    wv.resize(dpi::PhysicalSize::new(s.width, s.height));
                }
                WindowEvent::CursorMoved { position, .. } => {
                    cursor = DevicePoint::new(position.x as f32, position.y as f32);
                    wv.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(cursor.into())));
                }
                WindowEvent::MouseInput { state: st, button, .. } => {
                    let button = match button {
                        TaoButton::Left => MouseButton::Primary,
                        TaoButton::Right => MouseButton::Secondary,
                        TaoButton::Middle => MouseButton::Auxiliary,
                        _ => return,
                    };
                    let action = if st == ElementState::Pressed { MouseButtonAction::Down } else { MouseButtonAction::Up };
                    wv.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(action, button, cursor.into())));
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let (x, y, mode) = match delta {
                        MouseScrollDelta::LineDelta(dx, dy) => ((dx * 76.0) as f64, (dy * 76.0) as f64, WheelMode::DeltaLine),
                        MouseScrollDelta::PixelDelta(p) => (p.x, p.y, WheelMode::DeltaPixel),
                        _ => return,
                    };
                    wv.notify_input_event(InputEvent::Wheel(WheelEvent::new(WheelDelta { x, y, z: 0.0, mode }, cursor.into())));
                }
                _ => {}
            },
            Event::RedrawRequested(_) => {
                wv.paint();
                state.rendering.present();
            }
            _ => {}
        }
    });
}
