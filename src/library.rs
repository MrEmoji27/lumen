//! History and bookmarks.

#![allow(dead_code)]

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

const HISTORY_FILE: &str = "history.jsonl";
const BOOKMARKS_FILE: &str = "bookmarks.json";
const MAX_HISTORY: usize = 100_000;
const HALF_LIFE: u64 = 7 * 24 * 60 * 60;
const MARK_BOOST: f64 = 1e9;
const HOST_BOOST: f64 = 1e6;
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
    #[serde(default)]
    pub folder: String,
}

#[derive(Deserialize)]
struct BookFile {
    #[serde(default)]
    folders: Vec<String>,
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
}

#[derive(Serialize)]
struct BookFileRef<'a> {
    folders: &'a [String],
    bookmarks: Vec<&'a Bookmark>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Incoming {
    pub folder: String,
    pub title: String,
    pub url: String,
}

pub fn same_url_key(url: &str) -> String {
    let u = url.trim().split('#').next().unwrap_or("");
    u.trim_end_matches('/').to_lowercase()
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

struct Entry {
    v: Visit,
    hay: String,
    mask: [u64; 4],
    seq: u64,
}

struct Mark {
    b: Bookmark,
    hay: String,
    mask: [u64; 4],
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Oldest {
    last: u64,
    seq: u64,
    url: String,
}

pub struct Library {
    dir: PathBuf,
    hist: HashMap<String, Entry>,
    oldest: BinaryHeap<Reverse<Oldest>>,
    marks: Vec<Mark>,
    folders: Vec<String>,
    books_dirty: bool,
    pending: Vec<Visit>,
    rewrite: bool,
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
    pub fn load(dir: &Path) -> Library {
        let _ = fs::create_dir_all(dir);
        let mut lib = Library {
            dir: dir.to_path_buf(),
            hist: HashMap::new(),
            oldest: BinaryHeap::new(),
            marks: Vec::new(),
            folders: Vec::new(),
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

    pub fn set_title(&mut self, url: &str, title: &str) {
        if title.is_empty() {
            return;
        }
        if let Some(e) = self.hist.get_mut(url) {
            e.retitle(title);
            self.pending.push(e.v.clone());
        }
    }

    pub fn search(&self, q: &str, limit: usize) -> Vec<Hit> {
        if limit == 0 {
            return Vec::new();
        }
        let words = words_of(q);
        let typed = q.trim().to_lowercase();
        let now = unix_now();
        let mut hits: Vec<(f64, Hit)> = Vec::new();
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
            if host_is(&m.b.url, &typed) {
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
            if host_is(&e.v.url, &typed) {
                score += HOST_BOOST;
            }
            hits.push((score, Hit { url: e.v.url.clone(), title: e.v.title.clone(), kind: Kind::History }));
        }
        let by = |a: &(f64, Hit), b: &(f64, Hit)| b.0.total_cmp(&a.0).then_with(|| a.1.url.cmp(&b.1.url));
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit - 1, by);
            hits.truncate(limit);
        }
        hits.sort_by(by);
        hits.into_iter().map(|(_, h)| h).collect()
    }

    pub fn complete(&self, typed: &str) -> Option<String> {
        let t = typed.trim().to_lowercase();
        if t.is_empty() || t.contains(char::is_whitespace) || t.contains("://") {
            return None;
        }
        let page = t.contains('/');
        let keep_www = t.starts_with("www.");
        let now = unix_now();
        let mut best: HashMap<String, f64> = HashMap::new();
        let mut add = |url: &str, score: f64| {
            let Some(key) = fill_key(url, page, keep_www) else { return };
            if key.len() > t.len() && key.starts_with(&t) {
                *best.entry(key).or_default() += score;
            }
        };
        for m in &self.marks {
            add(&m.b.url, MARK_BOOST);
        }
        for e in self.hist.values() {
            add(&e.v.url, frecency(e.v.count, now.saturating_sub(e.v.last)));
        }
        best.into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.len().cmp(&a.0.len())).then_with(|| b.0.cmp(&a.0)))
            .map(|(k, _)| k)
    }

    pub fn recent(&self, filter: &str, offset: usize, limit: usize) -> Vec<Visit> {
        let words = words_of(filter);
        let mut rows: Vec<&Entry> = self.hist.values().filter(|e| matchable(&e.hay, &e.mask, &words)).collect();
        rows.sort_by(|a, b| b.v.last.cmp(&a.v.last).then(b.seq.cmp(&a.seq)).then(a.v.url.cmp(&b.v.url)));
        rows.into_iter().skip(offset).take(limit).map(|e| e.v.clone()).collect()
    }

    pub fn forget(&mut self, url: &str) {
        if self.hist.remove(url).is_some() {
            self.rewrite = true;
        }
    }

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

    pub fn bookmark(&mut self, url: &str, title: &str) -> u64 {
        if url.is_empty() {
            return 0;
        }
        if let Some(m) = self.marks.iter().find(|m| m.b.url == url) {
            return m.b.id;
        }
        let id = self.next_id;
        self.next_id += 1;
        let b = Bookmark { id, url: url.to_string(), title: title.to_string(), added: unix_now(), folder: String::new() };
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

    pub fn bookmarks(&self) -> Vec<Bookmark> {
        self.marks.iter().map(|m| m.b.clone()).collect()
    }

    pub fn mark_for_url(&self, url: &str) -> Option<Bookmark> {
        self.marks.iter().find(|m| m.b.url == url).map(|m| m.b.clone())
    }

    pub fn folders(&self) -> Vec<String> {
        let mut out = self.folders.clone();
        for m in &self.marks {
            if !m.b.folder.is_empty() && !out.contains(&m.b.folder) {
                out.push(m.b.folder.clone());
            }
        }
        out
    }

    fn ensure_folder(&mut self, name: &str) {
        if !name.is_empty() && !self.folders.iter().any(|f| f == name) {
            self.folders.push(name.to_string());
        }
    }

    pub fn add_folder(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || self.folders().iter().any(|f| f.eq_ignore_ascii_case(name)) {
            return false;
        }
        self.folders.push(name.to_string());
        self.books_dirty = true;
        true
    }

    pub fn rename_folder(&mut self, old: &str, new: &str) -> bool {
        let new = new.trim();
        if new.is_empty() || old == new || !self.folders().iter().any(|f| f == old) {
            return false;
        }
        if self.folders().iter().any(|f| f != old && f.eq_ignore_ascii_case(new)) {
            return false;
        }
        self.ensure_folder(old);
        for f in &mut self.folders {
            if f == old {
                *f = new.to_string();
            }
        }
        for m in &mut self.marks {
            if m.b.folder == old {
                m.b.folder = new.to_string();
            }
        }
        self.books_dirty = true;
        true
    }

    pub fn delete_folder(&mut self, name: &str) {
        self.folders.retain(|f| f != name);
        for m in &mut self.marks {
            if m.b.folder == name {
                m.b.folder.clear();
            }
        }
        self.books_dirty = true;
    }

    pub fn move_bookmark(&mut self, id: u64, folder: &str) {
        self.place(id, folder, None);
    }

    pub fn place(&mut self, id: u64, folder: &str, before: Option<u64>) {
        let Some(from) = self.marks.iter().position(|m| m.b.id == id) else { return };
        if before == Some(id) {
            return;
        }
        let mut m = self.marks.remove(from);
        m.b.folder = folder.to_string();
        self.ensure_folder(folder);
        let at = match before.and_then(|b| self.marks.iter().position(|x| x.b.id == b)) {
            Some(p) => p,
            None => self.marks.iter().rposition(|x| x.b.folder == folder).map_or(self.marks.len(), |p| p + 1),
        };
        self.marks.insert(at, m);
        self.books_dirty = true;
    }

    pub fn shift_bookmark(&mut self, id: u64, delta: i32) {
        let Some(from) = self.marks.iter().position(|m| m.b.id == id) else { return };
        let folder = self.marks[from].b.folder.clone();
        let same: Vec<usize> = (0..self.marks.len()).filter(|&i| self.marks[i].b.folder == folder).collect();
        let Some(at) = same.iter().position(|&i| i == from) else { return };
        let to = at as i32 + delta;
        if to < 0 || to as usize >= same.len() {
            return;
        }
        self.marks.swap(from, same[to as usize]);
        self.books_dirty = true;
    }

    pub fn edit_bookmark(&mut self, id: u64, title: &str, url: &str, folder: &str) -> bool {
        let url = url.trim();
        if url.is_empty() || self.marks.iter().any(|m| m.b.id != id && m.b.url == url) {
            return false;
        }
        self.ensure_folder(folder);
        let Some(m) = self.marks.iter_mut().find(|m| m.b.id == id) else { return false };
        m.b.title = title.trim().to_string();
        m.b.url = url.to_string();
        m.b.folder = folder.to_string();
        m.rebuild();
        self.books_dirty = true;
        true
    }

    pub fn delete_bookmark(&mut self, id: u64) {
        let before = self.marks.len();
        self.marks.retain(|m| m.b.id != id);
        if self.marks.len() != before {
            self.books_dirty = true;
        }
    }

    pub fn import(&mut self, items: &[Incoming]) -> (usize, usize) {
        let mut have: HashSet<String> = self.marks.iter().map(|m| same_url_key(&m.b.url)).collect();
        let (mut added, mut skipped) = (0, 0);
        for it in items {
            if it.url.trim().is_empty() || !have.insert(same_url_key(&it.url)) {
                skipped += 1;
                continue;
            }
            self.ensure_folder(&it.folder);
            let id = self.next_id;
            self.next_id += 1;
            let b = Bookmark { id, url: it.url.trim().to_string(), title: it.title.trim().to_string(), added: unix_now(), folder: it.folder.clone() };
            let at = self.marks.iter().rposition(|x| x.b.folder == it.folder).map_or(self.marks.len(), |p| p + 1);
            self.marks.insert(at, Mark::new(b));
            added += 1;
        }
        if added > 0 {
            self.books_dirty = true;
        }
        (added, skipped)
    }

    pub fn top_visits(&self, limit: usize) -> Vec<Visit> {
        let now = unix_now();
        let mut rows: Vec<(f64, &Entry)> = self.hist.values().map(|e| (frecency(e.v.count, now.saturating_sub(e.v.last)), e)).collect();
        rows.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.v.url.cmp(&b.1.v.url)));
        rows.into_iter().take(limit).map(|(_, e)| e.v.clone()).collect()
    }

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
            self.seq += 1;
            self.oldest.push(Reverse(Oldest { last: v.last, seq: self.seq, url: v.url.clone() }));
            self.hist.insert(v.url.clone(), Entry::new(v, self.seq));
        }
        let dropped = self.trim();
        if lines > self.hist.len() * 2 || dropped {
            self.write_history();
        }
    }

    fn read_bookmarks(&mut self) {
        let path = self.dir.join(BOOKMARKS_FILE);
        let Ok(text) = fs::read_to_string(&path) else { return };
        let (list, folders) = match serde_json::from_str::<BookFile>(&text) {
            Ok(f) => (f.bookmarks, f.folders),
            Err(_) => match serde_json::from_str::<Vec<Bookmark>>(&text) {
                Ok(l) => (l, Vec::new()),
                Err(_) => return,
            },
        };
        let mut seen: HashSet<String> = HashSet::new();
        for b in list {
            if b.url.is_empty() || !seen.insert(b.url.clone()) {
                continue;
            }
            self.next_id = self.next_id.max(b.id + 1);
            self.marks.push(Mark::new(b));
        }
        for f in folders {
            self.ensure_folder(&f);
        }
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
        let file = BookFileRef { folders: &self.folders, bookmarks: self.marks.iter().map(|m| &m.b).collect() };
        if let Ok(json) = serde_json::to_string_pretty(&file) {
            atomic_write(&self.dir.join(BOOKMARKS_FILE), &json);
        }
    }

    fn trim(&mut self) -> bool {
        let mut dropped = false;
        while self.hist.len() > MAX_HISTORY {
            let Some(Reverse(old)) = self.oldest.pop() else { break };
            let live = self.hist.get(&old.url).is_some_and(|e| e.v.last == old.last && e.seq == old.seq);
            if live {
                self.hist.remove(&old.url);
                dropped = true;
            }
        }
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

fn hay_of(url: &str, title: &str) -> String {
    format!("{}\n{}", url.to_lowercase(), title.to_lowercase())
}

fn mask_of(hay: &str) -> [u64; 4] {
    let mut mask = [0u64; 4];
    for b in hay.bytes() {
        mask[(b >> 6) as usize] |= 1u64 << (b & 63);
    }
    mask
}

fn matchable(hay: &str, mask: &[u64; 4], words: &[String]) -> bool {
    words.iter().all(|w| {
        w.as_bytes().iter().all(|&b| mask[(b >> 6) as usize] & (1u64 << (b & 63)) != 0) && hay.contains(w.as_str())
    })
}

fn words_of(q: &str) -> Vec<String> {
    q.split_whitespace().map(|w| w.to_lowercase()).collect()
}

fn frecency(count: u32, age: u64) -> f64 {
    (count as f64 + 1.0) * 0.5_f64.powf(age as f64 / HALF_LIFE as f64)
}

fn fill_key(url: &str, page: bool, keep_www: bool) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let rest = rest.split('#').next().unwrap_or(rest).to_lowercase();
    let rest = if keep_www { rest.as_str() } else { rest.strip_prefix("www.").unwrap_or(&rest) };
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let key = if page { rest.trim_end_matches('/') } else { &rest[..end] };
    (!key.is_empty()).then(|| key.to_string())
}

