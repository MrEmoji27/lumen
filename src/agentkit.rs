//! What an agent's tab has that a person's tab does not: a record of console messages,
//! errors, dialogs and network requests, and a line to the page's DevTools Protocol, which
//! is how an agent clicks and types like a person (trusted input, not synthetic events).

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
};

use serde_json::{Value, json};
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler, Microsoft::Web::WebView2::Win32::*,
    ScriptDialogOpeningEventHandler,
};
use windows::{
    Win32::System::Com::CoTaskMemFree,
    core::{HSTRING, PWSTR},
};
use wry::{WebView, WebViewExtWindows};

/// Kept per tab. Old lines fall off the front.
const KEEP: usize = 300;

#[derive(Default)]
pub struct Log {
    pub console: VecDeque<String>,
    pub net: VecDeque<Net>,
    /// How the next dialogs are answered: accept, and the text for a prompt.
    pub accept: bool,
    pub prompt_text: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Net {
    pub id: String,
    pub method: String,
    pub url: String,
    pub kind: String,
    pub status: u32,
    pub failed: String,
}

pub type Logs = Rc<RefCell<HashMap<u64, Log>>>;

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn push<T>(q: &mut VecDeque<T>, v: T) {
    if q.len() >= KEEP {
        q.pop_front();
    }
    q.push_back(v);
}

/// One DevTools Protocol call. `done` gets the result object, or the error.
pub fn cdp(wv: &WebView, method: &str, params: &Value, done: impl FnOnce(Result<Value, String>) + 'static) {
    let done = Rc::new(RefCell::new(Some(done)));
    let d2 = done.clone();
    let r = unsafe {
        wv.webview().CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params.to_string()),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |hr, out| {
                let v = serde_json::from_str::<Value>(&out).unwrap_or(Value::String(out));
                let res = if hr.is_ok() { Ok(v) } else { Err(v["message"].as_str().map(str::to_string).unwrap_or_else(|| v.to_string())) };
                if let Some(f) = d2.borrow_mut().take() {
                    f(res);
                }
                Ok(())
            })),
        )
    };
    if let Err(e) = r
        && let Some(f) = done.borrow_mut().take()
    {
        f(Err(e.to_string()));
    }
}

fn fire(wv: &WebView, method: &str) {
    cdp(wv, method, &json!({}), |_| {});
}

fn on_event(wv: &WebView, name: &str, f: impl Fn(Value) + 'static) {
    unsafe {
        let Ok(recv) = wv.webview().GetDevToolsProtocolEventReceiver(&HSTRING::from(name)) else { return };
        let mut token = 0i64;
        let _ = recv.add_DevToolsProtocolEventReceived(
            &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    let mut p = PWSTR::null();
                    if args.ParameterObjectAsJson(&mut p).is_ok()
                        && let Ok(v) = serde_json::from_str::<Value>(&take(p))
                    {
                        f(v);
                    }
                }
                Ok(())
            })),
            &mut token,
        );
    }
}

