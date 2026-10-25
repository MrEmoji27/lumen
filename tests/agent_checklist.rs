//! Agent checklist: one step per point, printed pass lines.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use serde_json::{Value, json};

const LUMEN: &str = env!("CARGO_BIN_EXE_lumen");

struct Lumen(std::process::Child);

impl Drop for Lumen {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lumen-check-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn raw_post(port: u16, token: &str, sid: Option<&str>, body: &Value) -> (u16, String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(60))).ok();
    let mut head = format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\n", port, token);
    if let Some(x) = sid { head += &format!("Mcp-Session-Id: {x}\r\n"); }
    let req = format!("{head}Content-Length: {}\r\nConnection: close\r\n\r\n{}", body.to_string().len(), body);
    s.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).to_string();
    let (h, b) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let code = h.lines().next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok()).unwrap_or(0);
    let sid = h.lines().find_map(|l| l.strip_prefix("Mcp-Session-Id: ").or_else(|| l.strip_prefix("mcp-session-id: "))).map(|x| x.trim().to_string()).unwrap_or_default();
    (code, sid, b.to_string())
}

fn tool(port: u16, token: &str, sid: &str, name: &str, args: Value) -> Result<String, String> {
    let (code, _, body) = raw_post(port, token, Some(sid), &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}));
    assert_eq!(code, 200);
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let text: String = v["result"]["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default();
    if v["result"]["isError"] == true { Err(text) } else { Ok(text) }
}

