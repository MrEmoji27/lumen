//! What Lumen blocked, and what that saved.
//!
//! The count is made where the block happens: a rule that fired says so in the
//! moment, with the page it fired for and the list the rule came from. Nothing
//! is replayed from logs afterwards.
//!
//! One file: `privacy_stats.json` in the data folder, written atomically, at
//! most every 60 seconds and never per request, a few KB a day, 365 days kept.
//! Counts are kept per day and per site, so any range can be added up without
//! touching a page or the network.
//!
//! What a tab leaves behind:
//! - a private tab counts in the day's totals and nowhere else: no site, no
//!   domain, nothing that ties a count to where the person went;
//! - an agent tab counts in a bucket of its own, never mixed into the person's;
//! - the per-page chip is memory only, and it resets when the page navigates.
//!
//! Bytes and time are estimates, and they say so. A blocked request has no
//! size, so it is given the running average of the same kind of request that
//! was let through; the time is those bytes over the measured download speed,
//! plus a fixed cost per request that was never paid.

use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Days kept. Older days go the next time the file is opened.
const KEEP_DAYS: i64 = 365;
/// Sites remembered for one day. Past it the quietest site of that day goes.
const MAX_SITES: usize = 30;
/// Blocked domains remembered at all, all time. Past it the quietest goes.
const MAX_DOMAINS: usize = 500;
/// What every blocked request is worth in time before anything is measured:
/// the connection setup and the round trip that never happened.
const PER_REQUEST_MS: u64 = 5;
/// How long counts may sit before they reach the disk. Never sooner.
const FLUSH_AFTER: Duration = Duration::from_secs(60);
/// The file in the data folder.
const FILE: &str = "privacy_stats.json";
/// The format's version. A file written by another version is not read.
const VERSION: u32 = 1;

/// The request kinds the averages are kept for. Anything else is `other`.
const KINDS: [&str; 6] = ["script", "image", "xhr", "media", "font", "other"];
/// Bytes per request of each kind, until real responses have measured them.
const SEED_AVG: [f64; 6] = [80_000.0, 120_000.0, 30_000.0, 1_500_000.0, 40_000.0, 40_000.0];
/// Bytes per second before a page has measured one: 4 MB/s, a plain connection.
const SEED_SPEED: f64 = 4.0 * 1024.0 * 1024.0;
/// How much a new sample moves the running averages.
const EMA: f64 = 0.1;
/// Smallest sample that says anything about a download speed.
const MIN_SPEED_BYTES: u64 = 64 * 1024;
const MIN_SPEED_SECS: f64 = 0.2;

// ------------------------------------------------------------------ the counts

/// Which list stopped a request. `Other` is a list with nothing to say about
/// the split, or a rule the engine fused and could no longer attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Ads,
    Trackers,
    Other,
}

/// What tab a count came from. `tab` is for the per-page chip only.
#[derive(Clone, Copy, Debug)]
pub struct Who {
    pub tab: u64,
    pub private: bool,
    pub agent: bool,
}

impl Who {
    pub fn new(tab: u64, private: bool, agent: bool) -> Self {
        Who { tab, private, agent }
    }
}

/// One line of counts: a day, a site, or the agent bucket. Zero fields are
/// left out of the file, so a quiet day is a short line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counts {
    #[serde(skip_serializing_if = "is_zero")]
    pub ads: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub trackers: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub other: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cosmetic: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub cookies: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub removeparam: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub scriptlets: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub sponsor: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub popups: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub webrtc: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub signals: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub wrong_password: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub malicious: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub danger: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub https_up: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub bytes: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub ms: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

impl Counts {
    /// Requests the door stopped. The headline number.
    pub fn blocked(&self) -> u64 {
        self.ads + self.trackers + self.other
    }

    fn add(&mut self, o: &Counts) {
        self.ads += o.ads;
        self.trackers += o.trackers;
        self.other += o.other;
        self.cosmetic += o.cosmetic;
        self.cookies += o.cookies;
        self.removeparam += o.removeparam;
        self.scriptlets += o.scriptlets;
        self.sponsor += o.sponsor;
        self.popups += o.popups;
        self.webrtc += o.webrtc;
        self.signals += o.signals;
        self.wrong_password += o.wrong_password;
        self.malicious += o.malicious;
        self.danger += o.danger;
        self.https_up += o.https_up;
        self.bytes += o.bytes;
        self.ms += o.ms;
    }
}

/// One bar of the chart: a local day and what happened on it.
#[derive(Clone, Debug)]
pub struct Bar {
    /// "YYYY-MM-DD".
    pub day: String,
    pub blocked: u64,
    #[allow(dead_code)]
    pub bytes: u64,
}

/// One site of the top list, or the whole of one site's detail view.
#[derive(Clone, Debug)]
pub struct SiteRow {
    pub host: String,
    pub counts: Counts,
    /// Share of the range's blocked requests, 0..1.
    pub share: f32,
    /// The last day this site was seen in the range. "YYYY-MM-DD".
    pub last: String,
}

