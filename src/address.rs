//! What the address bar was handed: a place to go, or words to search for.

use std::net::Ipv4Addr;

const CC: &str = "ac ad ae af ag ai al am ao aq ar as at au aw ax az ba bb bd be bf bg bh bi bj bm bn bo br bs bt bw by bz ca cc cd cf cg ch ci ck cl cm cn co cr cu cv cw cx cy cz de dj dk dm do dz ec ee eg er es et eu fi fj fk fm fo fr ga gd ge gf gg gh gi gl gm gn gp gq gr gs gt gu gw gy hk hm hn hr ht hu id ie il im in io iq ir is it je jm jo jp ke kg kh ki km kn kp kr kw ky kz la lb lc li lk lr ls lt lu lv ly ma mc md me mg mh mk ml mm mn mo mp mq mr ms mt mu mv mw mx my mz na nc ne nf ng ni nl no np nr nu nz om pa pe pf pg ph pk pl pm pn pr ps pt pw py qa re ro rs ru rw sa sb sc sd se sg sh si sk sl sm sn so sr ss st su sv sx sy sz tc td tf tg th tj tk tl tm tn to tr tt tv tw tz ua ug uk us uy uz va vc ve vg vi vn vu wf ws ye yt za zm zw";

const GENERIC: &[&str] = &[
    "academy", "agency", "app", "art", "asia", "biz", "blog", "cafe", "cash", "center", "chat", "city", "click", "club", "cloud", "codes", "com",
    "company", "computer", "coop", "dev", "digital", "directory", "domains", "download", "earth", "edu", "email", "energy", "engineer", "events",
    "expert", "family", "fun", "game", "games", "gmbh", "gov", "graphics", "group", "guide", "guru", "health", "help", "host", "house",
    "info", "institute", "int", "international", "jobs", "land", "life", "limited", "link", "live", "llc", "ltd", "mil", "mobi", "name", "net",
    "network", "news", "ninja", "one", "online", "org", "page", "photo", "photography", "pics", "pizza", "plus", "pro", "red", "rocks", "run",
    "school", "science", "services", "shop", "site", "social", "software", "solutions", "space", "store", "studio", "support", "systems", "team",
    "tech", "technology", "tel", "today", "tools", "top", "university", "vip", "website", "wiki", "work", "works", "world", "xyz", "zone",
];

pub fn known_tld(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    if l.len() == 2 {
        return CC.split_ascii_whitespace().any(|c| c == l);
    }
    l.starts_with("xn--") || GENERIC.contains(&l.as_str())
}

pub fn looks_like_host(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    let host = lower.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    if host.starts_with('[') && host.contains(']') {
        return true;
    }
    let host = host.rsplit_once(':').map_or(host, |(h, port)| {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) { h } else { host }
    });
    if host == "localhost" || host.parse::<Ipv4Addr>().is_ok() {
        return true;
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    let Some((name, tld)) = host.rsplit_once('.') else { return false };
    !name.is_empty() && !name.ends_with('.') && known_tld(tld)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn country_codes_are_pairs() {
        assert!(CC.split_ascii_whitespace().all(|c| c.len() == 2 && c.chars().all(|x| x.is_ascii_lowercase())));
    }

    #[test]
    fn places() {
        for w in [
            "music.youtube.com",
            "youtube.com",
            "example.co.uk",
            "localhost",
            "localhost:3000",
            "192.168.1.1",
            "192.168.1.1:8080/admin",
            "[::1]:8080",
            "github.com/rust-lang/rust",
            "foo.io",
            "foo.dev?q=1",
            "user@example.org",
            "example.com.",
            "straße.de",
        ] {
            assert!(looks_like_host(w), "{w} is a place");
        }
    }

    #[test]
    fn words_with_a_dot() {
        for w in ["youtube.music", "index.html", "node.js", "file.txt", "v1.2", "a.", ".com", "foo..com", "hello", "e.g"] {
            assert!(!looks_like_host(w), "{w} is not a place");
        }
    }

    #[test]
    fn tld_lookup() {
        assert!(known_tld("COM") && known_tld("uk") && known_tld("io") && known_tld("xn--p1ai"));
        assert!(!known_tld("music") && !known_tld("html") && !known_tld("zz") && !known_tld(""));
    }
}
