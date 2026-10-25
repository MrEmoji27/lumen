//! The journal: what Lumen did on this PC, in plain lines, for one person to read and fix what hurt.

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

use crate::store::Settings;

const KEEP_DAYS: i64 = 14;
const MAX_DAY: u64 = 20 << 20;
const MAX_BUF: usize = 64 << 10;
const FLUSH_AFTER: Duration = Duration::from_secs(60);
const SLOW_NAV_MS: u64 = 8_000;
const SLOW_FRAME_MS: f64 = 50.0;
const MAX_MSG: usize = 200;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn now_secs() -> u64 {
    now_ms() / 1000
}

fn local_date(secs: u64) -> String {
    use windows::Win32::{
        Foundation::{FILETIME, SYSTEMTIME},
        System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
    };
    let ft100 = (secs + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ft100 as u32, dwHighDateTime: (ft100 >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        let _ = FileTimeToSystemTime(&ft, &mut utc);
        let _ = SystemTimeToTzSpecificLocalTime(None, &utc, &mut local);
    }
    format!("{:04}-{:02}-{:02}", local.wYear, local.wMonth, local.wDay)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe as i64 * 365 + (yoe / 4) as i64 - (yoe / 100) as i64 + doy as i64;
    era as i64 * 146_097 + doe - 719_468
}

fn day_of(name: &str) -> Option<i64> {
    let (y, rest) = name.split_once('-')?;
    let (m, d) = rest.split_once('-')?;
    let (y, m, d): (i32, u32, u32) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

pub fn host_only(url: &str) -> String {
    let Some((scheme, after)) = url.split_once("://") else { return String::new() };
    if !matches!(scheme, "http" | "https") {
        return String::new();
    }
    let host = after.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = if host.starts_with('[') {
        host.split(']').next().unwrap_or(host).trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or(host)
    };
    host.to_ascii_lowercase()
}

pub fn scrub(msg: &str) -> String {
    let mut out = String::new();
    let mut rest = msg;
    while let Some(at) = rest.find("http") {
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let end = from.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | '>')).unwrap_or(from.len());
        let (url, tail) = from.split_at(end);
        let host = host_only(url);
        if host.is_empty() {
            out.push_str(&url[..url.len().min(4)]);
        } else {
            out.push_str(&host);
        }
        out.push('…');
        rest = tail;
    }
    out.push_str(rest);
    let clean: String = out.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut short = clean.trim().to_string();
    if short.chars().count() > MAX_MSG {
        let cut = short.char_indices().nth(MAX_MSG).map(|(i, _)| i).unwrap_or(short.len());
        short.truncate(cut);
        short.push('…');
    }
    short
}

fn cores() -> f64 {
    static N: OnceLock<f64> = OnceLock::new();
    *N.get_or_init(|| std::thread::available_parallelism().map_or(1.0, |n| n.get() as f64))
}

fn mb(bytes: u64) -> f64 {
    ((bytes as f64 / 1_048_576.0) * 10.0).round() / 10.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn s(text: &str) -> Value {
    Value::String(text.to_string())
}

fn line(kind: &str, at: u64, fields: &[(&str, Value)]) -> String {
    let mut out = format!("{{\"t\":{at},\"k\":{}", s(kind));
    for (key, value) in fields {
        out.push(',');
        out.push_str(&s(key).to_string());
        out.push(':');
        out.push_str(&value.to_string());
    }
    out.push('}');
    out
}

fn droppable(first: &str) -> bool {
    first.contains("\"k\":\"sample\"") || first.contains("\"k\":\"wake\"") || first.contains("\"k\":\"ui_slow\"") || first.contains("\"k\":\"ui_first\"")
}

fn day_path(dir: &Path, day: &str) -> PathBuf {
    dir.join(format!("{day}.jsonl"))
}

fn open_day(dir: &Path, day: &str) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(day_path(dir, day)).ok()
}

fn day_size(dir: &Path, day: &str) -> u64 {
    fs::metadata(day_path(dir, day)).map(|m| m.len()).unwrap_or(0)
}