/// Everything the report page draws, computed in one pass over the days.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    #[allow(dead_code)]
    pub today: Counts,
    #[allow(dead_code)]
    pub week: Counts,
    #[allow(dead_code)]
    pub month: Counts,
    pub all: Counts,
    /// Agent tabs, kept apart from all of the above.
    pub agent: Counts,
    /// The range the lists and the chart are for: "today" | "7" | "30" | "all".
    pub range: String,
    /// The same as `today`/`week`/`month`/`all`, whichever `range` picked.
    pub totals: Counts,
    /// The range immediately before the chosen one, for trends. Zero for "all".
    pub prev: Counts,
    /// Oldest first, gaps included, so the bars line up with the dates.
    pub bars: Vec<Bar>,
    /// Top sites of the range, busiest first.
    pub sites: Vec<SiteRow>,
    /// Top blocked domains of all time, busiest first. The lists say which
    /// company a rule belongs to, not which day it fired on.
    pub domains: Vec<(String, u64)>,
    /// The site whose row was clicked, with everything counted for it.
    pub site: Option<SiteRow>,
}

// ------------------------------------------------------------------- the file

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Day {
    counts: Counts,
    sites: BTreeMap<String, Counts>,
}

impl Default for Day {
    fn default() -> Self {
        Day { counts: Counts::default(), sites: BTreeMap::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct File {
    v: u32,
    /// Local day -> what happened that day.
    days: BTreeMap<String, Day>,
    /// Blocked domains, all time.
    domains: BTreeMap<String, u64>,
    /// Agent tabs, all time, never mixed into `days`.
    agent: Counts,
    /// Running average bytes per allowed request, by kind.
    avg: [f64; KINDS.len()],
    /// Running average bytes per second of a page load. 0 until one is measured.
    speed: f64,
}

impl Default for File {
    fn default() -> Self {
        File { v: VERSION, days: BTreeMap::new(), domains: BTreeMap::new(), agent: Counts::default(), avg: SEED_AVG, speed: 0.0 }
    }
}

/// The state behind one lock. Nothing else is shared.
struct Stats {
    file: File,
    dir: PathBuf,
    dirty: bool,
    last_save: Instant,
    /// Blocked requests since each tab's navigation started: the sidebar chip.
    tab: BTreeMap<u64, u64>,
}

impl Stats {
    /// Read the file back, or start empty. A file that will not parse or was
    /// written by another version is dropped whole: half a year of counts is
    /// not worth a guess.
    fn open(dir: PathBuf) -> Stats {
        let mut file = fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<File>(&text).ok())
            .filter(|f| f.v == VERSION)
            .unwrap_or_default();
        prune(&mut file);
        Stats { file, dir, dirty: false, last_save: Instant::now(), tab: BTreeMap::new() }
    }

    /// Counts to disk, whole file, rename over the old one. A crash between
    /// the write and the rename leaves the previous file intact.
    fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let Ok(text) = serde_json::to_string(&self.file) else {
            return;
        };
        let tmp = self.dir.join(format!("{FILE}.tmp"));
        if fs::write(&tmp, text).is_err() {
            return;
        }
        if fs::rename(&tmp, self.dir.join(FILE)).is_ok() {
            self.dirty = false;
            self.last_save = Instant::now();
        }
    }

    fn flush_if_due(&mut self) {
        if self.dirty && self.last_save.elapsed() >= FLUSH_AFTER {
            self.save();
        }
    }

    /// One count against `site` on `day`. The day always gets it; the site
    /// only when the tab may leave a trail. `chip` also counts it against the
    /// page it happened on.
    fn put(&mut self, day: &str, site: &str, who: Who, chip: bool, edit: impl Fn(&mut Counts)) {
        self.dirty = true;
        if chip {
            *self.tab.entry(who.tab).or_insert(0) += 1;
            if self.tab.len() > 4096 {
                self.tab.clear();
            }
        }
        if who.agent {
            edit(&mut self.file.agent);
            return;
        }
        let d = self.file.days.entry(day.to_string()).or_default();
        edit(&mut d.counts);
        if !who.private && !site.is_empty() {
            edit(&mut d.sites.entry(site.to_string()).or_default());
            evict(&mut d.sites, MAX_SITES, |c| c.blocked());
        }
    }

    /// A request the door stopped: one of its kind's averages, plus the fixed
    /// cost of a request that never went out.
    fn block(&mut self, day: &str, site: &str, domain: &str, cat: Cat, kind: &str, who: Who) {
        let (bytes, ms) = self.estimate(kind);
        let domain = if who.private || who.agent { "" } else { domain };
        self.put(day, site, who, true, |c| {
            match cat {
                Cat::Ads => c.ads += 1,
                Cat::Trackers => c.trackers += 1,
                Cat::Other => c.other += 1,
            }
            c.bytes += bytes;
            c.ms += ms;
        });
        if !domain.is_empty() {
            *self.file.domains.entry(domain.to_string()).or_default() += 1;
            evict(&mut self.file.domains, MAX_DOMAINS, |n| *n);
            self.dirty = true;
        }
    }

    /// A subresource from the malicious lists: blocked at the door. The domain
    /// joins the all-time list like any other blocked name.
    fn malicious(&mut self, day: &str, site: &str, domain: &str, who: Who) {
        let domain = if who.private || who.agent { "" } else { domain };
        self.put(day, site, who, true, |c| c.malicious += 1);
        if !domain.is_empty() {
            *self.file.domains.entry(domain.to_string()).or_default() += 1;
            evict(&mut self.file.domains, MAX_DOMAINS, |n| *n);
            self.dirty = true;
        }
    }

    fn estimate(&self, kind: &str) -> (u64, u64) {
        let speed = if self.file.speed > 0.0 { self.file.speed } else { SEED_SPEED };
        let bytes = self.file.avg[kind_index(kind)].round().max(1.0) as u64;
        let ms = (bytes as f64 / speed * 1000.0).round() as u64 + PER_REQUEST_MS;
        (bytes, ms)
    }

    /// A request that went through, with its size: this is where the averages
    /// come from. No size on the wire (chunked, or a 204) teaches nothing.
    fn note(&mut self, kind: &str, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let i = kind_index(kind);
        let avg = &mut self.file.avg[i];
        *avg = if *avg <= 0.0 { bytes as f64 } else { *avg * (1.0 - EMA) + bytes as f64 * EMA };
        self.dirty = true;
    }

    /// One page load: what it moved and how long it took. Too small or too
    /// quick to say anything is dropped rather than guessed at.
    fn nav_sample(&mut self, bytes: u64, secs: f64) {
        if bytes < MIN_SPEED_BYTES || secs < MIN_SPEED_SECS {
            return;
        }
        let now = (bytes as f64 / secs).clamp(100_000.0, 100_000_000.0);
        let before = if self.file.speed > 0.0 { self.file.speed } else { now };
        self.file.speed = before * 0.7 + now * 0.3;
        self.dirty = true;
    }

    /// The whole report, in one pass over the days of the range.
    fn snapshot(&self, range: &str, site: Option<&str>) -> Snapshot {
        let today = local_date(now_secs());
        let today_n = day_of(&today).unwrap_or(0);
        let in_range = |d: i64| match range {
            "today" => d == today_n,
            "7" => d > today_n - 7,
            "30" => d > today_n - 30,
            _ => true,
        };
        let mut totals = Counts::default();
        let mut sites: BTreeMap<String, (Counts, String)> = BTreeMap::new();
        let mut detail: Option<Counts> = None;
        let mut last = String::new();
        for (day, d) in &self.file.days {
            let Some(n) = day_of(day) else { continue };
            if !in_range(n) {
                continue;
            }
            totals.add(&d.counts);
            for (host, c) in &d.sites {
                let e = sites.entry(host.clone()).or_insert_with(|| (Counts::default(), String::new()));
                e.0.add(c);
                if day > &e.1 {
                    e.1 = day.clone();
                }
                if site == Some(host.as_str()) {
                    detail.get_or_insert_with(Counts::default).add(c);
                    if day > &last {
                        last = day.clone();
                    }
                }
            }
        }
        let mut rows: Vec<SiteRow> = sites
            .into_iter()
            .map(|(host, (counts, last))| SiteRow { share: 0.0, host, counts, last })
            .collect();
        rows.sort_by(|a, b| b.counts.blocked().cmp(&a.counts.blocked()).then_with(|| a.host.cmp(&b.host)));
        let blocked_total = totals.blocked().max(1);
        for r in rows.iter_mut().take(10) {
            r.share = r.counts.blocked() as f32 / blocked_total as f32;
        }
        rows.truncate(10);
        let mut domains: Vec<(String, u64)> = self.file.domains.iter().map(|(k, v)| (k.clone(), *v)).collect();
        domains.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        domains.truncate(10);
        let site = match (site, detail) {
            (Some(host), Some(counts)) => Some(SiteRow {
                share: counts.blocked() as f32 / blocked_total as f32,
                host: host.to_string(),
                counts,
                last,
            }),
            _ => None,
        };
        // The period immediately before the chosen one, for trends. "all" has
        // no previous period and stays zero.
        let prev = match range {
            "today" => self.sum(|d| d == today_n - 1),
            "7" => self.sum(|d| d > today_n - 14 && d <= today_n - 7),
            "30" => self.sum(|d| d > today_n - 60 && d <= today_n - 30),
            _ => Counts::default(),
        };
        Snapshot { today: self.sum(|d| d == today_n), week: self.sum(|d| d > today_n - 7), month: self.sum(|d| d > today_n - 30), all: self.sum(|_| true), agent: self.file.agent, range: range.to_string(), totals, prev, bars: bars(&self.file, today_n, range), sites: rows, domains, site }
    }

    /// Every day the predicate takes, added up.
    fn sum(&self, take: impl Fn(i64) -> bool) -> Counts {
        let mut out = Counts::default();
        for (day, d) in &self.file.days {
            if day_of(day).is_some_and(|n| take(n)) {
                out.add(&d.counts);
            }
        }
        out
    }
}

/// The chart: one bar per day, ending today, gaps included. At least a week,
/// at most a month, so one bar is never a chart and 366 is never noise.
fn bars(file: &File, today_n: i64, range: &str) -> Vec<Bar> {
    let span = match range {
        "today" => 1,
        "7" => 7,
        "30" => 30,
        _ => 366,
    };
    let n = span.clamp(7, 30);
    let mut out = Vec::with_capacity(n as usize);
    for i in (0..n).rev() {
        let key = day_name(today_n - i);
        let (blocked, bytes) = match file.days.get(&key) {
            Some(d) => (d.counts.blocked(), d.counts.bytes),
            None => (0, 0),
        };
        out.push(Bar { day: key, blocked, bytes });
    }
    out
}

/// The smallest counts leave first, whatever the values are.
fn evict<V>(map: &mut BTreeMap<String, V>, cap: usize, key: impl Fn(&V) -> u64) {
    while map.len() > cap {
        let Some(low) = map.iter().min_by_key(|(_, v)| key(v)).map(|(k, _)| k.clone()) else { break };
        map.remove(&low);
    }
}

/// Days older than KEEP_DAYS go. Called when the file is opened, so a year of
/// counting stops at a year.
fn prune(file: &mut File) {
    let today = day_of(&local_date(now_secs())).unwrap_or(0);
    file.days.retain(|day, _| day_of(day).is_some_and(|n| today - n < KEEP_DAYS));
}

// --------------------------------------------------------------- the helpers

fn kind_index(kind: &str) -> usize {
    match kind {
        "script" => 0,
        "image" => 1,
        "xmlhttprequest" | "fetch" | "xhr" => 2,
        "media" => 3,
        "font" => 4,
        _ => 5,
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The local calendar day of a unix timestamp, "YYYY-MM-DD". Windows' own
/// timezone conversion, so the day changes when the clock does, DST included.
/// The same conversion the journal makes.
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

/// Days since 1970-01-01 for a civil date. Howard Hinnant's algorithm.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe as i64 * 365 + (yoe / 4) as i64 - (yoe / 100) as i64 + doy as i64;
    era as i64 * 146_097 + doe - 719_468
}

/// The civil date for days since the epoch. The other half of the same
/// algorithm, so the chart can name a day that has no data.
fn day_name(z: i64) -> String {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", y + if m <= 2 { 1 } else { 0 }, m, d)
}

/// "2026-10-15" -> days since the epoch. Anything else is None.
fn day_of(name: &str) -> Option<i64> {
    let (y, rest) = name.split_once('-')?;
    let (m, d) = rest.split_once('-')?;
    let (y, m, d): (i32, u32, u32) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// The host of a URL, and only the host.
pub fn host(url: &str) -> String {
    crate::journal::host_only(url)
}

/// The part of a host that names the thing that runs it: `log.a.doubleclick.net`
/// is `doubleclick.net`. Two labels, unless the ending is a country's second
/// level, and never an address or a name with nothing under it.
pub fn registrable(host: &str) -> String {
    const SECOND_LEVEL: &[&str] = &[
        "co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au", "co.jp", "ne.jp", "or.jp", "com.br", "com.cn", "com.tw", "co.in", "co.nz", "co.za", "com.mx", "com.ar", "com.tr", "com.sg", "com.hk", "com.my", "com.cn",
    ];
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.parse::<std::net::Ipv4Addr>().is_ok() {
        return host;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return host;
    }
    let tail2 = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
    if SECOND_LEVEL.contains(&tail2.as_str()) && labels.len() >= 3 {
        return format!("{}.{}", labels[labels.len() - 3], tail2);
    }
    tail2
}

// ------------------------------------------------------------- the state

static STATE: Mutex<Option<Stats>> = Mutex::new(None);
/// Whether counting is on at all. Checked before any lock is taken, so a call
/// on a path that is not counting costs one relaxed load.
static ENABLED: AtomicBool = AtomicBool::new(false);

fn with(work: impl FnOnce(&mut Stats)) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(stats) = guard.as_mut() {
        work(stats);
    }
}

/// Counting on or off. Off saves what is pending and stops every call; on
/// opens the file if it was never read.
pub fn set_enabled(on: bool) {
    if on {
        let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(Stats::open(crate::store::data_dir()));
        }
        ENABLED.store(true, Ordering::Relaxed);
        return;
    }
    with(|stats| stats.save());
    ENABLED.store(false, Ordering::Relaxed);
}

/// Counts to disk, at most one write per FLUSH_AFTER, asked for from work the
/// loop was already doing. Nothing pending: nothing happens.
pub fn flush_if_due() {
    with(|stats| stats.flush_if_due());
}

/// Counts to disk now: the way out, and the last chance before the file's
/// owner leaves.
pub fn flush() {
    with(|stats| stats.save());
}

/// A request the door stopped.
pub fn blocked(site: &str, domain: &str, cat: Cat, kind: &str, who: Who) {
    let day = local_date(now_secs());
    with(|s| s.block(&day, site, domain, cat, kind, who));
}

/// Elements the cosmetic rules hid on a page.
pub fn hides(site: &str, n: u64, who: Who) {
    if n == 0 {
        return;
    }
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, true, |c| c.cosmetic += n));
}

