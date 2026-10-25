//! The agent side of the doctor: the bridge, the MCP registrations, the agents' disk.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
}

/// One finding: how it came out, what was found, what to do.
pub type Finding = (Level, String, &'static str);

/// Over this, the agents' own browsers and downloads are worth a look.
const AGENT_SPACE_WARN: u64 = 2 * 1024 * 1024 * 1024;

/// The version an exe carries in its build stamp: "0.6.7.7 (b2ce5c9 2026-10-25)".
pub fn exe_version(bytes: &[u8]) -> Option<String> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let ver = &bytes[start..i];
            let dots = ver.iter().filter(|b| **b == b'.').count();
            let rest = &bytes[i..];
            let stamped = rest.len() > 9 && rest.starts_with(b" (") && rest[2..9].iter().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
            if dots >= 2 && stamped && !ver.ends_with(b".") {
                return Some(String::from_utf8_lossy(ver).into_owned());
            }
            continue;
        }
        i += 1;
    }
    None
}

/// The bridge beside Lumen, and whether it is the same build.
pub fn bridge(lumen: &str, agent: Option<&str>) -> Finding {
    match agent {
        None => (Level::Warn, "lumen-agent.exe is not next to lumen.exe".into(), "Agents need it: reinstall Lumen."),
        Some(v) if v == lumen => (Level::Ok, format!("lumen-agent {v}, the same build"), ""),
        Some(v) => (
            Level::Warn,
            format!("lumen-agent is {v}, Lumen is {lumen}"),
            "Restart your MCP client so it starts the new lumen-agent, or reinstall so both come from one build.",
        ),
    }
}

/// Which agents have Lumen registered, and whether the exe each one starts is still there.
pub fn registrations(home: &Path) -> Vec<(&'static str, bool)> {
    let mut out = Vec::new();
    if let Some(cmd) = json_command(&home.join(".claude.json"), &["mcpServers", "lumen", "command"]) {
        out.push(("Claude Code", Path::new(&cmd).is_file()));
    }
    if let Some(cmd) = json_command(&home.join(".config").join("opencode").join("opencode.json"), &["mcp", "lumen", "command", "0"]) {
        out.push(("opencode", Path::new(&cmd).is_file()));
    }
    if let Some(cmd) = toml_command(&home.join(".codex").join("config.toml"), "mcp_servers.lumen") {
        out.push(("Codex", Path::new(&cmd).is_file()));
    }
    out
}

fn json_command(file: &Path, path: &[&str]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(file).ok()?).ok()?;
    let mut cur = &v;
    for key in path {
        cur = match key.parse::<usize>() {
            Ok(n) => cur.get(n)?,
            Err(_) => cur.get(key)?,
        };
    }
    cur.as_str().map(str::to_string)
}

fn toml_command(file: &Path, table: &str) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line.trim_matches(['[', ']']) == table;
            continue;
        }
        if inside && let Some(v) = line.strip_prefix("command") {
            let v = v.trim_start().strip_prefix('=')?.trim();
            return Some(v.trim_matches(['\'', '"']).to_string());
        }
    }
    None
}

/// The registrations, as one finding.
pub fn mcp(regs: &[(&'static str, bool)]) -> Finding {
    if regs.is_empty() {
        return (Level::Ok, "not set up with any agent (optional)".into(), "");
    }
    let gone: Vec<&str> = regs.iter().filter(|r| !r.1).map(|r| r.0).collect();
    let names: Vec<&str> = regs.iter().map(|r| r.0).collect();
    if gone.is_empty() {
        (Level::Ok, format!("registered with {}", names.join(", ")), "")
    } else {
        (
            Level::Warn,
            format!("{} start a lumen-agent.exe that is no longer there", gone.join(", ")),
            "Register it again with the path of this install, e.g. claude mcp add -s user lumen -- <path>\\lumen-agent.exe mcp",
        )
    }
}

/// The agents' strictly isolated browsers and their downloads, in bytes.
pub fn agent_bytes(data: &Path) -> u64 {
    [data.join("agent_envs"), data.join("Agent downloads")].iter().map(|d| dir_bytes(d)).sum()
}

fn dir_bytes(d: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(d) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_bytes(&e.path()),
            Ok(_) => e.metadata().map_or(0, |m| m.len()),
            Err(_) => 0,
        })
        .sum()
}

pub fn space(bytes: u64) -> Finding {
    let mb = bytes / (1024 * 1024);
    if bytes > AGENT_SPACE_WARN {
        (Level::Warn, format!("agents keep {mb} MB on disk"), "Old agent sessions can go: close Lumen and delete agent_envs in its data folder.")
    } else {
        (Level::Ok, format!("agents keep {mb} MB on disk"), "")
    }
}

/// lumen-agent.exe beside the running Lumen.
pub fn bridge_path() -> Option<PathBuf> {
    std::env::current_exe().ok().map(|e| e.with_file_name("lumen-agent.exe")).filter(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_stamp_gives_the_version() {
        let exe = b"\0\0garbage 1.2 (zz)\0lumen-agent 0.6.7.7 (b2ce5c9 2026-10-25)\0more";
        assert_eq!(exe_version(exe).as_deref(), Some("0.6.7.7"));
        assert_eq!(exe_version(b"0.6.7.5.1 (adbc692 2026-10-25 +dev)").as_deref(), Some("0.6.7.5.1"));
        assert_eq!(exe_version(b"no stamp here 1.0"), None);
    }

    #[test]
    fn a_bridge_from_another_build_is_a_warning() {
        assert_eq!(bridge("0.6.7.7", Some("0.6.7.7")).0, Level::Ok);
        assert_eq!(bridge("0.6.7.7", Some("0.6.8")).0, Level::Warn);
        assert_eq!(bridge("0.6.7.7", None).0, Level::Warn);
    }

    #[test]
    fn registrations_are_read_from_each_agents_file() {
        let home = std::env::temp_dir().join(format!("lumen-agent-doctor-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join(".config").join("opencode")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        let real = std::env::current_exe().unwrap();
        let real = real.display().to_string().replace('\\', "\\\\");
        fs::write(home.join(".claude.json"), format!(r#"{{"mcpServers": {{"lumen": {{"command": "{real}", "args": ["mcp"]}}}}}}"#)).unwrap();
        fs::write(home.join(".config").join("opencode").join("opencode.json"), r#"{"mcp": {"lumen": {"type": "local", "command": ["C:\\gone\\lumen-agent.exe", "mcp"]}}}"#).unwrap();
        fs::write(home.join(".codex").join("config.toml"), "[other]\ncommand = 'x'\n\n[mcp_servers.lumen]\ncommand = 'C:\\gone\\lumen-agent.exe'\nargs = [\"mcp\"]\n").unwrap();
        let regs = registrations(&home);
        assert_eq!(regs, vec![("Claude Code", true), ("opencode", false), ("Codex", false)]);
        let (level, found, _) = mcp(&regs);
        assert_eq!(level, Level::Warn);
        assert!(found.starts_with("opencode, Codex"));
        assert_eq!(mcp(&[]).0, Level::Ok);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn agent_space_warns_past_two_gigabytes() {
        assert_eq!(space(10 * 1024 * 1024).0, Level::Ok);
        assert_eq!(space(3 * 1024 * 1024 * 1024).0, Level::Warn);
    }
}