fn clean_old(dir: &Path, today: &str) {
    let Some(today) = day_of(today) else { return };
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(day) = name.strip_suffix(".jsonl").and_then(day_of) else { continue };
        if today - day >= KEEP_DAYS {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn previous_file(dir: &Path, today: &str) -> Option<PathBuf> {
    let mut best: Option<(i64, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(day) = name.strip_suffix(".jsonl").and_then(day_of) else { continue };
        if name == format!("{today}.jsonl") {
            continue;
        }
        if best.as_ref().is_none_or(|(seen, _)| day > *seen) {
            best = Some((day, entry.path()));
        }
    }
    best.map(|(_, path)| path)
}

fn last_run_file(dir: &Path, today: &str) -> Option<PathBuf> {
    let mine = day_path(dir, today);
    if fs::metadata(&mine).map(|m| m.len() > 0).unwrap_or(false) {
        return Some(mine);
    }
    previous_file(dir, today)
}

fn ended_uncleanly(path: &Path) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    let mut last: Option<(bool, u64)> = None;
    for raw in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(raw) else { continue };
        let (started, exited) = (v["k"] == "start", v["k"] == "exit");
        if started || exited {
            last = Some((started, v["t"].as_u64().unwrap_or(0)));
        }
    }
    match last {
        Some((true, at)) => Some(at),
        _ => None,
    }
}

fn crash_tail(after: SystemTime) -> Vec<String> {
    let path = crate::store::data_dir().join("crash.log");
    let Ok(meta) = fs::metadata(&path) else { return Vec::new() };
    if meta.modified().map_or(true, |m| m <= after) {
        return Vec::new();
    }
    let Ok(text) = fs::read_to_string(&path) else { return Vec::new() };
    let mut tail: Vec<String> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(3)
        .map(|l| l.chars().take(MAX_MSG).collect())
        .collect();
    tail.reverse();
    tail
}

struct Journal {
    dir: PathBuf,
    day: String,
    file: Option<File>,
    buf: String,
    written: u64,
    full: bool,
    cap: u64,
    buf_max: usize,
    last_flush: Instant,
    nav: HashMap<u64, PendingNav>,
    crashed: HashMap<u64, (&'static str, String)>,
    dl: HashMap<u64, Instant>,
    sessions: HashSet<String>,
    last_query: String,
    mem: (u64, u64, u64),
    cpu_at: Option<(Instant, u64)>,
}

struct PendingNav {
    at: Instant,
    host: String,
    agent: bool,
    blocked: Option<u64>,
}

impl Journal {
    fn open(dir: &Path) -> Journal {
        let day = local_date(now_secs());
        Journal {
            file: open_day(dir, &day),
            written: day_size(dir, &day),
            dir: dir.to_path_buf(),
            day,
            buf: String::new(),
            full: false,
            cap: MAX_DAY,
            buf_max: MAX_BUF,
            last_flush: Instant::now(),
            nav: HashMap::new(),
            crashed: HashMap::new(),
            dl: HashMap::new(),
            sessions: HashSet::new(),
            last_query: String::new(),
            mem: (0, 0, 0),
            cpu_at: None,
        }
    }

    #[cfg(test)]
    fn open_at(dir: &Path, day: &str) -> Journal {
        let mut journal = Journal::open(dir);
        journal.day = day.to_string();
        journal.file = open_day(dir, day);
        journal.written = day_size(dir, day);
        journal
    }

    fn write(&mut self, kind: &str, fields: &[(&str, Value)]) {
        let at = now_ms();
        let day = local_date(at / 1000);
        if day != self.day {
            self.rotate(&day);
        }
        if self.full {
            return;
        }
        let text = line(kind, at, fields);
        if self.written + self.buf.len() as u64 + text.len() as u64 + 1 > self.cap {
            self.full = true;
            return;
        }
        while self.buf.len() + text.len() + 1 > self.buf_max && !self.buf.is_empty() {
            self.drop_one();
        }
        self.buf.push_str(&text);
        self.buf.push('\n');
    }

    fn drop_one(&mut self) {
        let mut at = 0;
        while let Some(nl) = self.buf[at..].find('\n') {
            let end = at + nl + 1;
            if droppable(&self.buf[at..end]) {
                self.buf.replace_range(at..end, "");
                return;
            }
            at = end;
        }
        match self.buf.find('\n') {
            Some(nl) => self.buf.replace_range(..nl + 1, ""),
            None => self.buf.clear(),
        }
    }