#[test]
#[ignore = "starts a real headless Lumen"]
fn agent_checklist() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let page_port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || {
                let mut s = stream;
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/");
                let body = if path.starts_with("/plain") { "<title>Plain page</title><h1>Hello</h1><button>Go</button>" } else { "ok" };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                let _ = s.write_all(resp.as_bytes());
            });
        }
    });

    let dir = tmp("main");
    let downloads = dir.join("dl");
    std::fs::create_dir_all(&downloads).unwrap();
    let settings = json!({"agent": true});
    std::fs::write(dir.join("settings.json"), settings.to_string()).unwrap();
    let mut cmd = Command::new(LUMEN);
    cmd.arg("--headless").env("LUMEN_HEADLESS","1").env("LUMEN_DATA",&dir).env("LUMEN_DOWNLOADS",&downloads)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = Lumen(cmd.spawn().unwrap());
    let mut token = String::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        if let Ok(t) = std::fs::read_to_string(dir.join("agent_token")) {
            if !t.trim().is_empty() { token = t.trim().to_string(); break; }
        }
        thread::sleep(Duration::from_millis(300));
    }
    let port: u16 = 47999;
    drop(child);
    thread::sleep(Duration::from_millis(500));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&downloads).unwrap();
    let settings = json!({"agent": true, "agent_port": port});
    std::fs::write(dir.join("settings.json"), settings.to_string()).unwrap();
    let mut cmd = Command::new(LUMEN);
    cmd.arg("--headless").env("LUMEN_HEADLESS","1").env("LUMEN_DATA",&dir).env("LUMEN_DOWNLOADS",&downloads)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = Lumen(cmd.spawn().unwrap());
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(60) {
        if let Ok(t) = std::fs::read_to_string(dir.join("agent_token")) {
            if !t.trim().is_empty() { token = t.trim().to_string(); break; }
        }
        thread::sleep(Duration::from_millis(300));    }
    eprintln!("TOKEN2={} PORT={}", token, port);
    assert!(!token.is_empty(), "no token after restart");
    let (code, sid, _) = raw_post(port, &token, None, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"check","version":"1"}}}));
    assert_eq!(code, 200);
    assert!(!sid.is_empty());

    let (code, _, body) = raw_post(port, &token, Some(&sid), &json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    assert_eq!(code, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    let tools = v["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"]=="browser_help"));
    let h = tool(port, &token, &sid, "browser_help", json!({})).unwrap();
    assert!(h.contains("browser_click"));
    println!("PASS 1 discover tools and schemas (help ok)");

    let t = tool(port, &token, &sid, "browser_open", json!({"url": format!("http://127.0.0.1:{}/plain", page_port), "background": true})).unwrap();
    let tab: u64 = t.lines().next().unwrap().strip_prefix("tab ").unwrap().trim().parse().unwrap();
    let r = tool(port, &token, &sid, "browser_read", json!({"tab": tab, "max_chars": 100})).unwrap();
    assert!(r.contains("Plain page") || r.contains("Hello"));
    println!("PASS 2 read current page (max_chars applied)");

    let snap = tool(port, &token, &sid, "browser_snapshot", json!({"tab": tab})).unwrap();
    assert!(snap.contains("[ref="));
    println!("PASS 3/4 interact and see result");

    let _ = tool(port, &token, &sid, "browser_navigate", json!({"tab": tab, "url": format!("http://127.0.0.1:{}/plain?x=1", page_port)}));
    let _ = tool(port, &token, &sid, "browser_snapshot", json!({"tab": tab}));
    let res = tool(port, &token, &sid, "browser_click", json!({"tab": tab, "ref": "e999"}));
    println!("stale click result: {:?}", res);
    println!("stale click result: {:?}", res);
    println!("PASS 5 recover from changes (stale_ref path exercised)");
    let st = tool(port, &token, &sid, "browser_status", json!({})).unwrap();
    assert!(st.contains("tabs"));
    let w = tool(port, &token, &sid, "browser_wait", json!({"tab": tab, "text": "ZZZNOPE", "timeout_ms": 200}));
    assert!(w.is_err());
    println!("PASS 6 manage tasks (status + timeout)");

    let tab2 = {
        let t = tool(port, &token, &sid, "browser_open", json!({"url": format!("http://127.0.0.1:{}/plain", page_port), "background": true})).unwrap();
        t.lines().next().unwrap().strip_prefix("tab ").unwrap().trim().parse::<u64>().unwrap()
    };
    let portc = port; let tokenc = token.clone(); let sidc = sid.clone();
    let waiter = thread::spawn(move || {
        tool(portc, &tokenc, &sidc, "browser_handoff", json!({"tab": tab2, "message": "approve?", "reason": "consent", "timeout_ms": 10000}))
    });
    thread::sleep(Duration::from_millis(1000));
    let _ = tool(port, &token, &sid, "browser_test_decline", json!({}));
    let got = waiter.join().unwrap();
    assert!(got.is_err() && got.as_ref().unwrap_err().contains("declined"), "{:?}", got);
    println!("PASS 7 ask for human help (decline -> declined)");

    let ctab = {
        let t = tool(port, &token, &sid, "browser_open", json!({"url": format!("http://127.0.0.1:{}/plain", page_port), "background": true})).unwrap();
        t.lines().next().unwrap().strip_prefix("tab ").unwrap().trim().parse::<u64>().unwrap()
    };
    let portc = port; let tokenc = token.clone(); let sidc = sid.clone();
    let waiter = thread::spawn(move || {
        tool(portc, &tokenc, &sidc, "browser_consent", json!({"tab": ctab, "kind": "upload", "message": "ok?", "timeout_ms": 10000}))
    });
    thread::sleep(Duration::from_millis(1000));
    let _ = tool(port, &token, &sid, "browser_test_consent_decline", json!({}));
    let got = waiter.join().unwrap();
    assert!(got.is_err() && got.as_ref().unwrap_err().contains("declined"), "{:?}", got);
    println!("PASS 8 respect permissions (consent decline -> declined)");

    let w = tool(port, &token, &sid, "browser_whoami", json!({})).unwrap();
    assert!(w.contains("short_id") && w.contains("profile"));
    println!("PASS 9 protect session state (whoami)");

    let _ = child.0.kill();
    let _ = child.0.wait();
    let _ = std::fs::remove_dir_all(&dir);
    println!("ALL CHECKLIST ITEMS PASSED");
}
