//! What lumen.exe was started with, and how a second start hands it to the first.
//!
//! `lumen.exe <url-or-path>...` opens each as a tab. `--private` makes them private tabs.
//! A Lumen already running for the profile gets the list through a small file and a signal,
//! and the second process leaves.
//!
//! The command Windows runs for a link or a file is `"lumen.exe" --single-argument %1`.
//! Everything after that flag is ONE argument, taken from the raw command line, never split
//! into words. So a crafted link such as `https://x/ --headless` is a URL with a space in it,
//! not a URL plus a flag.
//!
//! Only three kinds of target are ever opened: http(s) URLs, `file:` URLs of page-like files,
//! and paths of such files. `javascript:`, `data:`, `about:` and every other scheme are dropped.

use serde::{Deserialize, Serialize};

/// One tab to open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Open {
    pub url: String,
    #[serde(default)]
    pub private: bool,
}

/// What the command line asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub opens: Vec<Open>,
    /// `--register` / `--unregister`: write or remove the default-browser entries, then leave.
    pub register: bool,
    pub unregister: bool,
}

/// The most tabs one start may open, and the longest address taken.
const MAX_OPENS: usize = 20;
const MAX_URL: usize = 8192;

/// File types that make sense as a page. The same list the registration claims.
const PAGE_EXTS: &[&str] = &["html", "htm", "shtml", "xht", "xhtml", "svg", "pdf", "webp", "mht", "mhtml"];

// ---------------------------------------------------------------- command line

/// A word of the command line and where it ended in the raw text.
struct Tok {
    text: String,
    end: usize,
}

/// Split like Windows does (CommandLineToArgvW): quotes group, `\"` is a quote, backslashes
/// are literal unless they sit before a quote. The program name keeps its own simple rule.
fn tokens(raw: &str) -> Vec<Tok> {
    let chars: Vec<(usize, char)> = raw.char_indices().collect();
    let at = |k: usize| chars.get(k).map(|c| c.1);
    let end_of = |k: usize| chars.get(k).map_or(raw.len(), |c| c.0);
    let mut out = Vec::new();
    let mut k = 0;
    while k < chars.len() {
        while at(k).is_some_and(char::is_whitespace) {
            k += 1;
        }
        if k >= chars.len() {
            break;
        }
        let mut text = String::new();
        if out.is_empty() {
            // argv[0]: up to the closing quote, or to the next space.
            if at(k) == Some('"') {
                k += 1;
                while at(k).is_some_and(|c| c != '"') {
                    text.push(chars[k].1);
                    k += 1;
                }
                k += 1;
            } else {
                while at(k).is_some_and(|c| !c.is_whitespace()) {
                    text.push(chars[k].1);
                    k += 1;
                }
            }
            out.push(Tok { text, end: end_of(k) });
            continue;
        }
        let mut quoted = false;
        while let Some(c) = at(k) {
            if c == '\\' {
                let mut n = 0;
                while at(k) == Some('\\') {
                    n += 1;
                    k += 1;
                }
                if at(k) == Some('"') {
                    text.extend(std::iter::repeat_n('\\', n / 2));
                    if n % 2 == 1 {
                        text.push('"');
                        k += 1;
                    }
                } else {
                    text.extend(std::iter::repeat_n('\\', n));
                }
            } else if c == '"' {
                if quoted && at(k + 1) == Some('"') {
                    text.push('"');
                    k += 2;
                } else {
                    quoted = !quoted;
                    k += 1;
                }
            } else if c.is_whitespace() && !quoted {
                break;
            } else {
                text.push(c);
                k += 1;
            }
        }
        out.push(Tok { text, end: end_of(k) });
    }
    out
}

/// The one argument after `--single-argument`: the rest of the line, as it is. One pair of
/// quotes around the whole thing is taken off; quotes anywhere else stay (and get encoded).
fn single_rest(rest: &str) -> &str {
    let s = rest.trim();
    match s.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        Some(inner) if !inner.contains('"') => inner,
        _ => s,
    }
}

