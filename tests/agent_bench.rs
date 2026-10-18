//! The agent bench: fixed tasks on local pages, through the agent door, timed.
//!
//!     cargo test -j 2 --test agent_bench -- --ignored --nocapture
//!
//! There is no model in here. The number of tool calls a task takes stands in for the model's
//! round trips, and the reply bytes for what it has to read before each decision: those two are
//! where an agent's time goes, not the browser. Wall time is measured too.
//!
//! One headless Lumen, its own LUMEN_DATA under %TEMP%, pages from a listener in this file,
//! never the internet. Each task runs RUNS times in the same Lumen (warm). At most two tabs
//! are open at once. Only the Lumen this test started is ever killed.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

const LUMEN: &str = env!("CARGO_BIN_EXE_lumen");
const LUMEN_AGENT: &str = env!("CARGO_BIN_EXE_lumen-agent");
const RUNS: usize = 3;
const PORT: u16 = 47691;

// ---------------------------------------------------------------- the pages

fn pages(path: &str) -> String {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    match path {
        "/title" => "<title>Bench title</title><h1>Hello</h1><p>A plain page.</p>".into(),
        "/links" => {
            let mut b = String::from("<title>Links</title><nav><a href='/'>Home</a> <a href='/about'>About</a></nav><h1>A hundred links</h1><ul>");
            for i in 0..100 {
                b += &format!("<li><a href='/item/{i}'>Item number {i}</a></li>");
            }
            b + "</ul>"
        }
        "/form" => r#"<title>Order</title><h1>Order</h1><form action="/done">
<label>Name <input name="name"></label>
<label>Email <input name="email" type="email"></label>
<label>City <input name="city"></label>
<label>Notes <textarea name="notes"></textarea></label>
<label>Size <select name="size"><option>Small</option><option>Medium</option><option>Large</option></select></label>
<label><input type="checkbox" name="gift"> Gift wrap</label>
<button>Send order</button></form>"#
            .into(),
        "/done" => format!("<title>Done</title><h1>Thank you</h1><p>{}</p>", query.replace('&', " ")),
        "/table" => r#"<title>Prices</title><h1>Prices</h1><div id="t">loading…</div><script>
setTimeout(()=>{let h='<table><thead><tr><th>Name</th><th>Price</th><th>Stock</th></tr></thead><tbody>';
for(let i=0;i<50;i++)h+='<tr><td>Kettle '+i+'</td><td>'+(20+i)+'.99</td><td>'+(i%7)+'</td></tr>';
document.getElementById('t').innerHTML=h+'</tbody></table>'},300)</script>"#
            .into(),
        "/search" => r#"<title>Search</title><h1>Search the shop</h1><form action="/results"><input name="q" aria-label="Search"><button>Search</button></form>"#.into(),
        "/results" => {
            let mut b = String::from("<title>Results</title><h1>Results</h1><ol>");
            for i in 0..10 {
                b += &format!("<li><a href='/item/{i}'>Kettle model {i}</a> <span>{}.99</span></li>", 20 + i);
            }
            b + "</ol>"
        }
        p if p.starts_with("/item/") => {
            let n = p.trim_start_matches("/item/");
            format!(
                "<title>Kettle {n}</title><article><h1>Kettle model {n}</h1><p>Boils a litre in two minutes. Steel, with a light.</p>\
                 <p>Price: {n}9.99</p><button onclick=\"this.textContent='In the cart'\">Add to cart</button></article>"
            )
        }
        _ => "<title>Home</title><h1>Home</h1>".into(),
    }
}

fn serve_pages() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for s in listener.incoming().flatten() {
            thread::spawn(move || page(s));
        }
    });
    port
}

fn page(mut s: TcpStream) {
    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut buf = [0u8; 8192];
    let n = s.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
    let body = pages(&path);
    let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}

// ---------------------------------------------------------------- the door

struct Door {
    port: u16,
    token: String,
    sid: String,
    calls: usize,
    bytes: usize,
    /// Per tool: calls and milliseconds, over the whole bench.
    per: std::collections::BTreeMap<String, (usize, f64)>,
}

impl Door {
    fn post(&self, body: &Value, sid: &str) -> (String, String) {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).expect("door");
        s.set_read_timeout(Some(Duration::from_secs(120))).ok();
        let body = body.to_string();
        let mut head = format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\n", self.port, self.token);
        if !sid.is_empty() {
            head += &format!("Mcp-Session-Id: {sid}\r\n");
        }
        write!(s, "{head}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut raw = String::new();
        s.read_to_string(&mut raw).unwrap();
        let (h, b) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let sid = h.lines().find_map(|l| l.strip_prefix("Mcp-Session-Id: ").or_else(|| l.strip_prefix("mcp-session-id: "))).unwrap_or("").to_string();
        (sid, b.to_string())
    }

