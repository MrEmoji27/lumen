//! History and bookmarks. Storage and search, nothing else.
//!
//! History is `history.jsonl`: one visit per line, appended as pages load.
//! Later lines win on load, so a re-visited page is one more line, not a
//! rewrite. When the file grows past twice the live entries it gets rewritten
//! compacted. Bookmarks are `bookmarks.json`, written whole when they change.
//! No windows, no webviews, no panics. A broken file reads as empty.

#![allow(dead_code)]

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// One visit per line, appended. Only rewritten when the file goes fat.
const HISTORY_FILE: &str = "history.jsonl";
/// The whole list, written atomically when it changes.
const BOOKMARKS_FILE: &str = "bookmarks.json";
/// URLs kept. Past this the least recently visited page falls off the end.
const MAX_HISTORY: usize = 100_000;
/// A visit from a week ago counts half as much as one from today.
const HALF_LIFE: u64 = 7 * 24 * 60 * 60;
/// Bookmarks first, then history. Far apart, so the two never mix.
const MARK_BOOST: f64 = 1e9;
/// A host that starts with what you typed beats a host that merely holds it.
const HOST_BOOST: f64 = 1e6;
/// Schemes that are not pages. Nobody wants them in history.
const DEAD_SCHEMES: [&str; 6] = ["about", "data", "blob", "javascript", "edge", "file"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub last: u64,
    pub count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub added: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Bookmark,
    History,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub url: String,
    pub title: String,
    pub kind: Kind,
}

/// One history row plus the search index that belongs to it.
struct Entry {
    v: Visit,
    /// Lowercase url and title. Built once per change, not once per keystroke.
    hay: String,
    /// Which bytes show up in `hay`. A query byte that is not here cannot match.
    mask: [u64; 4],
    /// Visit order. Breaks ties between pages seen in the same second.
    seq: u64,
}

/// One bookmark, same deal.
struct Mark {
    b: Bookmark,
    hay: String,
    mask: [u64; 4],
}

/// Heap key for the least recently visited page. String last, so ties are stable.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Oldest {
    last: u64,
    seq: u64,
    url: String,
}

pub struct Library {
    dir: PathBuf,
    /// url to entry. `visit` lands here first.
    hist: HashMap<String, Entry>,
    /// Min-heap of visit times, for the cap. Stale keys are skipped, not trusted.
    oldest: BinaryHeap<Reverse<Oldest>>,
    /// Newest first.
    marks: Vec<Mark>,
    books_dirty: bool,
    /// Visits to append to the file on the next flush.
    pending: Vec<Visit>,
    /// Something left history. The file must be rewritten, not appended to.
    rewrite: bool,
    /// Monotonic. Later is newer.
    seq: u64,
    next_id: u64,
}

impl Entry {
    fn new(v: Visit, seq: u64) -> Entry {
        let hay = hay_of(&v.url, &v.title);
        let mask = mask_of(&hay);
        Entry { v, hay, mask, seq }
    }

    fn retitle(&mut self, title: &str) {
        self.v.title = title.to_string();
        self.hay = hay_of(&self.v.url, title);
        self.mask = mask_of(&self.hay);
    }
}

impl Mark {
    fn new(b: Bookmark) -> Mark {
        let hay = hay_of(&b.url, &b.title);
        let mask = mask_of(&hay);
        Mark { b, hay, mask }
    }

    fn rebuild(&mut self) {
        self.hay = hay_of(&self.b.url, &self.b.title);
        self.mask = mask_of(&self.hay);
    }
}

impl Library {
    /// Load from `dir`. Missing or broken files read as empty. A file left fat
    /// by old re-visits is rewritten compacted right here.
    pub fn load(dir: &Path) -> Library {
        let _ = fs::create_dir_all(dir);
        let mut lib = Library {
            dir: dir.to_path_buf(),
            hist: HashMap::new(),
            oldest: BinaryHeap::new(),
            marks: Vec::new(),
            books_dirty: false,
            pending: Vec::new(),
            rewrite: false,
            seq: 0,
            next_id: 1,
        };
        lib.read_history();
        lib.read_bookmarks();
        lib
    }

