//! The agent door, end to end: real (headless) Lumens, real HTTP.
//!
//! Slow, and it starts browsers, so it is ignored by a plain `cargo test`:
//!
//!     cargo test -j 2 --test agent_door -- --ignored --nocapture
//!
//! What it starts, one after the other (never two at once):
//!   1. a Lumen with the agent switch off,
//!   2. a Lumen with the defaults and a short session timeout, where most of the checks run,
//!   3. a Lumen with uploads on and the profile shared.
//! Each has its own LUMEN_DATA and LUMEN_DOWNLOADS under %TEMP%, its own port, and a settings.json
//! of its own. Pages come from a tiny listener in this file, never from the internet. Only
//! processes this test started are ever killed.
//!
//! The debug build of Lumen has four test tools (browser_test_crash, _done, _state, _grant). They
//! stand in for what a person does with a mouse, and a release build does not have them.

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const LUMEN: &str = env!("CARGO_BIN_EXE_lumen");
const LUMEN_AGENT: &str = env!("CARGO_BIN_EXE_lumen-agent");

// -------------------------------------------------------------------- helpers

fn say(s: &str) {
    eprintln!("  {s}");
}

/// A step's title, with the seconds since the test began.
fn head(s: &str) {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    eprintln!("[{:>4} s] {s}", START.get_or_init(Instant::now).elapsed().as_secs());
}

fn poll(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        if f() {
            return;
        }
        thread::sleep(Duration::from_millis(400));
    }
    panic!("gave up waiting for {what} after {secs} s");
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lumen-door-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Resp {
    code: u16,
    headers: HashMap<String, String>,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// One HTTP request, one response. Whatever the server does with the connection afterwards.
fn http(port: u16, head: &str, body: &str, declared_len: Option<usize>) -> Resp {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("the door is not listening");
    s.set_read_timeout(Some(Duration::from_secs(120))).ok();
    let len = declared_len.unwrap_or(body.len());
    let req = format!("{head}Content-Length: {len}\r\nConnection: close\r\n\r\n{body}");
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let mut lines = head.lines();
    let code = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).unwrap_or(0);
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    Resp { code, headers, body: body.to_string() }
}

/// The door of one Lumen: where it listens and the token it wants.
#[derive(Clone)]
struct Door {
    port: u16,
    token: String,
}