    fn rotate(&mut self, day: &str) {
        self.flush();
        self.day = day.to_string();
        self.file = open_day(&self.dir, day);
        self.written = day_size(&self.dir, day);
        self.full = false;
    }

    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        if self.file.is_none() {
            self.file = open_day(&self.dir, &self.day);
        }
        let wrote = match self.file.as_mut() {
            Some(file) => file.write_all(self.buf.as_bytes()).is_ok() && file.flush().is_ok(),
            None => false,
        };
        if wrote {
            self.written += self.buf.len() as u64;
        } else {
            self.file = None;
        }
        self.buf.clear();
        self.last_flush = Instant::now();
    }

    fn flush_if_due(&mut self) {
        if !self.buf.is_empty() && self.last_flush.elapsed() >= FLUSH_AFTER {
            self.flush();
        }
    }

    fn cpu_percent(&mut self, now: Instant) -> f64 {
        let previous = self.cpu_at.replace((now, self.mem.2));
        let Some((at, before)) = previous else { return 0.0 };
        let secs = now.duration_since(at).as_secs_f64();
        if secs < 1.0 {
            return 0.0;
        }
        (self.mem.2.saturating_sub(before) as f64 / 1e7) / secs / cores() * 100.0
    }

    fn drain_slow(&mut self) {
        let count = SLOW_FRAMES.swap(0, Ordering::Relaxed);
        let worst = SLOW_WORST.swap(0, Ordering::Relaxed);
        if count == 0 {
            return;
        }
        self.write("ui_slow", &[("n", json!(count)), ("worst", json!(worst as f64 / 10.0))]);
    }

    fn nav_started(&mut self, tab: u64, url: &str, agent: bool, private: bool, blocked: Option<u64>) {
        if private {
            return;
        }
        let host = host_only(url);
        if host.is_empty() {
            return;
        }
        if self.nav.len() > 64 {
            self.nav.clear();
        }
        self.nav.insert(tab, PendingNav { at: Instant::now(), host, agent, blocked });
    }

    fn nav_done(&mut self, tab: u64, ok: bool, err: i32, http: i32, blocked: Option<u64>) {
        let Some(start) = self.nav.remove(&tab) else { return };
        let ms = start.at.elapsed().as_millis() as u64;
        let mut fields: Vec<(&str, Value)> = vec![("host", s(&start.host)), ("ms", json!(ms)), ("ok", json!(ok))];
        if !ok {
            fields.push(("err", json!(err)));
        }
        if http > 0 {
            fields.push(("http", json!(http)));
        }
        let count = match (start.blocked, blocked) {
            (Some(before), Some(after)) => Some(after.saturating_sub(before)),
            _ => None,
        };
        if let Some(count) = count.filter(|n| *n > 0) {
            fields.push(("blocked", json!(count)));
        }
        if start.agent {
            fields.push(("agent", json!(true)));
        }
        self.write(if ms > SLOW_NAV_MS { "slow_nav" } else { "nav" }, &fields);
        if let Some((kind, host)) = self.crashed.remove(&tab) {
            self.write("crash", &[("kind", s(kind)), ("host", s(&host)), ("reload", s(if ok { "ok" } else { "failed" }))]);
        }
    }

    fn crash(&mut self, tab: u64, kind: &'static str, host: &str, private: bool) {
        if private {
            return;
        }
        self.write("crash", &[("kind", s(kind)), ("host", s(host))]);
        self.crashed.insert(tab, (kind, host.to_string()));
    }

    fn feature(&mut self, name: &str, private: bool) {
        if private {
            return;
        }
        self.write("feature", &[("f", s(name))]);
    }

    fn history_search(&mut self, query: &str) {
        let query = query.trim();
        if query.chars().count() < 3 || self.last_query == query {
            return;
        }
        self.last_query = query.to_string();
        self.write("feature", &[("f", s("history_search"))]);
    }

