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

/// The profile's "agent" setting. Missing settings mean a fresh profile: off.
fn agent_switch_on() -> bool {
    std::fs::read_to_string(data_dir().join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| v["agent"] == true)
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
    // A Lumen started with the switch off would never answer. Say so instead of
    // starting one that waits there for nothing.
    if !agent_switch_on() {
        return Err(format!(
            "Lumen is not running, and its AI agent switch is off in {} (Settings > AI agent, or \"agent\": true in settings.json)",
            data_dir().display()
        ));
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

/// Which build this is: the version, then the commit it was built from and its date.
const VERSION: &str = concat!(env!("LUMEN_VERSION"), " (", env!("LUMEN_BUILD"), ")");

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
        Some("--version" | "version") => {
            println!("lumen-agent {}", VERSION);
            ExitCode::SUCCESS
        }
        Some("mcp") => stdio(&session),
        Some("journal") => journal_report(&args[1..]),
        Some(tool) => {
            let tool = tool.to_string();
            once(&tool, &args[1..], &session)
        }
    }
}

const HELP: &str = "lumen-agent: drive the Lumen browser from any AI agent.

  lumen-agent mcp                      MCP over stdio. Add to an agent, e.g.
                                         claude mcp add lumen -- <path>\\lumen-agent.exe mcp
  lumen-agent tools                    list the tools
  lumen-agent journal [--days N]        read Lumen's local usage journal and print
                                         a plain-text report (no Lumen needed)
  lumen-agent <tool> key=value ...     one call, e.g.
                                         lumen-agent browser_open url=example.com
                                         lumen-agent browser_click tab=3 ref=e5
                                         lumen-agent browser_type tab=3 ref=e2 \"text=hello there\" submit=true
  lumen-agent <tool> '{\"tab\": 3}'      JSON arguments

  --session NAME    CLI calls with the same name share tabs (default: one shared CLI session)

Lumen not running? It is started headless: off-screen, silent, out of your way.";

/// Lumen is up right now (no starting it).
fn running() -> Option<Door> {
    let door = Door::find().ok()?;
    matches!(door.post(&json!({"jsonrpc": "2.0", "id": 0, "method": "ping"}), ""), Ok((200, ..))).then_some(door)
}