    /// Record a visit. Upsert by URL: count grows, `last` moves to now, and an
    /// empty title keeps whatever the page said last time.
    pub fn visit(&mut self, url: &str, title: &str) {
        if url.is_empty() || dead_scheme(url) {
            return;
        }
        let now = unix_now();
        self.seq += 1;
        let seq = self.seq;
        self.oldest.push(Reverse(Oldest { last: now, seq, url: url.to_string() }));
        match self.hist.entry(url.to_string()) {
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                let e = slot.get_mut();
                e.v.count = e.v.count.saturating_add(1);
                e.v.last = now;
                e.seq = seq;
                if !title.is_empty() {
                    e.retitle(title);
                }
                self.pending.push(e.v.clone());
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                let v = Visit { url: url.to_string(), title: title.to_string(), last: now, count: 1 };
                self.pending.push(v.clone());
                slot.insert(Entry::new(v, seq));
                self.trim();
            }
        }
    }

    /// Update the title of a page already in history. Pages set theirs after load.
    pub fn set_title(&mut self, url: &str, title: &str) {
        if title.is_empty() {
            return;
        }
        if let Some(e) = self.hist.get_mut(url) {
            e.retitle(title);
            self.pending.push(e.v.clone());
        }
    }

    /// Ranked matches for the address bar. Bookmarks first, then history by
    /// frecency, host-prefix ahead of substring, no duplicate URLs.
    pub fn search(&self, q: &str, limit: usize) -> Vec<Hit> {
        if limit == 0 {
            return Vec::new();
        }
        let words = words_of(q);
        let typed = q.trim().to_lowercase();
        let now = unix_now();
        let mut hits: Vec<(f64, Hit)> = Vec::new();
        // A page you saved beats a page you saw. By a mile.
        let mut marked: HashSet<&str> = HashSet::new();
        for m in &self.marks {
            if !matchable(&m.hay, &m.mask, &words) {
                continue;
            }
            marked.insert(m.b.url.as_str());
            let mut score = MARK_BOOST;
            if host_starts_with(&m.b.url, &typed) {
                score += HOST_BOOST;
            }
            hits.push((score, Hit { url: m.b.url.clone(), title: m.b.title.clone(), kind: Kind::Bookmark }));
        }
        for e in self.hist.values() {
            if marked.contains(e.v.url.as_str()) || !matchable(&e.hay, &e.mask, &words) {
                continue;
            }
            let mut score = frecency(e.v.count, now.saturating_sub(e.v.last));
            if host_starts_with(&e.v.url, &typed) {
                score += HOST_BOOST;
            }
            hits.push((score, Hit { url: e.v.url.clone(), title: e.v.title.clone(), kind: Kind::History }));
        }
        // Ties fall to the url, so the order never depends on hash order.
        let by = |a: &(f64, Hit), b: &(f64, Hit)| b.0.total_cmp(&a.0).then_with(|| a.1.url.cmp(&b.1.url));
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit - 1, by);
            hits.truncate(limit);
        }
        hits.sort_by(by);
        hits.into_iter().map(|(_, h)| h).collect()
    }

    /// History page: newest first, optional filter (same word rule), paging.
    pub fn recent(&self, filter: &str, offset: usize, limit: usize) -> Vec<Visit> {
        let words = words_of(filter);
        let mut rows: Vec<&Entry> = self.hist.values().filter(|e| matchable(&e.hay, &e.mask, &words)).collect();
        rows.sort_by(|a, b| b.v.last.cmp(&a.v.last).then(b.seq.cmp(&a.seq)).then(a.v.url.cmp(&b.v.url)));
        rows.into_iter().skip(offset).take(limit).map(|e| e.v.clone()).collect()
    }

    /// One url out of history.
    pub fn forget(&mut self, url: &str) {
        if self.hist.remove(url).is_some() {
            self.rewrite = true;
        }
    }

    /// Drop visits with `last` at or after `since`. None means all of it.
    pub fn clear_history(&mut self, since: Option<u64>) {
        let before = self.hist.len();
        match since {
            Some(t) => self.hist.retain(|_, e| e.v.last < t),
            None => self.hist.clear(),
        }
        if self.hist.len() != before {
            self.rewrite = true;
        }
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.marks.iter().any(|m| m.b.url == url)
    }

    /// Add if missing, no-op if present. Either way the id comes back.
    pub fn bookmark(&mut self, url: &str, title: &str) -> u64 {
        if url.is_empty() {
            return 0;
        }
        if let Some(m) = self.marks.iter().find(|m| m.b.url == url) {
            return m.b.id;
        }
        let id = self.next_id;
        self.next_id += 1;
        let b = Bookmark { id, url: url.to_string(), title: title.to_string(), added: unix_now() };
        self.marks.insert(0, Mark::new(b));
        self.books_dirty = true;
        id
    }

    pub fn unbookmark(&mut self, url: &str) {
        let before = self.marks.len();
        self.marks.retain(|m| m.b.url != url);
        if self.marks.len() != before {
            self.books_dirty = true;
        }
    }

    pub fn rename_bookmark(&mut self, id: u64, title: &str) {
        if let Some(m) = self.marks.iter_mut().find(|m| m.b.id == id) {
            m.b.title = title.to_string();
            m.rebuild();
            self.books_dirty = true;
        }
    }

    /// Newest first.
    pub fn bookmarks(&self) -> Vec<Bookmark> {
        self.marks.iter().map(|m| m.b.clone()).collect()
    }

    /// Write what changed. Two checks when nothing did.
    pub fn flush(&mut self) {
        if self.books_dirty {
            self.books_dirty = false;
            self.write_bookmarks();
        }
        if self.rewrite {
            self.rewrite = false;
            self.pending.clear();
            self.write_history();
        } else if !self.pending.is_empty() {
            self.append_history();
            self.pending.clear();
        }
    }

    // ------------------------------------------------------------------ disk

    fn read_history(&mut self) {
        let path = self.dir.join(HISTORY_FILE);
        let Ok(text) = fs::read_to_string(&path) else { return };
        let mut lines = 0usize;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            lines += 1;
            let Ok(v) = serde_json::from_str::<Visit>(line) else { continue };
            if v.url.is_empty() {
                continue;
            }
            // Later lines win: the newest line for a url is the truth.
            self.seq += 1;
            self.oldest.push(Reverse(Oldest { last: v.last, seq: self.seq, url: v.url.clone() }));
            self.hist.insert(v.url.clone(), Entry::new(v, self.seq));
        }
        let dropped = self.trim();
        // Fat with re-visits, or over the cap: start the file over.
        if lines > self.hist.len() * 2 || dropped {
            self.write_history();
        }
    }

    fn read_bookmarks(&mut self) {
        let path = self.dir.join(BOOKMARKS_FILE);
        let Ok(text) = fs::read_to_string(&path) else { return };
        let Ok(list) = serde_json::from_str::<Vec<Bookmark>>(&text) else { return };
        let mut seen: HashSet<String> = HashSet::new();
        for b in list {
            if b.url.is_empty() || !seen.insert(b.url.clone()) {
                continue;
            }
            self.next_id = self.next_id.max(b.id + 1);
            self.marks.push(Mark::new(b));
        }
        self.marks.sort_by(|a, b| b.b.added.cmp(&a.b.added).then(b.b.id.cmp(&a.b.id)));
    }

    fn append_history(&mut self) {
        use std::io::Write;
        let path = self.dir.join(HISTORY_FILE);
        let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
        let mut out = String::new();
        for v in &self.pending {
            if let Ok(line) = serde_json::to_string(v) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        let _ = f.write_all(out.as_bytes());
    }

    /// Whole file, oldest line first, so a reload keeps the same order.
    fn write_history(&mut self) {
        let mut rows: Vec<&Entry> = self.hist.values().collect();
        rows.sort_by(|a, b| a.v.last.cmp(&b.v.last).then(a.seq.cmp(&b.seq)));
        let mut out = String::new();
        for e in rows {
            if let Ok(line) = serde_json::to_string(&e.v) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        atomic_write(&self.dir.join(HISTORY_FILE), &out);
    }

    fn write_bookmarks(&mut self) {
        let list: Vec<&Bookmark> = self.marks.iter().map(|m| &m.b).collect();
        if let Ok(json) = serde_json::to_string_pretty(&list) {
            atomic_write(&self.dir.join(BOOKMARKS_FILE), &json);
        }
    }

    /// Hold the line at MAX_HISTORY. True when a page fell off.
    fn trim(&mut self) -> bool {
        let mut dropped = false;
        while self.hist.len() > MAX_HISTORY {
            let Some(Reverse(old)) = self.oldest.pop() else { break };
            // A heap key is only real if the page still carries it.
            let live = self.hist.get(&old.url).is_some_and(|e| e.v.last == old.last && e.seq == old.seq);
            if live {
                self.hist.remove(&old.url);
                dropped = true;
            }
        }
        // A long session leaves the heap mostly stale. Rebuild it from the truth.
        if self.oldest.len() > self.hist.len() * 2 + 1024 {
            let fresh: Vec<Reverse<Oldest>> = self
                .hist
                .values()
                .map(|e| Reverse(Oldest { last: e.v.last, seq: e.seq, url: e.v.url.clone() }))
                .collect();
            self.oldest = BinaryHeap::from(fresh);
        }
        dropped
    }
}

/// The search haystack: url and title, lowercased, one allocation.
fn hay_of(url: &str, title: &str) -> String {
    format!("{}\n{}", url.to_lowercase(), title.to_lowercase())
}

/// Every byte the haystack holds. One lookup kills most entries.
fn mask_of(hay: &str) -> [u64; 4] {
    let mut mask = [0u64; 4];
    for b in hay.bytes() {
        mask[(b >> 6) as usize] |= 1u64 << (b & 63);
    }
    mask
}

/// Every word of the query must sit in the url or the title. The mask says no first.
fn matchable(hay: &str, mask: &[u64; 4], words: &[String]) -> bool {
    words.iter().all(|w| {
        w.as_bytes().iter().all(|&b| mask[(b >> 6) as usize] & (1u64 << (b & 63)) != 0) && hay.contains(w.as_str())
    })
}

fn words_of(q: &str) -> Vec<String> {
    q.split_whitespace().map(|w| w.to_lowercase()).collect()
}

/// Visits a lot, visited lately. Nobody wants last year's favourite.
fn frecency(count: u32, age: u64) -> f64 {
    (count as f64 + 1.0) * 0.5_f64.powf(age as f64 / HALF_LIFE as f64)
}

/// "github.com/x" starts with "github". The address bar wants that one first.
fn host_starts_with(url: &str, q: &str) -> bool {
    if q.is_empty() {
        return false;
    }
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    rest[..end].to_lowercase().starts_with(q)
}

/// about:, data:, blob:, file: and friends. Not pages; not history.
fn dead_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else { return false };
    DEAD_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Write beside the target, then move it. A crash never leaves half a file.
fn atomic_write(path: &Path, text: &str) {
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A folder nobody else is in. Cleaned first, so runs never see each other.
    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-library-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("test dir");
        dir
    }

    fn lines(path: &Path) -> usize {
        fs::read_to_string(path).map(|s| s.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0)
    }

    #[test]
    fn visits_upsert_and_count() {
        let mut lib = Library::load(&tmp_dir("upsert"));
        lib.visit("https://a.com/", "A");
        lib.visit("https://a.com/", "");
        lib.visit("https://a.com/", "A again");
        let rows = lib.recent("", 0, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].count, 3);
        // An empty title keeps what the page said last time.
        lib.visit("https://a.com/", "");
        assert_eq!(lib.recent("", 0, 10)[0].title, "A again");
        // The page seen last comes first.
        lib.visit("https://b.com/", "B");
        assert_eq!(lib.recent("", 0, 10)[0].url, "https://b.com/");
        assert_eq!(lib.recent("", 1, 10)[0].url, "https://a.com/");
    }

    #[test]
    fn dead_schemes_never_land_in_history() {
        let mut lib = Library::load(&tmp_dir("schemes"));
        for u in [
            "about:blank",
            "ABOUT:version",
            "data:text/html,hi",
            "blob:https://a.com/x",
            "javascript:void(0)",
            "edge://settings",
            "file:///C:/x",
            "",
        ] {
            lib.visit(u, "junk");
        }
        lib.visit("https://real.com/", "Real");
        assert_eq!(lib.recent("", 0, 10).len(), 1);
        assert!(lib.search("junk", 10).is_empty());
        assert_eq!(lib.search("real", 10).len(), 1);
    }

    #[test]
    fn bookmark_beats_history() {
        let mut lib = Library::load(&tmp_dir("bm-beats-hist"));
        // History has seen this page fifty times.
        for _ in 0..50 {
            lib.visit("https://gist.github.com/y", "gist page");
        }
        lib.bookmark("https://github.com/", "GitHub");
        let hits = lib.search("github", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].kind, Kind::Bookmark);
        assert_eq!(hits[0].url, "https://github.com/");
        assert_eq!(hits[1].kind, Kind::History);
    }

    #[test]
    fn host_prefix_beats_substring() {
        let mut lib = Library::load(&tmp_dir("host-prefix"));
        // Seen twenty times, so frecency alone would put it first.
        for _ in 0..20 {
            lib.visit("https://gist.github.com/y", "gist page");
        }
        lib.visit("https://github.com/x", "github page");
        let hits = lib.search("github", 10);
        assert_eq!(hits[0].url, "https://github.com/x");
        assert_eq!(hits[1].url, "https://gist.github.com/y");
    }

    #[test]
    fn every_word_must_match() {
        let mut lib = Library::load(&tmp_dir("words"));
        lib.visit("https://example.com/rust-lang", "The Rust Language");
        lib.visit("https://example.com/rust", "Rust");
        let hits = lib.search("rust lang", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/rust-lang");
        // Word order is free and case is ignored.
        assert_eq!(lib.search("LANG rust", 10).len(), 1);
        // A word in neither url nor title matches nothing.
        assert!(lib.search("rust python", 10).is_empty());
    }

    #[test]
    fn forget_drops_one_url() {
        let dir = tmp_dir("forget");
        let mut lib = Library::load(&dir);
        lib.visit("https://a.com/", "A");
        lib.visit("https://b.com/", "B");
        lib.forget("https://a.com/");
        let left: Vec<String> = lib.recent("", 0, 10).into_iter().map(|v| v.url).collect();
        assert_eq!(left, vec!["https://b.com/".to_string()]);
        // Forgetting what is not there stays quiet.
        lib.forget("https://nope.com/");
        assert_eq!(lib.recent("", 0, 10).len(), 1);
        // And it stays forgotten across a save.
        lib.flush();
        let after = Library::load(&dir);
        let left: Vec<String> = after.recent("", 0, 10).into_iter().map(|v| v.url).collect();
        assert_eq!(left, vec!["https://b.com/".to_string()]);
    }

    #[test]
    fn clear_history_since() {
        let dir = tmp_dir("clear");
        let now = unix_now();
        let rows = [
            Visit { url: "https://old.com/".into(), title: "old".into(), last: now - 100, count: 1 },
            Visit { url: "https://new-old.com/".into(), title: "new old".into(), last: now - 10, count: 1 },
            Visit { url: "https://new.com/".into(), title: "new".into(), last: now, count: 1 },
        ];
        let text: String = rows.iter().map(|r| serde_json::to_string(r).unwrap() + "\n").collect();
        fs::write(dir.join(HISTORY_FILE), text).unwrap();

        let mut lib = Library::load(&dir);
        // `last` at or after `since` goes. The older one stays.
        lib.clear_history(Some(now - 50));
        let left: Vec<String> = lib.recent("", 0, 10).into_iter().map(|v| v.url).collect();
        assert_eq!(left, vec!["https://old.com/".to_string()]);
        // None means all of it.
        lib.clear_history(None);
        assert!(lib.recent("", 0, 10).is_empty());
        lib.flush();
        assert_eq!(lines(&dir.join(HISTORY_FILE)), 0);
    }

    #[test]
    fn bookmarks_add_remove_and_are_idempotent() {
        let mut lib = Library::load(&tmp_dir("marks"));
        let id = lib.bookmark("https://a.com/", "A");
        assert!(id > 0);
        assert!(lib.is_bookmarked("https://a.com/"));
        // Already there: same id back, nothing touched.
        assert_eq!(lib.bookmark("https://a.com/", "A again"), id);
        assert_eq!(lib.bookmarks().len(), 1);
        assert_eq!(lib.bookmarks()[0].title, "A");
        // Rename goes by id.
        lib.rename_bookmark(id, "A renamed");
        assert_eq!(lib.bookmarks()[0].title, "A renamed");
        // Newest first, new ids.
        let id2 = lib.bookmark("https://b.com/", "B");
        assert!(id2 != id);
        assert_eq!(lib.bookmarks()[0].url, "https://b.com/");
        lib.unbookmark("https://a.com/");
        assert!(!lib.is_bookmarked("https://a.com/"));
        assert_eq!(lib.bookmarks().len(), 1);
        lib.unbookmark("https://gone.com/");
        assert_eq!(lib.bookmarks().len(), 1);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tmp_dir("roundtrip");
        {
            let mut lib = Library::load(&dir);
            lib.visit("https://a.com/", "A");
            lib.visit("https://a.com/", "");
            lib.visit("https://b.com/", "B");
            lib.set_title("https://b.com/", "B titled");
            lib.bookmark("https://c.com/", "C");
            lib.flush();
        }
        let lib = Library::load(&dir);
        let rows = lib.recent("", 0, 10);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].url, "https://b.com/");
        assert_eq!(rows[0].title, "B titled");
        assert_eq!(rows[1].count, 2);
        assert!(lib.is_bookmarked("https://c.com/"));
        assert_eq!(lib.bookmarks()[0].title, "C");
        assert_eq!(lib.search("b titled", 5).len(), 1);
    }

    #[test]
    fn a_broken_file_reads_as_empty() {
        let dir = tmp_dir("broken");
        fs::write(dir.join(HISTORY_FILE), "not json\n\n{\"url\":\"\",\"title\":\"x\",\"last\":1,\"count\":1}\n").unwrap();
        fs::write(dir.join(BOOKMARKS_FILE), "[[[").unwrap();
        let lib = Library::load(&dir);
        assert!(lib.recent("", 0, 10).is_empty());
        assert!(lib.bookmarks().is_empty());
    }

    #[test]
    fn a_fat_file_is_compacted_on_load() {
        let dir = tmp_dir("compact");
        let path = dir.join(HISTORY_FILE);
        let now = unix_now();
        let mut text = String::new();
        for i in 0..3u64 {
            let a = Visit { url: "https://a.com/".into(), title: format!("A{i}"), last: now + i, count: i as u32 };
            let b = Visit { url: "https://b.com/".into(), title: "B".into(), last: now + i, count: 1 };
            text += &(serde_json::to_string(&a).unwrap() + "\n");
            text += &(serde_json::to_string(&b).unwrap() + "\n");
        }
        fs::write(&path, &text).unwrap();
        assert_eq!(lines(&path), 6);

        let lib = Library::load(&dir);
        // Two live urls out of six lines: the file was rewritten.
        assert_eq!(lines(&path), 2);
        let rows = lib.recent("", 0, 10);
        assert_eq!(rows.len(), 2);
        // The last line for a url is the truth.
        let a = rows.iter().find(|r| r.url == "https://a.com/").expect("a.com");
        assert_eq!(a.title, "A2");
        assert_eq!(a.count, 2);
        // A file already lean is left alone.
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        Library::load(&dir);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    }

    #[test]
    fn history_holds_one_hundred_thousand_and_search_stays_fast() {
        let mut lib = Library::load(&tmp_dir("timing"));
        for i in 0..MAX_HISTORY as u32 {
            lib.visit(&format!("https://site{i}.example.com/page{i}"), &format!("Page number {i}"));
        }
        assert_eq!(lib.recent("", 0, 10).len(), 10);
        // The cap holds at MAX_HISTORY and the oldest page is the one that goes.
        lib.visit("https://newest.example.com/", "Newest");
        assert_eq!(lib.recent("", 0, 10)[0].url, "https://newest.example.com/");
        assert!(lib.recent("site0", 0, 10).is_empty());
        assert_eq!(lib.recent("site99999", 0, 10).len(), 1);

        // The part that matters: three letters, 100k rows, in a debug build.
        let t = Instant::now();
        assert!(lib.search("zq7", 10).is_empty());
        let hits = lib.search("site500", 10);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        assert!(!hits.is_empty());
        assert_eq!(hits[0].url, "https://site500.example.com/page500");
        assert!(ms < 50.0, "search took {ms:.1} ms");
    }
}
