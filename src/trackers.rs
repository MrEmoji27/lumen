//! Trackers for torrents: a list, a week old at most, fetched off the
//! startup path.
//!
//! A torrent handed three dead trackers finds nobody. The best public list
//! has fifty that answer, so we keep one on disk beside aria2's own files,
//! hand it to aria2 when it starts, and push a fresh one in while Lumen runs.
//! The fetch lives on its own thread: a slow CDN never delays a start, and a
//! failed fetch leaves last week's file exactly as it was.

use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, SystemTime},
};

use crate::aria::Aria;

/// Where the list comes from. Public, no key, no account, one line.
const URL: &str = "https://cdn.jsdelivr.net/gh/ngosang/trackerslist@master/trackers_best.txt";
/// The file we keep, in aria2's own directory.
const FILE: &str = "trackers.txt";
/// Ask for a new list when the old one is older than this. Once a week.
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// The whole list is a few KB of plain text. A body this big is not the list.
const MAX_BODY: usize = 512 << 10;
/// One tracker is a scheme, a host and a port. 300 chars is already generous.
const MAX_LINE: usize = 300;
/// Fifty trackers that answer beat five hundred that do not.
const MAX_LINES: usize = 50;
/// Getting the list is a weekly errand, not a race.
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a fresh list waits for aria2 to finish booting before it gives up.
const READY_TRIES: u32 = 400;
const READY_SLEEP: Duration = Duration::from_millis(250);

/// What aria2 gets at startup: whatever is on disk, comma joined. Reading a
/// file is not I/O worth fearing, so this runs on the boot thread.
pub fn startup(data_dir: &Path) -> String {
    join(&load(&aria2_dir(data_dir)))
}

/// Once a week: go ask for a new list, keep the old file if the network or
/// the answer is no good, then hand the new list to aria2 where it runs.
/// This is a thread of its own. Nothing waits on it.
pub fn refresh(data_dir: &Path, aria: &Aria) {
    let dir = aria2_dir(data_dir);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if !stale(&dir, SystemTime::now()) {
        return;
    }
    let Some(body) = fetch(URL) else {
        eprintln!("lumen: trackers: the list did not download, last week's file stands");
        return;
    };
    let Some(list) = accept(&dir, &body) else {
        eprintln!("lumen: trackers: what came back was no tracker list, last week's file stands");
        return;
    };
    wait_for(aria, &join(&list));
}

/// aria2's own directory, where its session and DHT file already live.
fn aria2_dir(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("aria2")
}

/// Everything that happens once the bytes are in hand. None means the bytes
/// were not a list, and then nothing on disk is touched.
fn accept(dir: &Path, body: &str) -> Option<Vec<String>> {
    let list = parse(body);
    if list.is_empty() {
        return None;
    }
    if !save(dir, &list) {
        eprintln!("lumen: trackers: could not write {} to disk", dir.join(FILE).display());
    }
    Some(list)
}

/// The list on disk, checked line by line. We wrote this file, but a file
/// anyone can edit is still a file anyone can edit.
pub fn load(dir: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(dir.join(FILE)) else { return Vec::new() };
    parse(&text)
}

/// True when there is no list on disk, or it was last written over a week ago.
pub fn stale(dir: &Path, now: SystemTime) -> bool {
    let modified = fs::metadata(dir.join(FILE)).and_then(|m| m.modified()).ok();
    old_enough(modified, now)
}

/// The freshness rule, whole and testable: missing is stale, older than a
/// week is stale, and a clock that ran backwards is stale too (it must not
/// freeze the refresh forever).
fn old_enough(modified: Option<SystemTime>, now: SystemTime) -> bool {
    match modified {
        None => true,
        Some(at) => match now.duration_since(at) {
            Ok(age) => age > MAX_AGE,
            Err(_) => true,
        },
    }
}

/// Every line a torrent can announce to, in order, without repeats, capped
/// at MAX_LINES. Junk falls out here so aria2 never sees it.
pub fn parse(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if out.len() >= MAX_LINES {
            break;
        }
        if !good(line) || out.iter().any(|seen| seen == line) {
            continue;
        }
        out.push(line.to_string());
    }
    out
}

