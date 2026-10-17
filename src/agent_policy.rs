//! Who may do what, in one place.
//!
//! Every tool call from an agent goes through `authorize` before anything runs. The rules are
//! a table (`RULES`), one row per tool, and a test fails if a tool has no row. A denial is an
//! MCP tool error that says which setting would allow the call, and it leaves one line in the
//! journal: the tool and a short reason code, never a path, a URL or page content.
//!
//! The rules in short:
//! - An agent uses the tabs its session opened. A tab of the person's is off limits until the
//!   person grants that one tab (right-click: Let agents use this tab). The grant ends when the
//!   tab goes to another site or closes.
//! - Never in the person's tabs: eval, upload, close, resize.
//! - `browser_eval` runs only in the agent's own tabs of the separate agent profile.
//! - `browser_upload` is off unless the person turns it on, and then reads one folder only.
//! - Only http(s) pages are downloaded; only http(s), data: and search words are opened.

use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::agent::Who;

/// What the policy needs to know about one tab.
#[derive(Clone, Debug, PartialEq)]
pub struct TabInfo {
    pub id: u64,
    /// Opened by an agent (or by a page in an agent's tab).
    pub agent: bool,
    /// For an agent tab, the session that owns it.
    pub owner: String,
    /// The person said agents may use this person's tab, and it is still on the same site.
    pub granted: bool,
}

/// How an agent stands to a tab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Access {
    /// The agent opened it.
    Own,
    /// The person's tab, granted to agents.
    Granted,
}

/// The one test of tab ownership. `on_agent` uses it for every primitive, `authorize` for every tool.
pub fn access(session: &str, agent: bool, owner: &str, granted: bool) -> Option<Access> {
    if agent {
        (!owner.is_empty() && owner == session).then_some(Access::Own)
    } else {
        granted.then_some(Access::Granted)
    }
}

/// A person's tab may be steered, not scripted: these DevTools calls run page code or reach files.
pub fn cdp_allowed_on_person_tab(method: &str) -> bool {
    !matches!(method, "Runtime.evaluate" | "Runtime.callFunctionOn" | "DOM.setFileInputFiles" | "Page.addScriptToEvaluateOnNewDocument")
}

/// The state a decision needs, read from the UI thread once per tool call.
#[derive(Clone, Debug, Default)]
pub struct PolicyCtx {
    pub tabs: Vec<TabInfo>,
    /// Settings > AI agent > Agents use my profile.
    pub shared_profile: bool,
    /// Settings > AI agent > Let agents upload files.
    pub upload_on: bool,
    pub upload_dir: PathBuf,
}

