//! Userscripts: the person's `*.user.js` files in `userscripts\` in the Lumen data folder.

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::borrow::Cow;

use serde_json::json;

fn dir() -> PathBuf {
    crate::store::data_dir().join("userscripts")
}

const MAX_SCRIPTS: usize = 100;
const MAX_SCRIPT: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunAt {
    Start,
    End,
    Idle,
}

#[derive(Clone, Debug, PartialEq)]
struct Script {
    name: String,
    matches: Vec<String>,
    excludes: Vec<String>,
    run_at: RunAt,
    source: String,
}

fn parse(text: &str) -> Option<Script> {
    let (mut name, mut matches, mut excludes) = (String::new(), Vec::new(), Vec::new());
    let mut run_at = RunAt::End;
    let (mut inside, mut closed) = (false, false);
    for line in text.lines() {
        let line = line.trim();
        if !inside {
            inside = line == "// ==UserScript==";
            continue;
        }
        if line == "// ==/UserScript==" {
            closed = true;
            break;
        }
        let Some((key, value)) = directive(line) else { continue };
        match key {
            "name" => name = value.to_string(),
            "match" if !value.is_empty() => matches.push(value.to_string()),
            "exclude" if !value.is_empty() => excludes.push(value.to_string()),
            "run-at" => {
                run_at = match value {
                    "document-start" => RunAt::Start,
                    "document-end" => RunAt::End,
                    "document-idle" => RunAt::Idle,
                    _ => return None,
                }
            }
            _ => {}
        }
    }
    (inside && closed).then(|| Script { name, matches, excludes, run_at, source: text.to_string() })
}

fn directive(line: &str) -> Option<(&str, &str)> {
    let d = line.strip_prefix("//")?.trim().strip_prefix('@')?;
    let (key, value) = d.split_once(char::is_whitespace).unwrap_or((d, ""));
    Some((key, value.trim()))
}

fn load(dir: &Path) -> Vec<Script> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".user.js")))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files.truncate(MAX_SCRIPTS);
    let mut out = Vec::new();
    for p in files {
        if std::fs::metadata(&p).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_SCRIPT {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Some(mut s) = parse(&text) else { continue };
        if s.name.trim().is_empty() {
            s.name = p.file_name().and_then(|n| n.to_str()).unwrap_or("userscript").trim_end_matches(".user.js").to_string();
        }
        out.push(s);
    }
    out
}

#[cfg(test)]
fn applies(s: &Script, url: &str) -> bool {
    s.matches.iter().any(|p| pattern_matches(p, url)) && !s.excludes.iter().any(|p| pattern_matches(p, url))
}

#[cfg(test)]
fn pattern_matches(pattern: &str, url: &str) -> bool {
    let Some((scheme, host, path)) = split_url(url) else { return false };
    let p = pattern.trim();
    if p.eq_ignore_ascii_case("<all_urls>") {
        return matches!(scheme, "http" | "https" | "file");
    }
    let Some((pscheme, rest)) = p.split_once("://") else { return false };
    let Some(at) = rest.find('/') else { return false };
    let (phost, ppath) = (&rest[..at], &rest[at..]);
    let scheme_ok = if pscheme == "*" { matches!(scheme, "http" | "https") } else { pscheme.eq_ignore_ascii_case(scheme) };
    let host_ok = if phost == "*" {
        !host.is_empty()
    } else if let Some(suffix) = phost.strip_prefix("*.") {
        let (suffix, host) = (suffix.to_ascii_lowercase(), host.to_ascii_lowercase());
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else {
        phost.eq_ignore_ascii_case(host)
    };
    scheme_ok && host_ok && wildcard(ppath, &path)
}

#[cfg(test)]
fn split_url(url: &str) -> Option<(&str, &str, Cow<'_, str>)> {
    let (scheme, rest) = url.split_once("://")?;
    let at = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, tail) = rest.split_at(at);
    let host = match (host.starts_with('['), host.find(']')) {
        (true, Some(i)) => &host[..=i],
        _ => host.split(':').next().unwrap_or(host),
    };
    let path: Cow<'_, str> = if tail.is_empty() {
        "/".into()
    } else if tail.starts_with('/') {
        tail.into()
    } else {
        format!("/{tail}").into()
    };
    Some((scheme, host, path))
}

