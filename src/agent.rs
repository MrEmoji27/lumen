//! A door for AI agents: a tiny MCP server (JSON-RPC over HTTP) on 127.0.0.1.
//!
//! The server runs on its own thread. Every tool is built from a few primitive
//! operations (open, eval, DevTools call...) that must run on the UI thread, so they
//! travel as `Ev::Agent(Call)` and come back through a channel.
//!
//! The way an agent sees a page is the way Playwright MCP and agent-browser do it: a
//! snapshot of the controls with short refs (e1, e2...), then actions by ref. Clicks, keys
//! and typing go through the DevTools Protocol, so the page gets trusted input.
//!
//! Many agents at once: every MCP session owns the tabs it opens and cannot touch another
//! session's. Guard rails: localhost only, a secret token on every request, Host and Origin
//! checked against DNS rebinding, bodies capped at 1 MB. Off unless enabled.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Sender, channel},
    },
    thread,
    time::{Duration, Instant},
};

use base64::Engine as _;
use serde_json::{Value, json};
use tao::event_loop::EventLoopProxy;

use crate::{
    agent_policy::{self, Approval, PolicyCtx, authorize},
    engine::Ev,
};

/// Who is asking: the MCP session, and the name its client gave.
#[derive(Clone, Debug)]
pub struct Who {
    pub session: String,
    pub name: String,
}

pub enum Op {
    List,
    Open { url: String, activate: bool },
    Navigate { tab: u64, url: String },
    Back { tab: u64 },
    Forward { tab: u64 },
    Reload { tab: u64 },
    Eval { tab: u64, js: String },
    /// One DevTools Protocol method on the tab's page.
    Cdp { tab: u64, method: String, params: Value },
    /// Console lines or network requests recorded for the tab.
    Log { tab: u64, network: bool, clear: bool },
    /// How the tab's next dialogs are answered.
    Dialogs { tab: u64, accept: bool, text: Option<String> },
    /// Bring the tab to the front, so the person sees it.
    Show { tab: u64 },
    /// Hand the tab to the person until they press Done.
    Handoff { tab: u64, message: String },
    Close { tab: u64 },
    /// Close every tab this session owns.
    End,
    Download { url: String, tab: Option<u64> },
    Downloads,
    /// The facts `agent_policy::authorize` needs: tabs, owners, grants, settings.
    Policy,
    /// Test hook, debug builds only: press Done on the waiting handoff.
    TestDone,
}

/// One primitive for the UI thread, and where to send the answer.
pub struct Call {
    pub op: Op,
    pub who: Who,
    pub reply: Sender<Result<Value, String>>,
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match &self.op {
            Op::List => "list".to_string(),
            Op::Open { url, .. } => format!("open {url}"),
            Op::Navigate { tab, url } => format!("navigate {tab} {url}"),
            Op::Back { tab } => format!("back {tab}"),
            Op::Forward { tab } => format!("forward {tab}"),
            Op::Reload { tab } => format!("reload {tab}"),
            Op::Eval { tab, .. } => format!("eval {tab}"),
            Op::Cdp { tab, method, .. } => format!("cdp {tab} {method}"),
            Op::Log { tab, network, .. } => format!("log {tab} net={network}"),
            Op::Dialogs { tab, accept, .. } => format!("dialogs {tab} accept={accept}"),
            Op::Show { tab } => format!("show {tab}"),
            Op::Handoff { tab, .. } => format!("handoff {tab}"),
            Op::Close { tab } => format!("close {tab}"),
            Op::End => "end".to_string(),
            Op::Download { url, .. } => format!("download {url}"),
            Op::Downloads => "downloads".to_string(),
            Op::Policy => "policy".to_string(),
            Op::TestDone => "test done".to_string(),
        };
        write!(f, "Call({name} by {})", self.who.name)
    }
}

pub struct Agent {
    pub token: String,
    on: Arc<AtomicBool>,
    port: Option<u32>,
    /// Who is connected. Cleared when the door shuts.
    sessions: Arc<Sessions>,
}

impl Agent {
    /// Load the token, or make one. 32 random bytes from the OS, hex.
    pub fn new(dir: &Path) -> Agent {
        let path = dir.join("agent_token");
        let token = std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string()).filter(|s| s.len() == 64).unwrap_or_else(|| {
            let t = random_hex(32);
            let _ = std::fs::write(&path, &t);
            t
        });
        Agent { token, on: Arc::new(AtomicBool::new(false)), port: None, sessions: Arc::new(Sessions::new(session_ttl())) }
    }

    /// Turn the door on or off. The listener starts once and stays; off means it refuses.
    pub fn set(&mut self, on: bool, port: u32, proxy: &EventLoopProxy<Ev>) {
        self.on.store(on, Ordering::SeqCst);
        if !on {
            // A shut door forgets its sessions: ids from before do not work when it opens again.
            self.sessions.clear();
        }
        if on {
            // lumen-agent answers the MCP handshake from this file while Lumen is not running,
            // so an agent's session can start without starting a browser.
            let card = json!({"tools": tools(), "instructions": INSTRUCTIONS, "version": env!("CARGO_PKG_VERSION")});
            let _ = std::fs::write(crate::store::data_dir().join("agent_tools.json"), card.to_string());
        }
        if on && self.port.is_none() {
            self.port = Some(port);
            let shared = Arc::new(Shared {
                on: self.on.clone(),
                token: self.token.clone(),
                port,
                proxy: proxy.clone(),
                sessions: self.sessions.clone(),
            });
            let sweeper = shared.clone();
            thread::Builder::new().name("lumen-agent-sweep".into()).spawn(move || sweep(sweeper)).ok();
            thread::Builder::new()
                .name("lumen-agent".into())
                .spawn(move || serve(shared))
                .ok();
        }
    }

    pub fn endpoint(&self, port: u32) -> String {
        format!("http://127.0.0.1:{port}/mcp")
    }

    /// Paste into a terminal and Claude Code can browse with Lumen. `lumen-agent` (next to
    /// lumen.exe) needs no token at all; this form is for clients that only speak HTTP.
    pub fn setup_command(&self, port: u32) -> String {
        let bridge = std::env::current_exe().ok().map(|p| p.with_file_name("lumen-agent.exe")).filter(|p| p.is_file());
        match bridge {
            // No secret in the command: lumen-agent reads the token itself.
            Some(b) => format!("claude mcp add -s user lumen -- \"{}\" mcp", b.display()),
            None => format!("claude mcp add --transport http lumen {} --header \"Authorization: Bearer {}\"", self.endpoint(port), self.token),
        }
    }
}

