//! Search history by what the pages said. The text of each page you open goes, gzipped, into
//! one append-only file. A search reads that file from start to end on its own thread. No
//! index, no database, nothing in memory between searches: a few thousand pages scan in tens
//! of milliseconds, and that is plenty for one person.

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender, channel},
    time::{SystemTime, UNIX_EPOCH},
};

use flate2::{Compression, read::MultiGzDecoder, write::GzEncoder};
use serde_json::{Value, json};

const FILE: &str = "pages.jsonl.gz";
/// Text kept per page. The start of a page says what it is about.
pub const MAX_TEXT: usize = 30_000;
/// Past this the file is rewritten with only the newest copy of each page.
const COMPACT_AT: u64 = 48 << 20;
/// Pages kept by a compaction, newest first.
const KEEP: usize = 6000;

/// One answer: (url, title, a few words around the match).
pub type Hit = (String, String, String);

enum Job {
    Add { url: String, title: String, text: String },
    Forget(String),
    Clear(Option<u64>),
    Search { q: String, limit: usize, reply: Box<dyn FnOnce(String, Vec<Hit>) + Send> },
}

/// The handle the UI thread holds. Every call returns at once.
#[derive(Clone)]
pub struct Recall {
    tx: Sender<Job>,
}

impl Recall {
    pub fn start(dir: &Path) -> Recall {
        let (tx, rx) = channel();
        let path = dir.join(FILE);
        std::thread::Builder::new()
            .name("lumen-recall".into())
            .spawn(move || worker(path, rx))
            .ok();
        Recall { tx }
    }

    pub fn add(&self, url: &str, title: &str, text: &str) {
        let text: String = text.chars().take(MAX_TEXT).collect();
        let _ = self.tx.send(Job::Add { url: url.into(), title: title.into(), text });
    }

    pub fn forget(&self, url: &str) {
        let _ = self.tx.send(Job::Forget(url.into()));
    }

    /// Everything, or everything from `since` (unix seconds) on.
    pub fn clear(&self, since: Option<u64>) {
        let _ = self.tx.send(Job::Clear(since));
    }