    fn init(&mut self) {
        let stamp = {
            let out = Command::new(LUMEN_AGENT).arg("--version").env("LUMEN_DATA", std::env::temp_dir()).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().trim_start_matches("lumen-agent ").to_string()
        };
        let (sid, _) = self.post(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "bench", "version": "1"}, "_meta": {"lumen": {"bridge": stamp}}}}),
            "",
        );
        assert!(!sid.is_empty(), "no session id");
        self.sid = sid;
    }

    /// One tool call: its text, counted.
    fn tool(&mut self, name: &str, args: Value) -> String {
        let t0 = Instant::now();
        let (_, body) = self.post(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name, "arguments": args}}), &self.sid.clone());
        self.calls += 1;
        let e = self.per.entry(name.to_string()).or_default();
        e.0 += 1;
        e.1 += t0.elapsed().as_secs_f64() * 1000.0;
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let text: String = v["result"]["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
        self.bytes += text.len();
        assert!(v["result"]["isError"] != true, "{name}: {text}");
        text
    }

    fn open(&mut self, url: &str) -> u64 {
        let t = self.tool("browser_open", json!({"url": url}));
        t.lines().next().and_then(|l| l.strip_prefix("tab ")).and_then(|n| n.trim().parse().ok()).expect("tab id")
    }

    fn close(&mut self, tab: u64) {
        // Cleanup, not part of a task: not counted.
        let (c, b) = (self.calls, self.bytes);
        self.tool("browser_close", json!({"tab": tab}));
        (self.calls, self.bytes) = (c, b);
    }
}

/// The ref of the first line of a snapshot that has all these words.
fn find_ref(snap: &str, words: &[&str]) -> String {
    let line = snap.lines().find(|l| words.iter().all(|w| l.contains(w)) && l.contains("[ref=")).unwrap_or_else(|| panic!("no {words:?} in:\n{snap}"));
    let at = line.find("[ref=").unwrap() + 5;
    line[at..].split(']').next().unwrap().to_string()
}

// ---------------------------------------------------------------- the tasks

type Task = fn(&mut Door, &str);

/// Read the title and URL of a page.
fn t_title(d: &mut Door, base: &str) {
    let tab = d.open(&format!("{base}/title"));
    let s = d.tool("browser_snapshot", json!({"tab": tab, "interactive": true}));
    assert!(s.contains("Bench title"));
    d.close(tab);
}

/// The hundred links of a page, text and URL.
fn t_links(d: &mut Door, base: &str) {
    let tab = d.open(&format!("{base}/links"));
    let s = d.tool("browser_eval", json!({"tab": tab, "script": "return [...document.links].map(a=>[a.textContent,a.href])"}));
    assert!(s.contains("Item number 99"), "{s}");
    d.close(tab);
}

/// Fill a form of five fields, a select and a checkbox, and send it.
fn t_form(d: &mut Door, base: &str) {
    let tab = d.open(&format!("{base}/form"));
    let s = d.tool("browser_snapshot", json!({"tab": tab, "interactive": true}));
    for (w, v) in [("Name", "Ada"), ("Email", "ada@example.com"), ("City", "London"), ("Notes", "Ring twice")] {
        d.tool("browser_type", json!({"tab": tab, "ref": find_ref(&s, &[w]), "text": v}));
    }
    d.tool("browser_select", json!({"tab": tab, "ref": find_ref(&s, &["combobox"]), "option": "Large"}));
    d.tool("browser_click", json!({"tab": tab, "ref": find_ref(&s, &["checkbox"])}));
    let out = d.tool("browser_click", json!({"tab": tab, "ref": find_ref(&s, &["Send order"])}));
    assert!(out.contains("/done"), "{out}");
    d.close(tab);
}

/// A table the page builds with a script after 300 ms, as rows.
fn t_table(d: &mut Door, base: &str) {
    let tab = d.open(&format!("{base}/table"));
    d.tool("browser_wait", json!({"tab": tab, "selector": "table tr:nth-child(50)"}));
    let s = d.tool("browser_read", json!({"tab": tab, "max_chars": 20000}));
    assert!(s.contains("Kettle 49"), "{s}");
    d.close(tab);
}