#[cfg(test)]
fn wildcard(pat: &str, text: &str) -> bool {
    let mut parts = pat.split('*');
    let first = parts.next().unwrap_or("");
    let Some(rest) = text.strip_prefix(first) else { return false };
    let mut rest = rest;
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else { return rest.is_empty() };
    for p in middle {
        if p.is_empty() {
            continue;
        }
        match rest.find(p) {
            Some(i) => rest = &rest[i + p.len()..],
            None => return false,
        }
    }
    last.is_empty() || (rest.len() >= last.len() && rest.ends_with(last))
}

pub fn bootstrap() -> Vec<String> {
    bootstrap_in(&dir())
}

fn bootstrap_in(dir: &Path) -> Vec<String> {
    load(dir).iter().map(wrap).collect()
}

fn wrap(s: &Script) -> String {
    let meta = json!({
        "name": s.name,
        "match": s.matches,
        "exclude": s.excludes,
        "at": match s.run_at { RunAt::Start => "start", RunAt::End => "end", RunAt::Idle => "idle" },
    });
    let (head, tail) = BOOTSTRAP.split_once("__RUN__").unwrap_or((BOOTSTRAP, ""));
    let head = head.replacen("__META__", &meta.to_string(), 1);
    format!("{head}\n{}\n{tail}", s.source)
}