/// What Lumen wrote about its tools the last time its agent door opened.
fn card() -> Option<Value> {
    std::fs::read_to_string(data_dir().join("agent_tools.json")).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// MCP over stdio: one JSON-RPC message per line in, one per line out.
/// Lumen is started only when a tool is really called; until then the handshake and the
/// tool list come from agent_tools.json, so opening an agent session costs nothing.
/// `named` (`--session NAME`) asks Lumen to keep this client's profile under that name,
/// so its logins outlive the session.
fn stdio(named: &str) -> ExitCode {
    let mut door = running();
    let card = card();
    if door.is_none() && card.is_none() {
        // Never ran with the agent on: nothing to answer from. Start it now.
        match reach() {
            Ok(d) => door = Some(d),
            Err(e) => return serve_error(&e),
        }
    }
    let mut session = String::new();
    let mut hello: Option<Value> = None;
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(mut msg) = serde_json::from_str::<Value>(&line) else { continue };
        let method = msg["method"].as_str().unwrap_or("").to_string();
        if method == "initialize" {
            // --session NAME: ask Lumen to keep this client's profile under that name.
            if let Some(params) = msg["params"].as_object_mut() {
                let mut meta = params.get("_meta").cloned().unwrap_or_else(|| json!({}));
                if !meta.is_object() { meta = json!({}); }
                let lumen = meta.get_mut("lumen").and_then(|x| x.as_object_mut());
                let mut lumen_map = lumen.cloned().unwrap_or_default();
                if !named.trim().is_empty() {
                    lumen_map.insert("profile".into(), json!(clean_name(named)));
                }
                lumen_map.insert("bridge".into(), json!(VERSION));
                meta.as_object_mut().unwrap().insert("lumen".into(), Value::Object(lumen_map));
                params.insert("_meta".into(), meta);
            }
            // Kept, so a session Lumen dropped (it was quiet for too long) can be opened again.
            hello = Some(msg.clone());
        }
        if door.is_none() {
            let id = msg.get("id").cloned();
            let local = match (method.as_str(), &card, id) {
                ("initialize", Some(c), Some(id)) => {
                    Some(json!({"jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "lumen", "version": c["version"].as_str().unwrap_or("")},
                        "instructions": c["instructions"]}}))
                }
                ("tools/list", Some(c), Some(id)) => Some(json!({"jsonrpc": "2.0", "id": id, "result": {"tools": c["tools"]}})),
                ("ping", _, Some(id)) => Some(json!({"jsonrpc": "2.0", "id": id, "result": {}})),
                (m, _, None) if m.starts_with("notifications/") => Some(Value::Null),
                _ => None,
            };
            if let Some(reply) = local {
                if !reply.is_null() && (writeln!(out, "{reply}").is_err() || out.flush().is_err()) {
                    break;
                }
                continue;
            }
            // A real call: now Lumen is needed. Start it, and open the session properly.
            match reach() {
                Ok(d) => {
                    if let Some(h) = &hello
                        && let Ok((200, sid, _)) = d.post(h, "")
                    {
                        session = sid;
                    }
                    door = Some(d);
                }
                Err(e) => {
                    let r = error_for(&msg, &e);
                    if !r.is_empty() && (writeln!(out, "{r}").is_err() || out.flush().is_err()) {
                        break;
                    }
                    continue;
                }
            }
        }
        let Some(door) = door.as_ref() else { continue };
        let mut sent = door.post(&msg, &session);
        if matches!(sent, Ok((404, ..)))
            && method != "initialize"
            && let Some(h) = &hello
            && let Ok((200, sid, _)) = door.post(h, "")
            && !sid.is_empty()
        {
            // Lumen forgot this session (idle for 15 minutes, or Lumen restarted). Open a new one
            // and say it again. Tabs of the old session are gone; the agent will be told so.
            session = sid;
            sent = door.post(&msg, &session);
        }
        let reply = match sent {
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
    if let Some(d) = door.as_ref()
        && !session.is_empty()
    {
        d.end(&session);
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

// ------------------------------------------------------------ named sessions

/// A `--session NAME` is only a name on this side. Lumen never takes a name for a session: it
/// hands out the id at `initialize`, and this program keeps that id in a file, one per name, so
/// the next one-shot call finds its tabs again.
fn clean_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || "-_".contains(*c)).take(40).collect();
    if n.is_empty() { "cli".into() } else { n }
}

fn session_dir() -> PathBuf {
    data_dir().join("agent_sessions")
}

fn session_file(name: &str) -> PathBuf {
    session_dir().join(format!("{}.id", clean_name(name)))
}

/// Only this Windows user may read the folder: no inherited rights, full control for the user.
/// Best effort. The folder sits under the user's own data folder anyway.
fn lock_to_user(dir: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    let Ok(user) = std::env::var("USERNAME") else { return };
    let _ = Command::new("icacls")
        .arg(dir)
        .args(["/inheritance:r", "/grant:r", &format!("{user}:(OI)(CI)F")])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status();
}

fn save_session(name: &str, id: &str) {
    let dir = session_dir();
    if !dir.is_dir() {
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        lock_to_user(&dir);
    }
    let path = session_file(name);
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, id).is_ok() && std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The id Lumen gave this name last time, if it is a plausible id.
fn saved_session(name: &str) -> String {
    let id = std::fs::read_to_string(session_file(name)).map(|s| s.trim().to_string()).unwrap_or_default();
    if id.len() >= 32 && id.chars().all(|c| c.is_ascii_alphanumeric()) { id } else { String::new() }
}

/// Ask Lumen for a new session under this name and remember it. A named session keeps
/// its profile (`_meta.lumen.profile`): its cookies and logins outlive the call, so the
/// next worker under the same name starts logged in.
fn open_session(door: &Door, name: &str) -> Result<String, String> {
    let label = if clean_name(name) == "cli" { "cli".to_string() } else { format!("cli-{}", clean_name(name)) };
    let hello = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": label, "version": env!("LUMEN_VERSION")},
        "_meta": {"lumen": {"profile": clean_name(name), "bridge": VERSION}}}});
    match door.post(&hello, "")? {
        (200, sid, _) if !sid.is_empty() => {
            save_session(name, &sid);
            Ok(sid)
        }
        (code, ..) => Err(format!("Lumen answered HTTP {code} to initialize")),
    }
}