    fn download(&mut self, id: u64, kind: &str, state: &str, bytes: i64, private: bool) {
        if private {
            return;
        }
        if !matches!(state, "done" | "failed") {
            self.dl.entry(id).or_insert_with(Instant::now);
            return;
        }
        let took = self.dl.remove(&id).map(|at| at.elapsed());
        let mut fields: Vec<(&str, Value)> = vec![("f", s("download")), ("kind", s(kind)), ("result", s(state))];
        if let Some(took) = took.filter(|t| t.as_secs_f64() >= 1.0 && bytes > 0) {
            fields.push(("ms", json!(took.as_millis() as u64)));
            fields.push(("avg", json!((bytes as f64 / took.as_secs_f64()).round() as u64)));
        }
        self.write("feature", &fields);
    }

    fn agent_session(&mut self, session: &str, name: &str) {
        if !self.sessions.insert(session.to_string()) {
            return;
        }
        let mut fields: Vec<(&str, Value)> = vec![("f", s("agent_session"))];
        if !name.is_empty() {
            fields.push(("who", s(name)));
        }
        self.write("feature", &fields);
    }
}

static STATE: Mutex<Option<Journal>> = Mutex::new(None);
static ENABLED: AtomicBool = AtomicBool::new(false);
static WAKES: AtomicU64 = AtomicU64::new(0);
static SLOW_FRAMES: AtomicU32 = AtomicU32::new(0);
static SLOW_WORST: AtomicU32 = AtomicU32::new(0);

fn with(work: impl FnOnce(&mut Journal)) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(journal) = guard.as_mut() {
        work(journal);
    }
}

pub fn on() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn period() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(n) = std::env::var("LUMEN_JOURNAL_SECS").ok().and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0) {
        return Duration::from_secs(n.clamp(1, 3600));
    }
    Duration::from_secs(60)
}

pub struct Sample {
    pub tabs: usize,
    pub live: usize,
    pub asleep: usize,
    pub discarded: usize,
    pub workspaces: usize,
    pub panel: &'static str,
}

pub fn start(settings: &Settings, restored: usize) -> bool {
    if !settings.journal {
        return false;
    }
    let mut unclean = false;
    let dir = crate::store::data_dir().join("journal");
    let _ = fs::create_dir_all(&dir);
    let today = local_date(now_secs());
    clean_old(&dir, &today);
    let mut journal = Journal::open(&dir);
    if let Some(path) = last_run_file(&dir, &today)
        && let Some(since) = ended_uncleanly(&path)
    {
        let day = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let log = crash_tail(UNIX_EPOCH + Duration::from_millis(since));
        let mut fields: Vec<(&str, Value)> = vec![("from", s(&day))];
        if !log.is_empty() {
            fields.push(("log", json!(log)));
        }
        journal.write("unclean_exit", &fields);
        unclean = true;
    }
    journal.write(
        "start",
        &[
            ("v", s(env!("LUMEN_VERSION"))),
            ("headless", json!(crate::headless())),
            ("theme", s(&settings.theme)),
            ("compact", json!(settings.compact)),
            ("dark_pages", json!(settings.dark_pages)),
            ("blocker", json!(settings.adblock)),
            ("sleep", json!(settings.sleep_after_min)),
            ("discard", json!(settings.discard_after_min)),
            ("restored", json!(restored)),
        ],
    );
    journal.flush();
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(journal);
    ENABLED.store(true, Ordering::Relaxed);
    unclean
}

pub fn stop(clean: bool, uptime: Duration) {
    let journal = STATE.lock().unwrap_or_else(|e| e.into_inner()).take();
    ENABLED.store(false, Ordering::Relaxed);
    if let Some(mut journal) = journal {
        journal.write("exit", &[("clean", json!(clean)), ("uptime", json!(uptime.as_secs()))]);
        journal.flush();
    }
}

pub fn set_enabled(on: bool) {
    if on {
        if STATE.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            ENABLED.store(true, Ordering::Relaxed);
        } else {
            let settings = crate::store::load_settings();
            start(&settings, 0);
        }
        return;
    }
    with(|journal| journal.flush());
    ENABLED.store(false, Ordering::Relaxed);
}

pub fn flush_due() {
    with(|journal| journal.flush_if_due());
}