const BOOTSTRAP: &str = r#"(()=>{
if(window!==window.top)return;
const s=__META__;
const H=(p,u)=>{const i=u.indexOf('://');if(i<0)return false;const us=u.slice(0,i);
 if(p==='<all_urls>')return us==='http'||us==='https'||us==='file';
 const j=p.indexOf('://');if(j<0)return false;const ps=p.slice(0,j),pr=p.slice(j+3),k=pr.indexOf('/');if(k<0)return false;
 if(!(ps==='*'?(us==='http'||us==='https'):ps.toLowerCase()===us))return false;
 const after=u.slice(i+3),v=after.search(/[\/?#]/),uh=(v<0?after:after.slice(0,v)).replace(/:\d+$/,''),ph=pr.slice(0,k),pp=pr.slice(k);
 if(ph!=='*'){const pl=ph.toLowerCase();if(pl.slice(0,2)==='*.'){const b=pl.slice(2);if(uh!==b&&!uh.endsWith('.'+b))return false}else if(uh!==pl)return false}
 const W=(pat,txt)=>{const parts=pat.split('*'),first=parts.shift();if(!txt.startsWith(first))return false;let rest=txt.slice(first.length);
  for(let x=0;x<parts.length;x++){const q=parts[x];if(x===parts.length-1){return q===''||rest.endsWith(q)}
   if(q==='')continue;const at=rest.indexOf(q);if(at<0)return false;rest=rest.slice(at+q.length)}
  return rest===''};
 const up=v<0?'/':(after[v]==='/'?after.slice(v):'/'+after.slice(v));
 return W(pp,up)};
const u=location.href;
if(!s.match||!s.match.some(p=>H(p,u)))return;if(s.exclude&&s.exclude.some(p=>H(p,u)))return;
const run=function(){__RUN__};
const go=()=>{try{run()}catch(e){try{console.error('[userscript "'+s.name+'"]: '+e)}catch(_){}}};
if(s.at==='start')go();
else if(s.at==='end'){if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',go,{once:true});else go()}
else if(document.readyState==='complete')go();else window.addEventListener('load',go,{once:true});
})()"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_is_read_and_the_defaults_are_filled_in() {
        let src = "// ==UserScript==\n// @name Hide Shorts shelves\n// @match *://*.youtube.com/*\n// @match https://m.youtube.com/*\n// @exclude *://music.youtube.com/*\n// @run-at document-idle\n// @grant none\n// ==/UserScript==\nconsole.log(1)\n";
        let s = parse(src).expect("a header with everything");
        assert_eq!(s.name, "Hide Shorts shelves");
        assert_eq!(s.matches, vec!["*://*.youtube.com/*", "https://m.youtube.com/*"]);
        assert_eq!(s.excludes, vec!["*://music.youtube.com/*"]);
        assert_eq!(s.run_at, RunAt::Idle);
        assert_eq!(s.source, src);
        let s = parse("// ==UserScript==\n// @name x\n// @match https://a/*\n// ==/UserScript==\n").expect("a small header");
        assert_eq!(s.run_at, RunAt::End);
        assert!(s.excludes.is_empty());
        let s = parse("// ==UserScript==\n// @name a\n// @match https://a/*\n// ==/UserScript==\n// ==UserScript==\n// @name b\n// ==/UserScript==\n").expect("two blocks");
        assert_eq!(s.name, "a");
        assert_eq!(s.matches.len(), 1);
    }

    #[test]
    fn a_bad_header_is_no_header() {
        assert!(parse("console.log('hello')").is_none());
        assert!(parse("// ==UserScript==\n// @name x\n// @match https://a/*\n").is_none());
        assert!(parse("// ==/UserScript==\n// @name x\n").is_none());
        assert!(parse("// ==UserScript==\n// @run-at document-later\n// ==/UserScript==\n").is_none());
    }

    #[test]
    fn match_patterns_follow_chrome() {
        let m = pattern_matches;
        assert!(m("<all_urls>", "https://a.example/x?y=1"));
        assert!(m("<all_urls>", "http://127.0.0.1:8765/p"));
        assert!(m("<all_urls>", "file:///C:/notes/x.html"));
        assert!(!m("<all_urls>", "about:blank"));
        assert!(!m("<all_urls>", "data:text/html,hi"));
        assert!(m("*://example.com/*", "http://example.com/"));
        assert!(m("*://example.com/*", "https://example.com/a/b"));
        assert!(!m("*://example.com/*", "file:///C:/x"));
        assert!(m("*://*.example.com/*", "https://example.com/x"));
        assert!(m("*://*.example.com/*", "https://a.b.example.com/x"));
        assert!(!m("*://*.example.com/*", "https://example.com.evil/x"));
        assert!(!m("*://*.example.com/*", "https://notexample.com/x"));
        assert!(m("http://example.com/*", "http://example.com:8080/x"));
        assert!(m("HTTPS://EXAMPLE.com/*", "https://example.com/x"));
        assert!(m("*://*.EXAMPLE.com/*", "https://www.example.com/x"));
        assert!(m("https://a.b/path*", "https://a.b/path"));
        assert!(m("https://a.b/path*", "https://a.b/path/deep?q=1"));
        assert!(!m("https://a.b/path*", "https://a.b/xpath"));
        assert!(!m("https://a.b/xpath", "https://a.b/xpathology"));
        assert!(m("https://a.b/*x*", "https://a.b/axbxc"));
        assert!(!m("https://a.b/path", "http://a.b/path"));
        for bad in ["", "example.com", "https://a.b", "file:///*", "*://*example.com/*"] {
            assert!(!m(bad, "https://a.b/x"), "{bad:?}");
        }
    }

    #[test]
    fn excludes_win_and_no_match_runs_nowhere() {
        let s = Script {
            name: "x".into(),
            matches: vec!["*://*.example.com/*".into()],
            excludes: vec!["*://music.example.com/*".into()],
            run_at: RunAt::End,
            source: String::new(),
        };
        assert!(applies(&s, "https://www.example.com/a"));
        assert!(!applies(&s, "https://music.example.com/a"));
        assert!(!applies(&s, "https://other.test/a"));
        let s = Script { matches: Vec::new(), ..s };
        assert!(!applies(&s, "https://www.example.com/a"));
    }

    #[test]
    fn the_folder_is_read_in_order_and_bad_files_are_skipped() {
        let dir = std::env::temp_dir().join(format!("lumen-userscripts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.user.js"), "// ==UserScript==\n// @match *://b.test/*\n// ==/UserScript==\n").unwrap();
        std::fs::write(dir.join("a.user.js"), "// ==UserScript==\n// @match *://a.test/*\n// ==/UserScript==\n").unwrap();
        std::fs::write(dir.join("c.js"), "// ==UserScript==\n// @match *://c.test/*\n// ==/UserScript==\n").unwrap();
        std::fs::write(dir.join("bad.user.js"), "no header here").unwrap();
        let got = load(&dir);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0].name, "a", "the file name stands in when @name is missing");
        assert_eq!(got[1].name, "b");
        let js = bootstrap_in(&dir);
        assert_eq!(js.len(), 2);
        assert!(js[0].contains("*://a.test/*") && js[0].contains("// ==UserScript=="), "{}", js[0]);
        assert!(!js[0].contains("new Function"), "a strict CSP refuses an eval");
        assert!(bootstrap_in(&dir.join("nope")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