/// True when aria2 can announce to this line exactly as it stands.
fn good(line: &str) -> bool {
    if line.is_empty() || line.len() > MAX_LINE {
        return false;
    }
    // A tracker is one token. Whitespace inside means two lines glued
    // together, or a comment somebody left in the file.
    if line.chars().any(|c| c.is_whitespace()) {
        return false;
    }
    let Some((scheme, rest)) = line.split_once("://") else { return false };
    if !matches!(scheme, "udp" | "http" | "https" | "wss") {
        return false;
    }
    // Something has to follow the scheme, or there is no host to dial.
    !rest.is_empty() && !rest.starts_with('/')
}

/// One line for `--bt-tracker=`: aria2 takes a comma separated list.
pub fn join(list: &[String]) -> String {
    list.join(",")
}

/// Write beside the target and move it, so a half list never survives.
fn save(dir: &Path, list: &[String]) -> bool {
    let path = dir.join(FILE);
    let tmp = path.with_extension("part");
    let text = format!("{}\n", list.join("\n"));
    if fs::write(&tmp, text.as_bytes()).is_err() {
        return false;
    }
    fs::rename(&tmp, &path).is_ok()
}

/// Bytes over the wire, or nothing. No file is touched in here.
fn fetch(url: &str) -> Option<String> {
    let config = ureq::config::Config::builder().timeout_global(Some(FETCH_TIMEOUT)).build();
    let agent = ureq::Agent::new_with_config(config);
    let mut reply = agent.get(url).call().ok()?;
    let body = reply.body_mut().read_to_vec().ok()?;
    if body.is_empty() || body.len() > MAX_BODY {
        return None;
    }
    String::from_utf8(body).ok()
}