impl Door {
    fn head(&self, method: &str, path: &str, host: &str, auth: Option<&str>, origin: Option<&str>, sid: Option<&str>) -> String {
        let mut h = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\n");
        if let Some(a) = auth {
            h += &format!("Authorization: {a}\r\n");
        }
        if let Some(o) = origin {
            h += &format!("Origin: {o}\r\n");
        }
        if let Some(s) = sid {
            h += &format!("Mcp-Session-Id: {s}\r\n");
        }
        h
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    /// A well-formed POST with the right token, optionally in a session.
    fn post(&self, sid: Option<&str>, body: &Value) -> Resp {
        let head = self.head("POST", "/mcp", &self.host(), Some(&self.auth()), None, sid);
        http(self.port, &head, &body.to_string(), None)
    }

    fn rpc(&self, sid: Option<&str>, method: &str, params: Value) -> Resp {
        self.post(sid, &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
    }

    /// initialize, with a label, and the session id Lumen answers with.
    fn init(&self, label: &str) -> String {
        let r = self.rpc(None, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": label, "version": "1"}}));
        assert_eq!(r.code, 200, "initialize: {}", r.body);
        r.headers.get("mcp-session-id").cloned().expect("initialize gave no Mcp-Session-Id")
    }

    /// A tool call. Ok(text) or Err(the tool's error text). A transport failure is a panic.
    fn tool(&self, sid: &str, name: &str, args: Value) -> Result<String, String> {
        let began = Instant::now();
        let r = self.rpc(Some(sid), "tools/call", json!({"name": name, "arguments": args}));
        if began.elapsed() > Duration::from_secs(8) {
            eprintln!("  (slow: {name} took {} s)", began.elapsed().as_secs());
        }
        assert_eq!(r.code, 200, "{name} answered HTTP {}: {}", r.code, r.body);
        let v = r.json();
        let text: String = v["result"]["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        if v["result"]["isError"] == true { Err(text) } else { Ok(text) }
    }

    fn ping(&self, sid: &str) -> u16 {
        self.rpc(Some(sid), "ping", json!({})).code
    }

    /// Open a page in a new tab of this session, in the background. The tab id.
    fn open(&self, sid: &str, url: &str) -> u64 {
        let t = self.tool(sid, "browser_open", json!({"url": url, "background": true})).unwrap_or_else(|e| panic!("open {url}: {e}"));
        t.lines().next().and_then(|l| l.strip_prefix("tab ")).and_then(|n| n.trim().parse().ok()).unwrap_or_else(|| panic!("no tab id in: {t}"))
    }

    /// The tabs this session can see, as browser_tabs reports them.
    fn tabs(&self, sid: &str) -> Vec<Value> {
        let t = self.tool(sid, "browser_tabs", json!({})).expect("browser_tabs");
        serde_json::from_str::<Value>(&t).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default()
    }

    fn has_tab(&self, sid: &str, tab: u64) -> bool {
        self.tabs(sid).iter().any(|t| t["tab"] == tab)
    }

    /// Every tab in the browser, whoever's. Test tool.
    fn state(&self, sid: &str) -> Vec<Value> {
        let t = self.tool(sid, "browser_test_state", json!({})).expect("browser_test_state");
        serde_json::from_str::<Value>(&t).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default()
    }
}

/// A Lumen this test started. It dies with the value.
struct Lumen {
    child: Child,
    dir: PathBuf,
    downloads: PathBuf,
    /// aria2c.exe was put in place, so downloads should work here.
    aria_seeded: bool,
    door: Door,
}

impl Lumen {
    /// Settings go in settings.json (agent and agent_port are filled in); `tabs` are restored as
    /// the person's own tabs; `env` is extra environment.
    fn start(name: &str, port: u16, mut settings: Value, tabs: &[String], env: &[(&str, &str)], seed_aria: bool) -> Lumen {
        let dir = tmp(name);
        let downloads = dir.join("dl");
        std::fs::create_dir_all(&downloads).unwrap();
        // aria2 is fetched on first start, over the internet. A test needs none of that: if the
        // person's copy of the program is there, put a copy of just the program in this folder.
        let mut aria_seeded = false;
        if seed_aria && let Some(exe) = dirs::data_dir().map(|p| p.join("Lumen").join("aria2").join("aria2c.exe")).filter(|p| p.is_file()) {
            let to = dir.join("aria2");
            aria_seeded = std::fs::create_dir_all(&to).is_ok() && std::fs::copy(&exe, to.join("aria2c.exe")).is_ok();
        }
        settings["agent_port"] = json!(port);
        std::fs::write(dir.join("settings.json"), settings.to_string()).unwrap();
        if !tabs.is_empty() {
            let saved: Vec<Value> = tabs.iter().map(|u| json!({"url": u, "title": "", "favicon": "", "pinned": false})).collect();
            std::fs::write(dir.join("session.json"), json!({"tabs": saved, "active": 0, "width": 1000.0, "height": 700.0}).to_string()).unwrap();
        }
        let mut cmd = Command::new(LUMEN);
        cmd.arg("--headless")
            .env("LUMEN_HEADLESS", "1")
            .env("LUMEN_DATA", &dir)
            .env("LUMEN_DOWNLOADS", &downloads)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("could not start lumen.exe");
        let mut l = Lumen { child, dir, downloads, aria_seeded, door: Door { port, token: String::new() } };
        l.wait_for_door();
        l
    }

    /// The token appears with the first start; the door answers (200 on, 503 off) when it is up.
    fn wait_for_door(&mut self) {
        let start = Instant::now();
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!("lumen.exe exited early: {status}");
            }
            if let Ok(t) = std::fs::read_to_string(self.dir.join("agent_token")) {
                self.door.token = t.trim().to_string();
            }
            if !self.door.token.is_empty() && TcpStream::connect(("127.0.0.1", self.door.port)).is_ok() {
                let r = self.door.rpc(None, "ping", json!({}));
                if r.code == 200 || r.code == 503 {
                    return;
                }
            }
            assert!(start.elapsed() < Duration::from_secs(90), "the door did not open in 90 s");
            thread::sleep(Duration::from_millis(300));
        }
    }

    fn state_file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Lumen {
    fn drop(&mut self) {
        // Ours, started above: its path is inside this worktree's target folder.
        let _ = self.child.kill();
        let _ = self.child.wait();
        thread::sleep(Duration::from_millis(1500));
        if std::env::var_os("DOOR_KEEP").is_some() {
            eprintln!("  kept {}", self.dir.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

// ------------------------------------------------------------------ the site

/// What the test pages saw: (page's name for itself, Cookie header) per probe.
type Probes = Arc<Mutex<Vec<(String, String)>>>;

/// Two origins (two ports) serving the same few pages. Returns the ports and the probe log.
fn site() -> (u16, u16, Probes) {
    let probes: Probes = Arc::new(Mutex::new(Vec::new()));
    let mut ports = Vec::new();
    for _ in 0..2 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        ports.push(listener.local_addr().unwrap().port());
        let probes = probes.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let probes = probes.clone();
                thread::spawn(move || serve_page(stream, &probes));
            }
        });
    }
    (ports[0], ports[1], probes)
}

fn serve_page(mut s: TcpStream, probes: &Probes) {
    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut buf = [0u8; 8192];
    let n = s.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]).to_string();
    let target = req.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let role = query.split('&').find_map(|kv| kv.strip_prefix("role=").or_else(|| kv.strip_prefix("from="))).unwrap_or("").to_string();
    let cookie = req.lines().find_map(|l| l.strip_prefix("Cookie: ").or_else(|| l.strip_prefix("cookie: "))).unwrap_or("").trim().to_string();
    let mut extra = String::new();
    let (ctype, body): (&str, Vec<u8>) = match path {
        "/page" => {
            // Each role sets a cookie of its own and keeps asking /probe, so the log shows
            // which cookies a page of each profile carries, before and after the other one.
            match role.as_str() {
                "normal" => extra += "Set-Cookie: n=1; Path=/\r\n",
                "agent" => extra += "Set-Cookie: a=1; Path=/\r\n",
                _ => {}
            }
            (
                "text/html",
                format!("<title>Page {role}</title><h1>Page {role}</h1><script>setInterval(()=>fetch('/probe?from={role}',{{credentials:'include'}}),400)</script>").into_bytes(),
            )
        }
        "/probe" => {
            probes.lock().unwrap().push((role, cookie));
            ("text/plain", b"ok".to_vec())
        }
        "/plain" => ("text/html", b"<title>Plain page</title><h1>Hello</h1><button>Go</button>".to_vec()),
        "/upload" => ("text/html", b"<title>Upload page</title><input type=file id=f name=f aria-label=Pick><div id=out></div>".to_vec()),
        "/file.bin" => {
            extra += "Content-Disposition: attachment; filename=\"file.bin\"\r\n";
            ("application/octet-stream", vec![7u8; 4096])
        }
        _ => ("text/plain", b"not found".to_vec()),
    };
    let code = if matches!(path, "/page" | "/probe" | "/plain" | "/upload" | "/file.bin") { "200 OK" } else { "404 Not Found" };
    let head = format!("HTTP/1.1 {code}\r\nContent-Type: {ctype}\r\n{extra}Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(head.as_bytes());
    let _ = s.write_all(&body);
}

// ------------------------------------------------------------------ scenarios

fn random_hex48() -> String {
    // Not secret, only unlike any real id: time and a counter, stretched.
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("{n:024x}{:024x}", n.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

#[test]
#[ignore = "starts real headless Lumens; run with --ignored"]
fn the_agent_door() {
    let (p1, p2, probes) = site();
    let page = |port: u16, path: &str| format!("http://127.0.0.1:{port}{path}");

    // --------------------------------------------------------------- 1. switch off
    head("1. agent switch off");
    {
        // A Lumen whose switch is off listens only if it was on once in this run: the debug hook
        // opens the door and shuts it again, as turning the switch off in Settings does.
        let off = Lumen::start("off", 47631, json!({"agent": true}), &[], &[("LUMEN_TEST_AGENT_OFF", "1")], false);
        let d = &off.door;
        let init = d.rpc(None, "initialize", json!({"protocolVersion": "2025-06-18", "clientInfo": {"name": "x"}}));
        assert_eq!(init.code, 503, "initialize with the door shut");
        assert_eq!(d.rpc(Some("whatever"), "tools/list", json!({})).code, 503, "tools/list with the door shut");
        assert_eq!(d.rpc(Some("whatever"), "tools/call", json!({"name": "browser_tabs", "arguments": {}})).code, 503, "tools/call with the door shut");
        assert_eq!(d.rpc(None, "ping", json!({})).code, 503);
        say("initialize, tools/list, tools/call and ping all get 503");
        // Wrong token and shut door: the shut door speaks first, and says nothing about the token.
        let head = d.head("POST", "/mcp", &d.host(), Some("Bearer nope"), None, None);
        assert_eq!(http(d.port, &head, "{}", None).code, 503);
    }

    // ------------------------------------------------------- 2. the defaults, short timeout
    let normal_tab = page(p1, "/page?role=normal");
    let lumen = Lumen::start("main", 47632, json!({"agent": true}), &[normal_tab], &[("LUMEN_AGENT_SESSION_SECS", "10")], true);
    let d = lumen.door.clone();

    head("2. the door: token, host, origin");
    {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
        let none = http(d.port, &d.head("POST", "/mcp", &d.host(), None, None, None), &body, None);
        assert_eq!(none.code, 401, "no token");
        let wrong = http(d.port, &d.head("POST", "/mcp", &d.host(), Some(&format!("Bearer {}", "0".repeat(d.token.len()))), None, None), &body, None);
        assert_eq!(wrong.code, 401, "wrong token, same length");
        let short = http(d.port, &d.head("POST", "/mcp", &d.host(), Some("Bearer x"), None, None), &body, None);
        assert_eq!(short.code, 401, "wrong token, short");
        let basic = http(d.port, &d.head("POST", "/mcp", &d.host(), Some(&d.token), None, None), &body, None);
        assert_eq!(basic.code, 401, "the token without 'Bearer'");
        say("missing or wrong token: 401");
        for host in ["evil.example", &format!("evil.example:{}", d.port), "127.0.0.1", &format!("127.0.0.1:{}.evil.example", d.port), "localhost"] {
            let r = http(d.port, &d.head("POST", "/mcp", host, Some(&d.auth()), None, None), &body, None);
            assert_eq!(r.code, 403, "Host {host}");
        }
        for origin in ["http://evil.example", "null", &format!("http://evil-127.0.0.1:{}", d.port), &format!("https://127.0.0.1:{}", d.port)] {
            let r = http(d.port, &d.head("POST", "/mcp", &d.host(), Some(&d.auth()), Some(origin), None), &body, None);
            assert_eq!(r.code, 403, "Origin {origin}");
        }
        let own_origin = format!("http://127.0.0.1:{}", d.port);
        let r = http(d.port, &d.head("POST", "/mcp", &d.host(), Some(&d.auth()), Some(&own_origin), None), &body, None);
        assert_eq!(r.code, 200, "the door's own origin is fine");
        say("bad Host or Origin: 403 (even with the right token)");
        assert_eq!(http(d.port, &d.head("POST", "/elsewhere", &d.host(), Some(&d.auth()), None, None), &body, None).code, 404);
        assert_eq!(http(d.port, &d.head("GET", "/mcp", &d.host(), Some(&d.auth()), None, None), "", None).code, 405);
        assert_eq!(http(d.port, &d.head("POST", "/mcp", &d.host(), Some(&d.auth()), None, None), "", Some(2_000_000)).code, 413);
        assert_eq!(http(d.port, &d.head("POST", "/mcp", &d.host(), Some(&d.auth()), None, None), "{not json", None).code, 400);
    }

    head("3. sessions are Lumen's to hand out");
    let a = d.init("claude-code");
    let b = d.init("claude-code");
    let alive = Alive::new(&d);
    alive.add(&a);
    alive.add(&b);
    {
        assert_ne!(a, b, "same label, two sessions");
        for id in [&a, &b] {
            assert!(id.len() >= 32 && id.chars().all(|c| c.is_ascii_hexdigit()), "session id {id}");
        }
        // Not a session: no header, a label, a guess, a well-formed stranger.
        let no_header = d.rpc(None, "tools/list", json!({}));
        assert_eq!(no_header.code, 400, "{}", no_header.body);
        assert_eq!(d.rpc(None, "ping", json!({})).code, 200, "ping needs no session");
        for fake in ["claude-code", "default", "cli", "cli-t1", &random_hex48(), &a[..a.len() - 1]] {
            let r = d.rpc(Some(fake), "tools/list", json!({}));
            assert_eq!(r.code, 404, "session {fake:?}: {}", r.body);
            assert!(r.json()["error"]["message"].as_str().unwrap_or("").contains("session"), "an MCP error body: {}", r.body);
            assert_eq!(d.rpc(Some(fake), "tools/call", json!({"name": "browser_tabs", "arguments": {}})).code, 404);
            assert_eq!(d.rpc(Some(fake), "ping", json!({})).code, 404);
            let head = d.head("DELETE", "/mcp", &d.host(), Some(&d.auth()), None, Some(fake));
            assert_eq!(http(d.port, &head, "", None).code, 404, "DELETE {fake:?}");
        }
        say("unknown session ids: 404 with an MCP error (tools/list, tools/call, ping, DELETE)");
        let t = d.rpc(Some(&a), "tools/list", json!({}));
        assert_eq!(t.code, 200);
        assert!(t.json()["result"]["tools"].as_array().is_some_and(|t| t.len() >= 30));
    }

    head("4. one agent cannot reach another's tabs");
    let a_tab = d.open(&a, &page(p1, "/plain"));
    {
        assert!(d.has_tab(&a, a_tab));
        assert!(!d.has_tab(&b, a_tab), "B lists A's tab");
        assert_eq!(d.tabs(&b).len(), 0);
        for (tool, args) in [
            ("browser_snapshot", json!({"tab": a_tab})),
            ("browser_read", json!({"tab": a_tab})),
            ("browser_click", json!({"tab": a_tab, "ref": "e1"})),
            ("browser_type", json!({"tab": a_tab, "ref": "e1", "text": "x"})),
            ("browser_navigate", json!({"tab": a_tab, "url": "https://example.com"})),
            ("browser_eval", json!({"tab": a_tab, "script": "return 1"})),
            ("browser_screenshot", json!({"tab": a_tab})),
            ("browser_console", json!({"tab": a_tab})),
            ("browser_show", json!({"tab": a_tab})),
            ("browser_handoff", json!({"tab": a_tab, "message": "hi"})),
            ("browser_close", json!({"tab": a_tab})),
        ] {
            let e = d.tool(&b, tool, args).expect_err(&format!("B must not {tool} A's tab"));
            assert!(e.contains("no tab"), "{tool}: {e}");
        }
        let still = d.tool(&a, "browser_snapshot", json!({"tab": a_tab})).expect("A still has its tab");
        assert!(still.contains("Plain page"), "{still}");
        // Naming A's session in a new initialize does not make it A's.
        let head = d.head("POST", "/mcp", &d.host(), Some(&d.auth()), None, Some(&a));
        let init = http(d.port, &head, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"clientInfo": {"name": "claude-code"}}}).to_string(), None);
        let c = init.headers.get("mcp-session-id").cloned().unwrap();
        assert_ne!(c, a, "an initialize that names a session still gets a new one");
        assert!(!d.has_tab(&c, a_tab));
        say("B (and a client that named A's session) cannot list, read, click, type, navigate, eval, show, hand off or close A's tab");
    }

    head("5. eval: own tabs of the isolated profile only");
    {
        let t = d.tool(&a, "browser_eval", json!({"tab": a_tab, "script": "return document.title"})).expect("eval in own tab");
        assert!(t.contains("Plain page"), "{t}");
        let state = d.state(&a);
        let people: Vec<&Value> = state.iter().filter(|t| t["agent"] == false).collect();
        assert!(!people.is_empty(), "the restored tab of the person is there: {state:?}");
        for t in people {
            let e = d.tool(&a, "browser_eval", json!({"tab": t["id"], "script": "return document.cookie"})).expect_err("eval in the person's tab");
            assert!(e.contains("no tab") && e.contains("Let agents use this tab"), "{e}");
        }
        say("eval in a tab that is not the agent's: refused, and the error says how the person allows tabs");
    }

    head("6. upload is off by default; file: pages and magnets are not for agents");
    {
        let up = d.open(&a, &page(p1, "/upload"));
        let snap = d.tool(&a, "browser_snapshot", json!({"tab": up})).unwrap();
        let line = snap.lines().find(|l| l.contains("file") && l.contains("[ref=")).unwrap_or_else(|| panic!("no file input in: {snap}"));
        let file_ref = line.split("[ref=").nth(1).unwrap().trim_end_matches(']').to_string();
        let secret = lumen.dir.join("secret.txt");
        std::fs::write(&secret, "do not send").unwrap();
        let e = d.tool(&a, "browser_upload", json!({"tab": up, "ref": file_ref, "paths": [secret.display().to_string()]})).expect_err("upload while off");
        assert!(e.contains("agent_upload") && e.contains("Let agents upload files"), "{e}");
        say("browser_upload while off: refused, names the setting");
        for url in ["file:///C:/Windows/win.ini", "edge://settings", "about:config", "magnet:?xt=urn:btih:abc"] {
            let e = d.tool(&a, "browser_open", json!({"url": url})).expect_err(url);
            assert!(e.contains("does not open"), "{url}: {e}");
            let e = d.tool(&a, "browser_navigate", json!({"tab": up, "url": url})).expect_err(url);
            assert!(e.contains("does not open"), "{url}: {e}");
        }
        for url in ["magnet:?xt=urn:btih:abc", "file:///C:/Windows/win.ini", "ftp://example.com/x"] {
            let e = d.tool(&a, "browser_download", json!({"url": url})).expect_err(url);
            assert!(e.contains("http(s)"), "{url}: {e}");
        }
        say("file:, edge:, about:, magnet: pages and non-http downloads: refused");
        d.tool(&a, "browser_close", json!({"tab": up})).unwrap();
    }

    head("7. profile isolation: cookies do not cross");
    {
        poll("the person's page to ask /probe", 60, || probes.lock().unwrap().iter().any(|(who, c)| who == "normal" && c.contains("n=1")));
        let before = probes.lock().unwrap().len();
        let tab = d.open(&a, &page(p1, "/page?role=agent"));
        // A page in a hidden tab asks about once a second; wait for a few asks, not one.
        poll("the agent's page to ask /probe", 45, || probes.lock().unwrap().iter().filter(|(who, _)| who == "agent").count() >= 3);
        // Let the person's page keep asking after the agent's cookie exists.
        let seen = probes.lock().unwrap().len();
        poll("the person's page to ask again", 40, || {
            d.ping(&a);
            probes.lock().unwrap().iter().skip(seen).filter(|(who, _)| who == "normal").count() >= 2
        });
        let log = probes.lock().unwrap().clone();
        assert!(before > 0);
        let agent: Vec<_> = log.iter().filter(|(w, _)| w == "agent").collect();
        let person: Vec<_> = log.iter().filter(|(w, _)| w == "normal").collect();
        assert!(agent.iter().all(|(_, c)| c.contains("a=1") && !c.contains("n=1")), "an agent tab carried the person's cookie: {agent:?}");
        assert!(person.iter().all(|(_, c)| c.contains("n=1") && !c.contains("a=1")), "the person's tab carried the agent's cookie: {person:?}");
        let js = d.tool(&a, "browser_eval", json!({"tab": tab, "script": "return document.cookie"})).unwrap();
        assert!(js.contains("a=1") && !js.contains("n=1"), "document.cookie in the agent tab: {js}");
        say(&format!("{} agent probes carried a=1 only; {} person probes carried n=1 only", agent.len(), person.len()));
        d.tool(&a, "browser_close", json!({"tab": tab})).unwrap();
    }

    head("8. the person's tab: nothing until granted, then steering but never scripting");
    {
        let state = d.state(&a);
        let person = state.iter().find(|t| t["agent"] == false && t["url"].as_str().unwrap_or("").contains("role=normal")).expect("the person's tab").clone();
        let n = person["id"].as_u64().unwrap();
        let e = d.tool(&a, "browser_snapshot", json!({"tab": n})).expect_err("not granted yet");
        assert!(e.contains("no tab"), "{e}");
        let g = d.tool(&a, "browser_test_grant", json!({"tab": n})).expect("grant");
        assert!(g.contains("\"granted\":true"), "{g}");
        let snap = d.tool(&a, "browser_snapshot", json!({"tab": n})).expect("a granted tab can be read");
        assert!(snap.contains("Page normal"), "{snap}");
        assert!(d.tabs(&a).iter().any(|t| t["tab"] == n && t["yours"] == false), "a granted tab shows up in browser_tabs, marked not yours");
        for (tool, args, want) in [
            ("browser_eval", json!({"tab": n, "script": "return document.cookie"}), "never in the person's tab"),
            ("browser_upload", json!({"tab": n, "ref": "e1", "paths": ["x"]}), "never in the person's tab"),
            ("browser_close", json!({"tab": n}), "never in the person's tab"),
            ("browser_resize", json!({"tab": n, "device": "mobile"}), "never in the person's tab"),
        ] {
            let e = d.tool(&a, tool, args).expect_err(tool);
            assert!(e.contains(want), "{tool}: {e}");
        }
        assert!(d.tool(&a, "browser_find", json!({"tab": n, "text": "Page"})).is_ok());
        say("granted tab: snapshot and find work; eval, upload, close, resize are refused");
        // Another site ends the grant.
        let _ = d.tool(&a, "browser_navigate", json!({"tab": n, "url": page(p2, "/plain")}));
        poll("the grant to end", 20, || d.state(&a).iter().any(|t| t["id"] == n && t["granted"] == false && t["url"].as_str().unwrap_or("").contains(&p2.to_string())));
        let e = d.tool(&a, "browser_snapshot", json!({"tab": n})).expect_err("the grant ended with the site");
        assert!(e.contains("no tab"), "{e}");
        say("moving the tab to another site ended the grant");
        // And the person can take it back by hand.
        d.tool(&a, "browser_test_grant", json!({"tab": n})).unwrap();
        assert!(d.tool(&a, "browser_snapshot", json!({"tab": n})).is_ok());
        d.tool(&a, "browser_test_grant", json!({"tab": n})).unwrap();
        assert!(d.tool(&a, "browser_snapshot", json!({"tab": n})).is_err());
    }

    head("9. handoff returns the same tab");
    {
        let h = d.init("handoff");
        let tab = d.open(&h, &page(p1, "/plain"));
        let waiter = {
            let (d, h) = (d.clone(), h.clone());
            thread::spawn(move || d.tool(&h, "browser_handoff", json!({"tab": tab, "message": "log in please"})))
        };
        // Done is pressed once the handoff is really waiting.
        poll("the handoff to be waiting", 20, || d.tool(&b, "browser_test_done", json!({})).is_ok());
        let got = waiter.join().unwrap().expect("handoff");
        assert!(got.contains("the person pressed Done") && got.contains("Plain page") && got.contains(&format!(":{p1}/plain")), "{got}");
        assert!(d.has_tab(&h, tab));
        say("handoff waited, Done was pressed, the snapshot came from the same tab");
        d.tool(&h, "browser_close", json!({"tab": tab})).unwrap();
    }

    head("10. a crashed renderer is reported and recovers on reload");
    {
        let tab = d.open(&a, &page(p1, "/plain"));
        d.tool(&a, "browser_test_crash", json!({"tab": tab})).expect("crash hook");
        poll("the tab to report crashed", 20, || d.tabs(&a).iter().any(|t| t["tab"] == tab && t["crashed"] == true));
        let e = d.tool(&a, "browser_snapshot", json!({"tab": tab})).expect_err("a dead page has no snapshot");
        assert!(e.contains("crashed"), "{e}");
        let again = d.tool(&a, "browser_reload", json!({"tab": tab})).expect("reload brings it back");
        assert!(again.contains("Plain page"), "{again}");
        assert!(d.tabs(&a).iter().any(|t| t["tab"] == tab && t["crashed"] == false));
        say("Page.crash: reported crashed, snapshot says so, reload recovers the page");
        d.tool(&a, "browser_close", json!({"tab": tab})).unwrap();
    }

    head("11. pdf and downloads go to Agent downloads");
    {
        let tab = d.open(&a, &page(p1, "/plain"));
        let out = d.tool(&a, "browser_pdf", json!({"tab": tab})).expect("pdf");
        let path = out.strip_prefix("saved ").and_then(|s| s.rsplit_once(" (")).map(|(p, _)| p.to_string()).unwrap_or_else(|| panic!("{out}"));
        let want = lumen.downloads.join("Agent downloads");
        assert!(Path::new(&path).starts_with(&want), "{path} is not under {}", want.display());
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"%PDF"), "not a PDF");
        say(&format!("pdf: {path}"));
        d.tool(&a, "browser_close", json!({"tab": tab})).unwrap();
        let url = page(p1, "/file.bin");
        let started = d.tool(&a, "browser_download", json!({"url": url}));
        assert!(started.is_ok(), "{started:?}");
        let mut done = None;
        for _ in 0..80 {
            let list = d.tool(&a, "browser_downloads", json!({})).unwrap();
            let rows: Vec<Value> = serde_json::from_str::<Value>(&list).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
            if let Some(r) = rows.iter().find(|r| r["state"] == "done") {
                done = Some(r["path"].as_str().unwrap_or("").to_string());
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }
        match done {
            Some(p) => {
                assert!(Path::new(&p).starts_with(&want), "{p} is not under {}", want.display());
                say(&format!("download: {p}"));
            }
            None if lumen.aria_seeded => panic!("the download never finished although aria2c.exe was in place"),
            None => say("download: there is no aria2 here (none to copy), so the folder was not checked live; the unit tests cover the decision"),
        }
    }