/// One message in the named session. A session Lumen no longer knows (idle too long, or Lumen
/// restarted) is replaced by a new one and the message is sent again.
fn post_named(door: &Door, msg: &Value, name: &str) -> Result<(u16, String, String), String> {
    let saved = saved_session(name);
    if !saved.is_empty() {
        let sent = door.post(msg, &saved)?;
        if sent.0 != 404 {
            return Ok(sent);
        }
    }
    let id = open_session(door, name)?;
    door.post(msg, &id)
}

fn once(tool: &str, rest: &[String], session: &str) -> ExitCode {
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
    let body = match post_named(&door, &msg, session) {
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

// ------------------------------------------------------------------ journal

/// `lumen-agent journal [--days N]`: read Lumen's local usage journal and print a
/// short plain-text report. No Lumen needed, no network, nothing written.
fn journal_report(args: &[String]) -> ExitCode {
    let mut days = 14i64;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let value = match a.strip_prefix("--days=") {
            Some(v) => Some(v.to_string()),
            None if a == "--days" => it.next().cloned(),
            None => {
                eprintln!("lumen-agent: journal takes --days N");
                return ExitCode::from(1);
            }
        };
        match value.and_then(|v| v.parse::<i64>().ok()) {
            Some(n) => days = n.clamp(1, 3650),
            None => {
                eprintln!("lumen-agent: --days wants a number");
                return ExitCode::from(1);
            }
        }
    }
    let dir = data_dir().join("journal");
    if !dir.is_dir() {
        println!("no journal at {} (Lumen keeps one by default: Settings > Privacy)", dir.display());
        return ExitCode::SUCCESS;
    }
    let days = read_days(&dir, days);
    if days.is_empty() {
        println!("nothing in {}", dir.display());
        return ExitCode::SUCCESS;
    }
    print!("{}", report(&days));
    ExitCode::SUCCESS
}

/// One day of the journal and its lines.
struct Day {
    date: String,
    lines: Vec<Value>,
}

/// Days since 1970-01-01 for a civil date. Howard Hinnant's algorithm.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe as i64 * 365 + (yoe / 4) as i64 - (yoe / 100) as i64 + doy as i64;
    era as i64 * 146_097 + doe - 719_468
}

/// "2026-10-15" -> days since the epoch. Anything else is None.
fn day_of(name: &str) -> Option<i64> {
    let (y, rest) = name.split_once('-')?;
    let (m, d) = rest.split_once('-')?;
    let (y, m, d): (i32, u32, u32) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

/// The day files in `dir`, oldest first, the last `days` of them. A line Lumen
/// never finished writing is skipped; everything else is ready to read.
fn read_days(dir: &std::path::Path, days: i64) -> Vec<Day> {
    let mut files: Vec<(i64, String, std::path::PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stamp = name.strip_suffix(".jsonl")?;
            let day = day_of(stamp)?;
            Some((day, stamp.to_string(), e.path()))
        })
        .collect();
    files.sort();
    let newest = files.last().map(|(day, ..)| *day).unwrap_or(0);
    files
        .into_iter()
        .filter(|(day, ..)| newest - day < days)
        .map(|(_, date, path)| Day {
            date,
            lines: std::fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .collect(),
        })
        .collect()
}

/// The middle number of a list, or 0 for none.
fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2.0 }
}

fn one(v: f64) -> String {
    format!("{v:.1}")
}

/// A page time in the unit that reads best: 812 ms, 4.2 s, 1.4 min.
fn secs(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{} ms", ms.round() as u64)
    } else if ms < 60_000.0 {
        format!("{} s", one(ms / 1000.0))
    } else {
        format!("{} min", one(ms / 60_000.0))
    }
}