/// Ten steps: search, open a result, read it, add it to the cart, go back, open another.
fn t_workflow(d: &mut Door, base: &str) {
    let tab = d.open(&format!("{base}/search"));
    let s = d.tool("browser_snapshot", json!({"tab": tab, "interactive": true}));
    d.tool("browser_type", json!({"tab": tab, "ref": find_ref(&s, &["Search\""]), "text": "kettle", "submit": true}));
    d.tool("browser_wait", json!({"tab": tab, "text": "Kettle model 3"}));
    let s = d.tool("browser_snapshot", json!({"tab": tab, "interactive": true}));
    d.tool("browser_click", json!({"tab": tab, "ref": find_ref(&s, &["Kettle model 3"])}));
    d.tool("browser_article", json!({"tab": tab}));
    let s = d.tool("browser_snapshot", json!({"tab": tab, "interactive": true}));
    d.tool("browser_click", json!({"tab": tab, "ref": find_ref(&s, &["Add to cart"])}));
    d.tool("browser_back", json!({"tab": tab}));
    let s = d.tool("browser_snapshot", json!({"tab": tab, "interactive": true}));
    let out = d.tool("browser_click", json!({"tab": tab, "ref": find_ref(&s, &["Kettle model 5"])}));
    assert!(out.contains("/item/5"), "{out}");
    d.close(tab);
}

/// Two pages read side by side.
fn t_two_tabs(d: &mut Door, base: &str) {
    let a = d.open(&format!("{base}/item/1"));
    let b = d.open(&format!("{base}/item/2"));
    let x = d.tool("browser_article", json!({"tab": a}));
    let y = d.tool("browser_article", json!({"tab": b}));
    assert!(x.contains("model 1") && y.contains("model 2"));
    d.close(a);
    d.close(b);
}

// ------------------------------------------------- the same tasks, with the newer tools

fn t_form_fill(d: &mut Door, base: &str) {
    let tab = d.open(&format!("{base}/form"));
    let s = d.tool("browser_snapshot", json!({"tab": tab, "interactive": true}));
    let f = |w: &[&str], v: &str| json!({"ref": find_ref(&s, w), "value": v});
    let fields = json!([f(&["Name"], "Ada"), f(&["Email"], "ada@example.com"), f(&["City"], "London"), f(&["Notes"], "Ring twice"),
        f(&["combobox"], "Large"), f(&["checkbox"], "true")]);
    let out = d.tool("browser_fill", json!({"tab": tab, "fields": fields, "submit": find_ref(&s, &["Send order"])}));
    assert!(out.contains("every one took") && out.contains("/done"), "{out}");
    d.close(tab);
}

fn t_links_tool(d: &mut Door, base: &str) {
    let tab = d.open(&format!("{base}/links"));
    let s = d.tool("browser_links", json!({"tab": tab, "max": 200}));
    assert!(s.contains("[Item number 99]"), "{s}");
    d.close(tab);
}

fn t_table_tool(d: &mut Door, base: &str) {
    let tab = d.open(&format!("{base}/table"));
    d.tool("browser_wait", json!({"tab": tab, "selector": "table tr:nth-child(50)"}));
    let s = d.tool("browser_table", json!({"tab": tab}));
    assert!(s.starts_with("50 rows") && s.contains("\"columns\":[\"Name\",\"Price\",\"Stock\"]") && s.contains("[\"Kettle 49\","), "{s}");
    d.close(tab);
}

fn t_workflow_script(d: &mut Door, base: &str) {
    let out = d.tool(
        "browser_script_run",
        json!({"name": "shop_compare", "input": {"start": format!("{base}/search"), "query": "kettle", "read": ["Kettle model 3", "Kettle model 5"], "buy": "Kettle model 3"}}),
    );
    assert!(out.contains("\"in_cart\": true") && out.contains("\"price\": \"59.99\""), "{out}");
    if out.lines().any(|l| l.split_whitespace().rev().nth(1).and_then(|n| n.parse::<u64>().ok()).is_some_and(|ms| ms > 1500)) {
        eprintln!("slow script run:\n{}", out.lines().take(20).collect::<Vec<_>>().join("\n"));
    }
}

fn t_two_tabs_batch(d: &mut Door, base: &str) {
    let out = d.tool(
        "browser_batch",
        json!({"result": "all", "steps": [
            {"tool": "browser_open", "args": {"url": format!("{base}/item/1")}},
            {"tool": "browser_open", "args": {"url": format!("{base}/item/2")}},
            {"tool": "browser_article", "args": {"tab": "$1"}},
            {"tool": "browser_article", "args": {"tab": "$2"}},
            {"tool": "browser_close", "args": {"tab": "$1"}},
            {"tool": "browser_close", "args": {"tab": "$2"}}]}),
    );
    assert!(out.contains("model 1") && out.contains("model 2") && !out.contains("ERR"), "{out}");
}