/// Text of one console argument, as DevTools would show it.
fn arg_text(a: &Value) -> String {
    match &a["value"] {
        Value::String(s) => s.clone(),
        Value::Null => a["description"].as_str().or(a["unserializableValue"].as_str()).unwrap_or(a["type"].as_str().unwrap_or("")).to_string(),
        v => v.to_string(),
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// Start recording for an agent's tab, and take over its dialogs (accept by default),
/// since a modal alert would freeze every script the agent runs.
pub fn watch(wv: &WebView, tab: u64, logs: &Logs) {
    logs.borrow_mut().entry(tab).or_insert_with(|| Log { accept: true, ..Default::default() });
    for m in ["Runtime.enable", "Network.enable", "Log.enable"] {
        fire(wv, m);
    }
    let l = logs.clone();
    on_event(wv, "Runtime.consoleAPICalled", move |v| {
        let args: Vec<String> = v["args"].as_array().map(|a| a.iter().map(arg_text).collect()).unwrap_or_default();
        let line = format!("[{}] {}", v["type"].as_str().unwrap_or("log"), clip(&args.join(" "), 600));
        if let Some(log) = l.borrow_mut().get_mut(&tab) {
            push(&mut log.console, line);
        }
    });
    let l = logs.clone();
    on_event(wv, "Runtime.exceptionThrown", move |v| {
        let d = &v["exceptionDetails"];
        let what = d["exception"]["description"].as_str().or(d["text"].as_str()).unwrap_or("exception");
        let at = d["url"].as_str().map(|u| format!(" ({u}:{})", d["lineNumber"].as_u64().unwrap_or(0) + 1)).unwrap_or_default();
        if let Some(log) = l.borrow_mut().get_mut(&tab) {
            push(&mut log.console, format!("[uncaught] {}{at}", clip(what, 600)));
        }
    });
    let l = logs.clone();
    on_event(wv, "Log.entryAdded", move |v| {
        let e = &v["entry"];
        let line = format!("[{} {}] {}", e["source"].as_str().unwrap_or(""), e["level"].as_str().unwrap_or(""), clip(e["text"].as_str().unwrap_or(""), 600));
        if let Some(log) = l.borrow_mut().get_mut(&tab) {
            push(&mut log.console, line);
        }
    });
    let l = logs.clone();
    on_event(wv, "Network.requestWillBeSent", move |v| {
        let n = Net {
            id: v["requestId"].as_str().unwrap_or("").into(),
            method: v["request"]["method"].as_str().unwrap_or("").into(),
            url: v["request"]["url"].as_str().unwrap_or("").into(),
            kind: v["type"].as_str().unwrap_or("").into(),
            status: 0,
            failed: String::new(),
        };
        if n.url.starts_with("data:") {
            return;
        }
        if let Some(log) = l.borrow_mut().get_mut(&tab) {
            push(&mut log.net, n);
        }
    });
    let l = logs.clone();
    on_event(wv, "Network.responseReceived", move |v| {
        let id = v["requestId"].as_str().unwrap_or("");
        if let Some(log) = l.borrow_mut().get_mut(&tab)
            && let Some(n) = log.net.iter_mut().rev().find(|n| n.id == id)
        {
            n.status = v["response"]["status"].as_u64().unwrap_or(0) as u32;
        }
    });
    let l = logs.clone();
    on_event(wv, "Network.loadingFailed", move |v| {
        let id = v["requestId"].as_str().unwrap_or("");
        if let Some(log) = l.borrow_mut().get_mut(&tab)
            && let Some(n) = log.net.iter_mut().rev().find(|n| n.id == id)
        {
            n.failed = v["blockedReason"].as_str().or(v["errorText"].as_str()).unwrap_or("failed").into();
        }
    });
    // Dialogs: Lumen answers them, the agent reads what they said.
    unsafe {
        if let Ok(s) = wv.webview().Settings() {
            let _ = s.SetAreDefaultScriptDialogsEnabled(false);
        }
        let l = logs.clone();
        let mut token = 0i64;
        let _ = wv.webview().add_ScriptDialogOpening(
            &ScriptDialogOpeningEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_SCRIPT_DIALOG_KIND::default();
                args.Kind(&mut kind)?;
                let mut m = PWSTR::null();
                args.Message(&mut m)?;
                let msg = take(m);
                let name = match kind {
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT => "alert",
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM => "confirm",
                    COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT => "prompt",
                    _ => "beforeunload",
                };
                let mut logs = l.borrow_mut();
                let log = logs.entry(tab).or_default();
                if log.accept {
                    if let (COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT, Some(t)) = (kind, &log.prompt_text) {
                        args.SetResultText(&HSTRING::from(t.as_str()))?;
                    }
                    args.Accept()?;
                }
                let answer = if log.accept { "accepted" } else { "dismissed" };
                push(&mut log.console, format!("[dialog {name}] {} -> {answer}", clip(&msg, 400)));
                Ok(())
            })),
            &mut token,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_args_read_like_devtools() {
        assert_eq!(arg_text(&json!({"type": "string", "value": "hi"})), "hi");
        assert_eq!(arg_text(&json!({"type": "number", "value": 3})), "3");
        assert_eq!(arg_text(&json!({"type": "object", "description": "Object"})), "Object");
        assert_eq!(arg_text(&json!({"type": "number", "unserializableValue": "NaN"})), "NaN");
        assert_eq!(clip("abcdef", 3), "abc…");
    }
}