    head("12. lumen-agent --session NAME keeps tabs without letting a name pick a session");
    {
        let run = |args: &[&str]| -> (i32, String) {
            let out = Command::new(LUMEN_AGENT).args(args).env("LUMEN_DATA", &lumen.dir).stdin(Stdio::null()).output().expect("lumen-agent");
            (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr))
        };
        let (code, out) = run(&["--session", "t1", "open", &format!("url={}", page(p1, "/plain")), "background=true"]);
        assert_eq!(code, 0, "{out}");
        let tab: u64 = out.lines().next().and_then(|l| l.strip_prefix("tab ")).and_then(|n| n.trim().parse().ok()).unwrap_or_else(|| panic!("{out}"));
        let (code, out) = run(&["--session", "t1", "tabs"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains(&format!("\"tab\": {tab}")), "the named session finds its tab again: {out}");
        let (_, other) = run(&["--session", "t2", "tabs"]);
        assert!(!other.contains(&format!("\"tab\": {tab}")), "another name must not see it: {other}");
        let file = lumen.state_file("agent_sessions").join("t1.id");
        let id = std::fs::read_to_string(&file).expect("the issued id is kept in a file").trim().to_string();
        assert!(id.len() >= 32 && id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        // The name itself is no key.
        for fake in ["cli-t1", "t1", "cli"] {
            assert_eq!(d.rpc(Some(fake), "tools/list", json!({})).code, 404, "{fake}");
        }
        // The file's id is the key: with it, the tab is there.
        assert!(d.has_tab(&id, tab));
        say("the CLI session lives in agent_sessions/t1.id; the name alone gets 404");
        let _ = d.tool(&id, "browser_close", json!({"tab": tab}));
    }

    head("13. a bridge that dies without goodbye: the session expires, a handoff in wait does not");
    {
        let x = d.init("killed-bridge");
        let keeper = d.init("keeper");
        alive.add(&keeper);
        let xt = d.open(&x, &page(p1, "/plain"));
        let h = d.init("waiting-handoff");
        let ht = d.open(&h, &page(p1, "/plain"));
        let waiter = {
            let (d, h) = (d.clone(), h.clone());
            thread::spawn(move || d.tool(&h, "browser_handoff", json!({"tab": ht, "message": "waiting"})))
        };
        // A real bridge (lumen-agent mcp) opens a tab, then goes quiet too.
        let mut bridge = Bridge::start(&lumen.dir);
        bridge.ask(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bridge-test", "version": "1"}}));
        bridge.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        let opened = bridge.call(2, "browser_open", json!({"url": page(p1, "/plain"), "background": true}));
        let bridge_tab: u64 = opened.lines().next().and_then(|l| l.strip_prefix("tab ")).and_then(|n| n.trim().parse().ok()).unwrap_or_else(|| panic!("{opened}"));
        thread::sleep(Duration::from_millis(1000));
        let tab_ids = |d: &Door| -> Vec<u64> { d.state(&keeper).iter().filter_map(|t| t["id"].as_u64()).collect() };
        assert!(tab_ids(&d).contains(&xt) && tab_ids(&d).contains(&bridge_tab));
        // X says nothing from here on, as if its bridge was killed. The keeper keeps asking.
        let start = Instant::now();
        poll("the silent session's tab to close", 60, || !tab_ids(&d).contains(&xt));
        let took = start.elapsed();
        assert!(took >= Duration::from_secs(5), "it expired too soon: {took:?}");
        assert_eq!(d.ping(&x), 404, "the expired session is unknown now");
        poll("the quiet bridge's tab to close as well", 30, || !tab_ids(&d).contains(&bridge_tab));
        // The bridge's next call finds its session gone, opens a new one and goes on.
        let after = bridge.call(3, "browser_tabs", json!({}));
        assert_eq!(after.trim(), "[]", "a fresh session has no tabs: {after}");
        let reopened = bridge.call(4, "browser_open", json!({"url": page(p1, "/plain"), "background": true}));
        assert!(reopened.starts_with("tab "), "{reopened}");
        say("a quiet bridge came back to a new session by itself (404, initialize again, retry)");
        drop(bridge);
        say(&format!("the silent session's tab closed after about {} s (limit 10 s in this run)", took.as_secs() + 1));
        // The handoff has waited longer than the limit by now, or soon will; give it the time.
        thread::sleep(Duration::from_secs(11).saturating_sub(start.elapsed()));
        assert!(tab_ids(&d).contains(&ht), "a session that waits on a handoff must keep its tab");
        assert_eq!(d.ping(&h), 200, "and its session");
        d.tool(&keeper, "browser_test_done", json!({})).expect("Done");
        let got = waiter.join().unwrap().expect("the handoff finished");
        assert!(got.contains("the person pressed Done"), "{got}");
        say("a handoff waiting longer than the limit kept its session and tab");
        // After Done the session is quiet like any other, and goes the same way.
        poll("the finished session's tab to close", 60, || !tab_ids(&d).contains(&ht));
    }

    head("14. denials are in the journal, without content");
    {
        let dir = lumen.state_file("journal");
        let secret_path = lumen.dir.join("secret.txt").display().to_string();
        poll("the journal to reach the disk", 120, || {
            read_journal(&dir).iter().any(|l| l["f"] == "agent" && l["m"].as_str().unwrap_or("").starts_with("denied browser_upload: upload_off"))
        });
        let lines = read_journal(&dir);
        let denials: Vec<&Value> = lines.iter().filter(|l| l["k"] == "error" && l["f"] == "agent").collect();
        assert!(denials.len() >= 3, "{denials:?}");
        for l in &denials {
            let m = l["m"].as_str().unwrap_or("");
            assert!(m.starts_with("denied browser_"), "{m}");
            assert!(!m.contains("secret") && !m.contains(&secret_path) && !m.contains("127.0.0.1") && !m.contains("win.ini"), "content in the journal: {m}");
        }
        let all: Vec<&str> = denials.iter().filter_map(|l| l["m"].as_str()).collect();
        say(&format!("{} denial lines, e.g. {:?}", all.len(), &all[..all.len().min(3)]));
        for want in ["denied browser_upload: upload_off", "denied browser_open: url_scheme", "denied browser_download: url_scheme"] {
            assert!(all.iter().any(|m| m.starts_with(want)), "{want} missing in {all:?}");
        }
    }
    drop(alive);
    drop(lumen);

    // ------------------------------------------------------- 3. uploads on, profile shared
    head("15. uploads on, profile shared");
    {
        let root = tmp("up-files");
        let up = root.join("uploads");
        let outside = root.join("outside");
        std::fs::create_dir_all(&up).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(up.join("ok.txt"), "fine").unwrap();
        std::fs::write(outside.join("secret.txt"), "not fine").unwrap();
        let junction = up.join("link");
        let made = Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&outside).output().map(|o| o.status.success()).unwrap_or(false);
        let settings = json!({"agent": true, "agent_upload": true, "agent_upload_dir": up.display().to_string(), "agent_shared_profile": true});
        let on = Lumen::start("upload", 47633, settings, &[], &[], false);
        let d = &on.door;
        let s = d.init("uploader");
        let tab = d.open(&s, &page(p1, "/upload"));
        let snap = d.tool(&s, "browser_snapshot", json!({"tab": tab})).unwrap();
        let line = snap.lines().find(|l| l.contains("file") && l.contains("[ref=")).unwrap_or_else(|| panic!("no file input in: {snap}"));
        let file_ref = line.split("[ref=").nth(1).unwrap().trim_end_matches(']').to_string();
        let try_upload = |path: String| d.tool(&s, "browser_upload", json!({"tab": tab, "ref": file_ref, "paths": [path]}));
        let outside_file = outside.join("secret.txt").display().to_string();
        let mut refused = vec![
            outside_file.clone(),
            format!("{}\\..\\outside\\secret.txt", up.display()),
            "..\\outside\\secret.txt".to_string(),
            format!("\\\\?\\{}", outside_file),
            "\\\\localhost\\c$\\Windows\\win.ini".to_string(),
            "C:\\Windows\\win.ini".to_string(),
            up.display().to_string(),
        ];
        if made {
            refused.push(format!("{}\\secret.txt", junction.display()));
            refused.push("link\\secret.txt".to_string());
        } else {
            say("could not make a junction here; that case was skipped");
        }
        for p in &refused {
            let e = try_upload(p.clone()).expect_err(p);
            assert!(e.contains("agent_upload_dir") && e.contains("inside"), "{p}: {e}");
        }
        say(&format!("{} paths outside the folder (.., UNC, verbatim, junction, the folder itself): all refused", refused.len()));
        let ok = try_upload(up.join("ok.txt").display().to_string()).expect("a file inside the folder");
        assert!(ok.contains("1 file"), "{ok}");
        // Shared profile: eval is refused and says which setting.
        let e = d.tool(&s, "browser_eval", json!({"tab": tab, "script": "return 1"})).expect_err("eval with a shared profile");
        assert!(e.contains("agent_shared_profile"), "{e}");
        // A relative name resolves inside the folder.
        try_upload("ok.txt".into()).expect("a name inside the folder");
        say("uploaded ok.txt (absolute and by name); eval refused under the shared profile");
        // The person's own tabs are still not the agent's, even with the shared profile.
        for t in d.state(&s).iter().filter(|t| t["agent"] == false) {
            assert!(d.tool(&s, "browser_snapshot", json!({"tab": t["id"]})).is_err(), "the shared profile must not open the person's tabs");
        }
        say("shared profile: the person's tabs stay shut");
    }
}

fn read_journal(dir: &Path) -> Vec<Value> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let text = std::fs::read_to_string(e.path()).unwrap_or_default();
        out.extend(text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()));
    }
    out
}

