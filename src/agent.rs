//! A door for AI agents: a tiny MCP server (JSON-RPC over HTTP) on 127.0.0.1.
//!
//! The server runs on its own thread. Every tool is built from a few primitive
//! operations (open, navigate, eval, screenshot...) that must run on the UI thread,
//! so they travel as `Ev::Agent(Call)` and come back through a channel.
//!
//! Guard rails: localhost only, a secret token on every request, Host and Origin
//! checked against DNS rebinding, bodies capped at 1 MB. Off unless enabled.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Sender, channel},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use tao::event_loop::EventLoopProxy;

use crate::engine::Ev;

pub enum Op {
    List,
    Open { url: String, activate: bool },
    Navigate { tab: u64, url: String },
    Back { tab: u64 },
    Eval { tab: u64, js: String },
    Shot { tab: u64, png: Sender<Result<Vec<u8>, String>> },
    Close { tab: u64 },
}

/// One primitive for the UI thread, and where to send the answer.
pub struct Call {
    pub op: Op,
    pub reply: Sender<Result<Value, String>>,
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match &self.op {
            Op::List => "list".to_string(),
            Op::Open { url, .. } => format!("open {url}"),
            Op::Navigate { tab, url } => format!("navigate {tab} {url}"),
            Op::Back { tab } => format!("back {tab}"),
            Op::Eval { tab, .. } => format!("eval {tab}"),
            Op::Shot { tab, .. } => format!("shot {tab}"),
            Op::Close { tab } => format!("close {tab}"),
        };
        write!(f, "Call({name})")
    }
}

pub struct Agent {
    pub token: String,
    on: Arc<AtomicBool>,
    port: Option<u32>,
}

impl Agent {
    /// Load the token, or make one. 32 random bytes from the OS, hex.
    pub fn new(dir: &Path) -> Agent {
        let path = dir.join("agent_token");
        let token = std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string()).filter(|s| s.len() == 64).unwrap_or_else(|| {
            let mut b = [0u8; 32];
            unsafe {
                use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
                let _ = BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
            }
            let t: String = b.iter().map(|x| format!("{x:02x}")).collect();
            let _ = std::fs::write(&path, &t);
            t
        });
        Agent { token, on: Arc::new(AtomicBool::new(false)), port: None }
    }

    /// Turn the door on or off. The listener starts once and stays; off means it refuses.
    pub fn set(&mut self, on: bool, port: u32, proxy: &EventLoopProxy<Ev>) {
        self.on.store(on, Ordering::SeqCst);
        if on && self.port.is_none() {
            self.port = Some(port);
            let shared = Arc::new(Shared { on: self.on.clone(), token: self.token.clone(), port, proxy: proxy.clone() });
            thread::Builder::new()
                .name("lumen-agent".into())
                .spawn(move || serve(shared))
                .ok();
        }
    }

    pub fn endpoint(&self, port: u32) -> String {
        format!("http://127.0.0.1:{port}/mcp")
    }

    /// Paste into a terminal and Claude Code can browse with Lumen.
    pub fn setup_command(&self, port: u32) -> String {
        format!("claude mcp add --transport http lumen {} --header \"Authorization: Bearer {}\"", self.endpoint(port), self.token)
    }
}

struct Shared {
    on: Arc<AtomicBool>,
    token: String,
    port: u32,
    proxy: EventLoopProxy<Ev>,
}

fn serve(sh: Arc<Shared>) {
    let Ok(listener) = TcpListener::bind(("127.0.0.1", sh.port as u16)) else { return };
    for stream in listener.incoming().flatten() {
        let sh = sh.clone();
        thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            handle(stream, &sh);
        });
    }
}