    /// `reply` gets the query back with the hits, so a stale answer can be told apart.
    pub fn search(&self, q: &str, limit: usize, reply: impl FnOnce(String, Vec<Hit>) + Send + 'static) {
        let _ = self.tx.send(Job::Search { q: q.into(), limit, reply: Box::new(reply) });
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn worker(path: PathBuf, rx: Receiver<Job>) {
    compact_if_big(&path);
    while let Ok(job) = rx.recv() {
        // Typing sends a search per key. Only the last one still matters.
        let mut job = job;
        while let Ok(next) = rx.try_recv() {
            match (job, next) {
                (Job::Search { .. }, n @ Job::Search { .. }) => job = n,
                (j, n) => {
                    run(&path, j);
                    job = n;
                }
            }
        }
        run(&path, job);
    }
}

fn run(path: &Path, job: Job) {
    match job {
        Job::Add { url, title, text } => append(path, &json!({"u": url, "t": title, "x": text, "at": now()})),
        Job::Forget(url) => append(path, &json!({"u": url, "del": 1})),
        Job::Clear(None) => {
            let _ = fs::remove_file(path);
        }
        Job::Clear(Some(since)) => rewrite(path, |r| r.at < since, usize::MAX),
        Job::Search { q, limit, reply } => {
            let hits = search(path, &q, limit);
            reply(q, hits);
        }
    }
}

/// One gzip member per record: appending never touches what is already written.
fn append(path: &Path, rec: &Value) {
    let Ok(f) = OpenOptions::new().create(true).append(true).open(path) else { return };
    let mut gz = GzEncoder::new(f, Compression::fast());
    let _ = writeln!(gz, "{rec}");
    let _ = gz.finish();
}

struct Rec {
    url: String,
    title: String,
    text: String,
    at: u64,
}

/// The newest copy of every page still remembered, in file order.
fn load(path: &Path) -> Vec<Rec> {
    let Ok(f) = fs::File::open(path) else { return Vec::new() };
    let mut by_url: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Option<Rec>> = Vec::new();
    for line in BufReader::new(MultiGzDecoder::new(f)).lines() {
        // A torn last record (power cut) ends the read, it does not end the file.
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let url = v["u"].as_str().unwrap_or("").to_string();
        if let Some(&k) = by_url.get(&url) {
            out[k] = None;
        }
        if v["del"] == 1 {
            by_url.remove(&url);
            continue;
        }
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        by_url.insert(url.clone(), out.len());
        out.push(Some(Rec { url, title: s("t"), text: s("x"), at: v["at"].as_u64().unwrap_or(0) }));
    }
    out.into_iter().flatten().collect()
}

fn rewrite(path: &Path, keep: impl Fn(&Rec) -> bool, max: usize) {
    let mut recs: Vec<Rec> = load(path).into_iter().filter(|r| keep(r)).collect();
    if recs.len() > max {
        recs.sort_by_key(|r| std::cmp::Reverse(r.at));
        recs.truncate(max);
        recs.sort_by_key(|r| r.at);
    }
    let tmp = path.with_extension("part");
    let Ok(f) = fs::File::create(&tmp) else { return };
    let mut gz = GzEncoder::new(f, Compression::default());
    for r in &recs {
        let _ = writeln!(gz, "{}", json!({"u": r.url, "t": r.title, "x": r.text, "at": r.at}));
    }
    if gz.finish().is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn compact_if_big(path: &Path) {
    if fs::metadata(path).is_ok_and(|m| m.len() > COMPACT_AT) {
        rewrite(path, |_| true, KEEP);
    }
}

fn search(path: &Path, q: &str, limit: usize) -> Vec<Hit> {
    let words: Vec<String> = q.split_whitespace().map(|w| w.to_lowercase()).filter(|w| w.chars().count() >= 2).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, u64, Hit)> = Vec::new();
    for r in load(path) {
        let title = r.title.to_lowercase();
        let text = r.text.to_lowercase();
        let mut score = 0;
        let mut all = true;
        for w in &words {
            let n = text.matches(w.as_str()).count() + 3 * title.matches(w.as_str()).count();
            if n == 0 {
                all = false;
                break;
            }
            score += n.min(20);
        }
        if all {
            let snip = snippet(&r.text, &text, &words[0]);
            scored.push((score, r.at, (r.url, r.title, snip)));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    scored.into_iter().take(limit).map(|s| s.2).collect()
}

/// About 90 characters from just before the first place `word` shows up in the text.
fn snippet(text: &str, lower: &str, word: &str) -> String {
    let Some(at) = lower.find(word) else { return String::new() };
    // `lower` and `text` can differ in byte length; work in characters.
    let ci = lower[..at].chars().count();
    let chars: Vec<char> = text.chars().collect();
    // The match near the start: the address bar is narrow and cuts the end off.
    let start = ci.saturating_sub(12);
    let end = (ci + 80).min(chars.len());
    let mut s: String = chars[start..end].iter().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    if start > 0 {
        s.insert(0, '…');
    }
    if end < chars.len() {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumen-recall-{name}-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let p = d.join(FILE);
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn finds_by_text_and_forgets() {
        let p = tmp("find");
        append(&p, &json!({"u": "https://a.com", "t": "Lighthouses", "x": "A lighthouse is a tower that emits light from lamps and lenses.", "at": 1}));
        append(&p, &json!({"u": "https://b.com", "t": "Bread", "x": "Flour, water, salt, and yeast. Lenses not included.", "at": 2}));
        let h = search(&p, "lamps lenses", 5);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].0, "https://a.com");
        assert!(h[0].2.contains("lamps"));
        assert_eq!(search(&p, "lenses", 5).len(), 2);
        // Newer copy of a page replaces the old one.
        append(&p, &json!({"u": "https://a.com", "t": "Lighthouses", "x": "Rewritten page about towers.", "at": 3}));
        assert!(search(&p, "lamps", 5).is_empty());
        append(&p, &json!({"u": "https://b.com", "del": 1}));
        assert!(search(&p, "yeast", 5).is_empty());
        // Clear from a time on keeps what came before.
        rewrite(&p, |r| r.at < 3, usize::MAX);
        assert!(search(&p, "towers", 5).is_empty());
    }

    /// Clear with a `since` keeps what is older; None takes everything. The jobs
    /// are one queue, so the search answer can only come after the clear ran.
    #[test]
    fn clear_with_since_or_everything() {
        let dir = std::env::temp_dir().join(format!("lumen-recall-clear-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let ask = |r: &Recall, word: &str| {
            let (tx, rx) = channel();
            r.search(word, 5, move |_, hits| { let _ = tx.send(hits); });
            rx.recv_timeout(std::time::Duration::from_secs(5)).expect("a search answer")
        };
        let r = Recall::start(&dir);
        r.add("https://old.com/", "Old", "alpha bravo charlie");
        // A range starting after every visit: nothing goes.
        r.clear(Some(now() + 3600));
        assert_eq!(ask(&r, "alpha").len(), 1);
        // A range starting at the epoch: everything from it on goes.
        r.clear(Some(0));
        assert!(ask(&r, "alpha").is_empty());
        r.add("https://new.com/", "New", "delta echo foxtrot");
        // None means all of it, range or not.
        r.clear(None);
        assert!(ask(&r, "delta").is_empty());
    }

    #[test]
    fn snippets_cut_on_chars() {
        let text = "Ünïcödé text before the word lighthouse and after it, going on for a good long while more, and more, and then some more again.";
        let s = snippet(text, &text.to_lowercase(), "lighthouse");
        assert!(s.contains("lighthouse"));
        assert!(s.ends_with('…'));
    }

    #[test]
    fn compaction_keeps_the_newest() {
        let p = tmp("compact");
        for i in 0..10u64 {
            append(&p, &json!({"u": format!("https://x.com/{i}"), "t": "", "x": format!("page number{i}"), "at": i}));
        }
        rewrite(&p, |_| true, 3);
        let left = load(&p);
        assert_eq!(left.iter().map(|r| r.at).collect::<Vec<_>>(), vec![7, 8, 9]);
    }
}