impl PolicyCtx {
    pub fn from_value(v: &Value) -> PolicyCtx {
        let tabs = v["tabs"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        Some(TabInfo {
                            id: t["id"].as_u64()?,
                            agent: t["agent"].as_bool().unwrap_or(false),
                            owner: t["owner"].as_str().unwrap_or("").to_string(),
                            granted: t["granted"].as_bool().unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        PolicyCtx {
            tabs,
            shared_profile: v["shared"].as_bool().unwrap_or(false),
            upload_on: v["upload"].as_bool().unwrap_or(false),
            upload_dir: upload_dir(v["upload_dir"].as_str().unwrap_or("")),
        }
    }
}

/// Which tabs a tool needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scope {
    /// No tab.
    Free,
    /// `tab`: the agent's own, or one the person granted.
    Tab,
    /// `tab` if given, same rule.
    TabOpt,
    /// `tab`: the agent's own only. Acts on the page in ways a visitor of someone else's tab should not.
    OwnTab,
}

/// What else must hold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gate {
    Nothing,
    /// `url` is something an agent may open.
    Url,
    /// `url` is http(s).
    Download,
    /// Own tab of the isolated agent profile.
    Eval,
    /// The setting is on and every path is inside the upload folder.
    Upload,
}

pub struct Rule {
    pub tool: &'static str,
    pub scope: Scope,
    pub gate: Gate,
}

const fn rule(tool: &'static str, scope: Scope, gate: Gate) -> Rule {
    Rule { tool, scope, gate }
}

/// Every tool, once. A new tool without a row here is refused, and a test fails.
pub const RULES: &[Rule] = &[
    rule("browser_open", Scope::Free, Gate::Url),
    rule("browser_navigate", Scope::Tab, Gate::Url),
    rule("browser_back", Scope::Tab, Gate::Nothing),
    rule("browser_forward", Scope::Tab, Gate::Nothing),
    rule("browser_reload", Scope::Tab, Gate::Nothing),
    rule("browser_snapshot", Scope::Tab, Gate::Nothing),
    rule("browser_read", Scope::Tab, Gate::Nothing),
    rule("browser_article", Scope::Tab, Gate::Nothing),
    rule("browser_find", Scope::Tab, Gate::Nothing),
    rule("browser_click", Scope::Tab, Gate::Nothing),
    rule("browser_hover", Scope::Tab, Gate::Nothing),
    rule("browser_type", Scope::Tab, Gate::Nothing),
    rule("browser_press", Scope::Tab, Gate::Nothing),
    rule("browser_select", Scope::Tab, Gate::Nothing),
    rule("browser_scroll", Scope::Tab, Gate::Nothing),
    rule("browser_drag", Scope::Tab, Gate::Nothing),
    rule("browser_upload", Scope::OwnTab, Gate::Upload),
    rule("browser_wait", Scope::Tab, Gate::Nothing),
    rule("browser_screenshot", Scope::Tab, Gate::Nothing),
    rule("browser_pdf", Scope::Tab, Gate::Nothing),
    rule("browser_eval", Scope::OwnTab, Gate::Eval),
    rule("browser_console", Scope::Tab, Gate::Nothing),
    rule("browser_network", Scope::Tab, Gate::Nothing),
    rule("browser_dialogs", Scope::Tab, Gate::Nothing),
    rule("browser_resize", Scope::OwnTab, Gate::Nothing),
    rule("browser_tabs", Scope::Free, Gate::Nothing),
    rule("browser_show", Scope::Tab, Gate::Nothing),
    rule("browser_handoff", Scope::Tab, Gate::Nothing),
    rule("browser_download", Scope::TabOpt, Gate::Download),
    rule("browser_downloads", Scope::Free, Gate::Nothing),
    rule("browser_close", Scope::OwnTab, Gate::Nothing),
    rule("browser_help", Scope::Free, Gate::Nothing),
    rule("browser_status", Scope::Free, Gate::Nothing),
    rule("browser_whoami", Scope::Free, Gate::Nothing),
    rule("browser_consent", Scope::Tab, Gate::Nothing),
];

/// Hooks for the integration tests. Debug builds only; a release build has no such tools.
#[cfg(debug_assertions)]
const TEST_RULES: &[Rule] = &[
    rule("browser_test_crash", Scope::OwnTab, Gate::Nothing),
    rule("browser_test_done", Scope::Free, Gate::Nothing),
    rule("browser_test_state", Scope::Free, Gate::Nothing),
    rule("browser_test_grant", Scope::Free, Gate::Nothing),
    rule("browser_test_decline", Scope::Free, Gate::Nothing),
    rule("browser_test_consent_approve", Scope::Free, Gate::Nothing),
    rule("browser_test_consent_decline", Scope::Free, Gate::Nothing),
];
#[cfg(not(debug_assertions))]
const TEST_RULES: &[Rule] = &[];

fn find(tool: &str) -> Option<&'static Rule> {
    RULES.iter().chain(TEST_RULES).find(|r| r.tool == tool)
}

/// A refusal: what to tell the agent, and a reason code for the journal.
#[derive(Debug, PartialEq)]
pub struct Denied {
    pub text: String,
    pub code: &'static str,
    /// Worth a journal line. A tab that is simply gone is not a denial.
    pub log: bool,
}