fn respond(mut s: TcpStream, code: &str, body: &str) {
    let ctype = if body.is_empty() { "" } else { "Content-Type: application/json\r\n" };
    let _ = write!(s, "HTTP/1.1 {code}\r\n{ctype}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}

fn handle(stream: TcpStream, sh: &Shared) {
    let mut r = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (mut len, mut auth, mut host, mut origin) = (0usize, String::new(), String::new(), String::new());
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).is_err() || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let v = v.trim().to_string();
            match k.trim().to_ascii_lowercase().as_str() {
                "content-length" => len = v.parse().unwrap_or(0),
                "authorization" => auth = v,
                "host" => host = v,
                "origin" => origin = v,
                _ => {}
            }
        }
    }
    // A web page cannot reach us by pretending to be localhost.
    let local = [format!("127.0.0.1:{}", sh.port), format!("localhost:{}", sh.port)];
    if !local.contains(&host) || !(origin.is_empty() || origin == "null" || local.iter().any(|l| origin.ends_with(l))) {
        return respond(stream, "403 Forbidden", "");
    }
    if !sh.on.load(Ordering::SeqCst) {
        return respond(stream, "503 Service Unavailable", "");
    }
    let expected = format!("Bearer {}", sh.token);
    if !same(auth.as_bytes(), expected.as_bytes()) {
        return respond(stream, "401 Unauthorized", "");
    }
    if path != "/mcp" {
        return respond(stream, "404 Not Found", "");
    }
    if method != "POST" {
        // No server-to-client stream; plain request and response is enough.
        return respond(stream, "405 Method Not Allowed", "");
    }
    if len > 1 << 20 {
        return respond(stream, "413 Payload Too Large", "");
    }
    let mut body = vec![0u8; len];
    if r.read_exact(&mut body).is_err() {
        return;
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return respond(stream, "400 Bad Request", &rpc_err(Value::Null, -32700, "parse error").to_string());
    };
    // Notifications get no answer.
    let Some(id) = msg.get("id").cloned() else { return respond(stream, "202 Accepted", "") };
    let out = match msg["method"].as_str().unwrap_or("") {
        "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "lumen", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Lumen browser. Open a tab with browser_open, read it with browser_read (it numbers clickable elements), act with browser_click / browser_type using those numbers, and check with browser_screenshot. Tabs you open are yours; the user sees them, marked as agent tabs."
        }}),
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools()}}),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = &msg["params"]["arguments"];
            let content = match run(&sh.proxy, name, args) {
                Ok(c) => json!({"content": c}),
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            };
            json!({"jsonrpc": "2.0", "id": id, "result": content})
        }
        _ => rpc_err(id, -32601, "method not found"),
    };
    respond(stream, "200 OK", &out.to_string());
}

/// Constant-time compare, so the token cannot be guessed by timing.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn rpc_err(id: Value, code: i32, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

// ------------------------------------------------------------------- tools