/// `lumen-agent mcp` as an agent's client runs it: JSON lines on stdin and stdout.
struct Bridge {
    child: Child,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Bridge {
    fn start(data: &Path) -> Bridge {
        let mut child = Command::new(LUMEN_AGENT)
            .arg("mcp")
            .env("LUMEN_DATA", data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("lumen-agent mcp");
        let out = child.stdout.take().unwrap();
        let (tx, lines) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for l in std::io::BufRead::lines(std::io::BufReader::new(out)).map_while(Result::ok) {
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Bridge { child, lines }
    }

    fn send(&mut self, msg: Value) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
    }

    /// A request and its answer (the next line).
    fn ask(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let line = self.lines.recv_timeout(Duration::from_secs(90)).unwrap_or_else(|_| panic!("the bridge did not answer {method}"));
        let v: Value = serde_json::from_str(&line).unwrap_or_else(|_| panic!("not JSON: {line}"));
        assert_eq!(v["id"], id, "{line}");
        v
    }

    /// A tool call; the text of its answer.
    fn call(&mut self, id: u64, tool: &str, args: Value) -> String {
        let v = self.ask(id, "tools/call", json!({"name": tool, "arguments": args}));
        assert!(v["error"].is_null() && v["result"]["isError"] != true, "{tool}: {v}");
        v["result"]["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Keeps sessions alive while the test waits on something slow. A session this test means to
/// let expire is never added.
struct Alive {
    ids: Arc<Mutex<Vec<String>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Alive {
    fn new(d: &Door) -> Alive {
        let ids: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (d, list, flag) = (d.clone(), ids.clone(), stop.clone());
        thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(2));
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let now: Vec<String> = list.lock().unwrap().clone();
                for id in now {
                    d.rpc(Some(&id), "ping", json!({}));
                }
            }
        });
        Alive { ids, stop }
    }

    fn add(&self, id: &str) {
        self.ids.lock().unwrap().push(id.to_string());
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