fn denied(code: &'static str, text: impl Into<String>) -> Denied {
    Denied { text: text.into(), code, log: true }
}

/// What a yes carries: the upload paths as checked, for the caller to use instead of the raw ones.
#[derive(Debug, Default, PartialEq)]
pub struct Approval {
    pub files: Vec<String>,
}

/// May this agent make this call? Called for every tool call, before it runs.
pub fn authorize(who: &Who, tool: &str, args: &Value, ctx: &PolicyCtx) -> Result<Approval, Denied> {
    let Some(rule) = find(tool) else {
        return Err(denied("unknown_tool", format!("unknown tool {tool}")));
    };
    if rule.scope != Scope::Free {
        match args["tab"].as_u64() {
            // A missing tab is the tool's own error to give ("missing tab id").
            None => {}
            Some(id) => {
                let known = ctx.tabs.iter().find(|t| t.id == id);
                let here = known.and_then(|t| access(&who.session, t.agent, &t.owner, t.granted));
                match (here, rule.scope) {
                    (None, _) => {
                        // The same words for a tab that is gone, another agent's, or the person's
                        // own: an agent learns nothing about tabs that are not its.
                        return Err(Denied {
                            text: format!(
                                "no tab {id} you may use; browser_tabs lists yours. The person's tabs are off limits unless they right-click one and choose \"Let agents use this tab\"."
                            ),
                            code: "tab_not_yours",
                            log: known.is_some(),
                        });
                    }
                    (Some(Access::Granted), Scope::OwnTab) => {
                        return Err(denied("person_tab", format!("{tool} works only in your own tabs, never in the person's tab {id}.")));
                    }
                    _ => {}
                }
            }
        }
    }
    match rule.gate {
        Gate::Nothing => Ok(Approval::default()),
        Gate::Url => {
            if let Some(url) = args["url"].as_str() {
                url_allowed(url).map_err(|scheme| {
                    denied(
                        "url_scheme",
                        format!("{tool} does not open {scheme} pages; agents get http(s) pages, data: and search words. Serve local files over http://localhost instead."),
                    )
                })?;
            }
            Ok(Approval::default())
        }
        Gate::Download => {
            let url = args["url"].as_str().unwrap_or("").trim().to_ascii_lowercase();
            if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(denied("url_scheme", format!("{tool} takes http(s) URLs only (no magnet links, files or other schemes).")));
            }
            Ok(Approval::default())
        }
        Gate::Eval => {
            if ctx.shared_profile {
                return Err(denied(
                    "eval_shared_profile",
                    "browser_eval runs only in the separate agent profile. Turn off Settings > AI agent > Agents use my profile (agent_shared_profile) to allow it.",
                ));
            }
            Ok(Approval::default())
        }
        Gate::Upload => {
            if !ctx.upload_on {
                return Err(denied(
                    "upload_off",
                    format!(
                        "browser_upload is off. Turn on Settings > AI agent > Let agents upload files (agent_upload); then only files inside {} can be uploaded.",
                        ctx.upload_dir.display()
                    ),
                ));
            }
            let raw: Vec<&str> = args["paths"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            if raw.len() > 20 {
                return Err(denied("upload_count", "browser_upload takes at most 20 files at a time."));
            }
            let mut files = Vec::new();
            for r in raw {
                match upload_path(&ctx.upload_dir, r) {
                    Ok(p) => files.push(p.display().to_string()),
                    Err(why) => {
                        return Err(denied(
                            "upload_path",
                            format!("browser_upload can only send files inside {} ({why}). Change the folder with agent_upload_dir in settings.json.", ctx.upload_dir.display()),
                        ));
                    }
                }
            }
            Ok(Approval { files })
        }
    }
}

// --------------------------------------------------------------------- urls

