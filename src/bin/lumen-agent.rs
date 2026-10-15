//! lumen-agent: any AI agent can drive Lumen through this.
//!
//!   lumen-agent mcp                       stdio MCP server (Claude Code, opencode, Codex, Cursor...)
//!   lumen-agent tools                     what tools there are
//!   lumen-agent <tool> key=value ...      one call from a shell, result on stdout
//!   lumen-agent <tool> '{"json": "args"}'
//!
//! It finds the running Lumen (port in settings.json, token in agent_token) and forwards
//! to its MCP endpoint. No Lumen running: it starts one headless (off-screen, silent) and
//! waits for it. So an agent's config needs only the path to this program, never a secret.
//!
//! Options: --session NAME (or LUMEN_SESSION) keeps CLI calls in one set of tabs. Images
//! go to files in %TEMP% and the path is printed.

use std::{
    io::{BufRead, Write},
    path::PathBuf,
    process::{Command, ExitCode},
    time::{Duration, Instant},
};

use base64::Engine as _;
use serde_json::{Value, json};

fn data_dir() -> PathBuf {
    std::env::var_os("LUMEN_DATA").map(PathBuf::from).unwrap_or_else(|| dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("Lumen"))
}

struct Door {
    url: String,
    token: String,
    agent: ureq::Agent,
}

impl Door {
    fn find() -> Result<Door, String> {
        let dir = data_dir();
        let settings: Value = std::fs::read_to_string(dir.join("settings.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let port = settings["agent_port"].as_u64().unwrap_or(47615);
        let token = std::fs::read_to_string(dir.join("agent_token")).map(|s| s.trim().to_string()).unwrap_or_default();
        let agent = ureq::Agent::new_with_config(
            ureq::config::Config::builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(16 * 60))).build(),
        );
        Ok(Door { url: format!("http://127.0.0.1:{port}/mcp"), token, agent })
    }

    /// POST one JSON-RPC message. Returns (HTTP status, session header, body).
    fn post(&self, msg: &Value, session: &str) -> Result<(u16, String, String), String> {
        let mut req = self.agent.post(&self.url).header("Authorization", &format!("Bearer {}", self.token)).header("Content-Type", "application/json");
        if !session.is_empty() {
            req = req.header("Mcp-Session-Id", session);
        }
        let mut res = req.send(msg.to_string()).map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let sid = res.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let body = res.body_mut().read_to_string().unwrap_or_default();
        Ok((status, sid, body))
    }

    fn end(&self, session: &str) {
        let _ = self.agent.delete(&self.url).header("Authorization", &format!("Bearer {}", self.token)).header("Mcp-Session-Id", session).call();
    }
}

/// Make sure a Lumen is listening. Start a headless one next to this program if none is.
fn reach() -> Result<Door, String> {
    let door = Door::find()?;
    let ping = json!({"jsonrpc": "2.0", "id": 0, "method": "ping"});
    match door.post(&ping, "") {
        Ok((200, ..)) => return Ok(door),
        Ok((503, ..)) => return Err("Lumen is running but its AI agent switch is off: Settings > AI agent".into()),
        Ok((401, ..)) => return Err("Lumen refused the token (agent_token changed?); restart Lumen".into()),
        Ok((code, ..)) => return Err(format!("Lumen answered {code}")),
        Err(_) => {}
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name("lumen.exe");
    if !exe.is_file() {
        return Err(format!("Lumen is not running and there is no {} to start", exe.display()));
    }
    start_detached(&exe)?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(25) {
        std::thread::sleep(Duration::from_millis(300));
        // A fresh profile writes its token on first start.
        let door = Door::find()?;
        match door.post(&ping, "") {
            Ok((200, ..)) => return Ok(door),
            Ok((503, ..)) => return Err("Lumen started, but its AI agent switch is off: Settings > AI agent".into()),
            _ => {}
        }
    }
    Err("started Lumen, but it did not answer within 25 s".into())
}

/// Start Lumen on its own: no console, no share in our stdin/stdout. A child that inherits
/// our output pipe would keep the caller's shell waiting until Lumen exits.
fn start_detached(exe: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    use windows::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, HANDLE_FLAGS, SetHandleInformation},
        System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        unsafe {
            if let Ok(h) = GetStdHandle(which) {
                let _ = SetHandleInformation(h, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0));
            }
        }
    }
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    Command::new(exe)
        .arg("--headless")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start Lumen: {e}"))
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut session = std::env::var("LUMEN_SESSION").unwrap_or_default();
    if let Some(i) = args.iter().position(|a| a == "--session") {
        args.remove(i);
        if i < args.len() {
            session = args.remove(i);
        }
    }
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help" | "help") => {
            println!("{}", HELP);
            ExitCode::SUCCESS
        }
        Some("mcp") => stdio(),
        Some(tool) => {
            let tool = tool.to_string();
            once(&tool, &args[1..], if session.is_empty() { "cli".into() } else { format!("cli-{session}") })
        }
    }
}

const HELP: &str = "lumen-agent: drive the Lumen browser from any AI agent.

  lumen-agent mcp                      MCP over stdio. Add to an agent, e.g.
                                         claude mcp add lumen -- <path>\\lumen-agent.exe mcp
  lumen-agent tools                    list the tools
  lumen-agent <tool> key=value ...     one call, e.g.
                                         lumen-agent browser_open url=example.com
                                         lumen-agent browser_click tab=3 ref=e5
                                         lumen-agent browser_type tab=3 ref=e2 \"text=hello there\" submit=true
  lumen-agent <tool> '{\"tab\": 3}'      JSON arguments

  --session NAME    CLI calls with the same name share tabs (default: one shared CLI session)