/// `n` bytes from the OS random generator, as hex. Never zeros after a failed call: if the
/// system generator says no, fall back to the standard library's per-process random keys.
fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    let ok = unsafe {
        use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
        BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG).is_ok()
    };
    if !ok {
        use std::hash::{BuildHasher, Hasher};
        for chunk in b.chunks_mut(8) {
            let r = std::collections::hash_map::RandomState::new().build_hasher().finish().to_le_bytes();
            chunk.copy_from_slice(&r[..chunk.len()]);
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------- sessions

/// A session id is 24 random bytes (192 bits). Only Lumen makes them, at `initialize`.
const SESSION_BYTES: usize = 24;
/// Most sessions at once. An agent that opens more than this is looping.
const MAX_SESSIONS: usize = 64;

/// How long a session may sit silent before Lumen closes its tabs. A bridge that was killed
/// never says goodbye; this is how its tabs go. Debug builds read LUMEN_AGENT_SESSION_SECS,
/// so a test need not wait 15 minutes.
fn session_ttl() -> Duration {
    if cfg!(debug_assertions)
        && let Some(secs) = std::env::var("LUMEN_AGENT_SESSION_SECS").ok().and_then(|v| v.parse::<u64>().ok())
    {
        return Duration::from_secs(secs.max(1));
    }
    Duration::from_secs(15 * 60)
}

struct Session {
    /// What the client called itself. A label for people to read, never a key.
    name: String,
    last: Instant,
    /// Requests being answered right now (a handoff waiting is one). A busy session never expires.
    busy: u32,
}

/// The sessions Lumen issued. A request names a session by its id and by nothing else.
pub struct Sessions {
    map: Mutex<HashMap<String, Session>>,
    ttl: Duration,
}

impl Sessions {
    pub fn new(ttl: Duration) -> Sessions {
        Sessions { map: Mutex::new(HashMap::new()), ttl }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Session>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new session. The label is cleaned: no control characters, 40 characters at most.
    pub fn issue(&self, label: &str, now: Instant) -> Result<String, String> {
        let name: String = label.chars().filter(|c| !c.is_control()).take(40).collect();
        let mut map = self.lock();
        if map.len() >= MAX_SESSIONS {
            return Err(format!("{MAX_SESSIONS} sessions are open already; close some first"));
        }
        let id = random_hex(SESSION_BYTES);
        map.insert(id.clone(), Session { name: if name.trim().is_empty() { "agent".into() } else { name }, last: now, busy: 0 });
        Ok(id)
    }

    /// A request starts. The label if the id is one Lumen issued and still holds; None if not.
    fn begin(&self, id: &str, now: Instant) -> Option<String> {
        let mut map = self.lock();
        let s = map.get_mut(id)?;
        s.busy += 1;
        s.last = now;
        Some(s.name.clone())
    }

    fn finish(&self, id: &str, now: Instant) {
        if let Some(s) = self.lock().get_mut(id) {
            s.busy = s.busy.saturating_sub(1);
            s.last = now;
        }
    }

    /// The client said goodbye. Its label, if the session existed.
    fn remove(&self, id: &str) -> Option<String> {
        self.lock().remove(id).map(|s| s.name)
    }

    /// Sessions silent for longer than the limit with nothing in flight: taken out and returned
    /// as (id, label), for the caller to close their tabs.
    pub fn expire(&self, now: Instant) -> Vec<(String, String)> {
        let mut map = self.lock();
        let dead: Vec<String> = map.iter().filter(|(_, s)| s.busy == 0 && now.saturating_duration_since(s.last) > self.ttl).map(|(id, _)| id.clone()).collect();
        dead.into_iter().filter_map(|id| map.remove(&id).map(|s| (id, s.name))).collect()
    }

    pub fn clear(&self) {
        self.lock().clear();
    }
}

/// Keeps a request counted as busy until it is answered, whichever way it ends.
struct Busy<'a> {
    sessions: &'a Sessions,
    id: String,
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.sessions.finish(&self.id, Instant::now());
    }
}

/// Closes the tabs of sessions that went quiet.
fn sweep(sh: Arc<Shared>) {
    let tick = (sh.sessions.ttl / 4).clamp(Duration::from_millis(250), Duration::from_secs(10));
    loop {
        thread::sleep(tick);
        for (session, name) in sh.sessions.expire(Instant::now()) {
            let _ = call(&sh.proxy, &Who { session, name }, Op::End);
        }
    }
}

/// Host and Origin must be this very door. Anything else is a web page pretending to be local,
/// or a DNS name pointed at 127.0.0.1. Exact match: no suffix tricks, and "null" (a sandboxed
/// frame) is not welcome.
fn origin_ok(host: &str, origin: &str, port: u32) -> bool {
    let local = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    local.iter().any(|l| l == host) && (origin.is_empty() || local.iter().any(|l| origin == format!("http://{l}")))
}

struct Shared {
    on: Arc<AtomicBool>,
    token: String,
    port: u32,
    proxy: EventLoopProxy<Ev>,
    sessions: Arc<Sessions>,
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

fn respond(mut s: TcpStream, code: &str, extra: &str, body: &str) {
    let ctype = if body.is_empty() { "" } else { "Content-Type: application/json\r\n" };
    let _ = write!(s, "HTTP/1.1 {code}\r\n{ctype}{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
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
    let (mut len, mut auth, mut host, mut origin, mut session) = (0usize, String::new(), String::new(), String::new(), String::new());
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
                "mcp-session-id" => session = v.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).take(64).collect(),
                _ => {}
            }
        }
    }
    // A web page cannot reach us by pretending to be localhost.
    if !origin_ok(&host, &origin, sh.port) {
        return respond(stream, "403 Forbidden", "", "");
    }
    if !sh.on.load(Ordering::SeqCst) {
        return respond(stream, "503 Service Unavailable", "", "");
    }
    let expected = format!("Bearer {}", sh.token);
    if !same(auth.as_bytes(), expected.as_bytes()) {
        return respond(stream, "401 Unauthorized", "", "");
    }
    if path != "/mcp" {
        return respond(stream, "404 Not Found", "", "");
    }
    if method == "DELETE" {
        // The client is done: its tabs go with it. An id Lumen never issued ends nothing.
        if session.is_empty() {
            return respond(stream, "400 Bad Request", "", "");
        }
        return match sh.sessions.remove(&session) {
            Some(name) => {
                let _ = call(&sh.proxy, &Who { session, name }, Op::End);
                respond(stream, "200 OK", "", "")
            }
            None => respond(stream, "404 Not Found", "", ""),
        };
    }
    if method != "POST" {
        // No server-to-client stream; plain request and response is enough.
        return respond(stream, "405 Method Not Allowed", "", "");
    }
    if len > 1 << 20 {
        return respond(stream, "413 Payload Too Large", "", "");
    }
    let mut body = vec![0u8; len];
    if r.read_exact(&mut body).is_err() {
        return;
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return respond(stream, "400 Bad Request", "", &rpc_err(Value::Null, -32700, "parse error").to_string());
    };
    let id = msg.get("id").cloned();
    // Only Lumen makes session ids, and only here. Whatever id the client sent with an
    // initialize is ignored; the client's name is a label for people, never a key.
    let mut extra = String::new();
    let name;
    let _busy;
    if msg["method"] == "initialize" {
        let label = msg["params"]["clientInfo"]["name"].as_str().unwrap_or("agent");
        match sh.sessions.issue(label, Instant::now()) {
            Ok(s) => session = s,
            Err(e) => return respond(stream, "429 Too Many Requests", "", &rpc_err(id.unwrap_or(Value::Null), -32000, &e).to_string()),
        }
        extra = format!("Mcp-Session-Id: {session}\r\n");
        name = sh.sessions.begin(&session, Instant::now()).unwrap_or_default();
        _busy = Busy { sessions: &sh.sessions, id: session.clone() };
    } else if session.is_empty() && msg["method"] == "ping" {
        // Is anyone home? The bridge asks this before it has a session. No tab is reachable from here.
        let Some(id) = id else { return respond(stream, "202 Accepted", "", "") };
        return respond(stream, "200 OK", "", &json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string());
    } else {
        let id = id.clone().unwrap_or(Value::Null);
        if session.is_empty() {
            return respond(stream, "400 Bad Request", "", &rpc_err(id, -32600, "no Mcp-Session-Id: send initialize first and use the id it returns").to_string());
        }
        match sh.sessions.begin(&session, Instant::now()) {
            Some(n) => name = n,
            // The MCP spec: 404 for an id the server does not know, and the client starts over.
            None => return respond(stream, "404 Not Found", "", &rpc_err(id, -32001, "unknown or expired session: send initialize again").to_string()),
        }
        _busy = Busy { sessions: &sh.sessions, id: session.clone() };
    }
    let who = Who { name, session };
    // Notifications get no answer.
    let Some(id) = id else { return respond(stream, "202 Accepted", &extra, "") };
    let out = match msg["method"].as_str().unwrap_or("") {
        "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "lumen", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS
        }}),
        "ping" => json!({"jsonrpc": "2.0", "id": id, "result": {}}),
        "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools()}}),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = &msg["params"]["arguments"];
            let content = match run(&sh.proxy, &who, name, args) {
                Ok(c) => json!({"content": c}),
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            };
            json!({"jsonrpc": "2.0", "id": id, "result": content})
        }
        _ => rpc_err(id, -32601, "method not found"),
    };
    respond(stream, "200 OK", &extra, &out.to_string());
}

const INSTRUCTIONS: &str = "Lumen is a real desktop browser. Open a tab with browser_open; it returns a snapshot: \
the page's controls, each with a ref like e5. Act with browser_click / browser_type / browser_select / browser_press \
using those refs, and take a new browser_snapshot after the page changes (refs are re-made each time). \
browser_read adds the page text; browser_article gives just the article, cheapest for reading. \
browser_wait waits for text or an element. browser_console and browser_network show what the page logged and \
fetched. When a login, captcha or a choice needs the person, call browser_handoff and wait. \
Tabs you open are yours alone; other agents cannot touch them. Close them when done. \
The person's own tabs are off limits unless they right-click one and choose \"Let agents use this tab\" \
(and even then no eval, upload or close). browser_upload is off unless the person turns it on, and reads one \
folder only. A refused call says which setting would allow it.";

/// Constant-time compare, so the token cannot be guessed by timing.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn rpc_err(id: Value, code: i32, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

// ------------------------------------------------------------------- tools