/// Pages an agent may open: http(s), data:, view-source: of those, or plain words (a search).
/// The error is the scheme to name in the refusal.
pub fn url_allowed(url: &str) -> Result<(), &'static str> {
    let lower = url.trim().to_ascii_lowercase();
    let lower = lower.strip_prefix("view-source:").unwrap_or(&lower);
    for (prefix, name) in [("file:", "file:"), ("about:", "about:"), ("edge:", "edge:"), ("chrome:", "chrome:"), ("magnet:", "magnet:"), ("ftp:", "ftp:"), ("javascript:", "javascript:")] {
        if lower.starts_with(prefix) && lower != "about:blank" {
            return Err(name);
        }
    }
    Ok(())
}

// ------------------------------------------------------------------- upload

/// The folder agents may upload from: the setting, or "Lumen agent uploads" in the home folder.
pub fn upload_dir(setting: &str) -> PathBuf {
    let s = setting.trim();
    if s.is_empty() {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Lumen agent uploads");
    }
    PathBuf::from(s)
}

/// `raw` as a real file inside `dir`, or why not. No `..`, no UNC or device paths, nothing that
/// leaves the folder through a symlink or junction: the answer is the canonical path, and that
/// is what gets uploaded.
pub fn upload_path(dir: &Path, raw: &str) -> Result<PathBuf, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return Err("not a usable path");
    }
    let norm = raw.replace('/', "\\");
    // Rooted without a drive, UNC (\\server\share), device (\\.\) and verbatim (\\?\) paths.
    if norm.starts_with('\\') {
        return Err("network, device and rooted paths are not allowed");
    }
    // A colon is only the drive's: "C:\..." Alternate streams and "C:file" are out.
    match norm.find(':') {
        None => {}
        Some(1) if norm.as_bytes()[0].is_ascii_alphabetic() && norm.as_bytes().get(2) == Some(&b'\\') => {}
        Some(_) => return Err("only a full drive path or a name inside the folder"),
    }
    if norm.split('\\').any(|c| c == "..") {
        return Err("no .. in paths");
    }
    if !dir.is_absolute() {
        return Err("the upload folder must be a full path");
    }
    let base = std::fs::canonicalize(dir).map_err(|_| "the upload folder does not exist")?;
    let wanted = if Path::new(&norm).is_absolute() { PathBuf::from(&norm) } else { dir.join(&norm) };
    let real = std::fs::canonicalize(&wanted).map_err(|_| "no such file")?;
    if !real.starts_with(&base) {
        return Err("it is outside the upload folder");
    }
    if !real.is_file() {
        return Err("only files, not folders");
    }
    if real.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("no .. in paths");
    }
    Ok(simplify(&real))
}

/// `\\?\C:\x` as `C:\x`, which DevTools and most programs read more happily.
fn simplify(p: &Path) -> PathBuf {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}

// ---------------------------------------------------------------- downloads

/// Agents' downloads and PDFs land here, apart from the person's own.
pub fn agent_downloads_dir() -> PathBuf {
    crate::store::download_dir().join("Agent downloads")
}

/// Whether a download row is an agent's, for browser_downloads: agents see their own files and
/// nothing of the person's downloads.
pub fn is_agent_download(path: &str, url: &str, started_by_agent: impl Fn(&str) -> bool) -> bool {
    if started_by_agent(url) {
        return true;
    }
    let root = agent_downloads_dir().display().to_string().to_ascii_lowercase();
    !path.is_empty() && path.to_ascii_lowercase().starts_with(&root)
}