fn host_starts_with(url: &str, q: &str) -> bool {
    if q.is_empty() {
        return false;
    }
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..end].to_lowercase();
    host.starts_with(q) || host.strip_prefix("www.").is_some_and(|h| h.starts_with(q))
}

fn host_is(url: &str, q: &str) -> bool {
    if q.is_empty() {
        return false;
    }
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..end].to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    host == q || host.split('.').next() == Some(q)
}

fn dead_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else { return false };
    DEAD_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str())
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

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

    #[test]
    fn the_address_bar_fills_in_the_site_you_go_to_most() {
        let dir = tmp_dir("complete");
        let mut lib = Library::load(&dir);
        for _ in 0..5 {
            lib.visit("https://www.youtube.com/watch?v=a", "A video");
        }
        lib.visit("https://www.youtube.com/feed/subscriptions", "Subs");
        lib.visit("https://yourdictionary.com/word", "Word");
        lib.visit("https://github.com/rust-lang/rust", "Rust");
        assert_eq!(lib.complete("yo").as_deref(), Some("youtube.com"));
        assert_eq!(lib.complete("YouT").as_deref(), Some("youtube.com"), "case does not matter");
        assert_eq!(lib.complete("your").as_deref(), Some("yourdictionary.com"));
        assert_eq!(lib.complete("github.com/r").as_deref(), Some("github.com/rust-lang/rust"));
        assert_eq!(lib.complete("youtube.com/f").as_deref(), Some("youtube.com/feed/subscriptions"));
        assert_eq!(lib.complete("youtube.com"), None);
        assert_eq!(lib.complete("you tube"), None);
        assert_eq!(lib.complete("https://yo"), None);
        assert_eq!(lib.complete("zz"), None);
        assert_eq!(lib.complete(""), None);
        lib.bookmark("https://yodel.example/", "Yodel");
        assert_eq!(lib.complete("yo").as_deref(), Some("yodel.example"));
        let _ = fs::remove_dir_all(&dir);
    }

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
        lib.visit("https://a.com/", "");
        assert_eq!(lib.recent("", 0, 10)[0].title, "A again");
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
        assert_eq!(lib.search("LANG rust", 10).len(), 1);
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
        lib.forget("https://nope.com/");
        assert_eq!(lib.recent("", 0, 10).len(), 1);
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
        lib.clear_history(Some(now - 50));
        let left: Vec<String> = lib.recent("", 0, 10).into_iter().map(|v| v.url).collect();
        assert_eq!(left, vec!["https://old.com/".to_string()]);
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
        assert_eq!(lib.bookmark("https://a.com/", "A again"), id);
        assert_eq!(lib.bookmarks().len(), 1);
        assert_eq!(lib.bookmarks()[0].title, "A");
        lib.rename_bookmark(id, "A renamed");
        assert_eq!(lib.bookmarks()[0].title, "A renamed");
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
        assert_eq!(lines(&path), 2);
        let rows = lib.recent("", 0, 10);
        assert_eq!(rows.len(), 2);
        let a = rows.iter().find(|r| r.url == "https://a.com/").expect("a.com");
        assert_eq!(a.title, "A2");
        assert_eq!(a.count, 2);
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
        lib.visit("https://newest.example.com/", "Newest");
        assert_eq!(lib.recent("", 0, 10)[0].url, "https://newest.example.com/");
        assert!(lib.recent("site0", 0, 10).is_empty());
        assert_eq!(lib.recent("site99999", 0, 10).len(), 1);

        let t = Instant::now();
        assert!(lib.search("zq7", 10).is_empty());
        let hits = lib.search("site500", 10);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        assert!(!hits.is_empty());
        assert_eq!(hits[0].url, "https://site500.example.com/page500");
        assert!(ms < 50.0, "search took {ms:.1} ms");
    }

    fn marks_in(lib: &Library, folder: &str) -> Vec<String> {
        lib.bookmarks().into_iter().filter(|b| b.folder == folder).map(|b| b.title).collect()
    }

    fn inc(folder: &str, title: &str, url: &str) -> Incoming {
        Incoming { folder: folder.into(), title: title.into(), url: url.into() }
    }

    #[test]
    fn a_bare_list_from_before_folders_still_loads() {
        let dir = tmp_dir("oldbooks");
        fs::write(dir.join(BOOKMARKS_FILE), r#"[{"id":3,"url":"https://a.com/","title":"A","added":10},{"id":4,"url":"https://b.com/","title":"B","added":20}]"#).unwrap();
        let lib = Library::load(&dir);
        let all = lib.bookmarks();
        assert_eq!(all.iter().map(|b| b.title.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert!(all.iter().all(|b| b.folder.is_empty()));
        assert!(lib.folders().is_empty());
    }

    #[test]
    fn folders_and_order_survive_a_restart() {
        let dir = tmp_dir("folders-disk");
        let mut lib = Library::load(&dir);
        let a = lib.bookmark("https://a.com/", "A");
        lib.bookmark("https://b.com/", "B");
        assert!(lib.add_folder("Work"));
        assert!(lib.add_folder("Empty"));
        lib.move_bookmark(a, "Work");
        lib.flush();
        let again = Library::load(&dir);
        assert_eq!(again.folders(), ["Work", "Empty"]);
        assert_eq!(marks_in(&again, "Work"), ["A"]);
        assert_eq!(marks_in(&again, ""), ["B"]);
    }

    #[test]
    fn folder_names_are_unique_and_not_blank() {
        let mut lib = Library::load(&tmp_dir("folder-names"));
        assert!(lib.add_folder("Work"));
        assert!(!lib.add_folder("work"));
        assert!(!lib.add_folder("   "));
        assert!(lib.add_folder("Home"));
        assert!(!lib.rename_folder("Home", "WORK"));
        assert!(!lib.rename_folder("Nope", "X"));
        assert!(lib.rename_folder("Home", "House"));
        assert_eq!(lib.folders(), ["Work", "House"]);
    }

    #[test]
    fn renaming_a_folder_takes_its_bookmarks_along() {
        let mut lib = Library::load(&tmp_dir("folder-rename"));
        let a = lib.bookmark("https://a.com/", "A");
        lib.move_bookmark(a, "Work");
        assert!(lib.rename_folder("Work", "Job"));
        assert_eq!(marks_in(&lib, "Job"), ["A"]);
        assert_eq!(lib.folders(), ["Job"]);
    }

    #[test]
    fn deleting_a_folder_keeps_its_bookmarks() {
        let mut lib = Library::load(&tmp_dir("folder-delete"));
        let a = lib.bookmark("https://a.com/", "A");
        lib.move_bookmark(a, "Work");
        lib.delete_folder("Work");
        assert!(lib.folders().is_empty());
        assert_eq!(marks_in(&lib, ""), ["A"]);
    }

    #[test]
    fn moving_up_and_down_stays_inside_the_folder() {
        let mut lib = Library::load(&tmp_dir("shift"));
        let a = lib.bookmark("https://a.com/", "A");
        let x = lib.bookmark("https://x.com/", "X");
        let b = lib.bookmark("https://b.com/", "B");
        lib.move_bookmark(x, "Work");
        assert_eq!(marks_in(&lib, ""), ["B", "A"]);
        lib.shift_bookmark(a, 1);
        assert_eq!(marks_in(&lib, ""), ["B", "A"], "at the end already");
        lib.shift_bookmark(b, -1);
        assert_eq!(marks_in(&lib, ""), ["B", "A"], "at the top already");
        lib.shift_bookmark(b, 1);
        assert_eq!(marks_in(&lib, ""), ["A", "B"]);
        assert_eq!(marks_in(&lib, "Work"), ["X"]);
    }

    #[test]
    fn dropping_places_before_a_bookmark_or_at_the_end_of_a_folder() {
        let mut lib = Library::load(&tmp_dir("place"));
        let a = lib.bookmark("https://a.com/", "A");
        let b = lib.bookmark("https://b.com/", "B");
        let c = lib.bookmark("https://c.com/", "C");
        lib.place(a, "", Some(c));
        assert_eq!(marks_in(&lib, ""), ["A", "C", "B"]);
        lib.place(a, "Work", None);
        lib.place(c, "Work", None);
        assert_eq!(marks_in(&lib, "Work"), ["A", "C"]);
        assert_eq!(marks_in(&lib, ""), ["B"]);
        lib.place(b, "", Some(b));
        assert_eq!(marks_in(&lib, ""), ["B"]);
    }

    #[test]
    fn editing_changes_title_address_and_folder_but_not_into_a_duplicate() {
        let mut lib = Library::load(&tmp_dir("edit"));
        let a = lib.bookmark("https://a.com/", "A");
        lib.bookmark("https://b.com/", "B");
        assert!(lib.edit_bookmark(a, "A2", "https://a2.com/", "Work"));
        let got = lib.mark_for_url("https://a2.com/").expect("moved to the new address");
        assert_eq!((got.title.as_str(), got.folder.as_str()), ("A2", "Work"));
        assert!(lib.folders().contains(&"Work".to_string()));
        assert!(!lib.edit_bookmark(a, "A3", "https://b.com/", ""));
        assert!(!lib.edit_bookmark(a, "A3", "  ", ""));
        assert_eq!(lib.mark_for_url("https://a2.com/").unwrap().title, "A2");
        assert!(lib.is_bookmarked("https://a2.com/") && !lib.is_bookmarked("https://a.com/"));
    }

    #[test]
    fn import_skips_what_is_already_here_in_any_folder() {
        let mut lib = Library::load(&tmp_dir("import"));
        let g = lib.bookmark("https://github.com/", "GitHub");
        lib.move_bookmark(g, "Work");
        let items = [
            inc("Chrome", "GitHub again", "https://GitHub.com"),
            inc("Chrome", "Rust", "https://www.rust-lang.org/"),
            inc("Chrome / Dev", "Docs", "https://doc.rust-lang.org/#top"),
            inc("Chrome / Dev", "Docs twice", "https://doc.rust-lang.org/"),
            inc("Chrome", "", ""),
        ];
        let (added, skipped) = lib.import(&items);
        assert_eq!((added, skipped), (2, 3));
        assert_eq!(marks_in(&lib, "Chrome"), ["Rust"]);
        assert_eq!(marks_in(&lib, "Chrome / Dev"), ["Docs"]);
        assert_eq!(lib.folders(), ["Work", "Chrome", "Chrome / Dev"]);
        assert_eq!(lib.import(&items), (0, 5));
    }

    #[test]
    fn import_keeps_the_other_browsers_order() {
        let mut lib = Library::load(&tmp_dir("import-order"));
        lib.import(&[inc("Edge", "One", "https://1.test/"), inc("Edge", "Two", "https://2.test/"), inc("Edge", "Three", "https://3.test/")]);
        assert_eq!(marks_in(&lib, "Edge"), ["One", "Two", "Three"]);
    }

    #[test]
    fn top_visits_come_most_visited_first() {
        let mut lib = Library::load(&tmp_dir("top"));
        lib.visit("https://once.com/", "Once");
        for _ in 0..5 {
            lib.visit("https://often.com/", "Often");
        }
        let top = lib.top_visits(10);
        assert_eq!(top[0].url, "https://often.com/");
        assert_eq!(lib.top_visits(1).len(), 1);
    }
}