pub fn tools() -> Value {
    let tab = json!({"type": "integer", "description": "Tab id from browser_open or browser_tabs"});
    let r#ref = json!({"type": "string", "description": "Element ref from the last snapshot, e.g. \"e5\""});
    let t = |name: &str, desc: &str, props: Value, req: &[&str]| json!({"name": name, "description": desc, "inputSchema": {"type": "object", "properties": props, "required": req}});
    json!([
        t("browser_open", "Open a URL (or search words) in a new tab of yours. Waits for the page. Returns the tab id and a snapshot.",
          json!({"url": {"type": "string"}, "background": {"type": "boolean", "description": "Do not bring the tab to the front"}}), &["url"]),
        t("browser_navigate", "Load a URL (or search words) in your tab and wait for it. Returns a snapshot.",
          json!({"tab": tab, "url": {"type": "string"}}), &["tab", "url"]),
        t("browser_back", "Go back in the tab's history.", json!({"tab": tab}), &["tab"]),
        t("browser_forward", "Go forward in the tab's history.", json!({"tab": tab}), &["tab"]),
        t("browser_reload", "Reload the tab.", json!({"tab": tab}), &["tab"]),
        t("browser_snapshot", "The page's structure: headings and every visible control (links, buttons, inputs, selects...) with a ref like e5 for acting on it. Cheap. Take a new one after the page changes.",
          json!({"tab": tab, "scope": {"type": "string", "description": "CSS selector to limit the snapshot to"}, "interactive": {"type": "boolean", "description": "Controls only, no headings (smaller)"}, "max_lines": {"type": "integer", "description": "Default 300"}}), &["tab"]),
        t("browser_read", "A snapshot plus the visible text of the page.",
          json!({"tab": tab, "max_chars": {"type": "integer", "description": "Text budget, default 6000"}}), &["tab"]),
        t("browser_article", "Just the main article as clean text (Mozilla Readability). Best for reading news, docs, posts. Falls back to page text.",
          json!({"tab": tab, "links": {"type": "boolean", "description": "Keep links as [text](url)"}, "max_chars": {"type": "integer", "description": "Default 20000"}}), &["tab"]),
        t("browser_find", "Find text on the page: matching lines of text, and refs of controls whose name matches.",
          json!({"tab": tab, "text": {"type": "string"}}), &["tab", "text"]),
        t("browser_click", "Click an element (by ref) or a point (x, y in CSS pixels) with a real mouse event. Waits for any navigation it causes.",
          json!({"tab": tab, "ref": r#ref, "x": {"type": "number"}, "y": {"type": "number"}, "button": {"type": "string", "enum": ["left", "right", "middle"]}, "double": {"type": "boolean"}}), &["tab"]),
        t("browser_hover", "Move the mouse over an element.", json!({"tab": tab, "ref": r#ref}), &["tab", "ref"]),
        t("browser_type", "Type into a field (input, textarea, contenteditable) as real keyboard text. Replaces what is there unless append=true. submit=true presses Enter after.",
          json!({"tab": tab, "ref": r#ref, "text": {"type": "string"}, "submit": {"type": "boolean"}, "append": {"type": "boolean"}}), &["tab", "ref", "text"]),
        t("browser_press", "Press a key or chord on the focused element: Enter, Escape, Tab, ArrowDown, PageDown, Backspace, Control+A, Shift+Tab, a...",
          json!({"tab": tab, "key": {"type": "string"}}), &["tab", "key"]),
        t("browser_select", "Choose an option in a <select> by its value or visible text.", json!({"tab": tab, "ref": r#ref, "option": {"type": "string"}}), &["tab", "ref", "option"]),
        t("browser_scroll", "Scroll: to=\"down\" | \"up\" | \"top\" | \"bottom\", or a ref to bring into view, or pixels (positive is down).",
          json!({"tab": tab, "to": {"type": "string"}, "pixels": {"type": "integer"}}), &["tab"]),
        t("browser_drag", "Drag one element onto another.", json!({"tab": tab, "from": r#ref, "to": r#ref}), &["tab", "from", "to"]),
        t("browser_upload", "Put local files into a file input.", json!({"tab": tab, "ref": r#ref, "paths": {"type": "array", "items": {"type": "string"}}}), &["tab", "ref", "paths"]),
        t("browser_wait", "Wait until text appears, text disappears, a CSS selector matches, or the URL contains something. Default timeout 15 s, max 120 s.",
          json!({"tab": tab, "text": {"type": "string"}, "text_gone": {"type": "string"}, "selector": {"type": "string"}, "url": {"type": "string"}, "timeout_ms": {"type": "integer"}}), &["tab"]),
        t("browser_screenshot", "Image of the tab. full_page for the whole page, ref for one element. JPEG by default (smaller); format=png for sharp text.",
          json!({"tab": tab, "full_page": {"type": "boolean"}, "ref": r#ref, "format": {"type": "string", "enum": ["jpeg", "png"]}}), &["tab"]),
        t("browser_pdf", "Save the page as a PDF in the Downloads folder. Returns the path.", json!({"tab": tab}), &["tab"]),
        t("browser_eval", "Run JavaScript in the page. The script is a function body; `return` a JSON-serializable value (promises are awaited).",
          json!({"tab": tab, "script": {"type": "string"}}), &["tab", "script"]),
        t("browser_console", "What the page logged: console messages, uncaught errors, browser warnings, and dialogs Lumen answered.",
          json!({"tab": tab, "clear": {"type": "boolean"}}), &["tab"]),
        t("browser_network", "Requests the page made: method, status, type, URL. Failed ones say why.",
          json!({"tab": tab, "filter": {"type": "string", "description": "Only URLs containing this"}, "clear": {"type": "boolean"}}), &["tab"]),
        t("browser_dialogs", "How alerts, confirms and prompts are answered from now on (default: accept). Lumen answers them; browser_console shows what they said.",
          json!({"tab": tab, "accept": {"type": "boolean"}, "prompt_text": {"type": "string"}}), &["tab", "accept"]),
        t("browser_resize", "Emulate a screen size (responsive testing). device=\"mobile\" | \"tablet\" | \"desktop\", or width and height. device=\"reset\" undoes it.",
          json!({"tab": tab, "device": {"type": "string"}, "width": {"type": "integer"}, "height": {"type": "integer"}}), &["tab"]),
        t("browser_tabs", "List your tabs.", json!({}), &[]),
        t("browser_show", "Bring your tab to the front so the person sees it.", json!({"tab": tab}), &["tab"]),
        t("browser_handoff", "Hand the tab to the person (log in, solve a captcha, choose something). Shows them your message and waits until they press Done (up to 15 min). Returns the page snapshot after.",
          json!({"tab": tab, "message": {"type": "string"}}), &["tab", "message"]),
        t("browser_download", "Download a file with Lumen's downloader (aria2). With a tab, its cookies go along. wait=true waits for it to finish and returns the path.",
          json!({"url": {"type": "string"}, "tab": tab, "wait": {"type": "boolean"}}), &["url"]),
        t("browser_downloads", "List downloads: state, size, path.", json!({}), &[]),
        t("browser_close", "Close your tab.", json!({"tab": tab}), &["tab"])
    ])
}

fn call(proxy: &EventLoopProxy<Ev>, who: &Who, op: Op) -> Result<Value, String> {
    call_for(proxy, who, op, Duration::from_secs(30))
}

fn call_for(proxy: &EventLoopProxy<Ev>, who: &Who, op: Op, wait: Duration) -> Result<Value, String> {
    let (tx, rx) = channel();
    proxy.send_event(Ev::Agent(Call { op, who: who.clone(), reply: tx })).map_err(|_| "browser is closing".to_string())?;
    rx.recv_timeout(wait).map_err(|_| "browser did not answer in time".to_string())?
}

fn eval(proxy: &EventLoopProxy<Ev>, who: &Who, tab: u64, js: &str) -> Result<Value, String> {
    call(proxy, who, Op::Eval { tab, js: js.to_string() })
}

fn cdp(proxy: &EventLoopProxy<Ev>, who: &Who, tab: u64, method: &str, params: Value) -> Result<Value, String> {
    call(proxy, who, Op::Cdp { tab, method: method.into(), params })
}

/// Until the document is complete, or 12 seconds pass. Then a breath for late scripts.
fn wait_load(proxy: &EventLoopProxy<Ev>, who: &Who, tab: u64) {
    thread::sleep(Duration::from_millis(250));
    for _ in 0..60 {
        // A fresh tab is about:blank and already "complete"; wait for the real page.
        let js = "document.readyState==='complete'&&location.href!=='about:blank'";
        if eval(proxy, who, tab, js).ok().and_then(|v| v.as_bool()) == Some(true) {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    thread::sleep(Duration::from_millis(300));
}

fn tab_arg(args: &Value) -> Result<u64, String> {
    args["tab"].as_u64().ok_or_else(|| "missing tab id".to_string())
}

/// "e5", "@e5", "5" or 5 -> "e5".
fn ref_arg(v: &Value) -> Option<String> {
    let s = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().trim_start_matches('@').to_string(),
        _ => return None,
    };
    if s.is_empty() {
        return None;
    }
    Some(if s.bytes().all(|b| b.is_ascii_digit()) { format!("e{s}") } else { s })
}

fn need_ref(args: &Value, key: &str) -> Result<String, String> {
    ref_arg(&args[key]).ok_or_else(|| format!("missing {key} (a ref like \"e5\" from browser_snapshot)"))
}

fn text(s: String) -> Vec<Value> {
    vec![json!({"type": "text", "text": s})]
}

/// The page script that finds a ref, first. Refs live in a Map (shadow DOM included) and
/// as data-lumen-ref attributes (survive a script that replaced the Map).
const EL_JS: &str = r#"const __el=(r)=>{const m=window.__lumenRefs;let e=m&&m.get(r);if(!e||!e.isConnected)e=document.querySelector('[data-lumen-ref="'+r+'"]');return e};"#;

fn with_el(r: &str, body: &str) -> String {
    format!("(()=>{{{EL_JS}const R={};const e=__el(R);if(!e)return {{error:'no element '+R+' on this page now; take a new browser_snapshot'}};{body}}})()", json!(r))
}

/// Scroll the element into view and say where its middle is, and what covers it if anything does.
fn point(proxy: &EventLoopProxy<Ev>, who: &Who, tab: u64, r: &str) -> Result<(f64, f64, String), String> {
    let v = eval(proxy, who, tab, &with_el(r, POINT_JS))?;
    if let Some(e) = v["error"].as_str() {
        return Err(e.to_string());
    }
    Ok((v["x"].as_f64().unwrap_or(0.0), v["y"].as_f64().unwrap_or(0.0), v["covered"].as_str().unwrap_or("").to_string()))
}

const POINT_JS: &str = r#"e.scrollIntoView({block:'center',inline:'center',behavior:'instant'});
const b=e.getBoundingClientRect();if(b.width<1||b.height<1)return {error:'element '+R+' has no size (hidden?)'};
const x=b.left+b.width/2,y=b.top+b.height/2;const t=document.elementFromPoint(x,y);
const cov=t&&t!==e&&!e.contains(t)&&!t.contains(e)?(t.tagName.toLowerCase()+(t.id?'#'+t.id:'')):'';
return {x,y,covered:cov};"#;

fn mouse(proxy: &EventLoopProxy<Ev>, who: &Who, tab: u64, kind: &str, x: f64, y: f64, button: &str, clicks: u32) -> Result<Value, String> {
    let mut p = json!({"type": kind, "x": x, "y": y});
    if kind != "mouseMoved" {
        p["button"] = json!(button);
        p["clickCount"] = json!(clicks);
    }
    cdp(proxy, who, tab, "Input.dispatchMouseEvent", p)
}

/// A key name or chord as DevTools key events: (key, code, virtual key, text, modifiers).
fn key_spec(chord: &str) -> Result<(String, String, u32, String, u32), String> {
    let parts: Vec<&str> = chord.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    let (last, mods) = parts.split_last().ok_or("empty key")?;
    let mut m = 0;
    for p in mods {
        m |= match p.to_ascii_lowercase().as_str() {
            "alt" => 1,
            "control" | "ctrl" => 2,
            "meta" | "cmd" | "win" => 4,
            "shift" => 8,
            other => return Err(format!("unknown modifier {other}")),
        };
    }
    let named = |k: &str, vk: u32, t: &str| Ok((k.to_string(), k.to_string(), vk, t.to_string(), m));
    match last.to_ascii_lowercase().as_str() {
        "enter" | "return" => named("Enter", 13, "\r"),
        "tab" => named("Tab", 9, ""),
        "escape" | "esc" => named("Escape", 27, ""),
        "backspace" => named("Backspace", 8, ""),
        "delete" | "del" => named("Delete", 46, ""),
        "space" | " " => Ok((" ".into(), "Space".into(), 32, if m & 7 == 0 { " ".into() } else { String::new() }, m)),
        "arrowup" | "up" => named("ArrowUp", 38, ""),
        "arrowdown" | "down" => named("ArrowDown", 40, ""),
        "arrowleft" | "left" => named("ArrowLeft", 37, ""),
        "arrowright" | "right" => named("ArrowRight", 39, ""),
        "home" => named("Home", 36, ""),
        "end" => named("End", 35, ""),
        "pageup" => named("PageUp", 33, ""),
        "pagedown" => named("PageDown", 34, ""),
        f if f.len() >= 2 && f.starts_with('f') && f[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) => {
            let n: u32 = f[1..].parse().unwrap_or(1);
            Ok((format!("F{n}"), format!("F{n}"), 111 + n, String::new(), m))
        }
        _ => {
            let mut chars = last.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { return Err(format!("unknown key {last}")) };
            let up = c.to_ascii_uppercase();
            let code = if c.is_ascii_alphabetic() {
                format!("Key{up}")
            } else if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                String::new()
            };
            let vk = if c.is_ascii_alphanumeric() { up as u32 } else { 0 };
            // Text only when the chord would type it: Shift is fine, Control/Alt/Meta are commands.
            let shown = if m & 8 != 0 { c.to_uppercase().to_string() } else { c.to_string() };
            Ok((shown.clone(), code, vk, if m & 7 == 0 { shown } else { String::new() }, m))
        }
    }
}

fn press(proxy: &EventLoopProxy<Ev>, who: &Who, tab: u64, chord: &str) -> Result<(), String> {
    let (key, code, vk, text, m) = key_spec(chord)?;
    let mut down = json!({"type": if text.is_empty() { "rawKeyDown" } else { "keyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": m});
    if !text.is_empty() {
        down["text"] = json!(text);
        down["unmodifiedText"] = json!(text);
    }
    cdp(proxy, who, tab, "Input.dispatchKeyEvent", down)?;
    cdp(proxy, who, tab, "Input.dispatchKeyEvent", json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk, "modifiers": m}))?;
    Ok(())
}

/// Wait for a navigation the action may have started; quick when nothing happens.
fn settle(proxy: &EventLoopProxy<Ev>, who: &Who, tab: u64, before: &str) {
    thread::sleep(Duration::from_millis(350));
    let now = eval(proxy, who, tab, "location.href+'|'+document.readyState").ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    if !now.starts_with(&format!("{before}|")) || !now.ends_with("|complete") {
        wait_load(proxy, who, tab);
    }
}

fn href(proxy: &EventLoopProxy<Ev>, who: &Who, tab: u64) -> String {
    eval(proxy, who, tab, "location.href").ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

/// Where an agent's files go: a subfolder of the person's download folder, kept apart from theirs.
fn agent_files_dir() -> PathBuf {
    let dir = agent_policy::agent_downloads_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Every tool call passes `authorize` first. Nothing below runs for a call that does not.
fn run(proxy: &EventLoopProxy<Ev>, who: &Who, name: &str, args: &Value) -> Result<Vec<Value>, String> {
    let ctx = PolicyCtx::from_value(&call(proxy, who, Op::Policy)?);
    match authorize(who, name, args, &ctx) {
        Ok(approval) => run_tool(proxy, who, name, args, &approval),
        Err(d) => {
            // The tool and why. No path, no URL, no page content.
            if d.log {
                crate::journal::error("agent", &format!("denied {name}: {}", d.code));
            }
            Err(d.text)
        }
    }
}

fn run_tool(proxy: &EventLoopProxy<Ev>, who: &Who, name: &str, args: &Value, approval: &Approval) -> Result<Vec<Value>, String> {
    let p = proxy;
    match name {
        #[cfg(debug_assertions)]
        "browser_test_crash" => {
            // The renderer dies and never answers; do not wait for it.
            let _ = call_for(p, who, Op::Cdp { tab: tab_arg(args)?, method: "Page.crash".into(), params: json!({}) }, Duration::from_secs(2));
            Ok(text("crashed".into()))
        }
        #[cfg(debug_assertions)]
        "browser_test_done" => {
            call(p, who, Op::TestDone)?;
            Ok(text("done pressed".into()))
        }
        "browser_open" => {
            let url = args["url"].as_str().ok_or("missing url")?.to_string();
            let activate = !args["background"].as_bool().unwrap_or(false);
            let tab = call(p, who, Op::Open { url, activate })?["tab"].as_u64().ok_or("no tab")?;
            wait_load(p, who, tab);
            Ok(text(format!("tab {tab}\n{}", snapshot(p, who, tab, &json!({}))?)))
        }
        "browser_navigate" => {
            let tab = tab_arg(args)?;
            let url = args["url"].as_str().ok_or("missing url")?.to_string();
            call(p, who, Op::Navigate { tab, url })?;
            wait_load(p, who, tab);
            Ok(text(snapshot(p, who, tab, &json!({}))?))
        }
        "browser_back" | "browser_forward" | "browser_reload" => {
            let tab = tab_arg(args)?;
            let op = match name {
                "browser_back" => Op::Back { tab },
                "browser_forward" => Op::Forward { tab },
                _ => Op::Reload { tab },
            };
            call(p, who, op)?;
            wait_load(p, who, tab);
            Ok(text(snapshot(p, who, tab, &json!({}))?))
        }
        "browser_snapshot" => Ok(text(snapshot(p, who, tab_arg(args)?, args)?)),
        "browser_read" => {
            let tab = tab_arg(args)?;
            let max = args["max_chars"].as_u64().unwrap_or(6000).clamp(200, 50_000);
            let mut out = snapshot(p, who, tab, &json!({}))?;
            let t = eval(p, who, tab, &format!("(document.body?document.body.innerText:'').replace(/\\n{{3,}}/g,'\\n\\n').slice(0,{max})"))?;
            out.push_str("\n\nText:\n");
            out.push_str(t.as_str().unwrap_or(""));
            Ok(text(out))
        }
        "browser_article" => {
            let tab = tab_arg(args)?;
            let max = args["max_chars"].as_u64().unwrap_or(20_000).clamp(500, 200_000) as usize;
            let links = args["links"].as_bool().unwrap_or(false);
            let js = format!("(()=>{{const a={};{ARTICLE_JS}}})()", crate::pagekit::extract_script());
            let v = eval(p, who, tab, &js.replace("__LINKS__", if links { "true" } else { "false" }))?;
            let body: String = v["text"].as_str().unwrap_or("").chars().take(max).collect();
            let mut out = format!("Title: {}\nURL: {}\n", v["title"].as_str().unwrap_or(""), v["url"].as_str().unwrap_or(""));
            if let Some(b) = v["byline"].as_str().filter(|b| !b.is_empty()) {
                out.push_str(&format!("By: {b}\n"));
            }
            if v["fallback"] == true {
                out.push_str("(no article found; this is the page text)\n");
            }
            out.push('\n');
            out.push_str(&body);
            Ok(text(out))
        }
        "browser_find" => {
            let tab = tab_arg(args)?;
            let q = args["text"].as_str().ok_or("missing text")?;
            let v = eval(p, who, tab, &FIND_JS.replace("__Q__", &json!(q).to_string()))?;
            let mut out = String::new();
            if let Some(els) = v["els"].as_array().filter(|a| !a.is_empty()) {
                out.push_str("Controls:\n");
                for e in els {
                    out.push_str(&format!("- {} \"{}\" [ref={}]\n", e[1].as_str().unwrap_or(""), e[2].as_str().unwrap_or(""), e[0].as_str().unwrap_or("")));
                }
            }
            if let Some(lines) = v["lines"].as_array().filter(|a| !a.is_empty()) {
                out.push_str("Text:\n");
                for l in lines {
                    out.push_str(&format!("- {}\n", l.as_str().unwrap_or("")));
                }
            }
            if out.is_empty() {
                out = format!("\"{q}\" is not on the page");
            }
            if v["fresh"] == true {
                out.push_str("\n(refs come from a new snapshot)");
            }
            Ok(text(out))
        }
        "browser_click" => {
            let tab = tab_arg(args)?;
            let button = args["button"].as_str().unwrap_or("left");
            let clicks = if args["double"].as_bool().unwrap_or(false) { 2 } else { 1 };
            let before = href(p, who, tab);
            let (x, y, note) = match ref_arg(&args["ref"]).or_else(|| ref_arg(&args["element"])) {
                Some(r) => {
                    let (x, y, covered) = point(p, who, tab, &r)?;
                    if !covered.is_empty() && button == "left" {
                        // Something sits on top (a banner, an overlay): click the element itself.
                        let v = eval(p, who, tab, &with_el(&r, "e.click();return {ok:1};"))?;
                        if let Some(e) = v["error"].as_str() {
                            return Err(e.to_string());
                        }
                        settle(p, who, tab, &before);
                        return Ok(text(format!("clicked {r} directly: it was under {covered}\n\n{}", snapshot(p, who, tab, &json!({}))?)));
                    }
                    (x, y, r)
                }
                None => {
                    let (Some(x), Some(y)) = (args["x"].as_f64(), args["y"].as_f64()) else { return Err("give a ref, or x and y".into()) };
                    (x, y, format!("({x}, {y})"))
                }
            };
            mouse(p, who, tab, "mouseMoved", x, y, button, 0)?;
            for c in 1..=clicks {
                mouse(p, who, tab, "mousePressed", x, y, button, c)?;
                mouse(p, who, tab, "mouseReleased", x, y, button, c)?;
            }
            settle(p, who, tab, &before);
            Ok(text(format!("clicked {note}\n\n{}", snapshot(p, who, tab, &json!({}))?)))
        }
        "browser_hover" => {
            let tab = tab_arg(args)?;
            let r = need_ref(args, "ref")?;
            let (x, y, _) = point(p, who, tab, &r)?;
            mouse(p, who, tab, "mouseMoved", x, y, "none", 0)?;
            thread::sleep(Duration::from_millis(300));
            Ok(text(format!("hovering {r}\n\n{}", snapshot(p, who, tab, &json!({"interactive": true}))?)))
        }
        "browser_type" => {
            let tab = tab_arg(args)?;
            let r = ref_arg(&args["ref"]).or_else(|| ref_arg(&args["element"])).ok_or("missing ref")?;
            let t = args["text"].as_str().ok_or("missing text")?;
            let append = args["append"].as_bool().unwrap_or(false);
            let v = eval(p, who, tab, &with_el(&r, &FOCUS_JS.replace("__APPEND__", if append { "true" } else { "false" })))?;
            if let Some(e) = v["error"].as_str() {
                return Err(e.to_string());
            }
            if !append && t.is_empty() {
                press(p, who, tab, "Delete")?;
            }
            if !t.is_empty() && cdp(p, who, tab, "Input.insertText", json!({"text": t})).is_err() {
                // No DevTools: set the value the way frameworks notice.
                eval(p, who, tab, &with_el(&r, &SET_JS.replace("__TEXT__", &json!(t).to_string())))?;
            }
            let shown = eval(p, who, tab, &with_el(&r, "return {v:e.isContentEditable?e.innerText:(e.type==='password'?'(hidden)':e.value)};"))?;
            let now = shown["v"].as_str().unwrap_or("").chars().take(120).collect::<String>();
            if args["submit"].as_bool().unwrap_or(false) {
                let before = href(p, who, tab);
                press(p, who, tab, "Enter")?;
                settle(p, who, tab, &before);
                return Ok(text(format!("typed into {r}, pressed Enter\n\n{}", snapshot(p, who, tab, &json!({}))?)));
            }
            Ok(text(format!("typed into {r}; it now holds \"{now}\"")))
        }
        "browser_press" => {
            let tab = tab_arg(args)?;
            let key = args["key"].as_str().ok_or("missing key")?;
            let before = href(p, who, tab);
            press(p, who, tab, key)?;
            settle(p, who, tab, &before);
            Ok(text(format!("pressed {key}\n\n{}", snapshot(p, who, tab, &json!({"interactive": true}))?)))
        }
        "browser_select" => {
            let tab = tab_arg(args)?;
            let r = need_ref(args, "ref")?;
            let opt = args["option"].as_str().ok_or("missing option")?;
            let v = eval(p, who, tab, &with_el(&r, &SELECT_JS.replace("__OPT__", &json!(opt).to_string())))?;
            if let Some(e) = v["error"].as_str() {
                return Err(e.to_string());
            }
            Ok(text(format!("{r} is now \"{}\"", v["chosen"].as_str().unwrap_or(""))))
        }
        "browser_scroll" => {
            let tab = tab_arg(args)?;
            let to = args["to"].as_str().unwrap_or("");
            let js = match (to, args["pixels"].as_i64()) {
                (_, Some(px)) => format!("window.scrollBy(0,{px});"),
                ("down", _) | ("", _) => "window.scrollBy(0,innerHeight*0.85);".into(),
                ("up", _) => "window.scrollBy(0,-innerHeight*0.85);".into(),
                ("top", _) => "window.scrollTo(0,0);".into(),
                ("bottom", _) => "window.scrollTo(0,document.documentElement.scrollHeight);".into(),
                (r, _) => {
                    let r = ref_arg(&json!(r)).unwrap_or_default();
                    let v = eval(p, who, tab, &with_el(&r, "e.scrollIntoView({block:'center'});return {ok:1};"))?;
                    if let Some(e) = v["error"].as_str() {
                        return Err(e.to_string());
                    }
                    String::new()
                }
            };
            let v = eval(p, who, tab, &format!("(()=>{{{js}return {{y:Math.round(scrollY),h:document.documentElement.scrollHeight,v:innerHeight}}}})()"))?;
            thread::sleep(Duration::from_millis(250));
            Ok(text(format!(
                "scrolled to {} of {} px (window {} px)\n\n{}",
                v["y"], v["h"], v["v"],
                snapshot(p, who, tab, &json!({"interactive": true}))?
            )))
        }
        "browser_drag" => {
            let tab = tab_arg(args)?;
            let (a, b) = (need_ref(args, "from")?, need_ref(args, "to")?);
            let (x1, y1, _) = point(p, who, tab, &a)?;
            let (x2, y2, _) = point(p, who, tab, &b)?;
            mouse(p, who, tab, "mouseMoved", x1, y1, "none", 0)?;
            mouse(p, who, tab, "mousePressed", x1, y1, "left", 1)?;
            for s in 1..=8 {
                let f = s as f64 / 8.0;
                mouse(p, who, tab, "mouseMoved", x1 + (x2 - x1) * f, y1 + (y2 - y1) * f, "none", 0)?;
                thread::sleep(Duration::from_millis(30));
            }
            mouse(p, who, tab, "mouseReleased", x2, y2, "left", 1)?;
            thread::sleep(Duration::from_millis(300));
            Ok(text(format!("dragged {a} onto {b}\n\n{}", snapshot(p, who, tab, &json!({"interactive": true}))?)))
        }
        "browser_upload" => {
            let tab = tab_arg(args)?;
            let r = need_ref(args, "ref")?;
            // The paths authorize checked and made canonical, not the ones the agent typed.
            let paths: Vec<String> = approval.files.clone();
            if paths.is_empty() {
                return Err("missing paths".into());
            }
            let root = cdp(p, who, tab, "DOM.getDocument", json!({"depth": 0}))?;
            let node = cdp(p, who, tab, "DOM.querySelector", json!({"nodeId": root["root"]["nodeId"], "selector": format!("[data-lumen-ref=\"{r}\"]")}))?;
            let id = node["nodeId"].as_u64().filter(|n| *n > 0).ok_or("that element is not a file input in the page (take a new snapshot)")?;
            cdp(p, who, tab, "DOM.setFileInputFiles", json!({"nodeId": id, "files": paths}))?;
            Ok(text(format!("{} file(s) put into {r}", paths.len())))
        }
        "browser_wait" => {
            let tab = tab_arg(args)?;
            let ms = args["timeout_ms"].as_u64().unwrap_or(15_000).clamp(100, 120_000);
            let cond = json!({"text": args["text"], "gone": args["text_gone"], "sel": args["selector"], "url": args["url"]});
            if cond.as_object().is_some_and(|o| o.values().all(Value::is_null)) {
                wait_load(p, who, tab);
                return Ok(text("page loaded".into()));
            }
            let js = WAIT_JS.replace("__C__", &cond.to_string());
            let start = Instant::now();
            loop {
                if eval(p, who, tab, &js)?.as_bool() == Some(true) {
                    return Ok(text(format!("done after {} ms\n\n{}", start.elapsed().as_millis(), snapshot(p, who, tab, &json!({"interactive": true}))?)));
                }
                if start.elapsed() > Duration::from_millis(ms) {
                    return Err(format!("still not there after {ms} ms"));
                }
                thread::sleep(Duration::from_millis(250));
            }
        }
        "browser_screenshot" => {
            let tab = tab_arg(args)?;
            let png = args["format"].as_str() == Some("png");
            let mut params = json!({"format": if png { "png" } else { "jpeg" }});
            if !png {
                params["quality"] = json!(75);
            }
            if let Some(r) = ref_arg(&args["ref"]) {
                let v = eval(p, who, tab, &with_el(&r, "e.scrollIntoView({block:'center'});const b=e.getBoundingClientRect();return {x:b.left+scrollX,y:b.top+scrollY,w:b.width,h:b.height};"))?;
                if let Some(e) = v["error"].as_str() {
                    return Err(e.to_string());
                }
                params["clip"] = json!({"x": v["x"], "y": v["y"], "width": v["w"], "height": v["h"], "scale": 1});
                params["captureBeyondViewport"] = json!(true);
            } else if args["full_page"].as_bool().unwrap_or(false) {
                let m = cdp(p, who, tab, "Page.getLayoutMetrics", json!({}))?;
                let size = &m["cssContentSize"];
                let (w, h) = (size["width"].as_f64().unwrap_or(1280.0), size["height"].as_f64().unwrap_or(800.0).min(16_000.0));
                params["clip"] = json!({"x": 0, "y": 0, "width": w, "height": h, "scale": 1});
                params["captureBeyondViewport"] = json!(true);
            }
            let shot = cdp(p, who, tab, "Page.captureScreenshot", params)?;
            let data = shot["data"].as_str().ok_or("no image came back")?;
            Ok(vec![json!({"type": "image", "mimeType": if png { "image/png" } else { "image/jpeg" }, "data": data})])
        }
        "browser_pdf" => {
            let tab = tab_arg(args)?;
            let title = eval(p, who, tab, "document.title")?.as_str().unwrap_or("page").to_string();
            let pdf = call_for(p, who, Op::Cdp { tab, method: "Page.printToPDF".into(), params: json!({"printBackground": true}) }, Duration::from_secs(90))?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(pdf["data"].as_str().ok_or("no PDF came back")?).map_err(|e| e.to_string())?;
            let safe: String = title.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).take(80).collect();
            let mut path = agent_files_dir().join(format!("{}.pdf", if safe.trim().is_empty() { "page" } else { safe.trim() }));
            let mut n = 1;
            while path.exists() {
                path = agent_files_dir().join(format!("{} ({n}).pdf", safe.trim()));
                n += 1;
            }
            std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
            Ok(text(format!("saved {} ({} KB)", path.display(), bytes.len() / 1024)))
        }
        "browser_eval" => {
            let tab = tab_arg(args)?;
            let script = args["script"].as_str().ok_or("missing script")?;
            // Through DevTools so a returned promise is awaited.
            let js = format!("(async()=>{{try{{const r=await (async()=>{{{script}\n}})();return r===undefined?null:r}}catch(e){{return 'error: '+e}}}})()");
            let v = cdp(p, who, tab, "Runtime.evaluate", json!({"expression": js, "awaitPromise": true, "returnByValue": true}))?;
            let out = if let Some(ex) = v["exceptionDetails"].as_object() {
                json!(format!("error: {}", ex.get("text").and_then(|t| t.as_str()).unwrap_or("")))
            } else {
                v["result"]["value"].clone()
            };
            Ok(text(serde_json::to_string_pretty(&out).unwrap_or_default()))
        }
        "browser_console" => {
            let tab = tab_arg(args)?;
            let v = call(p, who, Op::Log { tab, network: false, clear: args["clear"].as_bool().unwrap_or(false) })?;
            let lines: Vec<&str> = v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            Ok(text(if lines.is_empty() { "nothing logged".into() } else { lines.join("\n") }))
        }
        "browser_network" => {
            let tab = tab_arg(args)?;
            let v = call(p, who, Op::Log { tab, network: true, clear: args["clear"].as_bool().unwrap_or(false) })?;
            let f = args["filter"].as_str().unwrap_or("");
            let rows: Vec<String> = v
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|r| r["url"].as_str().is_some_and(|u| u.contains(f)))
                        .map(|r| {
                            let status = if !r["failed"].as_str().unwrap_or("").is_empty() {
                                format!("FAILED {}", r["failed"].as_str().unwrap_or(""))
                            } else if r["status"].as_u64() == Some(0) {
                                "pending".into()
                            } else {
                                r["status"].to_string()
                            };
                            format!("{} {} {} {}", r["method"].as_str().unwrap_or(""), status, r["kind"].as_str().unwrap_or(""), r["url"].as_str().unwrap_or(""))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let n = rows.len();
            let shown: Vec<String> = rows.into_iter().rev().take(150).collect::<Vec<_>>().into_iter().rev().collect();
            Ok(text(if n == 0 { "no requests recorded".into() } else { format!("{n} requests (newest last):\n{}", shown.join("\n")) }))
        }
        "browser_dialogs" => {
            let tab = tab_arg(args)?;
            let accept = args["accept"].as_bool().unwrap_or(true);
            call(p, who, Op::Dialogs { tab, accept, text: args["prompt_text"].as_str().map(String::from) })?;
            Ok(text(format!("dialogs will be {}", if accept { "accepted" } else { "dismissed" })))
        }
        "browser_resize" => {
            let tab = tab_arg(args)?;
            let (w, h, mobile) = match args["device"].as_str().unwrap_or("") {
                "reset" | "desktop" if args["width"].is_null() => {
                    cdp(p, who, tab, "Emulation.clearDeviceMetricsOverride", json!({}))?;
                    return Ok(text("back to the window's own size".into()));
                }
                "mobile" | "phone" => (390, 844, true),
                "tablet" => (820, 1180, true),
                _ => (args["width"].as_u64().ok_or("give device or width")?, args["height"].as_u64().unwrap_or(900), false),
            };
            cdp(p, who, tab, "Emulation.setDeviceMetricsOverride", json!({"width": w, "height": h, "deviceScaleFactor": 0, "mobile": mobile}))?;
            if mobile {
                cdp(p, who, tab, "Emulation.setTouchEmulationEnabled", json!({"enabled": true}))?;
            }
            Ok(text(format!("now {w}x{h}{}", if mobile { ", mobile" } else { "" })))
        }
        "browser_tabs" => {
            let v = call(p, who, Op::List)?;
            Ok(text(serde_json::to_string_pretty(&v).unwrap_or_default()))
        }
        "browser_show" => {
            let tab = tab_arg(args)?;
            call(p, who, Op::Show { tab })?;
            Ok(text(format!("tab {tab} is in front")))
        }
        "browser_handoff" => {
            let tab = tab_arg(args)?;
            let message = args["message"].as_str().unwrap_or("Please take over, then press Done.").to_string();
            call_for(p, who, Op::Handoff { tab, message }, Duration::from_secs(15 * 60)).map_err(|e| if e.contains("in time") { "the person did not press Done within 15 minutes".into() } else { e })?;
            wait_load(p, who, tab);
            Ok(text(format!("the person pressed Done\n\n{}", snapshot(p, who, tab, &json!({}))?)))
        }
        "browser_download" => {
            let url = args["url"].as_str().ok_or("missing url")?.to_string();
            call(p, who, Op::Download { url: url.clone(), tab: args["tab"].as_u64() })?;
            if !args["wait"].as_bool().unwrap_or(false) {
                return Ok(text("started; see browser_downloads".into()));
            }
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(600) {
                thread::sleep(Duration::from_millis(700));
                let list = call(p, who, Op::Downloads)?;
                if let Some(d) = list.as_array().and_then(|a| a.iter().find(|d| d["url"] == url.as_str())) {
                    match d["state"].as_str() {
                        Some("done") => return Ok(text(format!("downloaded {} ({} bytes)", d["path"].as_str().unwrap_or(""), d["size"]))),
                        Some("failed") => return Err(format!("download failed: {}", d["note"].as_str().unwrap_or(""))),
                        _ => {}
                    }
                }
            }
            Err("still downloading after 10 minutes; see browser_downloads".into())
        }
        "browser_downloads" => {
            let v = call(p, who, Op::Downloads)?;
            Ok(text(serde_json::to_string_pretty(&v).unwrap_or_default()))
        }
        "browser_close" => {
            call(p, who, Op::Close { tab: tab_arg(args)? })?;
            Ok(text("closed".into()))
        }
        _ => Err(format!("unknown tool {name}")),
    }
}

/// The page as an agent needs it: title, URL, then one line per heading and control.
fn snapshot(proxy: &EventLoopProxy<Ev>, who: &Who, tab: u64, args: &Value) -> Result<String, String> {
    let opt = json!({
        "scope": args["scope"].as_str().unwrap_or(""),
        "interactive": args["interactive"].as_bool().unwrap_or(false),
        "max": args["max_lines"].as_u64().unwrap_or(300).clamp(20, 2000),
    });
    let v = eval(proxy, who, tab, &SNAPSHOT_JS.replace("__OPT__", &opt.to_string()))?;
    if let Some(e) = v["error"].as_str() {
        return Err(e.to_string());
    }
    let mut out = format!("Title: {}\nURL: {}\n", v["title"].as_str().unwrap_or(""), v["url"].as_str().unwrap_or(""));
    let lines: Vec<&str> = v["lines"].as_array().map(|a| a.iter().filter_map(|l| l.as_str()).collect()).unwrap_or_default();
    if lines.is_empty() {
        out.push_str("\n(no controls or headings visible)");
    } else {
        out.push('\n');
        out.push_str(&lines.join("\n"));
    }
    if let Some(m) = v["more"].as_u64().filter(|m| *m > 0) {
        out.push_str(&format!("\n… {m} more elements; use scope or max_lines"));
    }
    if v["frames"].as_u64().is_some_and(|f| f > 0) {
        out.push_str(&format!("\n({} iframe(s) not looked inside)", v["frames"]));
    }
    Ok(out)
}

const SNAPSHOT_JS: &str = r#"(()=>{const O=__OPT__;
document.querySelectorAll('[data-lumen-ref]').forEach(e=>e.removeAttribute('data-lumen-ref'));
const refs=new Map();window.__lumenRefs=refs;let n=0,more=0,frames=0;
const vis=e=>{const b=e.getBoundingClientRect();if(b.width<1&&b.height<1)return false;const s=getComputedStyle(e);return s.visibility!=='hidden'&&s.display!=='none'&&s.opacity!=='0'};
const tx=s=>(s||'').replace(/\s+/g,' ').trim();
const name=e=>{const a=e.getAttribute('aria-label');if(a)return tx(a);
 const lb=e.getAttribute('aria-labelledby');if(lb){const t=lb.split(/\s+/).map(i=>document.getElementById(i)).filter(Boolean).map(x=>x.innerText).join(' ');if(tx(t))return tx(t)}
 if(e.labels&&e.labels.length){const t=[...e.labels].map(l=>l.innerText).join(' ');if(tx(t))return tx(t)}
 if(e.tagName==='IMG')return tx(e.alt);
 if(e.tagName!=='SELECT'&&e.tagName!=='TEXTAREA'&&e.tagName!=='INPUT'){const t=tx(e.innerText);if(t)return t;const i=e.querySelector&&e.querySelector('img[alt],svg[aria-label]');if(i)return tx(i.getAttribute('alt')||i.getAttribute('aria-label'))}
 return tx(e.getAttribute('placeholder')||e.getAttribute('title')||e.getAttribute('name')||(e.type==='submit'||e.type==='button'?e.value:'')||'')};
const role=e=>{const r=e.getAttribute('role');const t=e.tagName.toLowerCase();if(r&&r!=='presentation'&&r!=='none'&&r!=='generic')return r;
 if(t==='a')return e.hasAttribute('href')?'link':null;if(t==='button'||t==='summary')return 'button';
 if(t==='select')return e.multiple?'listbox':'combobox';if(t==='textarea')return 'textbox';
 if(t==='input'){const y=(e.type||'text').toLowerCase();if(y==='hidden')return null;return {checkbox:'checkbox',radio:'radio',button:'button',submit:'button',reset:'button',image:'button',range:'slider',file:'file',search:'searchbox'}[y]||'textbox'}
 if(e.isContentEditable&&e.getAttribute('contenteditable')!==null)return 'textbox';
 if(/^h[1-6]$/.test(t))return 'heading';if(t==='iframe'){frames++;return null}
 if(t==='img')return e.alt?'img':null;
 if(e.hasAttribute('onclick'))return 'clickable';return null};
const ACT=new Set(['link','button','textbox','searchbox','combobox','listbox','checkbox','radio','slider','file','switch','tab','menuitem','menuitemcheckbox','menuitemradio','option','treeitem','clickable','spinbutton']);
const lines=[];
const root=O.scope?document.querySelector(O.scope):document.body;if(!root)return {error:O.scope?'nothing matches '+O.scope:'the page has no body yet'};
const walk=(p,d)=>{for(const e of p.children){
 if(lines.length>=O.max){more++;continue}
 const r=role(e);let deeper=d;
 if(r&&(ACT.has(r)||(!O.interactive&&(r==='heading'||(r==='img'&&d<3))))&&vis(e)){
  const act=ACT.has(r);let s='  '.repeat(Math.min(d,8))+'- '+r;const nm=name(e).slice(0,100).replace(/"/g,"'");if(nm)s+=' "'+nm+'"';
  if(r==='heading')s+=' [h'+e.tagName[1]+']';
  if(act){n++;const id='e'+n;refs.set(id,e);e.setAttribute('data-lumen-ref',id);s+=' [ref='+id+']'}
  if(['checkbox','radio','switch','menuitemcheckbox'].includes(r)&&(e.checked||e.getAttribute('aria-checked')==='true'))s+=' checked';
  if(e.getAttribute('aria-expanded'))s+=' expanded='+e.getAttribute('aria-expanded');
  if(e.getAttribute('aria-selected')==='true')s+=' selected';
  if((r==='textbox'||r==='searchbox')&&e.type!=='password'&&e.value)s+=' value="'+String(e.value).slice(0,80).replace(/"/g,"'")+'"';
  if(r==='textbox'&&e.type==='password')s+=' password';
  if(r==='combobox'&&e.tagName==='SELECT')s+=' value="'+((e.selectedOptions[0]||{}).text||'').trim()+'" options: '+[...e.options].slice(0,15).map(o=>o.text.trim()).join(' | ')+(e.options.length>15?' | …':'');
  if(e.disabled||e.getAttribute('aria-disabled')==='true')s+=' disabled';
  if(r==='link'){const h=e.getAttribute('href');if(h&&!h.startsWith('javascript:')&&h!=='#')s+=' -> '+h.slice(0,120)}
  lines.push(s);deeper=d+1;
  if(act&&!['clickable','listbox','tab','menuitem','treeitem','option'].includes(r))continue;
 }
 if(e.shadowRoot)walk(e.shadowRoot,deeper);
 walk(e,deeper);
}};
walk(root,0);
return {title:document.title,url:location.href,lines,more,frames};})()"#;

const FOCUS_JS: &str = r#"e.scrollIntoView({block:'center'});e.focus();
if(document.activeElement!==e&&!e.contains(document.activeElement))return {error:R+' cannot take typing (not a field?)'};
if(!__APPEND__){if(e.isContentEditable){const s=getSelection(),g=document.createRange();g.selectNodeContents(e);s.removeAllRanges();s.addRange(g)}else if(e.select)e.select()}
else if(e.setSelectionRange&&typeof e.value==='string'){try{e.setSelectionRange(e.value.length,e.value.length)}catch(x){}}
return {ok:1};"#;

const SET_JS: &str = r#"const t=__TEXT__;if(e.isContentEditable){e.textContent=t}else{const d=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e),'value');if(d&&d.set)d.set.call(e,t);else e.value=t}
e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));return {ok:1};"#;

const SELECT_JS: &str = r#"if(e.tagName!=='SELECT')return {error:R+' is not a <select>; click it and pick from the list instead'};
const w=__OPT__;const o=[...e.options].find(o=>o.value===w)||[...e.options].find(o=>o.text.trim()===w)||[...e.options].find(o=>o.text.trim().toLowerCase().includes(w.toLowerCase()));
if(!o)return {error:'no option "'+w+'"; there are: '+[...e.options].map(o=>o.text.trim()).join(' | ')};
e.value=o.value;e.dispatchEvent(new Event('input',{bubbles:true}));e.dispatchEvent(new Event('change',{bubbles:true}));return {chosen:o.text.trim()};"#;

const WAIT_JS: &str = r#"(()=>{const c=__C__;const body=document.body?document.body.innerText:'';
if(c.text&&!body.includes(c.text))return false;if(c.gone&&body.includes(c.gone))return false;
if(c.sel&&!document.querySelector(c.sel))return false;if(c.url&&!location.href.includes(c.url))return false;return true})()"#;

const FIND_JS: &str = r#"(()=>{const q=__Q__.toLowerCase();let m=window.__lumenRefs,fresh=false;
const out={els:[],lines:[]};
if(!m||![...m.values()].some(e=>e.isConnected)){fresh=true}
const tx=s=>(s||'').replace(/\s+/g,' ').trim();
if(m)for(const [id,e] of m){if(!e.isConnected)continue;const n=tx(e.getAttribute('aria-label')||e.innerText||e.value||e.placeholder||e.title);if(n.toLowerCase().includes(q))out.els.push([id,(e.getAttribute('role')||e.tagName.toLowerCase()),n.slice(0,100)]);if(out.els.length>=20)break}
const seen=new Set();for(const l of (document.body?document.body.innerText:'').split('\n')){const t=tx(l);if(t&&t.toLowerCase().includes(q)&&!seen.has(t)){seen.add(t);out.lines.push(t.length>200?t.slice(0,200)+'…':t);if(out.lines.length>=25)break}}
out.fresh=fresh&&out.els.length===0;return out})()"#;

/// After `const a=<Readability extract>;` - the article as plain text with light markdown.
const ARTICLE_JS: &str = r#"const L=__LINKS__;
const md=(n)=>{let o='';for(const c of n.childNodes){if(c.nodeType===3){o+=c.textContent.replace(/\s+/g,' ');continue}if(c.nodeType!==1)continue;const t=c.tagName;
 if(/^H[1-6]$/.test(t))o+='\n\n'+'#'.repeat(+t[1])+' '+c.textContent.trim()+'\n\n';
 else if(['P','DIV','SECTION','ARTICLE','BLOCKQUOTE','FIGURE','TABLE','UL','OL','DL'].includes(t))o+='\n\n'+md(c)+'\n\n';
 else if(t==='LI')o+='\n- '+md(c).trim();else if(t==='TR')o+='\n'+md(c).trim();else if(t==='TD'||t==='TH')o+=' | '+md(c).trim();
 else if(t==='BR')o+='\n';else if(t==='PRE')o+='\n```\n'+c.textContent+'\n```\n';
 else if(t==='A'&&L){const h=c.getAttribute('href');const x=md(c).trim();o+=h&&x&&!h.startsWith('#')?'['+x+']('+h+')':x}
 else if(t==='IMG'||t==='SCRIPT'||t==='STYLE')continue;else o+=md(c)}return o};
const clean=s=>s.replace(/[ \t]+\n/g,'\n').replace(/\n{3,}/g,'\n\n').trim();
if(!a||!a.content)return {title:document.title,url:location.href,fallback:true,text:clean(document.body?document.body.innerText:'')};
const d=new DOMParser().parseFromString(a.content,'text/html');
return {title:a.title||document.title,url:location.href,byline:a.byline||'',text:clean(md(d.body))};"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_take_any_form() {
        assert_eq!(ref_arg(&json!("e5")).as_deref(), Some("e5"));
        assert_eq!(ref_arg(&json!("@e12")).as_deref(), Some("e12"));
        assert_eq!(ref_arg(&json!(7)).as_deref(), Some("e7"));
        assert_eq!(ref_arg(&json!("7")).as_deref(), Some("e7"));
        assert_eq!(ref_arg(&json!(null)), None);
        assert_eq!(ref_arg(&json!("")), None);
    }

    #[test]
    fn keys_become_devtools_events() {
        assert_eq!(key_spec("Enter").unwrap(), ("Enter".into(), "Enter".into(), 13, "\r".into(), 0));
        let (k, c, vk, t, m) = key_spec("Control+a").unwrap();
        assert_eq!((k.as_str(), c.as_str(), vk, t.as_str(), m), ("a", "KeyA", 65, "", 2));
        let (k, _, _, t, m) = key_spec("Shift+a").unwrap();
        assert_eq!((k.as_str(), t.as_str(), m), ("A", "A", 8));
        assert_eq!(key_spec("F5").unwrap().2, 116);
        assert_eq!(key_spec("Shift+Tab").unwrap().4, 8);
        assert!(key_spec("Hyper+x").is_err());
        assert!(key_spec("banana").is_err());
    }

    #[test]
    fn session_ids_are_long_random_and_only_issued_here() {
        let s = Sessions::new(Duration::from_secs(60));
        let now = Instant::now();
        let ids: Vec<String> = (0..20).map(|_| s.issue("claude-code", now).unwrap()).collect();
        for id in &ids {
            // 24 bytes = 192 bits, hex.
            assert_eq!(id.len(), SESSION_BYTES * 2);
            assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
            assert!(id.chars().any(|c| c != '0'), "never all zero");
        }
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "no repeats");
        // A name, a guess, an empty id and another agent's label are not sessions.
        for bad in ["claude-code", "default", "cli", "", "0000", &ids[0][..40]] {
            assert!(s.begin(bad, now).is_none(), "{bad:?} must not open a session");
        }
        assert_eq!(s.begin(&ids[0], now).as_deref(), Some("claude-code"));
    }

    #[test]
    fn a_label_is_only_a_label() {
        let s = Sessions::new(Duration::from_secs(60));
        let now = Instant::now();
        let a = s.issue("same-name", now).unwrap();
        let b = s.issue("same-name", now).unwrap();
        assert_ne!(a, b, "two clients with one name are two sessions");
        let c = s.issue("evil\n\tname with a very long tail that goes on and on and on", now).unwrap();
        let label = s.begin(&c, now).unwrap();
        assert!(label.chars().count() <= 40 && !label.chars().any(char::is_control), "{label:?}");
        assert_eq!(s.begin(&s.issue("", now).unwrap(), now).as_deref(), Some("agent"));
    }

    #[test]
    fn a_quiet_session_expires_a_busy_one_does_not() {
        let s = Sessions::new(Duration::from_secs(900));
        let t0 = Instant::now();
        let quiet = s.issue("quiet", t0).unwrap();
        let busy = s.issue("waiting", t0).unwrap();
        let chatty = s.issue("chatty", t0).unwrap();
        s.begin(&busy, t0).unwrap(); // a handoff, still waiting for the person
        // Fourteen minutes: nobody has gone yet.
        assert!(s.expire(t0 + Duration::from_secs(14 * 60)).is_empty());
        // The chatty one speaks again at minute 14 and finishes.
        s.begin(&chatty, t0 + Duration::from_secs(14 * 60)).unwrap();
        s.finish(&chatty, t0 + Duration::from_secs(14 * 60));
        // Minute 16: the quiet one is gone; the busy and the chatty are not.
        let gone = s.expire(t0 + Duration::from_secs(16 * 60));
        assert_eq!(gone, vec![(quiet.clone(), "quiet".to_string())]);
        assert!(s.begin(&quiet, t0).is_none(), "an expired id is unknown from then on");
        // The handoff ends at minute 16. The chatty one, last heard at 14, goes after 29;
        // the other counts its quiet from 16, so it is still here at 30.
        s.finish(&busy, t0 + Duration::from_secs(16 * 60));
        let gone: Vec<String> = s.expire(t0 + Duration::from_secs(30 * 60)).into_iter().map(|(id, _)| id).collect();
        assert_eq!(gone, vec![chatty]);
        let gone: Vec<String> = s.expire(t0 + Duration::from_secs(32 * 60)).into_iter().map(|(id, _)| id).collect();
        assert_eq!(gone, vec![busy]);
    }

    #[test]
    fn goodbye_and_a_shut_door_forget_sessions() {
        let s = Sessions::new(Duration::from_secs(60));
        let now = Instant::now();
        let a = s.issue("a", now).unwrap();
        let b = s.issue("b", now).unwrap();
        assert_eq!(s.remove(&a).as_deref(), Some("a"));
        assert!(s.remove(&a).is_none(), "second goodbye finds nothing");
        s.clear();
        assert!(s.begin(&b, now).is_none());
    }

    #[test]
    fn there_is_a_limit_to_sessions() {
        let s = Sessions::new(Duration::from_secs(60));
        let now = Instant::now();
        for _ in 0..MAX_SESSIONS {
            s.issue("x", now).unwrap();
        }
        assert!(s.issue("one more", now).is_err());
    }

    #[test]
    fn host_and_origin_must_be_exactly_this_door() {
        assert!(origin_ok("127.0.0.1:47615", "", 47615));
        assert!(origin_ok("localhost:47615", "http://localhost:47615", 47615));
        assert!(origin_ok("127.0.0.1:47615", "http://127.0.0.1:47615", 47615));
        for host in ["evil.com", "evil.com:47615", "127.0.0.1", "127.0.0.1:1", "127.0.0.1:47615.evil.com", ""] {
            assert!(!origin_ok(host, "", 47615), "host {host:?}");
        }
        for origin in ["http://evil.com", "null", "https://127.0.0.1:47615", "http://evil-127.0.0.1:47615", "http://127.0.0.1:47615.evil.com", "http://127.0.0.1:47615/x"] {
            assert!(!origin_ok("127.0.0.1:47615", origin, 47615), "origin {origin:?}");
        }
    }

    #[test]
    fn every_tool_has_a_schema() {
        let t = tools();
        let names: Vec<&str> = t.as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
        assert!(names.len() >= 30);
        for n in &names {
            assert!(n.starts_with("browser_"));
        }
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "no duplicate tool names");
    }
}