/// What `Op::Policy` answers, built from plain values so the UI thread can fill it in.
pub fn ctx_value(tabs: Vec<TabInfo>, shared: bool, upload: bool, upload_dir: &str) -> Value {
    json!({
        "tabs": tabs.iter().map(|t| json!({"id": t.id, "agent": t.agent, "owner": t.owner, "granted": t.granted})).collect::<Vec<_>>(),
        "shared": shared, "upload": upload, "upload_dir": upload_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn who(s: &str) -> Who {
        Who { session: s.into(), name: s.into() }
    }

    fn tab(id: u64, agent: bool, owner: &str, granted: bool) -> TabInfo {
        TabInfo { id, agent, owner: owner.into(), granted }
    }

    /// A folder to work in, empty, named for the test.
    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-policy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ctx(dir: &Path) -> PolicyCtx {
        PolicyCtx {
            tabs: vec![tab(1, true, "A", false), tab(2, true, "B", false), tab(3, false, "", false), tab(4, false, "", true), tab(5, true, "A", false)],
            shared_profile: false,
            upload_on: true,
            upload_dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn every_tool_has_a_rule_and_every_rule_a_tool() {
        let listed = crate::agent::tools();
        let names: Vec<&str> = listed.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        for n in &names {
            assert!(RULES.iter().any(|r| r.tool == *n), "{n} is a tool with no row in RULES");
        }
        for r in RULES {
            assert!(names.contains(&r.tool), "{} has a row but is not a tool", r.tool);
        }
        let mut seen: Vec<&str> = RULES.iter().map(|r| r.tool).collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), RULES.len(), "a tool has two rows");
        // A rule that wants a tab belongs to a tool that takes one.
        for t in listed.as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            let r = RULES.iter().find(|r| r.tool == name).unwrap();
            let takes_tab = !t["inputSchema"]["properties"]["tab"].is_null();
            assert_eq!(r.scope != Scope::Free, takes_tab, "{name}: scope and schema disagree about `tab`");
        }
    }

    #[test]
    fn an_unlisted_tool_is_refused() {
        let c = ctx(&tmp("unlisted"));
        let e = authorize(&who("A"), "browser_format_disk", &json!({}), &c).unwrap_err();
        assert_eq!(e.code, "unknown_tool");
    }

    #[test]
    fn a_session_reaches_its_own_tabs_and_no_others() {
        let c = ctx(&tmp("own"));
        for tool in ["browser_click", "browser_snapshot", "browser_close", "browser_eval", "browser_resize", "browser_reload"] {
            assert!(authorize(&who("A"), tool, &json!({"tab": 1, "script": "1"}), &c).is_ok(), "{tool} on its own tab");
            // Session B's tab, the person's tab, and a tab that does not exist: one answer.
            for other in [2u64, 3, 99] {
                let e = authorize(&who("A"), tool, &json!({"tab": other}), &c).unwrap_err();
                assert_eq!(e.code, "tab_not_yours", "{tool} on {other}");
                assert!(e.text.contains("Let agents use this tab"));
            }
        }
        // A tab that is simply gone is no denial worth a journal line; another agent's tab is.
        assert!(!authorize(&who("A"), "browser_click", &json!({"tab": 99}), &c).unwrap_err().log);
        assert!(authorize(&who("A"), "browser_click", &json!({"tab": 2}), &c).unwrap_err().log);
        // An empty session name owns nothing, even an agent tab with no owner.
        let mut c2 = c.clone();
        c2.tabs.push(tab(7, true, "", false));
        assert!(authorize(&who(""), "browser_click", &json!({"tab": 7}), &c2).is_err());
    }

    #[test]
    fn a_granted_person_tab_can_be_steered_never_scripted() {
        let c = ctx(&tmp("granted"));
        for tool in ["browser_click", "browser_type", "browser_snapshot", "browser_read", "browser_navigate", "browser_screenshot", "browser_show"] {
            assert!(authorize(&who("A"), tool, &json!({"tab": 4, "url": "https://example.com"}), &c).is_ok(), "{tool} on a granted tab");
        }
        for tool in ["browser_eval", "browser_upload", "browser_close", "browser_resize"] {
            let e = authorize(&who("A"), tool, &json!({"tab": 4, "paths": []}), &c).unwrap_err();
            assert_eq!(e.code, "person_tab", "{tool} on a granted tab");
        }
        // Any agent may use a granted tab: the grant is to agents, for that tab.
        assert!(authorize(&who("B"), "browser_click", &json!({"tab": 4}), &c).is_ok());
        // Not granted: no.
        assert!(authorize(&who("A"), "browser_click", &json!({"tab": 3}), &c).is_err());
    }

    #[test]
    fn dev_tools_calls_that_run_code_are_not_for_the_persons_tab() {
        for m in ["Runtime.evaluate", "Runtime.callFunctionOn", "DOM.setFileInputFiles"] {
            assert!(!cdp_allowed_on_person_tab(m), "{m}");
        }
        for m in ["Input.dispatchMouseEvent", "Input.dispatchKeyEvent", "Input.insertText", "Page.captureScreenshot", "Page.printToPDF", "DOM.getDocument"] {
            assert!(cdp_allowed_on_person_tab(m), "{m}");
        }
    }

    #[test]
    fn eval_needs_the_separate_profile() {
        let mut c = ctx(&tmp("eval"));
        assert!(authorize(&who("A"), "browser_eval", &json!({"tab": 1}), &c).is_ok());
        c.shared_profile = true;
        let e = authorize(&who("A"), "browser_eval", &json!({"tab": 1}), &c).unwrap_err();
        assert_eq!(e.code, "eval_shared_profile");
        assert!(e.text.contains("agent_shared_profile"), "{}", e.text);
        // The rest still works in the shared profile.
        assert!(authorize(&who("A"), "browser_click", &json!({"tab": 1}), &c).is_ok());
    }

    #[test]
    fn upload_is_off_until_the_person_turns_it_on() {
        let dir = tmp("upload-off");
        fs::write(dir.join("a.txt"), "x").unwrap();
        let mut c = ctx(&dir);
        c.upload_on = false;
        let e = authorize(&who("A"), "browser_upload", &json!({"tab": 1, "ref": "e1", "paths": [dir.join("a.txt").display().to_string()]}), &c).unwrap_err();
        assert_eq!(e.code, "upload_off");
        assert!(e.text.contains("agent_upload") && e.text.contains("Let agents upload files"), "{}", e.text);
        c.upload_on = true;
        let ok = authorize(&who("A"), "browser_upload", &json!({"tab": 1, "ref": "e1", "paths": ["a.txt", dir.join("a.txt").display().to_string()]}), &c).unwrap();
        assert_eq!(ok.files.len(), 2);
        assert!(ok.files.iter().all(|f| !f.starts_with(r"\\?\") && f.ends_with("a.txt")), "{:?}", ok.files);
    }

    #[test]
    fn upload_paths_stay_inside_the_folder() {
        let root = tmp("upload-paths");
        let dir = root.join("up");
        let outside = root.join("secret");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(dir.join("ok.txt"), "x").unwrap();
        fs::write(dir.join("sub").join("deep.txt"), "x").unwrap();
        fs::write(outside.join("key.txt"), "x").unwrap();
        fs::write(root.join("up-evil.txt"), "x").unwrap();
        let d = dir.display().to_string();
        let good = |p: &str| upload_path(&dir, p).unwrap_or_else(|e| panic!("{p}: {e}"));
        good("ok.txt");
        good("sub/deep.txt");
        good(&format!("{d}\\ok.txt"));
        good(&format!("{}/ok.txt", d.replace('\\', "/")));
        // A name and a full path for one file are the same canonical file.
        assert_eq!(good("ok.txt"), good(&format!("{d}\\ok.txt")));
        assert!(!good("ok.txt").display().to_string().starts_with(r"\\?\"));
    }

    #[test]
    fn upload_refuses_dotdot_unc_device_and_outside() {
        let root = tmp("upload-bad");
        let dir = root.join("up");
        let outside = root.join("secret");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(dir.join("ok.txt"), "x").unwrap();
        fs::write(outside.join("key.txt"), "x").unwrap();
        fs::write(root.join("up-evil.txt"), "x").unwrap();
        let d = dir.display().to_string();
        let o = outside.display().to_string();
        let bad = [
            // up and out
            format!("{d}\\..\\secret\\key.txt"),
            "..\\secret\\key.txt".into(),
            "../secret/key.txt".into(),
            "sub\\..\\..\\secret\\key.txt".into(),
            // even a .. that stays inside is refused
            "sub\\..\\ok.txt".into(),
            // a sibling whose name starts like the folder's
            format!("{}\\up-evil.txt", root.display()),
            // outside, plain
            format!("{o}\\key.txt"),
            "C:\\Windows\\win.ini".into(),
            // UNC, device, verbatim, rooted
            "\\\\server\\share\\x.txt".into(),
            "//server/share/x.txt".into(),
            "\\\\?\\C:\\Windows\\win.ini".into(),
            format!("\\\\?\\{d}\\ok.txt"),
            "\\\\.\\C:\\Windows\\win.ini".into(),
            "\\\\.\\pipe\\x".into(),
            "\\Windows\\win.ini".into(),
            // drive-relative, alternate streams, device names
            "C:win.ini".into(),
            format!("{d}\\ok.txt:hidden"),
            "NUL".into(),
            "CON".into(),
            // folders, missing files, junk
            "sub".into(),
            d.clone(),
            "nope.txt".into(),
            "".into(),
            "  ".into(),
            "ok.txt\0".into(),
            "ok\n.txt".into(),
        ];
        for p in &bad {
            assert!(upload_path(&dir, p).is_err(), "{p:?} must be refused");
        }
        assert!(upload_path(&dir, "ok.txt").is_ok());
        // The folder itself missing, or not a full path, is a refusal too.
        assert!(upload_path(&root.join("nope"), "ok.txt").is_err());
        assert!(upload_path(Path::new("up"), "ok.txt").is_err());
    }

    #[test]
    fn a_junction_cannot_carry_a_path_out() {
        let root = tmp("upload-junction");
        let dir = root.join("up");
        let outside = root.join("secret");
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("key.txt"), "x").unwrap();
        let link = dir.join("link");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&outside).output().map(|o| o.status.success()).unwrap_or(false);
        if !made {
            eprintln!("could not create a junction here; the junction check did not run");
            return;
        }
        assert!(link.join("key.txt").is_file(), "the junction works as a path");
        assert!(upload_path(&dir, "link\\key.txt").is_err(), "relative through the junction");
        assert!(upload_path(&dir, &link.join("key.txt").display().to_string()).is_err(), "absolute through the junction");
        // A junction to somewhere inside the folder is fine; it is the real file that counts.
        fs::create_dir_all(dir.join("real")).unwrap();
        fs::write(dir.join("real").join("a.txt"), "x").unwrap();
        let inside = dir.join("alias");
        if std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&inside).arg(dir.join("real")).output().map(|o| o.status.success()).unwrap_or(false) {
            assert!(upload_path(&dir, "alias\\a.txt").is_ok());
        }
    }

    #[test]
    fn uploads_are_for_own_tabs_and_need_every_path_inside() {
        let root = tmp("upload-own");
        let dir = root.join("up");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "x").unwrap();
        fs::write(root.join("b.txt"), "x").unwrap();
        let c = ctx(&dir);
        // A person's granted tab: no uploads, whatever the setting says.
        assert_eq!(authorize(&who("A"), "browser_upload", &json!({"tab": 4, "paths": ["a.txt"]}), &c).unwrap_err().code, "person_tab");
        // One bad path spoils the lot.
        let e = authorize(&who("A"), "browser_upload", &json!({"tab": 1, "paths": ["a.txt", root.join("b.txt").display().to_string()]}), &c).unwrap_err();
        assert_eq!(e.code, "upload_path");
        assert!(e.text.contains("agent_upload_dir"), "{}", e.text);
        // No paths at all passes the gate; the tool says "missing paths".
        assert!(authorize(&who("A"), "browser_upload", &json!({"tab": 1}), &c).is_ok());
    }

    #[test]
    fn schemes_agents_may_not_open() {
        for url in ["file:///C:/Users/me/secret.txt", "FILE:///x", "  file:///x", "view-source:file:///x", "about:config", "edge://settings", "chrome://flags", "magnet:?xt=urn:btih:abc", "javascript:alert(1)", "ftp://host/x"] {
            assert!(url_allowed(url).is_err(), "{url}");
        }
        for url in ["https://example.com", "http://localhost:3000/x", "example.com", "rust browsers", "about:blank", "data:text/html,hi", "view-source:https://example.com", "!g cats"] {
            assert!(url_allowed(url).is_ok(), "{url}");
        }
        let c = ctx(&tmp("urls"));
        assert_eq!(authorize(&who("A"), "browser_open", &json!({"url": "file:///C:/x"}), &c).unwrap_err().code, "url_scheme");
        assert_eq!(authorize(&who("A"), "browser_navigate", &json!({"tab": 1, "url": "edge://settings"}), &c).unwrap_err().code, "url_scheme");
        assert!(authorize(&who("A"), "browser_open", &json!({"url": "https://example.com"}), &c).is_ok());
    }

    #[test]
    fn downloads_are_http_only_and_torrents_never_come_through() {
        let c = ctx(&tmp("dl"));
        for url in ["magnet:?xt=urn:btih:abc", "file:///C:/x", "ftp://x/y", "javascript:1"] {
            assert_eq!(authorize(&who("A"), "browser_download", &json!({"url": url}), &c).unwrap_err().code, "url_scheme", "{url}");
        }
        assert!(authorize(&who("A"), "browser_download", &json!({"url": "https://example.com/a.torrent"}), &c).is_ok());
        // With a tab, the tab rule applies too.
        assert!(authorize(&who("A"), "browser_download", &json!({"url": "https://example.com/a", "tab": 2}), &c).is_err());
        assert!(authorize(&who("A"), "browser_download", &json!({"url": "https://example.com/a", "tab": 1}), &c).is_ok());
    }

    #[test]
    fn agents_see_only_their_own_downloads() {
        let started = |u: &str| u == "https://example.com/mine.bin";
        assert!(is_agent_download("", "https://example.com/mine.bin", started));
        let mine = agent_downloads_dir().join("x.pdf").display().to_string();
        assert!(is_agent_download(&mine, "https://other/x", started));
        assert!(is_agent_download(&mine.to_ascii_uppercase(), "https://other/x", started));
        let theirs = crate::store::download_dir().join("tax-return.pdf").display().to_string();
        assert!(!is_agent_download(&theirs, "https://bank.example/tax", started));
        assert!(!is_agent_download("", "https://bank.example/tax", started));
    }

    #[test]
    fn the_context_round_trips() {
        let dir = tmp("ctx");
        let v = ctx_value(vec![tab(1, true, "A", false), tab(4, false, "", true)], true, true, &dir.display().to_string());
        let c = PolicyCtx::from_value(&v);
        assert_eq!(c.tabs.len(), 2);
        assert!(c.shared_profile && c.upload_on);
        assert_eq!(c.upload_dir, dir);
        // Nothing sent: everything off, the default folder.
        let c = PolicyCtx::from_value(&json!({}));
        assert!(!c.shared_profile && !c.upload_on && c.tabs.is_empty());
        assert!(c.upload_dir.ends_with("Lumen agent uploads"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_hooks_are_debug_only_and_still_obey_ownership() {
        let c = ctx(&tmp("hooks"));
        assert!(authorize(&who("A"), "browser_test_crash", &json!({"tab": 1}), &c).is_ok());
        assert!(authorize(&who("A"), "browser_test_crash", &json!({"tab": 2}), &c).is_err());
        assert!(authorize(&who("A"), "browser_test_crash", &json!({"tab": 4}), &c).is_err());
    }
}