fn tools() -> Value {
    let tab = json!({"type": "integer", "description": "Tab id from browser_open or browser_tabs"});
    json!([
        {"name": "browser_open", "description": "Open a URL (or search words) in a new agent tab. Waits for the page to load. Returns the tab id and a page summary.",
         "inputSchema": {"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}},
        {"name": "browser_navigate", "description": "Load a URL (or search words) in an existing tab and wait for it.",
         "inputSchema": {"type": "object", "properties": {"tab": tab, "url": {"type": "string"}}, "required": ["tab", "url"]}},
        {"name": "browser_read", "description": "Read a page: title, URL, numbered interactive elements (links, buttons, inputs), and visible text. Call again after the page changes; numbers are re-assigned.",
         "inputSchema": {"type": "object", "properties": {"tab": tab, "max_chars": {"type": "integer", "description": "Text budget, default 6000"}}, "required": ["tab"]}},
        {"name": "browser_click", "description": "Click element number N from the last browser_read.",
         "inputSchema": {"type": "object", "properties": {"tab": tab, "element": {"type": "integer"}}, "required": ["tab", "element"]}},
        {"name": "browser_type", "description": "Type text into input number N from the last browser_read. Replaces its value. submit=true presses Enter / submits the form.",
         "inputSchema": {"type": "object", "properties": {"tab": tab, "element": {"type": "integer"}, "text": {"type": "string"}, "submit": {"type": "boolean"}}, "required": ["tab", "element", "text"]}},
        {"name": "browser_screenshot", "description": "PNG of what the tab shows.",
         "inputSchema": {"type": "object", "properties": {"tab": tab}, "required": ["tab"]}},
        {"name": "browser_eval", "description": "Run JavaScript in the page. The script is a function body; use `return` to send a JSON-serializable value back.",
         "inputSchema": {"type": "object", "properties": {"tab": tab, "script": {"type": "string"}}, "required": ["tab", "script"]}},
        {"name": "browser_back", "description": "Go back in the tab's history.",
         "inputSchema": {"type": "object", "properties": {"tab": tab}, "required": ["tab"]}},
        {"name": "browser_tabs", "description": "List the tabs you may use.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "browser_close", "description": "Close a tab.",
         "inputSchema": {"type": "object", "properties": {"tab": tab}, "required": ["tab"]}}
    ])
}

fn call(proxy: &EventLoopProxy<Ev>, op: Op) -> Result<Value, String> {
    let (tx, rx) = channel();
    proxy.send_event(Ev::Agent(Call { op, reply: tx })).map_err(|_| "browser is closing".to_string())?;
    rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "browser did not answer in time".to_string())?
}

fn eval(proxy: &EventLoopProxy<Ev>, tab: u64, js: &str) -> Result<Value, String> {
    call(proxy, Op::Eval { tab, js: js.to_string() })
}

/// Until the document is complete, or 12 seconds pass. Then a breath for late scripts.
fn wait_load(proxy: &EventLoopProxy<Ev>, tab: u64) {
    thread::sleep(Duration::from_millis(250));
    for _ in 0..60 {
        // A fresh tab is about:blank and already "complete"; wait for the real page.
        let js = "document.readyState==='complete'&&location.href!=='about:blank'";
        if eval(proxy, tab, js).ok().and_then(|v| v.as_bool()) == Some(true) {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    thread::sleep(Duration::from_millis(300));
}

fn tab_arg(args: &Value) -> Result<u64, String> {
    args["tab"].as_u64().ok_or_else(|| "missing tab id".to_string())
}

fn text(s: String) -> Vec<Value> {
    vec![json!({"type": "text", "text": s})]
}

fn run(proxy: &EventLoopProxy<Ev>, name: &str, args: &Value) -> Result<Vec<Value>, String> {
    match name {
        "browser_open" => {
            let url = args["url"].as_str().ok_or("missing url")?.to_string();
            let tab = call(proxy, Op::Open { url, activate: true })?["tab"].as_u64().ok_or("no tab")?;
            wait_load(proxy, tab);
            Ok(text(format!("tab {tab}\n{}", read(proxy, tab, 1500)?)))
        }
        "browser_navigate" => {
            let tab = tab_arg(args)?;
            let url = args["url"].as_str().ok_or("missing url")?.to_string();
            call(proxy, Op::Navigate { tab, url })?;
            wait_load(proxy, tab);
            Ok(text(read(proxy, tab, 1500)?))
        }
        "browser_read" => {
            let tab = tab_arg(args)?;
            let max = args["max_chars"].as_u64().unwrap_or(6000).clamp(200, 50_000) as usize;
            Ok(text(read(proxy, tab, max)?))
        }
        "browser_click" => {
            let tab = tab_arg(args)?;
            let n = args["element"].as_u64().ok_or("missing element")?;
            let r = eval(proxy, tab, &CLICK_JS.replace("__N__", &n.to_string()))?;
            let msg = r.as_str().unwrap_or("").to_string();
            if msg != "ok" {
                return Err(msg);
            }
            wait_load(proxy, tab);
            Ok(text(format!("clicked [{n}]\n{}", read(proxy, tab, 1500)?)))
        }
        "browser_type" => {
            let tab = tab_arg(args)?;
            let n = args["element"].as_u64().ok_or("missing element")?;
            let t = args["text"].as_str().ok_or("missing text")?;
            let submit = args["submit"].as_bool().unwrap_or(false);
            let js = TYPE_JS
                .replace("__N__", &n.to_string())
                .replace("__TEXT__", &Value::String(t.to_string()).to_string())
                .replace("__SUBMIT__", if submit { "true" } else { "false" });
            let r = eval(proxy, tab, &js)?;
            let msg = r.as_str().unwrap_or("").to_string();
            if msg != "ok" {
                return Err(msg);
            }
            if submit {
                wait_load(proxy, tab);
                return Ok(text(format!("typed into [{n}] and submitted\n{}", read(proxy, tab, 1500)?)));
            }
            Ok(text(format!("typed into [{n}]")))
        }
        "browser_screenshot" => {
            let tab = tab_arg(args)?;
            let (tx, rx) = channel();
            call(proxy, Op::Shot { tab, png: tx })?;
            let png = rx.recv_timeout(Duration::from_secs(15)).map_err(|_| "screenshot timed out")??;
            use base64::Engine as _;
            Ok(vec![json!({"type": "image", "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png)})])
        }
        "browser_eval" => {
            let tab = tab_arg(args)?;
            let script = args["script"].as_str().ok_or("missing script")?;
            let js = format!("(()=>{{try{{const r=(()=>{{{script}\n}})();return r===undefined?null:r}}catch(e){{return 'error: '+e}}}})()");
            let v = eval(proxy, tab, &js)?;
            Ok(text(serde_json::to_string_pretty(&v).unwrap_or_default()))
        }
        "browser_back" => {
            let tab = tab_arg(args)?;
            call(proxy, Op::Back { tab })?;
            wait_load(proxy, tab);
            Ok(text(read(proxy, tab, 1500)?))
        }
        "browser_tabs" => {
            let v = call(proxy, Op::List)?;
            Ok(text(serde_json::to_string_pretty(&v).unwrap_or_default()))
        }
        "browser_close" => {
            call(proxy, Op::Close { tab: tab_arg(args)? })?;
            Ok(text("closed".into()))
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

/// Title, URL, numbered elements, then text. Numbers are stamped on the elements
/// as data-lumen-id, so click and type find exactly what was listed.
fn read(proxy: &EventLoopProxy<Ev>, tab: u64, max: usize) -> Result<String, String> {
    let v = eval(proxy, tab, &READ_JS.replace("__MAX__", &max.to_string()))?;
    let mut out = format!(
        "Title: {}\nURL: {}\n",
        v["title"].as_str().unwrap_or(""),
        v["url"].as_str().unwrap_or("")
    );
    if let Some(els) = v["els"].as_array()
        && !els.is_empty()
    {
        out.push_str("\nElements (use the number with browser_click / browser_type):\n");
        for e in els {
            let (n, kind, label) = (e[0].as_u64().unwrap_or(0), e[1].as_str().unwrap_or(""), e[2].as_str().unwrap_or(""));
            out.push_str(&format!("[{n}] {kind} \"{label}\""));
            if let Some(h) = e[3].as_str().filter(|h| !h.is_empty()) {
                out.push_str(&format!(" -> {h}"));
            }
            out.push('\n');
        }
    }
    out.push_str("\nText:\n");
    out.push_str(v["text"].as_str().unwrap_or(""));
    Ok(out)
}

const READ_JS: &str = r#"(()=>{
const vis=e=>{const r=e.getBoundingClientRect();if(r.width<2||r.height<2)return false;const s=getComputedStyle(e);return s.visibility!=='hidden'&&s.display!=='none'&&s.opacity!=='0'};
const lab=e=>(e.getAttribute('aria-label')||e.innerText||e.value||e.placeholder||e.title||e.name||e.alt||'').replace(/\s+/g,' ').trim().slice(0,80);
document.querySelectorAll('[data-lumen-id]').forEach(e=>e.removeAttribute('data-lumen-id'));
const q='a[href],button,input:not([type=hidden]),textarea,select,[role=button],[role=link],[role=tab],[role=menuitem],[role=checkbox],[contenteditable=true],summary';
const els=[];let n=0;
for(const e of document.querySelectorAll(q)){if(els.length>=200)break;if(!vis(e))continue;n++;e.setAttribute('data-lumen-id',n);
const t=e.tagName.toLowerCase();const kind=t==='input'?'input['+(e.type||'text')+']':t==='a'?'link':(e.getAttribute('role')||t);
els.push([n,kind,lab(e),t==='a'?e.href:'']);}
const text=(document.body?document.body.innerText:'').replace(/\n{3,}/g,'\n\n').slice(0,__MAX__);
return {title:document.title,url:location.href,els,text};})()"#;

const CLICK_JS: &str = r#"(()=>{const e=document.querySelector('[data-lumen-id="__N__"]');
if(!e)return 'no element __N__ on this page; call browser_read again';
e.scrollIntoView({block:'center'});if(e.focus)e.focus();e.click();return 'ok';})()"#;

const TYPE_JS: &str = r#"(()=>{const e=document.querySelector('[data-lumen-id="__N__"]');
if(!e)return 'no element __N__ on this page; call browser_read again';
const t=__TEXT__;e.scrollIntoView({block:'center'});e.focus();
if(e.isContentEditable){e.textContent=t;}else{const p=Object.getPrototypeOf(e);const d=Object.getOwnPropertyDescriptor(p,'value');
if(d&&d.set)d.set.call(e,t);else e.value=t;}
e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));
if(__SUBMIT__){const k={key:'Enter',code:'Enter',keyCode:13,which:13,bubbles:true,cancelable:true};
// A page that handles Enter itself says so with preventDefault; then the form is not submitted a second time.
const free=e.dispatchEvent(new KeyboardEvent('keydown',k))&&e.dispatchEvent(new KeyboardEvent('keypress',k));e.dispatchEvent(new KeyboardEvent('keyup',k));
if(free&&e.form){if(e.form.requestSubmit)e.form.requestSubmit();else e.form.submit();}}
return 'ok';})()"#;
