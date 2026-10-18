//! Luau scripts: a batch with loops and ifs. The person writes them, as `.luau` files in
//! `scripts\` in the Lumen data folder; agents list them (`browser_scripts`) and run them
//! (`browser_script_run`). No tool runs Lua source an agent sends.
//!
//! A script is no more than its agent. Every `lumen.call` goes through `run`, so it is
//! authorized, and asks for consent, exactly as if the agent had made the call itself. The VM
//! is Luau in its sandbox: no io, no os, no require, no loadstring, no debug. A fresh VM per
//! run, 60 s, 32 MB, 200 calls. It runs on the agent's own request thread, never the UI's.
//!
//! A script's head says what it is and what it takes:
//!
//! ```lua
//! --- Every link on each page of a list.
//! --@input urls: list of URLs
//! --@input match: only links with this in them (optional)
//! ```

use super::*;
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Value as LuaValue, VmState};
use std::{cell::RefCell, rc::Rc};

const MAX_CALLS: usize = 200;
const MEMORY: usize = 32 << 20;
const SCRIPT_TIME: Duration = Duration::from_secs(60);
/// A script file bigger than this is not a script.
const MAX_SOURCE: u64 = 256 << 10;

/// The person's scripts folder.
pub fn dir() -> PathBuf {
    crate::store::data_dir().join("scripts")
}

/// A script's name as a file in that folder. Letters, digits, dash and underscore only, so a
/// name cannot walk out of it.
fn file(name: &str) -> Option<PathBuf> {
    clean_name(name.trim_end_matches(".luau")).map(|n| dir().join(format!("{n}.luau")))
}

/// What a script's head says: its first `---` line, and its `--@input` lines.
pub fn head(src: &str) -> (String, Vec<String>) {
    let mut about = String::new();
    let mut inputs = Vec::new();
    for line in src.lines().map(str::trim).take_while(|l| l.starts_with("--")) {
        if let Some(i) = line.strip_prefix("--@input") {
            inputs.push(i.trim().to_string());
        } else if about.is_empty()
            && let Some(a) = line.strip_prefix("---")
        {
            about = a.trim().to_string();
        }
    }
    (about, inputs)
}

/// `browser_scripts`: name, what it does, what it takes.
pub fn list() -> Result<Vec<Value>, String> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir())
        .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "luau")).collect())
        .unwrap_or_default();
    names.sort();
    if names.is_empty() {
        return Ok(text(format!("no scripts yet; the person puts .luau files in {}", dir().display())));
    }
    let mut out = String::new();
    for p in names {
        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let src = std::fs::read_to_string(&p).unwrap_or_default();
        let (about, inputs) = head(&src);
        out.push_str(&format!("- {name}: {}\n", if about.is_empty() { "(no description)" } else { &about }));
        for i in inputs {
            out.push_str(&format!("    input {i}\n"));
        }
    }
    Ok(text(out))
}

/// `browser_script_run`: the script's return value as JSON, after one line per call it made.
pub fn run_script(sh: &Arc<Shared>, who: &Who, args: &Value) -> Result<Vec<Value>, String> {
    let name = args["name"].as_str().unwrap_or("");
    let path = file(name).ok_or("give the script's name (browser_scripts lists them)")?;
    let meta = std::fs::metadata(&path).map_err(|_| format!("no script called {name}; browser_scripts lists them"))?;
    if meta.len() > MAX_SOURCE {
        return Err(format!("{name} is over {} KB; that is not a script", MAX_SOURCE >> 10));
    }
    let src = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
    let calls: Rc<RefCell<Vec<Done>>> = Rc::default();
    let result = execute(sh, who, &src, name, &args["input"], &calls);
    let calls = calls.borrow();
    let mut out = format!("{} calls\n{}", calls.len(), step_lines(&calls));
    match result {
        Ok(v) => {
            out.push_str("\nresult:\n");
            out.push_str(&serde_json::to_string_pretty(&v).unwrap_or_default());
            Ok(text(out))
        }
        Err(e) => Err(format!("{out}\n{name} failed: {e}")),
    }
}