/// Cookie banners hidden. A subset of `hides`, counted again on purpose: the
/// report says both numbers.
pub fn cookie_hides(site: &str, n: u64, who: Who) {
    if n == 0 {
        return;
    }
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, false, |c| c.cookies += n));
}

/// A tracking parameter `$removeparam` took out of a link.
pub fn removeparam(site: &str, who: Who) {
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, false, |c| c.removeparam += 1));
}

/// A page whose ad scriptlets ran. One per navigation: the engine hands the
/// page its whole script in one string, so one is what can be counted.
pub fn scriptlet(site: &str, who: Who) {
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, false, |c| c.scriptlets += 1));
}

/// SponsorBlock segments the page was given to skip.
pub fn sponsor(site: &str, n: u64, who: Who) {
    if n == 0 {
        return;
    }
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, false, |c| c.sponsor += n));
}

/// A popup the page tried to open and Lumen stopped: no user gesture, or a
/// `$popup` rule said so.
pub fn popup(site: &str, who: Who) {
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, true, |c| c.popups += 1));
}

/// An RTCPeerConnection the page made. Lumen forced the public interface only,
/// so the local IP never left; this counts the attempt.
pub fn rtc(site: &str, who: Who) {
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, false, |c| c.webrtc += 1));
}

/// A third-party request that carried Lumen's GPC / DNT headers.
pub fn signal(site: &str, who: Who) {
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, false, |c| c.signals += 1));
}