// ---------------------------------------------------------------- the run

struct KillLumen(Child);

impl Drop for KillLumen {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start() -> (KillLumen, PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("lumen-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dl")).unwrap();
    std::fs::write(dir.join("settings.json"), json!({"agent": true, "agent_port": PORT}).to_string()).unwrap();
    // The example scripts, as the person would put them in.
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    for e in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/scripts")).unwrap().flatten() {
        std::fs::copy(e.path(), dir.join("scripts").join(e.file_name())).unwrap();
    }
    let child = Command::new(LUMEN)
        .arg("--headless")
        .env("LUMEN_HEADLESS", "1")
        .env("LUMEN_DATA", &dir)
        .env("LUMEN_DOWNLOADS", dir.join("dl"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let lumen = KillLumen(child);
    let t0 = Instant::now();
    let token = loop {
        if let Ok(t) = std::fs::read_to_string(dir.join("agent_token"))
            && !t.trim().is_empty()
            && TcpStream::connect(("127.0.0.1", PORT)).is_ok()
        {
            break t.trim().to_string();
        }
        assert!(t0.elapsed() < Duration::from_secs(60), "no agent door");
        thread::sleep(Duration::from_millis(200));
    };
    (lumen, dir, token)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore = "starts a headless Lumen"]
fn bench() {
    let site = serve_pages();
    let base = format!("http://127.0.0.1:{site}");
    let (lumen, dir, token) = start();
    let mut d = Door { port: PORT, token, sid: String::new(), calls: 0, bytes: 0, per: Default::default() };
    d.init();
    let tasks: &[(&str, Task)] = &[
        ("read title and URL", t_title),
        ("extract 100 links", t_links),
        ("fill and send a form", t_form),
        ("table built by JS", t_table),
        ("ten-step workflow", t_workflow),
        ("two tabs, two reads", t_two_tabs),
        ("form, browser_fill", t_form_fill),
        ("links, browser_links", t_links_tool),
        ("table, browser_table", t_table_tool),
        ("workflow, Luau script", t_workflow_script),
        ("two tabs, browser_batch", t_two_tabs_batch),
    ];
    // Floor: what one call costs before any page work. ping is HTTP alone; browser_help
    // adds the policy round trip to the UI thread; browser_tabs adds a second one.
    for (what, name) in [("ping", ""), ("browser_help", "browser_help"), ("browser_tabs", "browser_tabs")] {
        let t0 = Instant::now();
        for _ in 0..20 {
            if name.is_empty() {
                d.post(&json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}), &d.sid.clone());
            } else {
                d.tool(name, json!({}));
            }
        }
        eprintln!("floor {what:<14} {:>6.1} ms a call", t0.elapsed().as_secs_f64() * 1000.0 / 20.0);
    }
    d.per.clear();
    let only: Option<String> = std::env::var("LUMEN_BENCH_ONLY").ok();
    let began = Instant::now();
    let mut rows = Vec::new();
    for (name, task) in tasks {
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let mut ms = Vec::new();
        let (mut calls, mut bytes) = (0, 0);
        for _ in 0..RUNS {
            (d.calls, d.bytes) = (0, 0);
            let t0 = Instant::now();
            task(&mut d, &base);
            ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            (calls, bytes) = (d.calls, d.bytes);
        }
        let (p50, p95) = (pct(&mut ms, 0.5), pct(&mut ms, 0.95));
        eprintln!("{name:<22} {p50:>6.0} ms p50 {p95:>6.0} ms p95 {calls:>3} calls {bytes:>7} bytes ~{:>5} tokens", bytes / 4);
        rows.push(format!("| {name} | {p50:.0} ms | {p95:.0} ms | {calls} | {bytes} | {} |", bytes / 4));
    }
    eprintln!("\n| Task | p50 | p95 | Calls | Reply bytes | ~Tokens |\n|---|---|---|---|---|---|\n{}", rows.join("\n"));
    for (tool, (n, ms)) in &d.per {
        eprintln!("  {tool:<22} {n:>4} calls {:>6.0} ms each", ms / *n as f64);
    }
    eprintln!("whole bench: {:.0} s", began.elapsed().as_secs_f64());
    drop(lumen);
    let _ = std::fs::remove_dir_all(&dir);
}
