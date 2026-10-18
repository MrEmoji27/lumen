//! What Lumen knows about a site, apart from its pages: the answers it remembers to
//! permission questions, how loud each kind of question is by default, the zoom a site is
//! read at, and whether plain http may be used. Pure data and small decisions, so the rules
//! can be tested without a window.

use std::collections::{BTreeMap, HashSet};

use crate::store::Rules;

/// One thing a site can ask for. `key` is what permissions.json stores, and what the
/// permission prompt says; `label` is what the Site settings page shows.
pub struct Kind {
    pub key: &'static str,
    pub label: &'static str,
}

/// Every kind there is, the common ones first. The popover under the lock shows the first
/// `COMMON` of them for any site; the rest only appear once a site has been given an answer.
/// The app launcher's own rules ("open mailto", see `extlink`) are not here: they are per
/// scheme, they are listed with the site's other answers, and only that page edits them.
pub const KINDS: &[Kind] = &[
    Kind { key: "camera", label: "Camera" },
    Kind { key: "microphone", label: "Microphone" },
    Kind { key: "location", label: "Location" },
    Kind { key: "notifications", label: "Notifications" },
    Kind { key: "clipboard", label: "Clipboard" },
    Kind { key: "multiple downloads", label: "Multiple downloads" },
    Kind { key: "motion sensors", label: "Motion sensors" },
    Kind { key: "MIDI devices", label: "MIDI devices" },
    Kind { key: "files", label: "Editing files" },
    Kind { key: "fonts", label: "Local fonts" },
    Kind { key: "window placement", label: "Window placement" },
    Kind { key: "autoplay", label: "Sound autoplay" },
];

/// How many of `KINDS` the lock's popover always lists.
pub const COMMON: usize = 5;

/// The words for a kind, or the key itself for one Lumen does not know. An "open <scheme>"
/// answer, which the app launcher writes, reads as the app it opens.
pub fn label(key: &str) -> String {
    if let Some(scheme) = key.strip_prefix("open ") {
        return format!("Open with {}", crate::extlink::app_name(scheme));
    }
    KINDS.iter().find(|k| k.key == key).map_or_else(|| key.to_string(), |k| k.label.to_string())
}

/// What happens when a site asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Block,
    /// Nothing is remembered and the default is to ask: the sidebar asks the person.
    Ask,
}

/// What a kind does when a site has no answer of its own: "block", or anything else (ask).
pub fn default_for(defaults: &BTreeMap<String, String>, kind: &str) -> Verdict {
    match defaults.get(kind).map(String::as_str) {
        Some("block") => Verdict::Block,
        _ => Verdict::Ask,
    }
}

/// The site's own answer wins; without one, the default for the kind.
pub fn verdict(rules: &Rules, defaults: &BTreeMap<String, String>, origin: &str, kind: &str) -> Verdict {
    match rules.get(origin).and_then(|r| r.get(kind)) {
        Some(true) => Verdict::Allow,
        Some(false) => Verdict::Block,
        None => default_for(defaults, kind),
    }
}

/// Remember an answer. `Ask` forgets it, and a site with nothing left is dropped.
pub fn set_rule(rules: &mut Rules, origin: &str, kind: &str, v: Verdict) {
    match v {
        Verdict::Ask => {
            if let Some(r) = rules.get_mut(origin) {
                r.remove(kind);
                if r.is_empty() {
                    rules.remove(origin);
                }
            }
        }
        v => {
            rules.entry(origin.to_string()).or_default().insert(kind.to_string(), v == Verdict::Allow);
        }
    }
}

/// The answer a site has for a kind: Some(allowed), or None for "ask".
#[allow(dead_code)]
pub fn answer(rules: &Rules, origin: &str, kind: &str) -> Option<bool> {
    rules.get(origin).and_then(|r| r.get(kind)).copied()
}

/// The pairs of origin and its host as the pages show it (no www.), for the lock popover.
pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or("");
    let h = rest.split(['/', '?', '#']).next().unwrap_or("");
    let h = h.rsplit('@').next().unwrap_or(h);
    h.to_string()
}