/// Read a whole raw command line (program name first).
pub fn parse(raw: &str) -> Parsed {
    let toks = tokens(raw);
    let mut p = Parsed::default();
    let mut targets: Vec<String> = Vec::new();
    let mut private = false;
    let mut only_words = false;
    for t in toks.iter().skip(1) {
        if only_words {
            targets.push(t.text.clone());
            continue;
        }
        match t.text.as_str() {
            "--" => only_words = true,
            "--single-argument" => {
                // Everything after it is one thing. Nothing after it is a flag.
                targets.push(single_rest(&raw[t.end.min(raw.len())..]).to_string());
                break;
            }
            "--private" | "--inprivate" | "--incognito" => private = true,
            "--register" => p.register = true,
            "--unregister" => p.unregister = true,
            // --headless, --flag=value, -x, and anything else we do not know.
            s if s.starts_with('-') => {}
            _ => targets.push(t.text.clone()),
        }
    }
    p.opens = targets
        .iter()
        .filter_map(|a| target_url(a))
        .take(MAX_OPENS)
        .map(|url| Open { url, private })
        .collect();
    p
}

/// This process's own command line.
pub fn from_process() -> Parsed {
    parse(&raw_command_line())
}

/// The command line exactly as Windows has it. `std::env::args` has already split it into
/// words, which is the one thing `--single-argument` must not have happen.
fn raw_command_line() -> String {
    unsafe extern "system" {
        fn GetCommandLineW() -> *const u16;
    }
    // SAFETY: the pointer is the process's own command line: valid, NUL-terminated, never freed.
    unsafe {
        let p = GetCommandLineW();
        if p.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

// --------------------------------------------------------------------- targets

/// An address ready to load, from one argument: an http(s) URL, a `file:` URL or a path of a
/// page-like file, or a bare host (`example.org`). None for everything else.
pub fn target_url(arg: &str) -> Option<String> {
    let s = arg.trim();
    if s.is_empty() || s.len() > MAX_URL || s.chars().any(|c| c == '\n' || c == '\r' || c == '\0') {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let after = &s[s.find("://")? + 3..];
        // A host must follow: no `https:///x`, no `https://?x`.
        if after.is_empty() || after.starts_with(['/', '?', '#']) {
            return None;
        }
        return Some(encode_url(s));
    }
    if lower.starts_with("file:") {
        let path_part = s.split(['?', '#']).next().unwrap_or(s);
        return has_page_ext(path_part).then(|| encode_url(s));
    }
    if let Some(url) = path_to_url(s) {
        return Some(url);
    }
    // Any other scheme (javascript:, data:, mailto:, about:, ms-settings:) is not ours to open.
    if has_scheme(s) {
        return None;
    }
    if !s.contains(char::is_whitespace) && crate::address::looks_like_host(s) {
        return Some(encode_url(&format!("https://{s}")));
    }
    None
}

/// `name:` where name is a scheme, and what follows is not a port (`localhost:3000`).
fn has_scheme(s: &str) -> bool {
    let Some((name, rest)) = s.split_once(':') else { return false };
    let scheme = !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    let port = rest.split(['/', '?', '#']).next().unwrap_or("");
    scheme && !(!port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
}

fn has_page_ext(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or("");
    name.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && PAGE_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// A Windows path of a page-like file as a `file:` URL. Relative paths are made absolute
/// against this process's folder, so they mean the same thing to the Lumen that gets them.
fn path_to_url(s: &str) -> Option<String> {
    if !has_page_ext(s) {
        return None;
    }
    let b = s.as_bytes();
    let drive = b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
    let unc = s.starts_with("\\\\") || s.starts_with("//");
    if !drive && !unc && has_scheme(s) {
        return None;
    }
    let full = if drive || unc {
        s.to_string()
    } else {
        std::path::absolute(s).ok()?.to_string_lossy().into_owned()
    };
    let slashed = full.replace('\\', "/");
    let url = match slashed.strip_prefix("//") {
        // \\server\share\a.html -> file://server/share/a.html
        Some(unc) => format!("file://{}", encode_path(unc.trim_start_matches('/'))),
        None => format!("file:///{}", encode_path(slashed.trim_start_matches('/'))),
    };
    Some(url)
}

/// Percent-encode a path for a `file:` URL: only what is safe stays as it is.
fn encode_path(p: &str) -> String {
    let mut out = String::with_capacity(p.len() + 8);
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A URL with the few bytes that must not travel raw made safe: spaces, control bytes,
/// quotes, angle brackets, backslashes. Everything else (including `%` and non-ASCII) stays.
fn encode_url(u: &str) -> String {
    let mut out = String::with_capacity(u.len() + 8);
    for c in u.chars() {
        match c {
            ' ' | '"' | '<' | '>' | '\\' | '`' | '{' | '}' | '|' | '^' => out.push_str(&format!("%{:02X}", c as u32)),
            c if c.is_control() => out.push_str(&format!("%{:02X}", c as u32 & 0xFF)),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------- queue

/// The folder where a second start leaves its list for the first.
fn queue_dir() -> std::path::PathBuf {
    crate::store::data_dir().join("launch")
}

#[derive(Serialize, Deserialize, Default)]
struct Note {
    /// The sender wants the window in front (a headless sender does not).
    show: bool,
    opens: Vec<Open>,
}

/// What the first Lumen found waiting.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Queue {
    pub opens: Vec<Open>,
    pub show: bool,
}

/// Leave `opens` for the running Lumen. A file written whole and renamed, so the reader never
/// sees half of it. True when the list was left.
pub fn forward(opens: &[Open], show: bool) -> bool {
    forward_in(&queue_dir(), opens, show)
}

fn forward_in(dir: &std::path::Path, opens: &[Open], show: bool) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let name = format!("{stamp}-{}", std::process::id());
    let (tmp, done) = (dir.join(format!("{name}.tmp")), dir.join(format!("{name}.json")));
    let note = Note { show, opens: opens.to_vec() };
    let Ok(json) = serde_json::to_string(&note) else { return false };
    std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, &done).is_ok()
}

/// Take everything waiting. Each address goes through the same check as a command line, so a
/// file dropped in that folder by anyone cannot open more than a command line could.
pub fn drain() -> Queue {
    drain_in(&queue_dir())
}

fn drain_in(dir: &std::path::Path) -> Queue {
    let mut q = Queue::default();
    let Ok(rd) = std::fs::read_dir(dir) else { return q };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    for f in files {
        let note: Note = std::fs::read_to_string(&f).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let _ = std::fs::remove_file(&f);
        q.show |= note.show;
        for o in note.opens {
            if q.opens.len() < MAX_OPENS
                && let Some(url) = target_url(&o.url)
            {
                q.opens.push(Open { url, private: o.private });
            }
        }
    }
    q
}

/// A first start sweeps notes older than a minute: left by a Lumen that never came.
pub fn sweep() {
    let Ok(rd) = std::fs::read_dir(queue_dir()) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 60);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(raw: &str) -> Vec<String> {
        parse(raw).opens.into_iter().map(|o| o.url).collect()
    }

    #[test]
    fn plain_urls_each_become_a_tab() {
        assert_eq!(urls(r#""C:\Lumen\lumen.exe" https://a.com/x http://b.org"#), ["https://a.com/x", "http://b.org"]);
        assert!(parse(r#""C:\Lumen\lumen.exe""#).opens.is_empty());
    }

    #[test]
    fn private_flag_makes_private_tabs() {
        let p = parse(r#"lumen.exe --private https://a.com https://b.com"#);
        assert_eq!(p.opens.len(), 2);
        assert!(p.opens.iter().all(|o| o.private));
        assert!(!parse("lumen.exe https://a.com").opens[0].private);
        // The flag may come after the address too.
        assert!(parse("lumen.exe https://a.com --private").opens[0].private);
    }

    #[test]
    fn paths_with_spaces_in_quotes_are_one_file() {
        let u = urls(r#"lumen.exe "C:\My Docs\a b.html" "D:\x\y.PDF""#);
        assert_eq!(u, ["file:///C:/My%20Docs/a%20b.html", "file:///D:/x/y.PDF"]);
    }

    #[test]
    fn page_like_files_only() {
        for ext in ["html", "htm", "pdf", "svg", "xhtml", "mht", "shtml", "xht", "webp"] {
            assert_eq!(urls(&format!(r"lumen.exe C:\a\b.{ext}")).len(), 1, "{ext}");
        }
        for bad in [r"C:\a\b.exe", r"C:\a\b.bat", r"C:\a\b.lnk", r"C:\a\b.js", r"C:\a\html", r"C:\a\.html"] {
            assert!(urls(&format!("lumen.exe {bad}")).is_empty(), "{bad}");
        }
    }

    #[test]
    fn unc_paths_and_odd_characters() {
        assert_eq!(urls(r"lumen.exe \\srv\share\a#1.html"), ["file://srv/share/a%231.html"]);
        assert_eq!(urls(r"lumen.exe C:\a\100%.html"), ["file:///C:/a/100%25.html"]);
        assert_eq!(urls(r"lumen.exe C:\ä\x.html"), ["file:///C:/%C3%A4/x.html"]);
    }

    #[test]
    fn relative_paths_become_absolute() {
        let u = urls("lumen.exe sub\\a.html");
        assert_eq!(u.len(), 1);
        assert!(u[0].starts_with("file:///") && u[0].ends_with("/sub/a.html"), "{}", u[0]);
    }

    #[test]
    fn unknown_flags_are_ignored() {
        let p = parse("lumen.exe --headless --foo=bar -x --disable-gpu https://a.com");
        assert_eq!(p.opens.len(), 1);
        assert!(!p.register && !p.unregister);
    }

    #[test]
    fn double_dash_ends_the_flags() {
        // After `--`, even something that looks like a flag is just a word (and a word that is no
        // address is dropped), but an address after it still opens.
        let p = parse("lumen.exe -- --private https://a.com");
        assert_eq!(p.opens, [Open { url: "https://a.com".into(), private: false }]);
        let p = parse("lumen.exe --private -- https://a.com");
        assert!(p.opens[0].private);
        assert!(parse("lumen.exe -- --register").opens.is_empty());
        assert!(!parse("lumen.exe -- --register").register);
    }

    #[test]
    fn register_flags() {
        assert!(parse("lumen.exe --register").register);
        assert!(parse("lumen.exe --unregister").unregister);
    }

    #[test]
    fn single_argument_takes_the_rest_whole() {
        let p = parse(r#""C:\Lumen\lumen.exe" --single-argument https://x.com/a b/c"#);
        assert_eq!(p.opens, [Open { url: "https://x.com/a%20b/c".into(), private: false }]);
        let p = parse(r#""C:\Lumen\lumen.exe" --single-argument C:\My Docs\a b.html"#);
        assert_eq!(p.opens[0].url, "file:///C:/My%20Docs/a%20b.html");
        // One pair of quotes round the whole is taken off.
        let p = parse(r#"lumen.exe --single-argument "C:\My Docs\a.html""#);
        assert_eq!(p.opens[0].url, "file:///C:/My%20Docs/a.html");
    }

    #[test]
    fn single_argument_cannot_inject_flags() {
        for evil in [
            "https://x/ --headless",
            "https://x/ --private https://y/",
            "https://x/ --register",
            "https://x/ --unregister",
            "https://x/\" --headless \"",
            "https://x/\"  --single-argument --register",
            "https://x/\n--register",
        ] {
            let p = parse(&format!(r#""C:\Lumen\lumen.exe" --single-argument {evil}"#));
            assert!(!p.register && !p.unregister, "{evil}");
            assert!(p.opens.len() <= 1, "{evil}");
            for o in &p.opens {
                assert!(!o.private, "{evil}");
                assert!(o.url.starts_with("https://x/"), "{evil}: {}", o.url);
                assert!(!o.url.contains(' ') && !o.url.contains('"') && !o.url.contains('\n'), "{evil}: {}", o.url);
            }
        }
        // The space is kept, as %20: the page is requested as written, flag text and all.
        assert_eq!(urls(r#"lumen.exe --single-argument https://x/ --headless"#), ["https://x/%20--headless"]);
    }

    #[test]
    fn single_argument_with_a_non_address_opens_nothing() {
        assert!(parse("lumen.exe --single-argument --register").opens.is_empty());
        assert!(!parse("lumen.exe --single-argument --register").register);
        assert!(parse("lumen.exe --single-argument javascript:alert(1)").opens.is_empty());
    }

    #[test]
    fn dangerous_schemes_never_open() {
        for s in [
            "javascript:alert(1)",
            "data:text/html,<script>1</script>",
            "about:blank",
            "mailto:a@b.com",
            "ms-msdt:/id",
            "file:///C:/Windows/System32/calc.exe",
            "edge://settings",
            "vbscript:x",
            "javascript:x@y.com",
        ] {
            assert!(target_url(s).is_none(), "{s}");
        }
    }

    #[test]
    fn file_urls_need_a_page_extension() {
        assert_eq!(target_url("file:///C:/a/b.html").as_deref(), Some("file:///C:/a/b.html"));
        assert_eq!(target_url("file:///C:/a/b%20c.pdf#page=2").as_deref(), Some("file:///C:/a/b%20c.pdf#page=2"));
        assert!(target_url("file:///C:/a/b.exe").is_none());
    }

    #[test]
    fn bare_hosts_become_https() {
        assert_eq!(target_url("example.org").as_deref(), Some("https://example.org"));
        assert_eq!(target_url("localhost:3000/x").as_deref(), Some("https://localhost:3000/x"));
        assert!(target_url("hello world").is_none());
        assert!(target_url("notahost").is_none());
    }

    #[test]
    fn urls_need_a_host() {
        assert!(target_url("https://").is_none());
        assert!(target_url("https:///etc").is_none());
        assert!(target_url("http://?x").is_none());
    }

    #[test]
    fn control_bytes_and_quotes_are_encoded() {
        assert_eq!(target_url("https://a.com/\"x\"").as_deref(), Some("https://a.com/%22x%22"));
        assert_eq!(target_url("https://a.com/<b>").as_deref(), Some("https://a.com/%3Cb%3E"));
        assert!(target_url("https://a.com/\r\nSet-Cookie: x").is_none());
    }

    #[test]
    fn at_most_twenty_tabs() {
        let many: String = (0..50).map(|n| format!(" https://a.com/{n}")).collect();
        assert_eq!(parse(&format!("lumen.exe{many}")).opens.len(), MAX_OPENS);
    }

    #[test]
    fn tokenizer_follows_windows_rules() {
        let t: Vec<String> = tokens(r#"a.exe "x y" z\"q "e""f" C:\p\ "C:\q\\""#).into_iter().map(|t| t.text).collect();
        assert_eq!(t[1], "x y");
        assert_eq!(t[2], "z\"q");
        assert_eq!(t[3], "e\"f");
        assert_eq!(t[4], r"C:\p\");
        assert_eq!(t[5], r"C:\q\");
    }

    #[test]
    fn the_queue_round_trips_and_rechecks() {
        let dir = std::env::temp_dir().join(format!("lumen-launch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ok = Open { url: "https://ok.com/".into(), private: true };
        assert!(forward_in(&dir, std::slice::from_ref(&ok), true));
        // A note nobody should have written: it is checked again on the way out.
        let evil = Note {
            show: false,
            opens: vec![
                Open { url: "javascript:alert(1)".into(), private: false },
                Open { url: r"C:\Windows\notepad.exe".into(), private: false },
                Open { url: "https://two.com/a b".into(), private: false },
            ],
        };
        std::fs::write(dir.join("0-evil.json"), serde_json::to_string(&evil).unwrap()).unwrap();
        std::fs::write(dir.join("junk.json"), "not json").unwrap();
        let q = drain_in(&dir);
        assert!(q.show);
        assert_eq!(q.opens, [Open { url: "https://two.com/a%20b".into(), private: false }, ok]);
        // Taken means gone.
        assert_eq!(drain_in(&dir), Queue::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