/// A KeePass unlock with the wrong password. No site: it is the vault itself.
pub fn wrong_password() {
    let day = local_date(now_secs());
    with(|s| s.put(&day, "", Who::new(0, false, false), false, |c| c.wrong_password += 1));
}

/// A top-level navigation to a dangerous site: the warning page showed.
pub fn danger(site: &str, who: Who) {
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, false, |c| c.danger += 1));
}

/// A subresource from the malicious lists: blocked at the door. The domain
/// joins the all-time list like any other blocked name.
pub fn malicious_block(site: &str, domain: &str, who: Who) {
    let day = local_date(now_secs());
    with(|s| s.malicious(&day, site, domain, who));
}

/// A navigation HTTPS-first upgraded.
pub fn https_upgrade(site: &str, who: Who) {
    let day = local_date(now_secs());
    with(|s| s.put(&day, site, who, false, |c| c.https_up += 1));
}

/// A request that went through, with its size: where the averages come from.
pub fn note_response(kind: &str, bytes: u64) {
    with(|s| s.note(kind, bytes));
}

/// One page load: what it moved and how long it took.
pub fn nav_sample(bytes: u64, secs: f64) {
    with(|s| s.nav_sample(bytes, secs));
}

/// A new page in this tab: the chip starts over.
pub fn nav_start(tab: u64) {
    with(|s| {
        s.tab.insert(tab, 0);
        if s.tab.len() > 4096 {
            s.tab.clear();
        }
    });
}