/// A default-behaviour map from the pairs the Settings page edits.
pub fn set_default(defaults: &mut BTreeMap<String, String>, kind: &str, v: Verdict) {
    if v == Verdict::Block {
        defaults.insert(kind.to_string(), "block".into());
    } else {
        defaults.remove(kind);
    }
}

// ------------------------------------------------------------------- zoom

/// Zoom by host, in percent. Only hosts that differ from the default zoom are kept.
pub type Zooms = BTreeMap<String, u32>;

/// Chromium's range: a quarter to five times.
pub const ZOOM_MIN: u32 = 25;
pub const ZOOM_MAX: u32 = 500;

pub fn clamp_pct(p: u32) -> u32 {
    p.clamp(ZOOM_MIN, ZOOM_MAX)
}

/// The host a zoom belongs to: the lower-cased name of an http(s) page, no port.
pub fn zoom_host(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    let h = host_of(&lower);
    // [::1]:8080 keeps its brackets; a name loses :8080.
    let h = if h.starts_with('[') { h.split_once(']').map_or(h.clone(), |(a, _)| format!("{a}]")) } else { h.split(':').next().unwrap_or("").to_string() };
    (!h.is_empty()).then_some(h)
}

/// WebView2's factor (1.25) as a whole percent (125).
pub fn pct_of(factor: f64) -> u32 {
    clamp_pct((factor * 100.0).round().max(0.0) as u32)
}

pub fn factor_of(pct: u32) -> f64 {
    clamp_pct(pct) as f64 / 100.0
}

/// The zoom a page of `host` opens at.
pub fn zoom_for(zooms: &Zooms, default_pct: u32, host: &str) -> u32 {
    zooms.get(host).copied().map_or(clamp_pct(default_pct), clamp_pct)
}

/// A person changed the zoom of `host` to `pct`. Returns whether the map changed. A zoom equal
/// to the default is no entry at all, so changing the default moves every site that never
/// had a choice of its own.
pub fn remember_zoom(zooms: &mut Zooms, default_pct: u32, host: &str, pct: u32) -> bool {
    let pct = clamp_pct(pct);
    if pct == clamp_pct(default_pct) {
        zooms.remove(host).is_some()
    } else {
        zooms.insert(host.to_string(), pct) != Some(pct)
    }
}

// ------------------------------------------------------------------ https

/// What the address bar and links are allowed to do with plain http.
#[derive(Default, Clone)]
pub struct HttpsPolicy {
    pub on: bool,
    /// Hosts the person chose to keep on http, for good (settings.json).
    pub exceptions: HashSet<String>,
    /// Hosts the person chose to open once; they last until Lumen closes.
    pub passes: HashSet<String>,
}

/// Names that only mean something on a home or office network: no public certificate exists
/// for them, so upgrading would only break them.
const LOCAL_SUFFIXES: &[&str] = &[".local", ".lan", ".internal", ".home", ".home.arpa", ".localdomain", ".intranet", ".corp", ".private", ".localhost"];

/// Hosts that never go to https: this machine, numbers, a one-word name, a private-network name.
pub fn is_local_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    let h = h.trim_end_matches('.');
    h == "localhost"
        || h.starts_with('[')
        || h.parse::<std::net::IpAddr>().is_ok()
        || !h.contains('.')
        || LOCAL_SUFFIXES.iter().any(|s| h.ends_with(s))
}

/// An http page that should be tried as https first: the https address, or None when the page
/// is not plain http, is on a local network, has a port of its own (a dev server), or the
/// person said http is fine for the host.
pub fn https_upgrade(url: &str, p: &HttpsPolicy) -> Option<String> {
    if !p.on {
        return None;
    }
    let rest = url.get(..7).filter(|s| s.eq_ignore_ascii_case("http://")).map(|_| &url[7..])?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = if hostport.starts_with('[') {
        (hostport, "")
    } else {
        hostport.split_once(':').unwrap_or((hostport, ""))
    };
    if host.is_empty() || is_local_host(host) {
        return None;
    }
    if !port.is_empty() && port != "80" {
        return None;
    }
    let host = host.to_ascii_lowercase();
    if p.exceptions.contains(&host) || p.passes.contains(&host) {
        return None;
    }
    let tail = &rest[authority.len()..];
    let cleaned = if port == "80" { authority.strip_suffix(":80").unwrap_or(authority) } else { authority };
    Some(format!("https://{cleaned}{tail}"))
}

