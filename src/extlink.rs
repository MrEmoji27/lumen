//! Links that are not web pages: mailto:, tel:, spotify:, zoommtg:, steam:, ms-settings: ...

use std::{collections::HashMap, time::{Duration, Instant}};

use crate::store::Rules;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Open,
    Ask,
    Drop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Once,
    Always,
    Cancel,
}

const NEVER: &[&str] = &[
    "javascript", "vbscript", "data", "file", "blob", "about", "chrome", "edge", "view-source", "ms-msdt", "ms-appinstaller", "ms-officecmd",
    "search-ms", "search", "shell", "ms-cxh", "ms-cxh-full", "ms-gamebarservices", "ms-gamingoverlay", "ms-ipp", "ms-ipps", "ldap", "ldaps", "ftp", "ws", "wss", "http", "https",
    "magnet", "ms-browser-extension", "ms-search", "ms-nfp", "ms-shell", "ms-quick-assist",
];

const MAX_URI: usize = 2048;

const COOLDOWN: Duration = Duration::from_secs(30);

pub fn scheme_of(uri: &str) -> Option<String> {
    if uri.len() > MAX_URI || uri.chars().any(|c| c.is_control()) {
        return None;
    }
    let (name, rest) = uri.split_once(':')?;
    let ok = name.len() >= 2
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    let port = rest.split(['/', '?', '#']).next().unwrap_or("");
    if !ok || (!port.is_empty() && port.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let s = name.to_ascii_lowercase();
    (!NEVER.contains(&s.as_str())).then_some(s)
}

pub fn rule_kind(scheme: &str) -> String {
    format!("open {scheme}")
}

pub fn decide(rules: &Rules, origin: &str, scheme: &str, user: bool, agent: bool) -> Decision {
    if agent {
        return Decision::Drop;
    }
    match rules.get(origin).and_then(|r| r.get(&rule_kind(scheme))).copied() {
        Some(true) => Decision::Open,
        Some(false) => Decision::Drop,
        None if !user => Decision::Drop,
        None => Decision::Ask,
    }
}

pub fn app_name(scheme: &str) -> String {
    let known = match scheme {
        "mailto" => "your mail app",
        "tel" | "callto" | "sms" | "smsto" => "your phone app",
        "spotify" => "Spotify",
        "zoommtg" | "zoomus" => "Zoom",
        "steam" => "Steam",
        "discord" => "Discord",
        "slack" => "Slack",
        "msteams" => "Microsoft Teams",
        "skype" => "Skype",
        "tg" => "Telegram",
        "whatsapp" => "WhatsApp",
        "vscode" | "vscode-insiders" => "Visual Studio Code",
        "obsidian" => "Obsidian",
        "notion" => "Notion",
        "figma" => "Figma",
        "itms" | "itms-apps" | "music" => "iTunes",
        "ms-settings" => "Windows Settings",
        "ms-word" => "Word",
        "ms-excel" => "Excel",
        "ms-powerpoint" => "PowerPoint",
        "ms-outlook" | "outlook" => "Outlook",
        "ms-windows-store" => "the Microsoft Store",
        "webcal" | "webcals" => "your calendar",
        "facetime" => "FaceTime",
        s if s.starts_with("ms-") => return "a Windows app".into(),
        s => return s.to_string(),
    };
    known.to_string()
}

pub fn prompt_words(origin: &str, scheme: &str) -> (String, String) {
    let app = app_name(scheme);
    let site = origin.split_once("://").map_or(origin, |(_, h)| h);
    let site = if site.is_empty() || site == "null" { "This page" } else { site };
    let title = format!("Open {app}?");
    (title, format!("{site} wants to open {app}."))
}

#[derive(Default)]
pub struct Gate {
    asking: HashMap<(String, String), ()>,
    muted: HashMap<(String, String), Instant>,
}

impl Gate {
    pub fn begin(&mut self, origin: &str, scheme: &str, now: Instant) -> bool {
        let k = (origin.to_string(), scheme.to_string());
        self.muted.retain(|_, until| *until > now);
        if self.asking.contains_key(&k) || self.muted.contains_key(&k) {
            return false;
        }
        self.asking.insert(k, ());
        true
    }

    pub fn finish(&mut self, origin: &str, scheme: &str, answer: Answer, now: Instant) {
        let k = (origin.to_string(), scheme.to_string());
        self.asking.remove(&k);
        if answer == Answer::Cancel {
            self.muted.insert(k, now + COOLDOWN);
        }
    }
}

pub fn launch(uri: &str) -> bool {
    use windows::{
        Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        core::{HSTRING, w},
    };
    if scheme_of(uri).is_none() {
        return false;
    }
    let r = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(uri), None, None, SW_SHOWNORMAL) };
    r.0 as usize > 32
}

#[cfg(test)]
mod tests {
    use super::*;

    const SITE: &str = "https://open.spotify.com";