/// Hand the list to aria2 once the engine is up, without a restart. If the
/// engine never comes up the list is already on disk for the next start.
fn wait_for(aria: &Aria, joined: &str) {
    for _ in 0..READY_TRIES {
        if aria.ready() {
            aria.set_trackers(joined);
            return;
        }
        thread::sleep(READY_SLEEP);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn case(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-trackers-{what}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn good_lines_survive_and_junk_falls_out() {
        let list = parse(concat!(
            "udp://tracker.opentrackr.org:1337/announce\n",
            "https://tracker.example.com/announce\n",
            "wss://tracker.webtorrent.io\n",
            "\n",
            "   \n",
            "http://open.tracker.example:80/announce\n",
            "ftp://nope.example/announce\n",
            "gopher://nope.example/announce\n",
            "not a url at all\n",
            "udp://tracker.example.com:1337/announce\n",
            "udp://tracker.example.com:1337/announce\n",
        ));
        assert_eq!(
            list,
            vec![
                "udp://tracker.opentrackr.org:1337/announce",
                "https://tracker.example.com/announce",
                "wss://tracker.webtorrent.io",
                "http://open.tracker.example:80/announce",
                "udp://tracker.example.com:1337/announce",
            ]
        );
    }

    #[test]
    fn a_line_only_aria2_can_dial_is_a_line_we_keep() {
        assert!(good("udp://tracker.example.com:1337/announce"));
        assert!(good("wss://tracker.example.com/announce"));
        // Wrong scheme, no host, or a second line welded on: all out.
        assert!(!good("ftp://tracker.example.com/announce"));
        assert!(!good("udp://"));
        assert!(!good("http:///announce"));
        assert!(!good("tracker.example.com:1337"));
        assert!(!good("udp://tracker.example.com:1337 announce"));
        assert!(!good(""));
        // A tracker is a token, not a line of prose: 300 chars is the fence.
        let prefix = "udp://t.example:1337/";
        let at_limit = format!("{prefix}{}", "a".repeat(MAX_LINE - prefix.len()));
        let one_over = format!("{prefix}{}", "a".repeat(MAX_LINE - prefix.len() + 1));
        assert_eq!(at_limit.len(), MAX_LINE);
        assert!(good(&at_limit));
        assert!(!good(&one_over));
    }

    #[test]
    fn fifty_is_the_ceiling() {
        let text: String = (0..MAX_LINES + 30).map(|i| format!("udp://t{i}.example:1337/announce\n")).collect();
        assert_eq!(parse(&text).len(), MAX_LINES);
    }

    #[test]
    fn the_list_joins_the_way_aria2_wants_it() {
        let list = parse("udp://a.example:1/announce\nhttps://b.example/announce\n");
        assert_eq!(join(&list), "udp://a.example:1/announce,https://b.example/announce");
        assert_eq!(join(&[]), "");
    }

    #[test]
    fn a_cache_is_fresh_for_a_week_then_it_is_not() {
        let now = SystemTime::now();
        let day = Duration::from_secs(24 * 60 * 60);
        // Never fetched, or gone: go get it.
        assert!(old_enough(None, now));
        assert!(!old_enough(Some(now - 6 * day), now));
        assert!(old_enough(Some(now - 8 * day), now));
        // A file from the future means the clock moved. Fetch, do not wait.
        assert!(old_enough(Some(now + Duration::from_secs(60)), now));
    }

    #[test]
    fn what_is_on_disk_answers_the_same_question() {
        let dir = case("fresh");
        // No file yet counts as stale.
        assert!(stale(&dir, SystemTime::now()));
        assert!(save(&dir, &["udp://a.example:1/announce".to_string()]));
        assert!(!stale(&dir, SystemTime::now()));
        // The list comes back off disk, and a second save replaces it whole.
        assert_eq!(load(&dir), vec!["udp://a.example:1/announce".to_string()]);
        assert!(save(&dir, &["udp://b.example:2/announce".to_string(), "https://c.example/announce".to_string()]));
        assert_eq!(load(&dir).len(), 2);
        assert!(load(&dir).iter().all(|l| l.starts_with("udp://b") || l.starts_with("https://c")));
    }

    #[test]
    fn an_answer_that_is_not_a_list_leaves_the_old_file_alone() {
        let dir = case("keep");
        assert!(save(&dir, &["udp://old.example:1/announce".to_string()]));
        // A CDN error page, a 404 body, an empty file: none of them are lists.
        assert!(accept(&dir, "").is_none());
        assert!(accept(&dir, "<!DOCTYPE html><html>gone</html>").is_none());
        assert!(accept(&dir, "ftp://nope.example/announce\n").is_none());
        assert_eq!(load(&dir), vec!["udp://old.example:1/announce".to_string()]);
        // A real list does replace it.
        assert!(accept(&dir, "udp://new.example:2/announce\n").is_some());
        assert_eq!(load(&dir), vec!["udp://new.example:2/announce".to_string()]);
    }

    #[test]
    fn startup_hands_boot_what_is_on_disk() {
        let data = case("boot");
        // Nothing fetched yet: boot passes no --bt-tracker at all.
        assert_eq!(startup(&data), "");
        let dir = data.join("aria2");
        fs::create_dir_all(&dir).expect("aria2's own directory");
        assert!(save(&dir, &["udp://a.example:1/announce".to_string(), "udp://b.example:2/announce".to_string()]));
        assert_eq!(startup(&data), "udp://a.example:1/announce,udp://b.example:2/announce");
        // A file full of junk hands aria2 nothing rather than something wrong.
        fs::write(dir.join(FILE), "not a tracker\n").expect("write");
        assert_eq!(startup(&data), "");
    }

    #[test]
    fn a_fresh_list_is_handed_to_aria2_as_one_string() {
        // The shape changeGlobalOption is handed, checked against aria2 1.37
        // over the wire before this was wired up.
        let list = parse("udp://a.example:1/announce\nudp://b.example:2/announce\n");
        let value = serde_json::json!({"bt-tracker": join(&list)});
        assert_eq!(value["bt-tracker"], "udp://a.example:1/announce,udp://b.example:2/announce");
    }
}

#[cfg(test)]
mod live {
    //! Needs the internet. Run with: cargo test trackers -- --ignored

    use super::*;

    #[test]
    #[ignore = "downloads a tracker list off the internet"]
    fn the_public_list_parses_into_trackers() {
        let Some(body) = fetch(URL) else { panic!("the list did not download") };
        let list = parse(&body);
        assert!(!list.is_empty(), "the list came down but held nothing usable");
        assert!(list.len() <= MAX_LINES);
        assert!(list.iter().all(|l| good(l)), "a bad line got through");
        assert!(join(&list).len() < 10_000, "the string handed to aria2 stays small");
    }
}