/// What the current page had stopped so far, for the sidebar chip.
pub fn tab_blocked(tab: u64) -> u64 {
    if !ENABLED.load(Ordering::Relaxed) {
        return 0;
    }
    STATE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|s| s.tab.get(&tab)).copied().unwrap_or(0)
}

/// The report page's whole state.
pub fn snapshot(range: &str, site: Option<&str>) -> Snapshot {
    if !ENABLED.load(Ordering::Relaxed) {
        return Snapshot { range: range.to_string(), ..Default::default() };
    }
    STATE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.snapshot(range, site)).unwrap_or_default()
}

/// Today, for the dashboard's privacy tile.
pub fn today() -> Counts {
    snapshot("today", None).today
}

/// Everything counted, gone. The averages and the measured speed stay: they
/// are measurements of this machine, not of what was blocked.
pub fn reset() {
    with(|s| {
        s.file.days.clear();
        s.file.domains.clear();
        s.file.agent = Counts::default();
        s.tab.clear();
        s.dirty = true;
        s.save();
    });
}

// -------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-stats-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn day(n: i64) -> String {
        day_name(n)
    }

    #[test]
    fn counts_go_where_the_tab_allows_them() {
        let mut s = Stats::open(tmp("who"));
        let d = day(20_000);
        let me = Who::new(1, false, false);
        let secret = Who::new(2, true, false);
        let agent = Who::new(3, false, true);
        s.block(&d, "news.example", "doubleclick.net", Cat::Ads, "script", me);
        s.block(&d, "secret.example", "evil.example", Cat::Trackers, "image", secret);
        s.block(&d, "agent.example", "ads.example", Cat::Ads, "script", agent);
        s.put(&d, "news.example", me, true, |c| c.cosmetic += 4);
        s.put(&d, "agent.example", agent, false, |c| c.cosmetic += 9);

        let day_file = &s.file.days[&d];
        // Totals take the private tab; the site list does not.
        assert_eq!(day_file.counts.blocked(), 2, "the private tab counts in the totals");
        assert_eq!(day_file.counts.ads, 1);
        assert_eq!(day_file.counts.trackers, 1);
        assert_eq!(day_file.counts.cosmetic, 4, "private cosmetic hides land in the totals too");
        assert!(day_file.sites.contains_key("news.example"));
        assert!(!day_file.sites.contains_key("secret.example"), "a private tab leaves no site");
        assert!(!day_file.sites.contains_key("agent.example"), "an agent tab leaves no site either");
        assert_eq!(s.file.agent.ads, 1, "agent tabs have their own bucket");
        assert_eq!(s.file.agent.cosmetic, 9);
        assert_eq!(s.file.agent.blocked(), 1, "and nothing of theirs leaks into the days");
        // Domains follow the same rule as sites.
        assert_eq!(s.file.domains.get("doubleclick.net"), Some(&1));
        assert!(!s.file.domains.contains_key("evil.example"));
        assert!(!s.file.domains.contains_key("ads.example"));
        // The chip counts every tab's own page, private included: it is memory.
        assert_eq!(s.tab[&1], 2);
        assert_eq!(s.tab[&2], 1);
        assert_eq!(s.tab[&3], 1);
    }

    #[test]
    fn the_day_rolls_over_underneath_the_counts() {
        let mut s = Stats::open(tmp("roll"));
        let before = day(20_000);
        let after = day(20_001);
        s.block(&before, "news.example", "a.example", Cat::Ads, "script", Who::new(1, false, false));
        s.block(&after, "news.example", "a.example", Cat::Ads, "script", Who::new(1, false, false));
        assert_eq!(s.file.days.len(), 2, "a new day is a new line, not a running total");
        assert_eq!(s.file.days[&before].counts.blocked(), 1);
        assert_eq!(s.file.days[&after].counts.blocked(), 1);
        assert_eq!(s.file.days[&after].sites["news.example"].ads, 1);
        // The bars name every day of a range, today last, empty ones included.
        let today = local_date(now_secs());
        let today_n = day_of(&today).unwrap();
        let bars = bars(&s.file, today_n, "today");
        assert_eq!(bars.len(), 7, "one day is not a chart, so a week is drawn");
        assert_eq!(bars.last().unwrap().day, today);
        assert!(bars.iter().all(|b| b.day <= today));
        let day_after = day_name(today_n + 1);
        assert_eq!(day_of(&day_after), Some(today_n + 1));
        assert_eq!(day_name(today_n), today, "the chart's names are the same dates it finds");
    }

    /// The estimate: a blocked request takes the average of its kind, and the
    /// time is those bytes over the speed plus the fixed cost.
    #[test]
    fn the_estimate_is_averages_over_speed_plus_a_fixed_cost() {
        let mut s = Stats::open(tmp("estimate"));
        s.file.avg[kind_index("script")] = 100_000.0;
        s.file.speed = 1_000_000.0;
        let (bytes, ms) = s.estimate("script");
        assert_eq!(bytes, 100_000);
        assert_eq!(ms, 100 + PER_REQUEST_MS, "100 KB over 1 MB/s is 100 ms, plus 5");
        // An unknown kind falls into the last bucket, not to zero.
        assert!(s.estimate("websocket").0 > 0);

        // Allowed requests move the average: 10% towards each new sample.
        s.note("script", 200_000);
        assert_eq!(s.file.avg[kind_index("script")], 100_000.0 * 0.9 + 200_000.0 * 0.1);
        s.note("script", 0);
        assert_eq!(s.file.avg[kind_index("script")], 100_000.0 * 0.9 + 200_000.0 * 0.1, "a response with no size teaches nothing");

        // Speed: one real load moves a second's worth of it, and a token one
        // is dropped rather than guessed at.
        s.nav_sample(4 * 1024 * 1024, 1.0);
        assert!((s.file.speed - (1_000_000.0 * 0.7 + 4_194_304.0 * 0.3)).abs() < 1.0, "moved 30% towards the new sample: {}", s.file.speed);
        let seen = s.file.speed;
        s.nav_sample(1024, 0.01);
        assert_eq!(s.file.speed, seen, "too small and too quick to say anything");
        // Nothing measured yet: the seed is used, and the maths still works.
        let mut fresh = Stats::open(tmp("estimate2"));
        fresh.file.avg[kind_index("image")] = 50_000.0;
        let (bytes, ms) = fresh.estimate("image");
        assert_eq!(bytes, 50_000);
        assert_eq!(ms, (50_000.0 / SEED_SPEED * 1000.0).round() as u64 + PER_REQUEST_MS);
    }

    #[test]
    fn old_days_go_and_the_new_one_stays() {
        let dir = tmp("prune");
        let today = day_of(&local_date(now_secs())).unwrap();
        let mut file = File::default();
        for ago in [0, 1, 364, 365, 400] {
            file.days.insert(day_name(today - ago), Day { counts: Counts { ads: 1, ..Default::default() }, sites: BTreeMap::new() });
        }
        fs::write(dir.join(FILE), serde_json::to_string(&file).unwrap()).unwrap();

        // Judged against today, not against the day they were written.
        prune(&mut file);
        assert!(!file.days.contains_key(&day_name(today - 365)), "365 days back is out");
        assert!(!file.days.contains_key(&day_name(today - 400)));
        assert!(file.days.contains_key(&day_name(today - 364)), "364 days back is still in");
        assert!(file.days.contains_key(&day_name(today)));
        assert_eq!(file.days.len(), 3);

        // Opening the file does the same trim.
        let s = Stats::open(dir);
        assert_eq!(s.file.days.len(), 3, "the old days went on the way in");
    }

    #[test]
    fn the_file_round_trips_and_stays_small() {
        let dir = tmp("round");
        let mut s = Stats::open(dir.clone());
        let site = |n: usize| format!("site{n}.example");
        let dom = |n: usize| format!("track{n}.example");
        let today_n = day_of(&local_date(now_secs())).unwrap();
        // A heavy day: a person with thirty sites open and a hundred trackers.
        let heavy = day_name(today_n);
        for n in 0..30 {
            for k in 0..8 {
                s.block(&heavy, &site(n), &dom(k * 10 + n % 10), Cat::Ads, "script", Who::new(1, false, false));
            }
            s.put(&heavy, &site(n), Who::new(1, false, false), true, |c| c.cosmetic += 5);
        }
        // A year of quieter days around it, every one inside the window.
        for ago in 1..365u64 {
            let d = day_name(today_n - ago as i64);
            for n in 0..12 {
                s.block(&d, &site(n), &dom(n), Cat::Trackers, "image", Who::new(1, false, false));
            }
        }
        s.dirty = true;
        s.save();
        let bytes = fs::metadata(dir.join(FILE)).unwrap().len();
        assert!(bytes < 1_000_000, "a year of counting is {bytes} bytes, wanted under 1 MB");
        let one_day = serde_json::to_string(&s.file.days[&heavy]).unwrap().len();
        assert!(one_day < 6 * 1024, "a heavy day is {one_day} bytes, wanted a few KB");

        // Back in, and every total is where it was.
        let r = Stats::open(dir.clone());
        let want = s.file.days.values().fold(Counts::default(), |mut a, d| {
            a.add(&d.counts);
            a
        });
        let got = r.file.days.values().fold(Counts::default(), |mut a, d| {
            a.add(&d.counts);
            a
        });
        assert_eq!(want, got, "the round trip lost nothing");
        assert_eq!(r.file.days[&heavy].sites.len(), MAX_SITES, "the cap held on the heavy day");
        assert!(s.file.domains.len() <= MAX_DOMAINS);
        // A file that is not this format does not half-load.
        fs::write(dir.join(FILE), "not json at all").unwrap();
        let junk = Stats::open(dir);
        assert!(junk.file.days.is_empty(), "junk starts empty instead of failing");
    }

    #[test]
    fn the_chip_counts_a_page_and_starts_over() {
        let mut s = Stats::open(tmp("chip"));
        s.put(&day(20_000), "news.example", Who::new(7, false, false), true, |c| c.ads += 1);
        s.put(&day(20_000), "news.example", Who::new(7, false, false), true, |c| c.cosmetic += 3);
        s.put(&day(20_000), "elsewhere.example", Who::new(8, false, false), false, |c| c.removeparam += 1);
        assert_eq!(s.tab[&7], 2, "the chip counts blocks on this page");
        assert_eq!(s.tab.get(&8).copied().unwrap_or(0), 0, "a tracking parameter is not an element on the page");
        s.put(&day(20_000), "x.example", Who::new(7, false, false), true, |c| c.ads += 1);
        s.tab.insert(7, 0);
        assert_eq!(s.tab[&7], 0, "the next page starts at zero");
    }

    #[test]
    fn hosts_become_the_company_that_runs_them() {
        assert_eq!(registrable("log.a.doubleclick.net"), "doubleclick.net");
        assert_eq!(registrable("doubleclick.net"), "doubleclick.net");
        assert_eq!(registrable("ads.pubmatic.co.uk"), "pubmatic.co.uk");
        assert_eq!(registrable("sub.example.co.uk"), "example.co.uk", "a second-level ending keeps three labels");
        assert_eq!(registrable("127.0.0.1"), "127.0.0.1");
        assert_eq!(registrable("localhost"), "localhost");
        assert_eq!(registrable(""), "");
        assert_eq!(host("https://en.wikipedia.org/wiki/A?x=1"), "en.wikipedia.org");
    }

    #[test]
    fn a_snapshot_sums_the_ranges_and_picks_the_site() {
        let mut s = Stats::open(tmp("snap"));
        let today = local_date(now_secs());
        let today_n = day_of(&today).unwrap();
        let yday = day_name(today_n - 1);
        let old = day_name(today_n - 10);
        s.block(&today, "news.example", "a.example", Cat::Ads, "script", Who::new(1, false, false));
        s.block(&yday, "news.example", "b.example", Cat::Trackers, "image", Who::new(1, false, false));
        s.block(&old, "other.example", "c.example", Cat::Ads, "script", Who::new(1, false, false));
        s.put(&today, "news.example", Who::new(1, false, false), false, |c| c.cosmetic += 7);
        s.file.agent.ads = 3;
        s.file.speed = 1.0;

        let snap = s.snapshot("7", Some("news.example"));
        assert_eq!(snap.today.blocked(), 1);
        assert_eq!(snap.week.blocked(), 2, "yesterday is in the last 7 days, ten days ago is not");
        assert_eq!(snap.month.blocked(), 3);
        assert_eq!(snap.all.blocked(), 3);
        assert_eq!(snap.agent.ads, 3, "the agent bucket rides along, separate");
        assert_eq!(snap.totals.blocked(), 2, "the selected range's own totals");
        assert_eq!(snap.bars.len(), 7);
        assert_eq!(snap.bars.last().unwrap().blocked, 1);
        assert_eq!(snap.sites.len(), 1, "news.example is in the last 7 days, other.example is not");
        assert_eq!(snap.sites[0].host, "news.example");
        assert!((snap.sites[0].share - 1.0).abs() < 0.01, "it is the only site of the range");
        assert_eq!(s.snapshot("all", None).sites.len(), 2, "a month back both are there");
        let detail = snap.site.expect("the clicked site is answered");
        assert_eq!(detail.counts.blocked(), 2, "both of its days added up");
        assert_eq!(detail.counts.cosmetic, 7);
        assert_eq!(detail.last, today, "last seen today");
        // A site that is not there is answered with nothing, not with zeros.
        assert!(s.snapshot("7", Some("gone.example")).site.is_none());
        // Domains are all time, busiest first.
        assert_eq!(snap.domains[0].0, "a.example");
    }

    /// Every new counter lands in the day's totals and, unless the tab is
    /// private or the agent's, on the site.
    #[test]
    fn the_new_counters_go_where_the_tab_allows_them() {
        let mut s = Stats::open(tmp("new"));
        let d = day(20_000);
        let me = Who::new(1, false, false);
        let secret = Who::new(2, true, false);
        let agent = Who::new(3, false, true);
        s.put(&d, "news.example", me, true, |c| c.popups += 1);
        s.put(&d, "news.example", me, false, |c| c.webrtc += 2);
        s.put(&d, "news.example", me, false, |c| c.signals += 3);
        s.put(&d, "news.example", me, false, |c| c.wrong_password += 1);
        s.put(&d, "evil.example", secret, false, |c| c.danger += 1);
        s.put(&d, "agent.example", agent, false, |c| c.https_up += 4);
        s.malicious(&d, "news.example", "bad.example", me);
        s.malicious(&d, "secret.example", "evil.example", secret);
        s.malicious(&d, "agent.example", "bad.example", agent);

        let day_file = &s.file.days[&d];
        assert_eq!(day_file.counts.popups, 1);
        assert_eq!(day_file.counts.webrtc, 2);
        assert_eq!(day_file.counts.signals, 3);
        assert_eq!(day_file.counts.wrong_password, 1);
        assert_eq!(day_file.counts.danger, 1, "a private tab's warning counts in the totals");
        assert_eq!(day_file.counts.malicious, 2, "the private one counts in the totals too");
        // The agent's own https_up and malicious went to its bucket, not the day.
        assert_eq!(s.file.agent.https_up, 4);
        assert_eq!(s.file.agent.malicious, 1);
        assert_eq!(day_file.counts.https_up, 0, "the agent never touched the day's https_up");
        assert_eq!(day_file.counts.malicious, 2, "only the two non-agent ones landed on the day");
        // Sites: private and agent leave no site.
        assert_eq!(day_file.sites["news.example"].popups, 1);
        assert_eq!(day_file.sites["news.example"].malicious, 1);
        assert!(!day_file.sites.contains_key("evil.example"));
        assert!(!day_file.sites.contains_key("agent.example"));
        // Domains follow the same rule as sites: only the person's non-private tab.
        assert_eq!(s.file.domains.get("bad.example"), Some(&1), "only the person's hit is recorded");
        assert!(!s.file.domains.contains_key("evil.example"));
        // The chip: popup and malicious count, the rest do not. It is memory,
        // so the private tab's own page counts in it too.
        assert_eq!(s.tab[&1], 2, "the popup and the malicious block on this page");
        assert_eq!(s.tab[&2], 1, "the private tab's malicious block counts in its own chip");
    }

    /// The previous period: yesterday for "today", the block before for "7"
    /// and "30", nothing for "all".
    #[test]
    fn the_prev_period_is_the_one_immediately_before() {
        let mut s = Stats::open(tmp("prev"));
        let today_n = day_of(&local_date(now_secs())).unwrap();
        let put = |s: &mut Stats, ago: i64| {
            let d = day_name(today_n - ago);
            s.block(&d, "news.example", "a.example", Cat::Ads, "script", Who::new(1, false, false));
        };
        put(&mut s, 0); // today
        put(&mut s, 1); // yesterday: prev of "today"
        put(&mut s, 8); // in the previous 7
        put(&mut s, 9); // also in the previous 7
        put(&mut s, 31); // in the previous 30, not the previous 7
        put(&mut s, 40); // in the previous 30

        let snap = s.snapshot("today", None);
        assert_eq!(snap.totals.blocked(), 1);
        assert_eq!(snap.prev.blocked(), 1, "yesterday is the previous day");

        let snap = s.snapshot("7", None);
        assert_eq!(snap.totals.blocked(), 2, "today and yesterday are both in the last 7");
        assert_eq!(snap.prev.blocked(), 2, "the two days in the block before the 7");

        let snap = s.snapshot("30", None);
        assert_eq!(snap.totals.blocked(), 4, "today, yesterday and the two inside 30");
        assert_eq!(snap.prev.blocked(), 2, "the two in the block before the 30");

        let snap = s.snapshot("all", None);
        assert_eq!(snap.totals.blocked(), 6);
        assert_eq!(snap.prev.blocked(), 0, "all has no previous period");
    }

    /// An older file loads and the new fields read zero.
    #[test]
    fn an_older_file_loads_with_the_new_fields_zero() {
        let dir = tmp("old");
        // As version 1 wrote it before the new counters existed.
        let old = r#"{"v":1,"days":{"2026-10-15":{"counts":{"ads":5,"trackers":3,"bytes":1000,"ms":50},"sites":{"a.example":{"ads":5}}}},"domains":{"x.example":5},"agent":{},"avg":[80000,120000,30000,1500000,40000,40000],"speed":0}"#;
        fs::write(dir.join(FILE), old).unwrap();
        let s = Stats::open(dir);
        let d = &s.file.days["2026-10-15"];
        assert_eq!(d.counts.ads, 5);
        assert_eq!(d.counts.trackers, 3);
        assert_eq!(d.counts.popups, 0, "new fields default to zero");
        assert_eq!(d.counts.webrtc, 0);
        assert_eq!(d.counts.danger, 0);
        assert_eq!(d.counts.malicious, 0);
        assert_eq!(s.file.agent.https_up, 0);
    }
}