    fn rules(origin: &str, kind: &str, allow: bool) -> Rules {
        let mut r = Rules::new();
        r.entry(origin.into()).or_default().insert(kind.into(), allow);
        r
    }

    #[test]
    fn schemes_are_read_and_checked() {
        assert_eq!(scheme_of("spotify:track:123").as_deref(), Some("spotify"));
        assert_eq!(scheme_of("MAILTO:a@b.com").as_deref(), Some("mailto"));
        assert_eq!(scheme_of("ms-settings:network").as_deref(), Some("ms-settings"));
        assert_eq!(scheme_of("zoommtg://zoom.us/join?confno=1").as_deref(), Some("zoommtg"));
        assert_eq!(scheme_of("steam://run/440").as_deref(), Some("steam"));
        assert_eq!(scheme_of("tel:+15551234").as_deref(), Some("tel"));
    }

    #[test]
    fn dangerous_or_broken_addresses_are_refused() {
        for u in [
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "data:text/html,x",
            "ms-msdt:/id PCWDiagnostic /skip force /param \"IT_RebrowseForFile=?",
            "search-ms:query=x&crumb=location:\\\\evil\\share",
            "ms-appinstaller:?source=https://evil/x.appx",
            "ms-officecmd:{}",
            "https://example.com",
            "magnet:?xt=urn:btih:1",
            "no scheme here",
            "localhost:3000",
            "1abc:x",
            "a:b",
            "spotify:track\r\nx",
            "",
        ] {
            assert_eq!(scheme_of(u), None, "{u}");
        }
        assert_eq!(scheme_of(&format!("spotify:{}", "a".repeat(3000))), None);
    }

    #[test]
    fn the_first_time_it_asks() {
        assert_eq!(decide(&Rules::new(), SITE, "spotify", true, false), Decision::Ask);
    }

    #[test]
    fn always_opens_without_asking() {
        let r = rules(SITE, "open spotify", true);
        assert_eq!(decide(&r, SITE, "spotify", true, false), Decision::Open);
        assert_eq!(decide(&r, SITE, "spotify", false, false), Decision::Open);
    }

    #[test]
    fn always_is_for_that_site_and_that_app() {
        let r = rules(SITE, "open spotify", true);
        assert_eq!(decide(&r, "https://evil.example", "spotify", true, false), Decision::Ask);
        assert_eq!(decide(&r, SITE, "steam", true, false), Decision::Ask);
        assert_eq!(decide(&r, "http://open.spotify.com", "spotify", true, false), Decision::Ask);
    }

    #[test]
    fn a_block_rule_is_silent() {
        let r = rules(SITE, "open spotify", false);
        assert_eq!(decide(&r, SITE, "spotify", true, false), Decision::Drop);
    }

    #[test]
    fn a_page_nobody_clicked_does_not_get_to_ask() {
        assert_eq!(decide(&Rules::new(), SITE, "spotify", false, false), Decision::Drop);
    }

    #[test]
    fn agent_tabs_never_open_anything() {
        let r = rules(SITE, "open spotify", true);
        assert_eq!(decide(&r, SITE, "spotify", true, true), Decision::Drop);
        assert_eq!(decide(&Rules::new(), SITE, "spotify", true, true), Decision::Drop);
    }

    #[test]
    fn other_permission_rules_do_not_count() {
        let r = rules(SITE, "camera", true);
        assert_eq!(decide(&r, SITE, "spotify", true, false), Decision::Ask);
    }

    #[test]
    fn the_words() {
        assert_eq!(app_name("spotify"), "Spotify");
        assert_eq!(app_name("zoommtg"), "Zoom");
        assert_eq!(app_name("ms-gamebar"), "a Windows app");
        assert_eq!(app_name("weirdthing"), "weirdthing");
        assert_eq!(prompt_words(SITE, "spotify"), ("Open Spotify?".into(), "open.spotify.com wants to open Spotify.".into()));
        assert_eq!(prompt_words("https://a.com", "mailto").0, "Open your mail app?");
        assert_eq!(prompt_words("null", "steam").1, "This page wants to open Steam.");
    }

    #[test]
    fn gate_asks_one_at_a_time_and_remembers_a_no() {
        let mut g = Gate::default();
        let t0 = Instant::now();
        assert!(g.begin(SITE, "spotify", t0));
        assert!(!g.begin(SITE, "spotify", t0));
        assert!(g.begin(SITE, "steam", t0));
        assert!(g.begin("https://b.com", "spotify", t0));
        g.finish(SITE, "spotify", Answer::Cancel, t0);
        assert!(!g.begin(SITE, "spotify", t0 + Duration::from_secs(5)));
        assert!(g.begin(SITE, "spotify", t0 + COOLDOWN + Duration::from_secs(1)));
        g.finish(SITE, "steam", Answer::Once, t0);
        assert!(g.begin(SITE, "steam", t0));
    }
}
