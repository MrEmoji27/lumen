//! Shift+click to peek at a link without leaving the current tab.

use tao::event_loop::EventLoopProxy;
use webview2_com::{
    AcceleratorKeyPressedEventHandler, ExecuteScriptCompletedHandler, Microsoft::Web::WebView2::Win32::*,
    WebMessageReceivedEventHandler,
};
use windows::{
    Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SHIFT},
    core::{HSTRING, PWSTR},
};
use wry::{WebView, WebViewExtWindows};

use crate::engine::Ev;

pub const GLANCE_JS: &str = r#"(()=>{if(window!==window.top)return;
const W=window.chrome&&window.chrome.webview;if(!W)return;
document.addEventListener('click',e=>{if(!e.shiftKey)return;
const a=e.target.closest('a');if(a){e.preventDefault();e.stopImmediatePropagation();
W.postMessage({lumen:'glance',url:a.href})}},true);
})();"#;

pub fn hook(wv: &WebView, id: u64, proxy: EventLoopProxy<Ev>) {
    let core = wv.webview();
    let mut token = 0i64;
    unsafe {
        let p = proxy;
        let _ = core.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| {
                let Some(args) = args else { return Ok(()) };
                let mut msg = PWSTR::null();
                if args.WebMessageAsJson(&mut msg).is_err() {
                    return Ok(());
                }
                let json = take(msg);
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else { return Ok(()) };
                if v["lumen"] != "glance" {
                    return Ok(());
                }
                if let Some(url) = v["url"].as_str() {
                    let _ = p.send_event(Ev::Glance(id, url.to_string()));
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

const CHECK_FOCUS_JS: &str = "(()=>{const a=document.activeElement;return !!(a&&(['INPUT','TEXTAREA','SELECT'].includes(a.tagName)||a.isContentEditable))})()";

pub fn hook_keys(wv: &WebView, proxy: EventLoopProxy<Ev>) {
    let controller = wv.controller();
    let mut token = 0i64;
    let p0 = proxy;
    unsafe {
        let _ = controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |sender, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                args.KeyEventKind(&mut kind)?;
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }
                let mut vk = 0u32;
                args.VirtualKey(&mut vk)?;
                const ESC: u32 = 0x1B;
                const RETURN: u32 = 0x0D;
                let down = |k: u16| GetAsyncKeyState(k as i32) < 0;
                if vk == ESC && !down(VK_SHIFT.0) {
                    args.SetHandled(true)?;
                    let _ = p0.send_event(Ev::GlanceKey("esc"));
                } else if vk == RETURN
                    && let Some(controller) = sender
                    && let Ok(core) = controller.CoreWebView2()
                {
                    let p = p0.clone();
                    let _ = core.ExecuteScript(
                        &HSTRING::from(CHECK_FOCUS_JS),
                        &ExecuteScriptCompletedHandler::create(Box::new(move |_, json| {
                            if json != "true" {
                                let _ = p.send_event(Ev::GlanceKey("enter"));
                            }
                            Ok(())
                        })),
                    );
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _)) };
    s
}