/// One run: a fresh, sandboxed VM, the `lumen` table, the input, the source.
fn execute(sh: &Arc<Shared>, who: &Who, src: &str, name: &str, input: &Value, calls: &Rc<RefCell<Vec<Done>>>) -> Result<Value, String> {
    let lua = Lua::new_with(StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH | StdLib::BIT | StdLib::BUFFER, LuaOptions::default())
        .map_err(|e| e.to_string())?;
    let _ = lua.set_memory_limit(MEMORY);
    let deadline = Instant::now() + SCRIPT_TIME;
    lua.set_interrupt(move |_| {
        if Instant::now() > deadline {
            return Err(mlua::Error::RuntimeError("out of time: a script has 60 s".into()));
        }
        Ok(VmState::Continue)
    });
    let g = lua.globals();
    let e = |e: mlua::Error| e.to_string();
    // What Luau's base library has that a script must not: code from strings, other files,
    // reaching into environments.
    for gone in ["loadstring", "require", "getfenv", "setfenv", "collectgarbage", "load", "dofile"] {
        g.set(gone, LuaValue::Nil).map_err(e)?;
    }
    let lumen = lua.create_table().map_err(e)?;

    // lumen.call(tool, args): the tool's answer as text; a refusal or failure is a Lua error.
    let call_fn = tool_fn(&lua, sh, who, calls, deadline, true).map_err(e)?;
    lumen.set("call", call_fn).map_err(e)?;
    // lumen.try(tool, args): ok, text. For when a failure is part of the plan.
    lumen.set("try", tool_fn(&lua, sh, who, calls, deadline, false).map_err(e)?).map_err(e)?;
    // lumen.open(url): the new tab's id, and its answer.
    let open = lua
        .create_function(|lua, url: String| {
            let call: Function = lua.globals().get::<mlua::Table>("lumen")?.get("call")?;
            let args = lua.create_table()?;
            args.set("url", url)?;
            let out: String = call.call(("browser_open", args))?;
            let tab = out.lines().next().and_then(|l| l.strip_prefix("tab ")).and_then(|n| n.trim().parse::<u64>().ok());
            Ok((tab, out))
        })
        .map_err(e)?;
    lumen.set("open", open).map_err(e)?;
    // lumen.ref(snapshot, word, ...): the ref of the first control line with every word.
    let find_ref = lua
        .create_function(|_, (snap, words): (String, mlua::Variadic<String>)| {
            Ok(snap.lines().filter(|l| l.contains("[ref=") && words.iter().all(|w| l.contains(w.as_str()))).find_map(|l| {
                let at = l.find("[ref=")? + 5;
                l[at..].split(']').next().map(String::from)
            }))
        })
        .map_err(e)?;
    lumen.set("ref", find_ref).map_err(e)?;
    // lumen.log(...): lines that come back with the answer, under the calls.
    let log = calls.clone();
    let log_fn = lua
        .create_function(move |_, parts: MultiValue| {
            let line = parts.iter().map(|v| v.to_string().unwrap_or_default()).collect::<Vec<_>>().join(" ");
            log.borrow_mut().push(Done { tool: format!("log: {}", clip(&line, 200)), ms: 0, tab: None, out: Ok(Vec::new()) });
            Ok(())
        })
        .map_err(e)?;
    lumen.set("log", log_fn).map_err(e)?;
    // lumen.sleep(ms): at most 5 s at a time, and never past the deadline.
    let sleep = lua
        .create_function(move |_, ms: u64| {
            let left = deadline.saturating_duration_since(Instant::now());
            thread::sleep(Duration::from_millis(ms.min(5000)).min(left));
            Ok(())
        })
        .map_err(e)?;
    lumen.set("sleep", sleep).map_err(e)?;
    let json_t = lua.create_table().map_err(e)?;
    json_t
        .set(
            "encode",
            lua.create_function(|lua, v: LuaValue| {
                let j: Value = lua.from_value(v)?;
                Ok(j.to_string())
            })
            .map_err(e)?,
        )
        .map_err(e)?;
    json_t
        .set(
            "decode",
            lua.create_function(|lua, s: String| {
                let j: Value = serde_json::from_str(&s).map_err(mlua::Error::external)?;
                lua.to_value(&j)
            })
            .map_err(e)?,
        )
        .map_err(e)?;
    lumen.set("json", json_t).map_err(e)?;
    g.set("lumen", lumen).map_err(e)?;
    let none = json!({});
    g.set("input", lua.to_value(if input.is_null() { &none } else { input }).map_err(e)?).map_err(e)?;
    // Globals read-only from here, and the script's own in a table of its own.
    lua.sandbox(true).map_err(e)?;
    let out: LuaValue = lua.load(src).set_name(format!("={name}")).eval().map_err(|e| script_error(&e))?;
    lua.from_value::<Value>(out).map_err(|e| format!("its answer is not JSON: {e}"))
}