fn duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// The whole report: what happened, in the order a person would ask.
fn report(days: &[Day]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let lines: Vec<&Value> = days.iter().flat_map(|d| d.lines.iter()).collect();
    let of = |name: &str| -> Vec<&Value> { lines.iter().copied().filter(|v| v["k"] == name).collect() };
    let num = |v: &Value, key: &str| v[key].as_f64().unwrap_or(0.0);

    let span = match (days.first(), days.last()) {
        (Some(first), Some(last)) if days.len() > 1 => format!("{} … {}", first.date, last.date),
        (Some(day), _) => day.date.clone(),
        _ => String::new(),
    };
    let _ = writeln!(out, "Lumen journal — {span} ({} day(s), {} lines)", days.len(), lines.len());

    // Sessions: how it ran, and how it ended.
    let starts = of("start");
    let exits = of("exit");
    let mut uptimes: Vec<f64> = exits.iter().filter_map(|v| v["uptime"].as_u64()).map(|s| s as f64).collect();
    let total: u64 = uptimes.iter().sum::<f64>() as u64;
    let _ = writeln!(out, "\nSessions");
    let _ = writeln!(out, "  {} runs, {} clean exits, {} never said goodbye", starts.len(), exits.len(), of("unclean_exit").len());
    if !uptimes.is_empty() {
        let _ = writeln!(out, "  uptime {} in all (median {} per run)", duration(total), duration(median(&mut uptimes) as u64));
    }
    let headless = starts.iter().filter(|v| v["headless"] == true).count();
    let version = starts.iter().filter_map(|v| v["v"].as_str()).next().unwrap_or("");
    let _ = writeln!(out, "  version {version}, {headless} of them headless");
    for u in of("unclean_exit") {
        let log = u["log"].as_array().map(|a| a.len()).unwrap_or(0);
        let _ = writeln!(out, "  unclean exit after {} (crash.log had {log} newer line(s))", u["from"].as_str().unwrap_or("?"));
    }

    // Crashes, and whether the reload took. A crash is one line at the time and
    // a second one when the tab loads again, so the second is not a second crash.
    let crash_lines = of("crash");
    let crashes: Vec<&Value> = crash_lines.iter().copied().filter(|v| v["reload"].is_null()).collect();
    let reloads: Vec<&Value> = crash_lines.iter().copied().filter(|v| v["reload"].is_string()).collect();
    if !crash_lines.is_empty() {
        let count = |kind: &str| crashes.iter().filter(|v| v["kind"] == kind).count();
        let ok = reloads.iter().filter(|v| v["reload"] == "ok").count();
        let _ = writeln!(out, "\nCrashes");
        let _ = writeln!(out, "  {} renderer, {} browser process", count("renderer"), count("browser"));
        if !reloads.is_empty() {
            let _ = writeln!(out, "  reload after the crash: {} ok, {} failed", ok, reloads.len() - ok);
        }
    }

    // Memory and CPU, per day. The medians are of the minute samples.
    let samples = of("sample");
    if !samples.is_empty() {
        let _ = writeln!(out, "\nMemory per day (MB: median / max — lumen is Lumen's own process)");
        for day in days {
            let s: Vec<&Value> = day.lines.iter().filter(|v| v["k"] == "sample").collect();
            if s.is_empty() {
                continue;
            }
            let own: Vec<f64> = s.iter().map(|v| num(v, "lumen")).collect();
            let all: Vec<f64> = s.iter().map(|v| num(v, "total")).collect();
            let max = |v: &[f64]| v.iter().copied().fold(0.0, f64::max);
            let (own_mid, all_mid) = (median(&mut own.clone()), median(&mut all.clone()));
            let _ = writeln!(
                out,
                "  {}  lumen {}/{}  total {}/{}",
                day.date,
                one(own_mid),
                one(max(&own)),
                one(all_mid),
                one(max(&all))
            );
        }
        let mut cpu: Vec<f64> = samples.iter().map(|v| num(v, "cpu")).collect();
        let idle = median(&mut cpu);
        let busy = cpu.iter().copied().fold(0.0, f64::max);
        let _ = writeln!(out, "  cpu: median {}% of the machine, worst {}%", one(idle), one(busy));
    }

    // Pages: the slowest hosts, and the ones that failed.
    let navs: Vec<&Value> = lines.iter().copied().filter(|v| matches!(v["k"].as_str(), Some("nav" | "slow_nav"))).collect();
    let host = |v: &Value| v["host"].as_str().unwrap_or("").to_string();
    if !navs.is_empty() {
        let mut by_host: std::collections::BTreeMap<String, Vec<f64>> = Default::default();
        let mut errors: std::collections::BTreeMap<String, Vec<i64>> = Default::default();
        for v in &navs {
            by_host.entry(host(v)).or_default().push(num(v, "ms"));
            if v["ok"] == false {
                errors.entry(host(v)).or_default().push(v["err"].as_i64().unwrap_or(0));
            }
        }
        let mut ranked: Vec<(String, f64, usize)> = by_host
            .iter()
            .map(|(h, v)| {
                let mut v = v.clone();
                (h.clone(), median(&mut v), v.len())
            })
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let _ = writeln!(out, "\nSlowest hosts (median load time)");
        for (h, ms, loads) in ranked.iter().take(15) {
            let bad = errors.get(h).map_or(0, Vec::len);
            let note = if bad > 0 { format!(", {bad} error(s)") } else { String::new() };
            let _ = writeln!(out, "  {:>9}  {h}  ({loads} load(s){note})", secs(*ms));
        }
        if !errors.is_empty() {
            let _ = writeln!(out, "\nNav errors by host");
            let mut listed: Vec<(&String, &Vec<i64>)> = errors.iter().collect();
            listed.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
            for (h, codes) in listed {
                let mut codes = codes.clone();
                codes.sort();
                let code = codes[codes.len() / 2];
                let _ = writeln!(out, "  {}  {h}  (error {code})", codes.len());
            }
        }
    }

    // Everything Lumen had to say to the person.
    let errors = of("error");
    let panics = of("panic");
    if !errors.is_empty() || !panics.is_empty() {
        let _ = writeln!(out, "\nErrors and panics");
        let mut grouped: std::collections::BTreeMap<(String, String), usize> = Default::default();
        for v in &errors {
            *grouped.entry((v["f"].as_str().unwrap_or("").to_string(), v["m"].as_str().unwrap_or("").to_string())).or_default() += 1;
        }
        let mut listed: Vec<((String, String), usize)> = grouped.into_iter().collect();
        listed.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        for ((feature, msg), n) in listed.iter().take(20) {
            let _ = writeln!(out, "  {n:>4}  {feature}: {msg}");
        }
        let _ = writeln!(out, "  panics: {}", panics.len());
    }

    // Frames that stuttered.
    let slow: Vec<&Value> = of("ui_slow");
    if !slow.is_empty() {
        let frames: f64 = slow.iter().map(|v| num(v, "n")).sum();
        let worst = slow.iter().map(|v| num(v, "worst")).fold(0.0, f64::max);
        let _ = writeln!(out, "\nui_slow: {} frame(s) over 50 ms over {} minute(s), worst {}", frames as u64, slow.len(), secs(worst));
    }

    // What got used.
    let features = of("feature");
    if !features.is_empty() {
        let _ = writeln!(out, "\nFeatures");
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for v in &features {
            *counts.entry(v["f"].as_str().unwrap_or("").to_string()).or_default() += 1;
        }
        let mut listed: Vec<(&String, &usize)> = counts.iter().collect();
        listed.sort_by_key(|(name, _)| (*name).clone());
        let mut row: Vec<String> = listed.iter().map(|(name, n)| format!("{name} {n}")).collect();
        row.retain(|r| !r.starts_with("download"));
        let _ = writeln!(out, "  {}", row.join(", "));
        let downloads: Vec<&&Value> = features.iter().filter(|v| v["f"] == "download").collect();
        if !downloads.is_empty() {
            let done = downloads.iter().filter(|v| v["result"] == "done").count();
            let mut avg: Vec<f64> = downloads.iter().filter_map(|v| v["avg"].as_f64()).collect();
            let kind = |k: &str| downloads.iter().filter(|v| v["kind"] == k).count();
            let speed = if avg.is_empty() { String::new() } else { format!(", median {} MB/s", one(median(&mut avg) / 1_048_576.0)) };
            let _ = writeln!(
                out,
                "  download {} ({} done, {} failed; {} file, {} torrent, {} video{speed})",
                downloads.len(),
                done,
                downloads.len() - done,
                kind("file"),
                kind("torrent"),
                kind("video")
            );
        }
    }

    // Idle wakes, which is how a spin shows up.
    let wakes = of("wake");
    if !wakes.is_empty() {
        let mut per: Vec<f64> = wakes.iter().map(|v| num(v, "n")).collect();
        let worst = per.iter().copied().fold(0.0, f64::max);
        let _ = writeln!(out, "\nWakes: median {} per minute, most {}", median(&mut per), worst as u64);
    }
    out
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

    #[test]
    fn a_session_name_cannot_walk_out_of_its_folder() {
        assert_eq!(clean_name("..\\..\\x"), "x");
        assert_eq!(clean_name("a/b:c"), "abc");
        assert_eq!(clean_name(""), "cli");
        assert_eq!(clean_name("...."), "cli");
        assert_eq!(clean_name(&"z".repeat(100)).len(), 40);
        assert!(session_file("../../evil").starts_with(session_dir()));
    }

    #[test]
    fn a_saved_id_must_look_like_one() {
        // No file: no id. The file is whatever the user's disk holds, so it is checked.
        assert_eq!(saved_session("no-such-name-ever-written"), "");
    }

    /// A fixture the size of a short session, read back the way Lumen wrote it.
    fn fixture() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-agent-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let start = r#"{"t":1,"k":"start","v":"0.1.0","headless":true,"theme":"dark","compact":false,"dark_pages":false,"blocker":true,"sleep":5,"discard":30,"restored":2}"#;
        std::fs::write(
            dir.join("2026-01-01.jsonl"),
            format!(
                "{start}\n\
                 {{\"t\":2,\"k\":\"nav\",\"host\":\"slow.example\",\"ms\":9000,\"ok\":true}}\n\
                 {{\"t\":3,\"k\":\"nav\",\"host\":\"slow.example\",\"ms\":11000,\"ok\":true}}\n\
                 {{\"t\":4,\"k\":\"nav\",\"host\":\"bad.example\",\"ms\":300,\"ok\":false,\"err\":6}}\n\
                 {{\"t\":5,\"k\":\"sample\",\"lumen\":80.0,\"total\":400.0,\"cpu\":0.5,\"tabs\":2,\"live\":1,\"asleep\":1,\"discarded\":0,\"ws\":1,\"panel\":\"none\"}}\n\
                 {{\"t\":6,\"k\":\"error\",\"f\":\"download\",\"m\":\"aria2 refused it\"}}\n\
                 {{\"t\":7,\"k\":\"wake\",\"n\":12}}\n\
                 {{\"t\":8,\"k\":\"exit\",\"clean\":true,\"uptime\":3600}}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("2026-01-02.jsonl"),
            format!(
                "{start}\n\
                 {{\"t\":10,\"k\":\"crash\",\"kind\":\"renderer\",\"host\":\"bad.example\"}}\n\
                 {{\"t\":11,\"k\":\"crash\",\"kind\":\"renderer\",\"host\":\"bad.example\",\"reload\":\"ok\"}}\n\
                 {{\"t\":12,\"k\":\"panic\",\"m\":\"index out of bounds  at chrome/mod.rs:1\"}}\n\
                 {{\"t\":13,\"k\":\"feature\",\"f\":\"reader\"}}\n\
                 {{\"t\":14,\"k\":\"feature\",\"f\":\"download\",\"kind\":\"file\",\"result\":\"done\",\"ms\":12000,\"avg\":1048576}}\n\
                 {{\"t\":15,\"k\":\"nav\",\"host\":\"slow.example\",\"ms\":10000,\"ok\":true}}\n\
                 {{\"t\":16,\"k\":\"ui_slow\",\"n\":3,\"worst\":120.0}}\n\
                 {{\"t\":17,\"k\":\"truncated\n"
            ),
        )
        .unwrap();
        dir
    }

    #[test]
    fn the_report_reads_a_small_fixture() {
        let dir = fixture();
        let days = read_days(&dir, 14);
        assert_eq!(days.len(), 2, "both days are read");
        // The half-written last line was left where it was.
        assert_eq!(days[1].lines.len(), 8, "{:?}", days[1].lines);
        // --days takes the newest only.
        assert_eq!(read_days(&dir, 1).len(), 1);
        let text = report(&days);
        for want in [
            "2 runs",
            "clean exits, 0 never said goodbye",
            "1 renderer, 0 browser",
            "reload after the crash: 1 ok, 0 failed",
            "slow.example",
            "bad.example",
            "error 6",
            "1  download: aria2 refused it",
            "panics: 1",
            "reader 1",
            "download 1 (1 done, 0 failed; 1 file, 0 torrent, 0 video",
            "ui_slow: 3 frame(s)",
            "Wakes: median 12 per minute",
            "total 400.0",
        ] {
            assert!(text.contains(want), "{want:?} is missing from:\n{text}");
        }
        // The slowest host is first among the pages that were timed, and its
        // median is the middle of 9 s, 10 s and 11 s.
        let ranked = text.find("Slowest hosts").unwrap();
        let slow = text[ranked..].find("slow.example").unwrap();
        assert!(slow < text[ranked..].len());
        assert!(text.contains("10.0 s"), "the median of 9 s, 10 s and 11 s: {text}");
    }

}