/// Whether a site answers where https lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Something listens on the https port: go there. A bad certificate is the browser's to report.
    Open,
    /// The name exists and the https port is shut or silent: this site has no https.
    Shut,
    /// The name did not resolve, or there is no network: not the site's fault. Let https show why.
    Unknown,
}

/// Try the https port of `host_port` ("example.com:443"). Blocks up to `wait`; call it from a thread.
pub fn probe(host_port: &str, wait: std::time::Duration) -> Probe {
    use std::net::{TcpStream, ToSocketAddrs};
    let Ok(addrs) = host_port.to_socket_addrs() else { return Probe::Unknown };
    let mut tried = false;
    for a in addrs {
        tried = true;
        if TcpStream::connect_timeout(&a, wait).is_ok() {
            return Probe::Open;
        }
    }
    if tried { Probe::Shut } else { Probe::Unknown }
}

/// The host of an http URL, lower-cased, without port: the key exceptions use.
pub fn exception_host(url: &str) -> Option<String> {
    zoom_host(url)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The page shown when a site would not talk https. Plain words, two ways forward, one way out.
/// The links use a scheme of Lumen's own, which the navigation hook picks up.
pub fn interstitial_html(http_url: &str, dark: bool) -> String {
    let host = esc(&host_of(http_url));
    let url = esc(http_url);
    let (bg, fg, dim, line, accent) = if dark { ("#161413", "#ece8e3", "#a29c94", "#2c2927", "#e0b070") } else { ("#f7f4f0", "#1d1a17", "#6e675f", "#ddd7cf", "#9a5b13") };
    format!(
        "<!doctype html><meta charset=utf-8><title>{host} does not use https</title>\
<style>body{{margin:0;background:{bg};color:{fg};font:15px/1.55 'Segoe UI',system-ui,sans-serif;display:grid;place-items:center;min-height:100vh}}\
main{{max-width:520px;padding:32px}}h1{{font:600 24px 'Segoe UI',system-ui,sans-serif;margin:0 0 10px}}p{{color:{dim};margin:0 0 22px}}\
code{{color:{fg}}}a{{display:inline-block;margin:0 10px 10px 0;padding:8px 16px;border:1px solid {line};border-radius:8px;color:{fg};text-decoration:none}}\
a.go{{background:{accent};border-color:{accent};color:{bg};font-weight:600}}a:hover{{border-color:{fg}}}</style>\
<main><h1>This site does not use https</h1>\
<p>Lumen tried <code>{host}</code> over a secure connection first, and it did not work. \
Over plain http, anyone on the way can read what you send and change what comes back. Do not type a password here.</p>\
<a href=\"lumen-http:back\">Go back</a>\
<a href=\"lumen-http:once:{url}\">Continue this time</a>\
<a class=go href=\"lumen-http:always:{url}\">Always allow http for this site</a></main>"
    )
}

/// What a click on the page above means.
#[derive(Debug, PartialEq)]
pub enum HttpChoice {
    Back,
    Once(String),
    Always(String),
}

/// Read one of the page's links, if it is one.
pub fn parse_http_choice(link: &str) -> Option<HttpChoice> {
    let rest = link.strip_prefix("lumen-http:")?;
    if rest == "back" {
        return Some(HttpChoice::Back);
    }
    let (kind, url) = rest.split_once(':')?;
    if !url.to_ascii_lowercase().starts_with("http://") {
        return None;
    }
    match kind {
        "once" => Some(HttpChoice::Once(url.to_string())),
        "always" => Some(HttpChoice::Always(url.to_string())),
        _ => None,
    }
}

/// The page shown when a top-level navigation hits a host on the malware or
/// phishing lists. Plain words, two ways forward. The links use a scheme of
/// Lumen's own, which the navigation hook picks up.
pub fn danger_html(url: &str, dark: bool) -> String {
    let host = esc(&host_of(url));
    let url = esc(url);
    let (bg, fg, dim, line, accent) = if dark { ("#161413", "#ece8e3", "#a29c94", "#2c2927", "#e0b070") } else { ("#f7f4f0", "#1d1a17", "#6e675f", "#ddd7cf", "#9a5b13") };
    format!(
        "<!doctype html><meta charset=utf-8><title>{host} is a dangerous site</title>\
<style>body{{margin:0;background:{bg};color:{fg};font:15px/1.55 'Segoe UI',system-ui,sans-serif;display:grid;place-items:center;min-height:100vh}}\
main{{max-width:520px;padding:32px}}h1{{font:600 24px 'Segoe UI',system-ui,sans-serif;margin:0 0 10px}}p{{color:{dim};margin:0 0 22px}}\
code{{color:{fg}}}a{{display:inline-block;margin:0 10px 10px 0;padding:8px 16px;border:1px solid {line};border-radius:8px;color:{fg};text-decoration:none}}\
a.go{{background:{accent};border-color:{accent};color:{bg};font-weight:600}}a:hover{{border-color:{fg}}}</style>\
<main><h1>Lumen blocked a dangerous site</h1>\
<p><code>{host}</code> is on a malware and phishing blocklist. Opening it could put this computer or your accounts at risk. \
If you did not mean to go here, go back.</p>\
<a href=\"lumen-danger:back\">Go back</a>\
<a class=go href=\"lumen-danger:continue:{url}\">Continue anyway</a></main>"
    )
}

/// Lumen's page for a page that did not load: what went wrong, in words, and Try again. It
/// takes the place of Chromium's own error page. `err` is WebView2's WebErrorStatus.
pub fn error_html(url: &str, err: i32, dark: bool, back: bool) -> String {
    let host = esc(&host_of(url));
    let (title, words) = error_words(err, &host);
    let url = esc(url);
    let (bg, fg, dim, line, accent) = if dark { ("#161413", "#ece8e3", "#a29c94", "#2c2927", "#ece8e3") } else { ("#f7f4f0", "#1d1a17", "#6e675f", "#ddd7cf", "#1d1a17") };
    let back = if back { "<a href=\"lumen-error:back\">Go back</a>" } else { "" };
    format!(
        "<!doctype html><meta charset=utf-8><title>{host}</title>\
<style>body{{margin:0;background:{bg};color:{fg};font:15px/1.55 'Segoe UI',system-ui,sans-serif;display:grid;place-items:center;min-height:100vh}}\
main{{max-width:520px;padding:32px}}h1{{font:600 24px 'Segoe UI',system-ui,sans-serif;margin:0 0 10px}}p{{color:{dim};margin:0 0 22px}}\
small{{display:block;color:{dim};opacity:.7;margin:-10px 0 22px;font-size:12px}}\
a{{display:inline-block;margin:0 10px 10px 0;padding:8px 16px;border:1px solid {line};border-radius:8px;color:{fg};text-decoration:none}}\
a.go{{background:{accent};border-color:{accent};color:{bg};font-weight:600}}a:hover{{border-color:{fg}}}</style>\
<main><h1>{title}</h1><p>{words}</p><small>WebView2 error {err}</small>\
<a class=go href=\"lumen-error:retry:{url}\">Try again</a>{back}</main>"
    )
}

/// The title and words for a WebErrorStatus. The host is already escaped.
fn error_words(err: i32, host: &str) -> (String, String) {
    match err {
        13 => (format!("Lumen could not find {host}"), "Check the address for a typo. If it is right, the site may be down, or this PC may be offline.".into()),
        11 => ("This PC is offline".into(), "Lumen could not reach the internet. Check the Wi-Fi or the cable, then try again.".into()),
        7 => (format!("{host} took too long"), "The site did not answer in time. It may be busy or down.".into()),
        6 | 12 => (format!("{host} did not answer"), "The site refused the connection, or nothing is there. It may be down.".into()),
        9 | 10 => (format!("The connection to {host} was cut"), "Something between this PC and the site dropped the connection. Trying again often works.".into()),
        1..=5 => (
            format!("{host} is not who it says it is"),
            "Its security certificate is not valid, so Lumen stopped before sending anything. Someone may be pretending to be this site.".into(),
        ),
        15 => (format!("{host} redirects in a loop"), "The site keeps sending Lumen somewhere else. Clearing the site's data may fix it.".into()),
        _ => (format!("{host} did not load"), "Something went wrong between this PC and the site.".into()),
    }
}

/// A click on the error page above.
#[derive(Debug, PartialEq)]
pub enum ErrorChoice {
    Back,
    Retry(String),
}

/// Read one of the error page's links. Only http(s) may be retried.
pub fn parse_error_choice(link: &str) -> Option<ErrorChoice> {
    let rest = link.strip_prefix("lumen-error:")?;
    if rest == "back" {
        return Some(ErrorChoice::Back);
    }
    let url = rest.strip_prefix("retry:")?;
    url.to_ascii_lowercase().starts_with("http").then(|| ErrorChoice::Retry(url.to_string()))
}

/// What a click on the page above means.
#[derive(Debug, PartialEq)]
pub enum DangerChoice {
    Back,
    Continue(String),
}

/// Read one of the page's links, if it is one. Only http(s) may be continued:
/// a page cannot smuggle a file or script through the link.
pub fn parse_danger_choice(link: &str) -> Option<DangerChoice> {
    let rest = link.strip_prefix("lumen-danger:")?;
    if rest == "back" {
        return Some(DangerChoice::Back);
    }
    let (kind, url) = rest.split_once(':')?;
    if kind != "continue" {
        return None;
    }
    if !url.to_ascii_lowercase().starts_with("http") {
        return None;
    }
    Some(DangerChoice::Continue(url.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        let mut r = Rules::new();
        set_rule(&mut r, "https://meet.example", "camera", Verdict::Allow);
        set_rule(&mut r, "https://meet.example", "microphone", Verdict::Block);
        r
    }

    #[test]
    fn a_remembered_answer_beats_the_default() {
        let mut d = BTreeMap::new();
        set_default(&mut d, "camera", Verdict::Block);
        let r = rules();
        assert_eq!(verdict(&r, &d, "https://meet.example", "camera"), Verdict::Allow);
        assert_eq!(verdict(&r, &d, "https://meet.example", "microphone"), Verdict::Block);
        // No answer for this site: the default for the kind decides.
        assert_eq!(verdict(&r, &d, "https://other.example", "camera"), Verdict::Block);
        assert_eq!(verdict(&r, &d, "https://other.example", "location"), Verdict::Ask);
    }

    #[test]
    fn asking_forgets_the_answer_and_empty_sites_go() {
        let mut r = rules();
        set_rule(&mut r, "https://meet.example", "camera", Verdict::Ask);
        assert_eq!(answer(&r, "https://meet.example", "camera"), None);
        assert_eq!(answer(&r, "https://meet.example", "microphone"), Some(false));
        set_rule(&mut r, "https://meet.example", "microphone", Verdict::Ask);
        assert!(r.is_empty());
        // Forgetting what was never there is nothing.
        set_rule(&mut r, "https://nobody.example", "camera", Verdict::Ask);
        assert!(r.is_empty());
    }

    #[test]
    fn rules_and_defaults_survive_the_files() {
        let r = rules();
        let back: Rules = serde_json::from_str(&serde_json::to_string(&r).expect("rules to json")).expect("rules from json");
        assert_eq!(back, r);
        let mut s = crate::store::Settings::default();
        set_default(&mut s.perm_defaults, "notifications", Verdict::Block);
        let back: crate::store::Settings = serde_json::from_str(&serde_json::to_string(&s).expect("settings to json")).expect("settings from json");
        assert_eq!(default_for(&back.perm_defaults, "notifications"), Verdict::Block);
        assert_eq!(default_for(&back.perm_defaults, "camera"), Verdict::Ask);
        // Back to asking leaves nothing behind.
        let mut d = back.perm_defaults;
        set_default(&mut d, "notifications", Verdict::Ask);
        assert!(d.is_empty());
    }

    #[test]
    fn an_old_settings_file_asks_about_everything() {
        let mut v = serde_json::to_value(crate::store::Settings::default()).expect("settings to json");
        v.as_object_mut().expect("an object").remove("perm_defaults");
        let s: crate::store::Settings = serde_json::from_value(v).expect("old file loads");
        assert!(s.perm_defaults.is_empty());
        assert_eq!(default_for(&s.perm_defaults, "location"), Verdict::Ask);
    }

    #[test]
    fn every_kind_webview2_names_has_a_row() {
        for k in ["camera", "microphone", "location", "notifications", "clipboard", "multiple downloads", "motion sensors", "MIDI devices", "files", "fonts", "window placement", "autoplay"] {
            assert!(KINDS.iter().any(|x| x.key == k), "{k} has no row on the Site settings page");
        }
        assert_eq!(label("camera"), "Camera");
        assert_eq!(label("something"), "something");
        // The app launcher's per-scheme answers read as the app they open.
        assert_eq!(label("open mailto"), "Open with your mail app");
        assert_eq!(label("open spotify"), "Open with Spotify");
    }

    #[test]
    fn zoom_hosts_come_from_http_pages_only() {
        assert_eq!(zoom_host("https://Docs.Example.com:8443/a?b#c").as_deref(), Some("docs.example.com"));
        assert_eq!(zoom_host("http://localhost:3000/").as_deref(), Some("localhost"));
        assert_eq!(zoom_host("http://[::1]:3000/x").as_deref(), Some("[::1]"));
        assert_eq!(zoom_host("about:blank"), None);
        assert_eq!(zoom_host("data:text/html,hi"), None);
        assert_eq!(zoom_host("file:///c:/a.html"), None);
    }

    #[test]
    fn zoom_map_keeps_only_what_differs_from_the_default() {
        let mut z = Zooms::new();
        assert!(remember_zoom(&mut z, 100, "a.test", 150));
        assert_eq!(zoom_for(&z, 100, "a.test"), 150);
        assert_eq!(zoom_for(&z, 100, "b.test"), 100);
        // The same value again changes nothing.
        assert!(!remember_zoom(&mut z, 100, "a.test", 150));
        // Back to the default: the entry goes.
        assert!(remember_zoom(&mut z, 100, "a.test", 100));
        assert!(z.is_empty());
        // With a default of 125, a site read at 100 is a choice and is kept; at 125 it is not.
        assert!(remember_zoom(&mut z, 125, "c.test", 100));
        assert_eq!(zoom_for(&z, 125, "c.test"), 100);
        assert_eq!(zoom_for(&z, 125, "d.test"), 125);
        assert!(remember_zoom(&mut z, 125, "c.test", 125));
        assert!(z.is_empty());
    }

    #[test]
    fn zoom_stays_inside_what_chromium_allows() {
        assert_eq!(pct_of(1.25), 125);
        assert_eq!(pct_of(0.0), ZOOM_MIN);
        assert_eq!(pct_of(9.0), ZOOM_MAX);
        assert_eq!(factor_of(150), 1.5);
        assert_eq!(factor_of(5), 0.25);
        let mut z = Zooms::new();
        remember_zoom(&mut z, 100, "x.test", 10_000);
        assert_eq!(z["x.test"], ZOOM_MAX);
    }

    fn policy() -> HttpsPolicy {
        HttpsPolicy { on: true, exceptions: ["old.example".to_string()].into(), passes: HashSet::new() }
    }

    #[test]
    fn plain_http_goes_to_https_first() {
        let p = policy();
        assert_eq!(https_upgrade("http://example.com/a?b=1#c", &p).as_deref(), Some("https://example.com/a?b=1#c"));
        assert_eq!(https_upgrade("HTTP://Example.com", &p).as_deref(), Some("https://Example.com"));
        assert_eq!(https_upgrade("http://example.com:80/x", &p).as_deref(), Some("https://example.com/x"));
        assert_eq!(https_upgrade("http://user:pw@example.com/x", &p).as_deref(), Some("https://user:pw@example.com/x"));
    }

    #[test]
    fn https_first_leaves_alone_what_it_cannot_help() {
        let p = policy();
        // Already secure, or not http at all.
        assert_eq!(https_upgrade("https://example.com/", &p), None);
        assert_eq!(https_upgrade("data:text/html,hi", &p), None);
        assert_eq!(https_upgrade("about:blank", &p), None);
        assert_eq!(https_upgrade("file:///c:/a.html", &p), None);
        // This machine, numbers, one-word names, home networks, dev servers.
        for u in [
            "http://localhost/", "http://localhost:3000/", "http://127.0.0.1:8080/", "http://192.168.1.10/", "http://[::1]/", "http://nas/",
            "http://printer.local/", "http://router.lan/", "http://example.com:8080/", "http://app.localhost/",
        ] {
            assert_eq!(https_upgrade(u, &p), None, "{u}");
        }
        // A host the person said http is fine for, and one they opened once.
        assert_eq!(https_upgrade("http://old.example/x", &p), None);
        let mut p = policy();
        p.passes.insert("once.example".into());
        assert_eq!(https_upgrade("http://once.example/x", &p), None);
        assert!(https_upgrade("http://other.example/x", &p).is_some());
        // Off means off.
        p.on = false;
        assert_eq!(https_upgrade("http://other.example/x", &p), None);
    }

    #[test]
    fn the_probe_tells_an_open_port_from_a_shut_one() {
        use std::time::Duration;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let open = listener.local_addr().expect("its address").to_string();
        assert_eq!(probe(&open, Duration::from_secs(2)), Probe::Open);
        drop(listener);
        // The port was just given back: nothing listens now.
        assert_eq!(probe(&open, Duration::from_secs(2)), Probe::Shut);
        // A name that cannot exist says nothing about the site.
        assert_eq!(probe("no-such-host.invalid:443", Duration::from_secs(2)), Probe::Unknown);
    }

    #[test]
    fn the_warning_page_links_come_back_as_choices() {
        let html = interstitial_html("http://old.example/a?x=1&y=2", true);
        assert!(html.contains("old.example"));
        assert!(html.contains("lumen-http:once:http://old.example/a?x=1&amp;y=2"));
        assert_eq!(parse_http_choice("lumen-http:back"), Some(HttpChoice::Back));
        assert_eq!(parse_http_choice("lumen-http:once:http://old.example/a?x=1&y=2"), Some(HttpChoice::Once("http://old.example/a?x=1&y=2".into())));
        assert_eq!(parse_http_choice("lumen-http:always:http://old.example/"), Some(HttpChoice::Always("http://old.example/".into())));
        // Nothing that is not http can be smuggled through the link.
        assert_eq!(parse_http_choice("lumen-http:once:file:///c:/secret"), None);
        assert_eq!(parse_http_choice("lumen-http:once:javascript:alert(1)"), None);
        assert_eq!(parse_http_choice("https://example.com/"), None);
    }

    #[test]
    fn the_danger_page_links_come_back_as_choices() {
        let html = danger_html("https://evil.example.com/steal?x=1&y=2", true);
        assert!(html.contains("evil.example.com"));
        assert!(html.contains("lumen-danger:continue:https://evil.example.com/steal?x=1&amp;y=2"));
        assert_eq!(parse_error_choice("lumen-error:back"), Some(ErrorChoice::Back));
        assert_eq!(parse_error_choice("lumen-error:retry:https://a.example/x?y=1"), Some(ErrorChoice::Retry("https://a.example/x?y=1".into())));
        assert_eq!(parse_error_choice("lumen-error:retry:file:///c:/secret"), None);
        assert!(error_html("https://a.example/", 13, false, true).contains("could not find a.example"));
        assert!(error_html("https://<b>/", 99, true, false).contains("&lt;b&gt;"), "the host is escaped");
        assert_eq!(parse_danger_choice("lumen-danger:back"), Some(DangerChoice::Back));
        assert_eq!(parse_danger_choice("lumen-danger:continue:https://evil.example.com/steal?x=1&y=2"), Some(DangerChoice::Continue("https://evil.example.com/steal?x=1&y=2".into())));
        assert_eq!(parse_danger_choice("lumen-danger:continue:http://evil.example.com/"), Some(DangerChoice::Continue("http://evil.example.com/".into())));
        // A page cannot smuggle a file or script through the link.
        assert_eq!(parse_danger_choice("lumen-danger:continue:file:///c:/secret"), None);
        assert_eq!(parse_danger_choice("lumen-danger:continue:javascript:alert(1)"), None);
        assert_eq!(parse_danger_choice("lumen-danger:allow:https://x.example/"), None);
        assert_eq!(parse_danger_choice("https://example.com/"), None);
    }
}