/// `lumen.call` (`raise`: an error is a Lua error) or `lumen.try` (an error is `false, text`).
fn tool_fn(lua: &Lua, sh: &Arc<Shared>, who: &Who, calls: &Rc<RefCell<Vec<Done>>>, deadline: Instant, raise: bool) -> mlua::Result<Function> {
    let (sh, who, calls) = (sh.clone(), who.clone(), calls.clone());
    lua.create_function(move |lua, (tool, args): (String, Option<LuaValue>)| {
        let args: Value = match args {
            Some(a) => lua.from_value(a)?,
            None => json!({}),
        };
        // An empty Lua table reads as a list: arguments are always an object.
        let args = if args.as_object().is_some() { args } else { json!({}) };
        let n = calls.borrow().iter().filter(|d| !d.tool.starts_with("log: ")).count();
        let d = if n >= MAX_CALLS {
            Done { tool: tool.clone(), ms: 0, tab: None, out: Err(format!("a script makes at most {MAX_CALLS} calls")) }
        } else if Instant::now() >= deadline {
            Done { tool: tool.clone(), ms: 0, tab: None, out: Err("out of time: a script has 60 s".into()) }
        } else if cancelled() {
            Done { tool: tool.clone(), ms: 0, tab: None, out: Err("the client went away; the script stopped here".into()) }
        } else {
            let mut tabs = Tabs::Batch { opened: Vec::new() };
            run_step(&sh, &who, &tool, &args, None, 0, &mut tabs)
        };
        let res = d.out.as_ref().map(|c| content_text(c)).map_err(|e| format!("{tool}: {e}"));
        calls.borrow_mut().push(d);
        match (res, raise) {
            (Ok(t), true) => Ok(MultiValue::from_vec(vec![LuaValue::String(lua.create_string(&t)?)])),
            (Err(e), true) => Err(mlua::Error::RuntimeError(e)),
            (Ok(t), false) => Ok(MultiValue::from_vec(vec![LuaValue::Boolean(true), LuaValue::String(lua.create_string(&t)?)])),
            (Err(e), false) => Ok(MultiValue::from_vec(vec![LuaValue::Boolean(false), LuaValue::String(lua.create_string(&e)?)])),
        }
    })
}

/// A script's error as one line with its line number: `links:12: attempt to index nil`.
fn script_error(e: &mlua::Error) -> String {
    let s = match e {
        mlua::Error::CallbackError { cause, .. } => return script_error(cause),
        mlua::Error::RuntimeError(m) | mlua::Error::SyntaxError { message: m, .. } => m.clone(),
        mlua::Error::MemoryError(_) => "out of memory: a script has 32 MB".into(),
        other => other.to_string(),
    };
    s.lines().next().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A VM like a run's, with no browser behind it: what the sandbox lets through.
    fn bare(src: &str) -> Result<Value, String> {
        let lua = Lua::new_with(StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH | StdLib::BIT | StdLib::BUFFER, LuaOptions::default()).unwrap();
        let _ = lua.set_memory_limit(MEMORY);
        let deadline = Instant::now() + Duration::from_millis(300);
        lua.set_interrupt(move |_| {
            if Instant::now() > deadline {
                return Err(mlua::Error::RuntimeError("out of time: a script has 60 s".into()));
            }
            Ok(VmState::Continue)
        });
        for gone in ["loadstring", "require", "getfenv", "setfenv", "collectgarbage", "load", "dofile"] {
            lua.globals().set(gone, LuaValue::Nil).unwrap();
        }
        lua.sandbox(true).unwrap();
        let v: LuaValue = lua.load(src).set_name("=t").eval().map_err(|e| script_error(&e))?;
        lua.from_value::<Value>(v).map_err(|e| e.to_string())
    }

    #[test]
    fn the_sandbox_has_no_way_out() {
        for name in ["io", "os", "require", "loadstring", "debug", "package", "dofile", "getfenv"] {
            assert_eq!(bare(&format!("return {name} == nil")), Ok(json!(true)), "{name} is reachable");
        }
        assert_eq!(bare("return string.upper('ok')"), Ok(json!("OK")));
    }

    #[test]
    fn an_endless_loop_dies_at_the_deadline() {
        let e = bare("while true do end").unwrap_err();
        assert!(e.contains("out of time"), "{e}");
    }

    #[test]
    fn a_memory_bomb_dies_at_the_limit() {
        let e = bare("local t = {} for i = 1, 1e9 do t[i] = string.rep('x', 1024) .. i end").unwrap_err();
        assert!(e.contains("out of memory"), "{e}");
    }

    #[test]
    fn an_error_says_its_line() {
        let e = bare("local a = 1\nlocal b = nil\nreturn b.c").unwrap_err();
        assert!(e.starts_with("t:3:"), "{e}");
    }

    #[test]
    fn a_head_says_what_the_script_takes() {
        let (about, inputs) = head("--- Every link on each page.\n--@input urls: list of URLs\n--@input match: text (optional)\nlocal x = 1\n--@input late: not read\n");
        assert_eq!(about, "Every link on each page.");
        assert_eq!(inputs, vec!["urls: list of URLs", "match: text (optional)"]);
    }

    #[test]
    fn a_name_cannot_walk_out_of_the_folder() {
        assert_eq!(file("..\\..\\evil").map(|p| p.file_name().unwrap().to_string_lossy().to_string()), Some("evil.luau".into()));
        assert_eq!(file("links.luau").map(|p| p.file_name().unwrap().to_string_lossy().to_string()), Some("links.luau".into()));
        assert!(file("").is_none());
    }
}