Lumen not running? It is started headless: off-screen, silent, out of your way.";

/// MCP over stdio: one JSON-RPC message per line in, one per line out.
fn stdio() -> ExitCode {
    let door = match reach() {
        Ok(d) => d,
        Err(e) => {
            // Still speak MCP, so the client shows the reason instead of a dead server.
            return serve_error(&e);
        }
    };
    let mut session = String::new();
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let reply = match door.post(&msg, &session) {
            Ok((200, sid, body)) => {
                if !sid.is_empty() {
                    session = sid;
                }
                body
            }
            Ok((202, ..)) => continue,
            Ok((code, _, _)) => error_for(&msg, &format!("Lumen answered HTTP {code}")),
            Err(e) => error_for(&msg, &format!("Lumen is gone: {e}")),
        };
        if reply.is_empty() {
            continue;
        }
        if writeln!(out, "{}", reply.trim()).is_err() || out.flush().is_err() {
            break;
        }
    }
    // The client hung up: its tabs go too.
    if !session.is_empty() {
        door.end(&session);
    }
    ExitCode::SUCCESS
}

fn error_for(msg: &Value, why: &str) -> String {
    match msg.get("id") {
        Some(id) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": why}}).to_string(),
        None => String::new(),
    }
}

/// Lumen cannot be reached: answer initialize honestly and every call with the reason.
fn serve_error(why: &str) -> ExitCode {
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = msg.get("id") else { continue };
        let reply = match msg["method"].as_str() {
            Some("initialize") => json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"), "capabilities": {"tools": {}}, "serverInfo": {"name": "lumen", "version": "offline"}, "instructions": why}}),
            Some("tools/list") => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": []}}),
            _ => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": why}}),
        };
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
    ExitCode::from(2)
}

/// key=value pairs (numbers and true/false understood, [..] and {..} read as JSON) or one JSON object.
fn parse_args(a: &[String]) -> Result<Value, String> {
    if a.len() == 1 && a[0].trim_start().starts_with('{') {
        return serde_json::from_str(&a[0]).map_err(|e| format!("bad JSON: {e}"));
    }
    let mut m = serde_json::Map::new();
    for kv in a {
        let (k, v) = kv.split_once('=').ok_or_else(|| format!("expected key=value, got {kv}"))?;
        let val = match v {
            "true" => json!(true),
            "false" => json!(false),
            _ if (k == "tab" || k.ends_with("_ms") || k == "pixels" || k == "width" || k == "height" || k == "x" || k == "y" || k.starts_with("max")) && v.parse::<f64>().is_ok() => {
                serde_json::from_str(v).unwrap_or(json!(v))
            }
            _ if v.starts_with('[') || v.starts_with('{') => serde_json::from_str(v).unwrap_or(json!(v)),
            _ => json!(v),
        };
        m.insert(k.to_string(), val);
    }
    Ok(Value::Object(m))
}

fn once(tool: &str, rest: &[String], session: String) -> ExitCode {
    let door = match reach() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("lumen-agent: {e}");
            return ExitCode::from(2);
        }
    };
    let msg = if tool == "tools" {
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
    } else {
        let tool = if tool.starts_with("browser_") { tool.to_string() } else { format!("browser_{tool}") };
        let args = match parse_args(rest) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("lumen-agent: {e}");
                return ExitCode::from(2);
            }
        };
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}})
    };
    let body = match door.post(&msg, &session) {
        Ok((200, _, b)) => b,
        Ok((code, _, _)) => {
            eprintln!("lumen-agent: Lumen answered HTTP {code}");
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("lumen-agent: {e}");
            return ExitCode::from(2);
        }
    };
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    if let Some(e) = v["error"]["message"].as_str() {
        eprintln!("lumen-agent: {e}");
        return ExitCode::from(1);
    }
    if tool == "tools" {
        for t in v["result"]["tools"].as_array().into_iter().flatten() {
            println!("{:<20} {}", t["name"].as_str().unwrap_or("").trim_start_matches("browser_"), t["description"].as_str().unwrap_or(""));
        }
        return ExitCode::SUCCESS;
    }
    for c in v["result"]["content"].as_array().into_iter().flatten() {
        match c["type"].as_str() {
            Some("image") => {
                let ext = if c["mimeType"] == "image/png" { "png" } else { "jpg" };
                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
                let path = std::env::temp_dir().join(format!("lumen-{stamp}.{ext}"));
                match base64::engine::general_purpose::STANDARD.decode(c["data"].as_str().unwrap_or("")).map(|b| std::fs::write(&path, b)) {
                    Ok(Ok(())) => println!("image saved: {}", path.display()),
                    _ => println!("(an image came back but could not be saved)"),
                }
            }
            _ => println!("{}", c["text"].as_str().unwrap_or("")),
        }
    }
    if v["result"]["isError"] == true { ExitCode::from(1) } else { ExitCode::SUCCESS }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_args_become_json() {
        let a: Vec<String> = ["tab=3", "ref=e5", "text=hello world", "submit=true", "paths=[\"C:/a.txt\"]", "timeout_ms=500"].iter().map(|s| s.to_string()).collect();
        let v = parse_args(&a).unwrap();
        assert_eq!(v, json!({"tab": 3, "ref": "e5", "text": "hello world", "submit": true, "paths": ["C:/a.txt"], "timeout_ms": 500}));
        // A number where text is meant stays text.
        assert_eq!(parse_args(&["text=42".into()]).unwrap(), json!({"text": "42"}));
        assert_eq!(parse_args(&["{\"tab\": 1}".into()]).unwrap(), json!({"tab": 1}));
        assert!(parse_args(&["oops".into()]).is_err());
    }
}