pub fn mem_read(own: u64, total: u64, cpu: u64) {
    with(|journal| journal.mem = (own, total, cpu));
}

pub fn sample(parts: &Sample) {
    with(|journal| {
        let now = Instant::now();
        journal.drain_slow();
        let wakes = WAKES.swap(0, Ordering::Relaxed);
        let cpu = journal.cpu_percent(now);
        journal.write(
            "sample",
            &[
                ("lumen", json!(mb(journal.mem.0))),
                ("total", json!(mb(journal.mem.1))),
                ("cpu", json!(round2(cpu))),
                ("tabs", json!(parts.tabs)),
                ("live", json!(parts.live)),
                ("asleep", json!(parts.asleep)),
                ("discarded", json!(parts.discarded)),
                ("ws", json!(parts.workspaces)),
                ("panel", s(parts.panel)),
            ],
        );
        journal.write("wake", &[("n", json!(wakes))]);
    });
}

pub fn wake() {
    if ENABLED.load(Ordering::Relaxed) {
        WAKES.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn first_frame(ms: f64) {
    with(|journal| journal.write("ui_first", &[("ms", json!((ms * 10.0).round() / 10.0))]));
}

pub fn frame(ms: f64) {
    if ms < SLOW_FRAME_MS || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    SLOW_FRAMES.fetch_add(1, Ordering::Relaxed);
    let tenths = (ms * 10.0).min(u32::MAX as f64) as u32;
    let mut seen = SLOW_WORST.load(Ordering::Relaxed);
    while tenths > seen {
        match SLOW_WORST.compare_exchange_weak(seen, tenths, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(current) => seen = current,
        }
    }
}

pub fn nav_started(tab: u64, url: &str, agent: bool, private: bool, blocked: Option<u64>) {
    with(|journal| journal.nav_started(tab, url, agent, private, blocked));
}

pub fn nav_done(tab: u64, ok: bool, err: i32, http: i32, blocked: Option<u64>) {
    with(|journal| journal.nav_done(tab, ok, err, http, blocked));
}

pub fn crash(tab: u64, kind: &'static str, host: &str, private: bool) {
    with(|journal| journal.crash(tab, kind, host, private));
}

pub fn error(feature: &str, msg: &str) {
    let msg = scrub(msg);
    with(|journal| journal.write("error", &[("f", s(feature)), ("m", s(&msg))]));
}

pub fn panic(msg: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let msg = scrub(msg);
    match STATE.try_lock() {
        Ok(mut guard) => {
            if let Some(journal) = guard.as_mut() {
                journal.write("panic", &[("m", s(&msg))]);
                journal.flush();
            }
        }
        Err(_) => {
            let dir = crate::store::data_dir().join("journal");
            if let Some(mut file) = open_day(&dir, &local_date(now_secs())) {
                let _ = file.write_all(format!("{}\n", line("panic", now_ms(), &[("m", s(&msg))])).as_bytes());
            }
        }
    }
}

pub fn low_memory(unloaded: usize) {
    with(|journal| journal.write("low_memory", &[("n", Value::from(unloaded as u64))]));
}

pub fn feature(name: &str, private: bool) {
    with(|journal| journal.feature(name, private));
}

pub fn history_search(query: &str) {
    with(|journal| journal.history_search(query));
}

pub fn download(id: u64, kind: &str, state: &str, bytes: i64, private: bool) {
    with(|journal| journal.download(id, kind, state, bytes, private));
}

pub fn agent_session(session: &str, name: &str) {
    with(|journal| journal.agent_session(session, name));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-journal-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn written(dir: &Path) -> Vec<Value> {
        let text = fs::read_to_string(day_path(dir, &local_date(now_secs()))).unwrap_or_default();
        text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    fn kinds(dir: &Path) -> Vec<String> {
        written(dir).iter().map(|v| v["k"].as_str().unwrap_or("").to_string()).collect()
    }

    #[test]
    fn a_line_is_time_kind_and_fields() {
        let text = line("nav", 1_700_000_000_000, &[("host", s("en.wikipedia.org")), ("ms", json!(812)), ("ok", json!(true))]);
        assert_eq!(text, r#"{"t":1700000000000,"k":"nav","host":"en.wikipedia.org","ms":812,"ok":true}"#);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["t"], 1_700_000_000_000u64);
        assert_eq!(v["k"], "nav");
        let v: Value = serde_json::from_str(&line("error", 1, &[("m", s("said \"no\"\\n"))])).unwrap();
        assert_eq!(v["m"], "said \"no\"\\n");
        let dir = tmp("one");
        let mut journal = Journal::open(&dir);
        journal.write("nav", &[("host", s("example.com"))]);
        journal.flush();
        let raw = fs::read_to_string(day_path(&dir, &local_date(now_secs()))).unwrap();
        assert!(raw.ends_with("}\n"));
        assert_eq!(raw.lines().count(), 1);
    }

    #[test]
    fn hosts_are_hosts_and_nothing_else() {
        assert_eq!(host_only("https://en.wikipedia.org/wiki/Lighthouse?x=1#top"), "en.wikipedia.org");
        assert_eq!(host_only("http://User:Pw@Example.COM:8443/a/b?c=d"), "example.com");
        assert_eq!(host_only("https://[::1]:8765/agent.html"), "::1");
        assert_eq!(host_only("http://127.0.0.1:8765/x"), "127.0.0.1");
        assert_eq!(host_only("about:blank"), "");
        assert_eq!(host_only("file:///C:/x.html"), "");
        assert_eq!(host_only("data:text/html,hi"), "");
        assert_eq!(host_only("ftp://example.com/x"), "");
        assert_eq!(host_only(""), "");
    }

    #[test]
    fn a_nav_line_keeps_no_path_and_no_query() {
        let dir = tmp("nav");
        let mut journal = Journal::open(&dir);
        journal.nav_started(1, "https://secret.example/login?token=abc#frag", false, false, None);
        journal.nav_done(1, true, 0, 200, None);
        journal.flush();
        let raw = fs::read_to_string(day_path(&dir, &local_date(now_secs()))).unwrap();
        assert!(raw.contains("\"host\":\"secret.example\""), "{raw}");
        assert!(!raw.contains("token"), "{raw}");
        assert!(!raw.contains("/login"), "{raw}");
        assert!(!raw.contains("frag"), "{raw}");
        journal.nav_started(2, "https://example.com", false, false, Some(10));
        journal.nav_done(2, false, 6, 0, Some(13));
        journal.flush();
        let last = written(&dir).pop().unwrap();
        assert_eq!(last["k"], "nav");
        assert_eq!(last["host"], "example.com");
        assert_eq!(last["ok"], false);
        assert_eq!(last["err"], 6);
        assert_eq!(last["blocked"], 3);
        assert!(last["http"].is_null(), "no status came back, so none is written");
        journal.nav.insert(3, PendingNav { at: Instant::now() - Duration::from_secs(9), host: "slow.example".into(), agent: true, blocked: None });
        journal.nav_done(3, true, 0, 200, None);
        journal.flush();
        let last = written(&dir).pop().unwrap();
        assert_eq!(last["k"], "slow_nav");
        assert_eq!(last["agent"], true);
    }

    #[test]
    fn a_private_tab_writes_nothing() {
        let dir = tmp("private");
        let mut journal = Journal::open(&dir);
        journal.write("start", &[]);
        journal.nav_started(9, "https://secret.example/login?token=abc", false, true, Some(0));
        journal.nav_done(9, true, 0, 200, Some(3));
        journal.crash(9, "renderer", "secret.example", true);
        journal.feature("reader", true);
        journal.download(3, "file", "done", 10, true);
        journal.flush();
        assert_eq!(kinds(&dir), vec!["start"], "nothing else: {:?}", written(&dir));
        journal.crash(4, "renderer", "example.com", false);
        journal.feature("reader", false);
        journal.flush();
        assert!(kinds(&dir).contains(&"feature".to_string()));
    }

    #[test]
    fn the_day_rotates_and_old_files_go() {
        let dir = tmp("days");
        let today = local_date(now_secs());
        let days_ago = |n: u64| local_date(now_secs() - n * 86_400);
        for day in [days_ago(1), days_ago(15), days_ago(30)] {
            fs::write(day_path(&dir, &day), "{\"t\":1,\"k\":\"start\"}\n").unwrap();
        }
        clean_old(&dir, &today);
        assert!(day_path(&dir, &days_ago(1)).is_file(), "yesterday is kept");
        assert!(!day_path(&dir, &days_ago(15)).exists(), "15 days old is out");
        assert!(!day_path(&dir, &days_ago(30)).exists(), "30 days old is out");
        let odd = dir.join("notes.txt");
        fs::write(&odd, "hi").unwrap();
        clean_old(&dir, &today);
        assert!(odd.is_file());

        let other = "1999-01-01";
        let mut journal = Journal::open_at(&dir, other);
        journal.write("nav", &[("host", s("example.com"))]);
        journal.flush();
        assert_eq!(day_size(&dir, other), 0, "yesterday's file is not written into");
        assert_eq!(kinds(&dir).len(), 1, "today's file took the line");
        assert_eq!(journal.day, today);
    }

    #[test]
    fn a_day_stops_at_the_cap() {
        let dir = tmp("cap");
        let mut journal = Journal::open(&dir);
        journal.cap = 120;
        for _ in 0..20 {
            journal.write("nav", &[("host", s("example.com")), ("ms", json!(5))]);
        }
        journal.flush();
        let day = local_date(now_secs());
        let at_cap = day_size(&dir, &day);
        assert!(at_cap <= 120, "the cap held: {at_cap}");
        assert!(journal.full, "and the day knows it is full");
        let kept = written(&dir).len();
        assert!(kept > 0 && kept < 20, "{kept} lines of 20 made it in");

        for _ in 0..5 {
            journal.write("nav", &[("host", s("example.com"))]);
        }
        journal.flush();
        assert_eq!(day_size(&dir, &day), at_cap, "a full day does not grow");
        assert!(journal.buf.is_empty());

        journal.rotate(&local_date(now_secs() + 86_400));
        assert!(!journal.full);
        assert_eq!(journal.day, local_date(now_secs() + 86_400));
        assert_eq!(day_size(&dir, &journal.day), 0);
    }

    #[test]
    fn the_buffer_never_grows_past_its_cap() {
        let dir = tmp("buf");
        let mut journal = Journal::open(&dir);
        journal.buf_max = 200;
        journal.write("sample", &[("lumen", json!(80.0))]);
        journal.write("nav", &[("host", s("one.example"))]);
        journal.write("nav", &[("host", s("two.example"))]);
        journal.write("nav", &[("host", s("three.example"))]);
        assert!(journal.buf.len() <= 200, "buffer is {} bytes", journal.buf.len());
        assert!(!journal.buf.contains("\"k\":\"sample\""));
        assert!(journal.buf.contains("one.example"), "the oldest nav is not what went first");
        assert!(journal.buf.contains("three.example"));
        for raw in journal.buf.lines() {
            let v: Value = serde_json::from_str(raw).expect("whole lines only");
            assert!(v["k"].is_string());
        }

        let mut journal = Journal::open(&dir);
        journal.buf_max = 200;
        for n in 0..12 {
            journal.write("crash", &[("kind", s("renderer")), ("host", s(&format!("h{n}.example")))]);
        }
        assert!(journal.buf.len() <= 200, "buffer is {} bytes", journal.buf.len());
        assert!(journal.buf.contains("h11.example"), "the newest line is kept");
        assert!(!journal.buf.contains("h0.example"), "the oldest went");
    }

    #[test]
    fn an_unclean_end_is_seen_at_the_next_start() {
        let dir = tmp("unclean");
        let clean = dir.join("2026-01-01.jsonl");
        fs::write(&clean, "{\"t\":1,\"k\":\"start\"}\n{\"t\":2,\"k\":\"nav\",\"host\":\"x\"}\n{\"t\":3,\"k\":\"exit\",\"clean\":true}\n").unwrap();
        assert_eq!(ended_uncleanly(&clean), None);
        let crashed = dir.join("2026-01-02.jsonl");
        fs::write(&crashed, "{\"t\":4,\"k\":\"start\"}\n{\"t\":5,\"k\":\"garbage\n").unwrap();
        assert_eq!(ended_uncleanly(&crashed), Some(4));
        let empty = dir.join("2025-12-31.jsonl");
        fs::write(&empty, "").unwrap();
        assert_eq!(ended_uncleanly(&empty), None);
        let today = local_date(now_secs());
        assert_eq!(last_run_file(&dir, &today), Some(crashed));
        let mine = day_path(&dir, &today);
        assert!(ended_uncleanly(&mine).is_none(), "and nothing is there yet");
        fs::write(&mine, "{\"t\":9,\"k\":\"start\"}\n").unwrap();
        assert_eq!(last_run_file(&dir, &today), Some(mine.clone()));
        assert_eq!(ended_uncleanly(&mine), Some(9));
        fs::write(&mine, "{\"t\":9,\"k\":\"start\"}\n{\"t\":10,\"k\":\"exit\",\"clean\":true,\"uptime\":5}\n").unwrap();
        assert_eq!(ended_uncleanly(&mine), None);
        assert_eq!(last_run_file(&dir, &today), Some(mine));
    }

    #[test]
    fn an_error_message_carries_no_url() {
        assert_eq!(
            scrub("yt-dlp gave up: https://www.youtube.com/watch?v=abc123&list=PLxyz and stopped"),
            "yt-dlp gave up: www.youtube.com… and stopped"
        );
        assert!(!scrub("ERROR: Unsupported URL: https://x.com/secret?a=1").contains("secret"));
        assert!(scrub(&"a".repeat(500)).chars().count() <= MAX_MSG + 1);
        assert_eq!(scrub("first\nsecond"), "first second");
        assert_eq!(scrub("plain: nothing here"), "plain: nothing here");
    }

    #[test]
    fn a_crash_says_whether_the_reload_took() {
        let dir = tmp("crash");
        let mut journal = Journal::open(&dir);
        journal.crash(5, "renderer", "example.com", false);
        journal.flush();
        let first = written(&dir).pop().unwrap();
        assert_eq!(first["k"], "crash");
        assert_eq!(first["kind"], "renderer");
        assert_eq!(first["host"], "example.com");
        assert!(first["reload"].is_null());

        journal.nav_started(5, "https://example.com/", false, false, None);
        journal.nav_done(5, true, 0, 200, None);
        journal.flush();
        let lines = written(&dir);
        let last = lines.last().unwrap();
        assert_eq!(last["k"], "crash");
        assert_eq!(last["reload"], "ok");

        journal.crash(6, "browser", "example.com", false);
        journal.nav_started(6, "https://example.com/", false, false, None);
        journal.nav_done(6, false, 6, 0, None);
        journal.flush();
        assert_eq!(written(&dir).last().unwrap()["reload"], "failed");
    }

    #[test]
    fn downloads_end_with_their_average() {
        let dir = tmp("dl");
        let mut journal = Journal::open(&dir);
        journal.download(7, "file", "progress", 0, false);
        journal.download(7, "file", "done", 1024 * 1024, false);
        journal.flush();
        let last = written(&dir).pop().unwrap();
        assert_eq!(last["f"], "download");
        assert_eq!(last["kind"], "file");
        assert_eq!(last["result"], "done");

        journal.download(8, "torrent", "progress", 10, false);
        journal.download(8, "torrent", "failed", 10, false);
        journal.download(9, "file", "progress", 10, false);
        journal.flush();
        let lines = written(&dir);
        assert_eq!(lines.last().unwrap()["result"], "failed");
        assert_eq!(lines.iter().filter(|l| l["k"] == "feature").count(), 2);
    }

    #[test]
    fn one_line_per_agent_session_and_per_query() {
        let dir = tmp("session");
        let mut journal = Journal::open(&dir);
        journal.agent_session("s1", "Claude");
        journal.agent_session("s1", "Claude");
        journal.agent_session("s2", "");
        journal.history_search("ru");
        journal.history_search("rus");
        journal.history_search("rus");
        journal.history_search("rust");
        journal.flush();
        let lines = written(&dir);
        let count = |name: &str| lines.iter().filter(|l| l["f"] == name).count();
        assert_eq!(count("agent_session"), 2);
        assert_eq!(count("history_search"), 2, "typing the same query is one line: {lines:?}");
    }
}
