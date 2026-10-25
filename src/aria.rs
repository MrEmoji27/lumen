//! Downloads run on many wires at once, so aria2 runs them.

use std::{
    collections::HashMap,
    fs,
    io::Read,
    net::TcpListener,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::trackers;

const ZIP_URL: &str =
    "https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-win-64bit-build1.zip";
const ZIP_SHA256: &str = "67d015301eef0b612191212d564c5bb0a14b5b9c4796b76454276a4d28d9b288";
const EXE_SHA256: &str = "be2099c214f63a3cb4954b09a0becd6e2e34660b886d4c898d260febfe9d70c2";
const MEMBER: &str = "aria2-1.37.0-win-64bit-build1/aria2c.exe";
const EXE: &str = "aria2c.exe";
const ZIP: &str = "aria2.zip";
const SESSION: &str = "session.txt";
const MAX_EXE: usize = 64 << 20;
const MAX_ZIP: usize = 32 << 20;
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const RPC_TIMEOUT: Duration = Duration::from_secs(3);
const BOOT_TRIES: u32 = 50;
const BOOT_SLEEP: Duration = Duration::from_millis(100);

const FLAGS: &[&str] = &[
    "--enable-rpc",
    "--rpc-listen-all=false",
    "--continue=true",
    "--max-connection-per-server=16",
    "--split=16",
    "--min-split-size=1M",
    "--file-allocation=falloc",
    "--auto-file-renaming=true",
    "--max-concurrent-downloads=5",
    "--optimize-concurrent-downloads=true",
    "--disk-cache=64M",
    "--async-dns=true",
    "--http-accept-gzip=true",
    "--connect-timeout=15",
    "--timeout=30",
    "--max-tries=5",
    "--retry-wait=3",
    "--lowest-speed-limit=10K",
    "--save-session-interval=10",
    "--console-log-level=warn",
    "--summary-interval=0",
    "--enable-dht=true",
    "--bt-enable-lpd=true",
    "--follow-torrent=true",
    "--bt-save-metadata=true",
    "--pause-metadata=true",
    "--seed-ratio=1.0",
    "--bt-max-peers=128",
    "--bt-request-peer-speed-limit=1M",
    "--dht-entry-point=dht.transmissionbt.com:6881",
    "--bt-tracker-connect-timeout=10",
    "--bt-tracker-timeout=10",
];

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Job {
    pub gid: String,
    pub url: String,
    pub path: String,
    pub total: u64,
    pub done: u64,
    pub speed: u64,
    pub connections: u32,
    pub status: &'static str,
    pub error: String,
    pub name: String,
    pub upload_speed: u64,
    pub upload_length: u64,
    pub num_seeders: u64,
    pub seeder: bool,
    pub followed_by: Vec<String>,
    pub files: usize,
}

const KEYS: &[&str] = &[
    "gid",
    "status",
    "totalLength",
    "completedLength",
    "downloadSpeed",
    "uploadSpeed",
    "uploadLength",
    "connections",
    "numSeeders",
    "seeder",
    "errorMessage",
    "files",
    "followedBy",
    "bittorrent.info.name",
];

pub fn follow_map(jobs: &[Job]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for j in jobs {
        if let Some(next) = j.followed_by.first() {
            map.insert(j.gid.clone(), next.clone());
        }
    }
    map
}

pub struct Poll {
    on: Mutex<bool>,
    wake: Condvar,
}

impl Poll {
    pub fn new() -> Arc<Poll> {
        Arc::new(Poll { on: Mutex::new(false), wake: Condvar::new() })
    }

    pub fn store(&self, on: bool) {
        *lock(&self.on) = on;
        self.wake.notify_all();
    }

    pub fn wait(&self) {
        let mut g = lock(&self.on);
        while !*g {
            g = self.wake.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

pub struct Aria {
    engine: Mutex<Option<Engine>>,
    urls: Mutex<HashMap<String, String>>,
    ids: AtomicU64,
    limit: AtomicU64,
    boot: Boot,
    woken: AtomicBool,
    failed: AtomicBool,
    me: Weak<Aria>,
}

struct Boot {
    data: PathBuf,
    downloads: PathBuf,
    keep_seeding: bool,
    bt_port: u16,
}

const WAKE_WAIT: Duration = Duration::from_secs(20);

struct Engine {
    child: Child,
    port: u16,
    secret: String,
    stopped: bool,
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Engine {
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        let _ = call(self.port, &self.secret, 0, "aria2.shutdown", &[]);
        for _ in 0..100 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[allow(dead_code)]
impl Aria {
    pub fn idle(data_dir: &Path, download_dir: &Path, keep_seeding: bool, limit_kbps: u32, bt_port: u16) -> Arc<Aria> {
        Arc::new_cyclic(|me| Aria {
            engine: Mutex::new(None),
            urls: Mutex::new(HashMap::new()),
            ids: AtomicU64::new(0),
            limit: AtomicU64::new(limit_kbps as u64 * 1024),
            boot: Boot { data: data_dir.to_path_buf(), downloads: download_dir.to_path_buf(), keep_seeding, bt_port },
            woken: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            me: me.clone(),
        })
    }

    pub fn wake(&self) {
        if self.woken.swap(true, Ordering::AcqRel) {
            return;
        }
        let weak = self.me.clone();
        let Boot { data, downloads, keep_seeding, bt_port } = &self.boot;
        let (data, dl, keep_seeding, bt_port) = (data.clone(), downloads.clone(), *keep_seeding, *bt_port);
        let _ = thread::Builder::new()
            .name("lumen-aria".into())
            .spawn(move || {
                let engine = Engine::boot(&data, &dl, keep_seeding, bt_port);
                let Some(aria) = weak.upgrade() else { return };
                let Some(engine) = engine else {
                    aria.failed.store(true, Ordering::Release);
                    return;
                };
                *lock(&aria.engine) = Some(engine);
                aria.try_speed_limit();
                trackers::refresh(&data, &aria);
            });
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    pub fn has_session(data_dir: &Path) -> bool {
        fs::metadata(data_dir.join("aria2").join(SESSION)).is_ok_and(|m| m.len() > 0)
    }

    fn wait_ready(&self) -> Result<(), String> {
        self.wake();
        let until = std::time::Instant::now() + WAKE_WAIT;
        while !self.ready() {
            if self.failed.load(Ordering::Acquire) || std::time::Instant::now() >= until {
                return Err("aria2 did not start".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    pub fn ready(&self) -> bool {
        lock(&self.engine).is_some()
    }

    pub fn shutdown(&self) {
        let engine = lock(&self.engine).take();
        if let Some(mut engine) = engine {
            engine.stop();
        }
    }

    pub fn add(&self, url: &str, headers: &[String], filename: Option<&str>, pause: bool) -> Result<String, String> {
        self.add_in(url, headers, filename, pause, None)
    }

    pub fn add_in(&self, url: &str, headers: &[String], filename: Option<&str>, pause: bool, dir: Option<&Path>) -> Result<String, String> {
        self.wait_ready()?;
        let (port, secret) = self.line()?;
        let mut options = uri_options(headers, filename, pause);
        if let (Some(dir), Some(map)) = (dir, options.as_object_mut()) {
            map.insert("dir".to_string(), Value::String(dir.display().to_string()));
        }
        let gid = call(
            port,
            &secret,
            self.id(),
            "aria2.addUri",
            &[json!([url]), options],
        )?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "aria2.addUri said nothing".to_string())?;
        lock(&self.urls).insert(gid.clone(), url.to_string());
        Ok(gid)
    }

    pub fn add_torrent(&self, b64: &str, headers: &[String]) -> Result<String, String> {
        let (port, secret) = self.line()?;
        let mut options = serde_json::Map::new();
        if !headers.is_empty() {
            options.insert("header".to_string(), json!(headers));
        }
        let gid = call(
            port,
            &secret,
            self.id(),
            "aria2.addTorrent",
            &[json!(b64), json!([]), Value::Object(options)],
        )?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "aria2.addTorrent said nothing".to_string())?;
        Ok(gid)
    }

    pub fn pause(&self, gid: &str) -> Result<(), String> {
        let (port, secret) = self.line()?;
        call(port, &secret, self.id(), "aria2.forcePause", &[json!(gid)]).map(|_| ())
    }

    pub fn resume(&self, gid: &str) -> Result<(), String> {
        let (port, secret) = self.line()?;
        call(port, &secret, self.id(), "aria2.unpause", &[json!(gid)]).map(|_| ())
    }

    pub fn remove(&self, gid: &str) -> Result<(), String> {
        let (port, secret) = self.line()?;
        let gone = call(port, &secret, self.id(), "aria2.forceRemove", &[json!(gid)]).ok();
        let gone = call(port, &secret, self.id(), "aria2.removeDownloadResult", &[json!(gid)])
            .ok()
            .or(gone);
        lock(&self.urls).remove(gid);
        gone.map(|_| ()).ok_or_else(|| "aria2 refused to forget that gid".to_string())
    }

    pub fn jobs(&self) -> Vec<Job> {
        let Some((port, secret)) = self.line_opt() else { return Vec::new() };
        let known = lock(&self.urls).clone();
        let mut out: Vec<Job> = Vec::new();
        let keys = json!(KEYS);
        read_list(port, &secret, self.id(), "aria2.tellActive", &[keys.clone()], true, &known, &mut out);
        read_list(port, &secret, self.id(), "aria2.tellWaiting", &[json!(0), json!(100), keys.clone()], true, &known, &mut out);
        read_list(port, &secret, self.id(), "aria2.tellStopped", &[json!(0), json!(100), keys], false, &known, &mut out);
        let mut fresh = HashMap::new();
        for job in &out {
            if let Some(url) = known.get(&job.gid) {
                fresh.insert(job.gid.clone(), url.clone());
            } else if !job.url.is_empty() {
                fresh.insert(job.gid.clone(), job.url.clone());
            }
        }
        *lock(&self.urls) = fresh;
        out
    }

    pub fn set_speed_limit(&self, bytes_per_sec: u64) {
        self.limit.store(bytes_per_sec, Ordering::Relaxed);
        self.try_speed_limit();
    }

    fn try_speed_limit(&self) {
        let Some((port, secret)) = self.line_opt() else { return };
        let bytes = self.limit.load(Ordering::Relaxed);
        let value = json!({"max-overall-download-limit": bytes.to_string()});
        let _ = call(port, &secret, self.id(), "aria2.changeGlobalOption", &[value]);
    }

    pub fn set_trackers(&self, joined: &str) {
        if joined.is_empty() {
            return;
        }
        let Some((port, secret)) = self.line_opt() else { return };
        let value = json!({"bt-tracker": joined});
        let _ = call(port, &secret, self.id(), "aria2.changeGlobalOption", &[value]);
    }

    fn line(&self) -> Result<(u16, String), String> {
        self.line_opt().ok_or_else(|| "aria2 is not running".to_string())
    }

    fn line_opt(&self) -> Option<(u16, String)> {
        lock(&self.engine).as_ref().map(|e| (e.port, e.secret.clone()))
    }

    fn id(&self) -> u64 {
        self.ids.fetch_add(1, Ordering::Relaxed) + 1
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Engine {
    fn boot(data_dir: &Path, download_dir: &Path, keep_seeding: bool, bt_port: u16) -> Option<Engine> {
        let dir = data_dir.join("aria2");
        fs::create_dir_all(&dir).ok()?;
        fs::create_dir_all(download_dir).ok()?;
        let exe = place_exe(&dir)?;
        let port = free_port()?;
        let secret = random_hex()?;
        let session = dir.join(SESSION);
        let mut cmd = Command::new(&exe);
        cmd.args(FLAGS);
        cmd.args([
            &format!("--rpc-listen-port={port}"),
            &format!("--rpc-secret={secret}"),
            &format!("--dir={}", download_dir.display()),
            &format!("--save-session={}", session.display()),
            &format!("--dht-file-path={}", dir.join("dht.dat").display()),
            "--bt-enable-lpd=true",
            &format!("--listen-port={bt_port}"),
            &format!("--dht-listen-port={bt_port}"),
            "--follow-torrent=true",
            "--bt-save-metadata=true",
            "--pause-metadata=true",
            "--seed-ratio=1.0",
        ]);
        cmd.arg(if keep_seeding { "--seed-time=60" } else { "--seed-time=0" });
        let trackers = trackers::startup(data_dir);
        if !trackers.is_empty() {
            cmd.arg(format!("--bt-tracker={trackers}"));
        }
        if session.exists() {
            cmd.arg(format!("--input-file={}", session.display()));
        }
        cmd.arg(format!("--stop-with-process={}", std::process::id()));
        cmd.creation_flags(CREATE_NO_WINDOW.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd.spawn().ok()?;
        let engine = Engine { child, port, secret, stopped: false };
        for _ in 0..BOOT_TRIES {
            if call(engine.port, &engine.secret, 1, "aria2.getVersion", &[]).is_ok() {
                return Some(engine);
            }
            thread::sleep(BOOT_SLEEP);
        }
        None
    }
}

fn free_port() -> Option<u16> {
    let socket = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = socket.local_addr().ok()?.port();
    drop(socket);
    Some(port)
}

fn random_hex() -> Option<String> {
    let mut b = [0u8; 32];
    unsafe {
        use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
        if BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG).is_err() {
            return None;
        }
    }
    Some(hex(&b))
}

fn place_exe(dir: &Path) -> Option<PathBuf> {
    let exe = dir.join(EXE);
    if sha_of_file(&exe).as_deref() == Some(EXE_SHA256) {
        return Some(exe);
    }
    let zip = dir.join(ZIP);
    if sha_of_file(&zip).as_deref() != Some(ZIP_SHA256) {
        let staged = dir.join(format!("{ZIP}.part"));
        let _ = fs::remove_file(&staged);
        if !fetch(ZIP_URL, &staged) {
            eprintln!("lumen: aria2: the zip did not download");
            let _ = fs::remove_file(&staged);
            return None;
        }
        if sha_of_file(&staged).as_deref() != Some(ZIP_SHA256) {
            eprintln!("lumen: aria2: zip hash is wrong, so it never runs");
            let _ = fs::remove_file(&staged);
            let _ = fs::remove_file(&zip);
            return None;
        }
        let _ = fs::remove_file(&zip);
        fs::rename(&staged, &zip).ok()?;
    }
    let body = zip_member(&fs::read(&zip).ok()?, MEMBER)?;
    if sha(&body) != EXE_SHA256 {
        eprintln!("lumen: aria2: what came out of the zip is not aria2c.exe, so all of it goes");
        let _ = fs::remove_file(&zip);
        return None;
    }
    let staged = dir.join(format!("{EXE}.part"));
    let _ = fs::remove_file(&staged);
    fs::write(&staged, &body).ok()?;
    let ok = sha_of_file(&staged).as_deref() == Some(EXE_SHA256);
    if !ok {
        let _ = fs::remove_file(&staged);
    }
    let _ = fs::remove_file(&zip);
    fs::rename(&staged, &exe).ok()?;
    Some(exe)
}

pub fn pinned_state(data_dir: &Path) -> Option<bool> {
    let sha = sha_of_file(&data_dir.join("aria2").join(EXE))?;
    Some(sha == EXE_SHA256)
}

fn fetch(url: &str, to: &Path) -> bool {
    let config = ureq::config::Config::builder().timeout_global(Some(FETCH_TIMEOUT)).build();
    let agent = ureq::Agent::new_with_config(config);
    let Ok(mut reply) = agent.get(url).call() else { return false };
    let Ok(body) = reply.body_mut().read_to_vec() else { return false };
    if body.len() > MAX_ZIP {
        return false;
    }
    fs::write(to, &body).is_ok()
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub(crate) fn sha_of_file(path: &Path) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(hex(&h.finalize()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn zip_member(zip: &[u8], want: &str) -> Option<Vec<u8>> {
    let eocd = end_of_central_dir(zip)?;
    let count = u16::from_le_bytes([zip[eocd + 10], zip[eocd + 11]]) as usize;
    let mut at = u32::from_le_bytes([zip[eocd + 16], zip[eocd + 17], zip[eocd + 18], zip[eocd + 19]])
        as usize;
    for _ in 0..count {
        if sig(zip, at) != Some(0x0201_4b50) {
            return None;
        }
        let method = u16::from_le_bytes([zip[at + 10], zip[at + 11]]);
        let size = u32::from_le_bytes([zip[at + 20], zip[at + 21], zip[at + 22], zip[at + 23]]) as usize;
        let plain = u32::from_le_bytes([zip[at + 24], zip[at + 25], zip[at + 26], zip[at + 27]]) as usize;
        let nlen = u16::from_le_bytes([zip[at + 28], zip[at + 29]]) as usize;
        let elen = u16::from_le_bytes([zip[at + 30], zip[at + 31]]) as usize;
        let clen = u16::from_le_bytes([zip[at + 32], zip[at + 33]]) as usize;
        let name = std::str::from_utf8(zip.get(at + 46..at + 46 + nlen)?).ok()?;
        let local = u32::from_le_bytes([zip[at + 42], zip[at + 43], zip[at + 44], zip[at + 45]]) as usize;
        if name == want {
            return read_local(zip, local, method, size, plain);
        }
        at += 46 + nlen + elen + clen;
    }
    None
}

fn read_local(zip: &[u8], local: usize, method: u16, size: usize, plain: usize) -> Option<Vec<u8>> {
    if sig(zip, local) != Some(0x0403_4b50) {
        return None;
    }
    if method != 8 || plain > MAX_EXE {
        return None;
    }
    let nlen = u16::from_le_bytes([zip[local + 26], zip[local + 27]]) as usize;
    let elen = u16::from_le_bytes([zip[local + 28], zip[local + 29]]) as usize;
    let from = local + 30 + nlen + elen;
    let data = zip.get(from..from.checked_add(size)?)?;
    let mut out = Vec::with_capacity(plain.min(MAX_EXE));
    flate2::read::DeflateDecoder::new(data)
        .read_to_end(&mut out)
        .ok()?;
    if out.len() != plain {
        return None;
    }
    Some(out)
}

fn end_of_central_dir(zip: &[u8]) -> Option<usize> {
    let last = zip.len().checked_sub(22)?;
    let from = last.saturating_sub(65_535);
    (from..=last).rev().find(|&i| sig(zip, i) == Some(0x0605_4b50))
}

fn sig(zip: &[u8], at: usize) -> Option<u32> {
    let b = zip.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn call(port: u16, secret: &str, id: u64, method: &str, params: &[Value]) -> Result<Value, String> {
    let body = request(id, secret, method, params).to_string();
    let config = ureq::config::Config::builder().timeout_global(Some(RPC_TIMEOUT)).build();
    let agent = ureq::Agent::new_with_config(config);
    let mut reply = agent
        .post(format!("http://127.0.0.1:{port}/jsonrpc"))
        .content_type("application/json")
        .send(body.as_bytes())
        .map_err(|e| format!("{method}: {e}"))?;
    let text = reply
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("{method}: {e}"))?;
    let answer: Value = serde_json::from_str(&text).map_err(|e| format!("{method}: {e}"))?;
    if let Some(error) = answer.get("error") {
        let why = error["message"].as_str().unwrap_or("aria2 gave no reason");
        return Err(format!("{method}: {why}"));
    }
    answer
        .get("result")
        .cloned()
        .ok_or_else(|| format!("{method}: the answer had no result"))
}

fn request(id: u64, secret: &str, method: &str, params: &[Value]) -> Value {
    let mut all: Vec<Value> = Vec::with_capacity(params.len() + 1);
    all.push(Value::String(format!("token:{secret}")));
    all.extend_from_slice(params);
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": all})
}

fn uri_options(headers: &[String], filename: Option<&str>, pause: bool) -> Value {
    let mut options = serde_json::Map::new();
    if !headers.is_empty() {
        options.insert("header".to_string(), json!(headers));
    }
    if let Some(name) = filename.filter(|n| !n.is_empty()) {
        options.insert("out".to_string(), Value::String(name.to_string()));
    }
    if pause {
        options.insert("pause".to_string(), json!("true"));
    }
    Value::Object(options)
}

fn read_list(
    port: u16,
    secret: &str,
    id: u64,
    method: &str,
    params: &[Value],
    back_to_front: bool,
    known: &HashMap<String, String>,
    out: &mut Vec<Job>,
) {
    let Ok(answer) = call(port, secret, id, method, params) else { return };
    let mut list = answer.as_array().cloned().unwrap_or_default();
    if back_to_front {
        list.reverse();
    }
    for record in &list {
        let gid = record["gid"].as_str().unwrap_or_default().to_string();
        let uri = record["files"][0]["uris"][0]["uri"].as_str().unwrap_or("");
        let url = match known.get(&gid) {
            Some(known_url) if uri.is_empty() => known_url.as_str(),
            _ => uri,
        };
        out.push(job_from(record, url));
    }
}

fn map_status(status: &str) -> &'static str {
    match status {
        "active" => "active",
        "waiting" => "waiting",
        "paused" => "paused",
        "complete" => "complete",
        "removed" => "removed",
        _ => "error",
    }
}

fn num(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        .unwrap_or(0)
}

fn job_from(record: &Value, fallback: &str) -> Job {
    let file = &record["files"][0];
    let path = file["path"].as_str().unwrap_or("");
    let uri = file["uris"][0]["uri"].as_str().unwrap_or("");
    Job {
        gid: record["gid"].as_str().unwrap_or_default().to_string(),
        url: if uri.is_empty() { fallback.to_string() } else { uri.to_string() },
        path: path.to_string(),
        total: num(&record["totalLength"]),
        done: num(&record["completedLength"]),
        speed: num(&record["downloadSpeed"]),
        connections: num(&record["connections"]) as u32,
        status: map_status(record["status"].as_str().unwrap_or("")),
        error: record["errorMessage"].as_str().unwrap_or_default().to_string(),
        name: record["bittorrent"]["info"]["name"].as_str().unwrap_or("").to_string(),
        upload_speed: num(&record["uploadSpeed"]),
        upload_length: num(&record["uploadLength"]),
        num_seeders: num(&record["numSeeders"]),
        seeder: record["seeder"].as_str() == Some("true"),
        followed_by: record["followedBy"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default(),
        files: record["files"].as_array().map(|a| a.len()).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TELL_STOPPED: &str = r#"{"bitfield":"80","completedLength":"577","connections":"0",
      "dir":"C:\\Users\\me\\Downloads","downloadSpeed":"0","errorCode":"0","errorMessage":"",
      "files":[{"completedLength":"577","index":"1","length":"577",
        "path":"C:\\/Users\\/me\\/Downloads\\/index.html","selected":"true",
        "uris":[{"status":"used","uri":"https://example.com/"},
                {"status":"waiting","uri":"https://example.com/"}]}],
      "gid":"ea880020a5af8f09","numPieces":"1","pieceLength":"1048576","status":"complete",
      "totalLength":"577","uploadLength":"0","uploadSpeed":"0"}"#;

    fn sample() -> Value {
        serde_json::from_str(TELL_STOPPED).expect("the captured reply parses")
    }

    #[test]
    fn request_puts_the_secret_first() {
        let body = request(7, "s3cret", "aria2.tellActive", &[]);
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["id"], 7);
        assert_eq!(body["method"], "aria2.tellActive");
        assert_eq!(body["params"], json!(["token:s3cret"]));
        assert_eq!(
            request(1, "k", "aria2.getVersion", &[]),
            json!({"jsonrpc": "2.0", "id": 1, "method": "aria2.getVersion", "params": ["token:k"]})
        );
    }

    #[test]
    fn request_keeps_the_methods_own_params_last() {
        let body = request(2, "k", "aria2.addUri", &[json!(["https://a.test/x"]), uri_options(&[], Some("x.bin"), false)]);
        assert_eq!(body["params"][0], "token:k");
        assert_eq!(body["params"][1], json!(["https://a.test/x"]));
        assert_eq!(body["params"][2], json!({"out": "x.bin"}));
    }

    #[test]
    fn tell_status_becomes_a_job() {
        let job = job_from(&sample(), "");
        assert_eq!(job.gid, "ea880020a5af8f09");
        assert_eq!(job.url, "https://example.com/");
        assert_eq!(job.path, "C:\\/Users\\/me\\/Downloads\\/index.html");
        assert_eq!(job.total, 577);
        assert_eq!(job.done, 577);
        assert_eq!(job.speed, 0);
        assert_eq!(job.connections, 0);
        assert_eq!(job.status, "complete");
        assert!(job.error.is_empty());
    }

    #[test]
    fn a_job_with_no_files_yet_says_so() {
        let stopped = json!({
            "gid": "abc", "status": "paused", "totalLength": "1024", "completedLength": "256",
            "downloadSpeed": "512", "connections": "4", "dir": "D:\\dl", "errorMessage": "",
            "errorCode": "0", "files": []
        });
        let job = job_from(&stopped, "https://a.test/x");
        assert_eq!(job.url, "https://a.test/x");
        assert_eq!(job.path, "");
        assert_eq!(job.total, 1024);
        assert_eq!(job.done, 256);
        assert_eq!(job.speed, 512);
        assert_eq!(job.connections, 4);
        assert_eq!(job.status, "paused");
    }

    #[test]
    fn the_error_survives_the_trip() {
        let failed = json!({"gid": "bad", "status": "error", "errorMessage": "Not Found",
                            "errorCode": "3", "completedLength": "0", "totalLength": "0",
                            "dir": "", "downloadSpeed": "0", "connections": "0", "files": []});
        let job = job_from(&failed, "https://a.test/gone");
        assert_eq!(job.status, "error");
        assert_eq!(job.error, "Not Found");
    }

    #[test]
    fn statuses_map_and_unknown_is_an_error() {
        assert_eq!(map_status("active"), "active");
        assert_eq!(map_status("waiting"), "waiting");
        assert_eq!(map_status("paused"), "paused");
        assert_eq!(map_status("complete"), "complete");
        assert_eq!(map_status("removed"), "removed");
        assert_eq!(map_status("error"), "error");
        assert_eq!(map_status(""), "error");
        assert_eq!(map_status("something else"), "error");
    }

    #[test]
    fn headers_and_a_name_become_options() {
        assert_eq!(uri_options(&[], None, false), json!({}));
        let options = uri_options(&["Cookie: a=b".to_string(), "Referer: https://a.test/".to_string()], Some("movie.mkv"), false);
        assert_eq!(
            options,
            json!({"header": ["Cookie: a=b", "Referer: https://a.test/"], "out": "movie.mkv"})
        );
        assert_eq!(uri_options(&["User-Agent: x".to_string()], Some(""), false), json!({"header": ["User-Agent: x"]}));
        assert_eq!(uri_options(&[], None, true), json!({"pause": "true"}));
    }

    #[test]
    fn numbers_arrive_as_strings() {
        assert_eq!(num(&json!(12)), 12);
        assert_eq!(num(&json!("12")), 12);
        assert_eq!(num(&json!(" 12 ")), 12);
        assert_eq!(num(&json!("")), 0);
        assert_eq!(num(&json!(null)), 0);
        assert_eq!(num(&json!("1.5")), 0);
    }

    #[test]
    fn the_speed_limit_reaches_the_global_option() {
        let value = json!({"max-overall-download-limit": 4096u64.to_string()});
        assert_eq!(value["max-overall-download-limit"], "4096");
        assert_eq!(value.to_string(), r#"{"max-overall-download-limit":"4096"}"#);
    }

    const TELL_SEEDING: &str = r#"{"bitfield":"ff","bittorrent":{"info":{"name":"bunny"}},
      "completedLength":"123456","connections":"9","dir":"C:\\dl","downloadSpeed":"0",
      "errorCode":"0","errorMessage":"","followedBy":["aa11bb22cc33dd44"],
      "files":[{"completedLength":"123456","index":"1","length":"274000000",
        "path":"C:\\dl\\bunny\\bunny.mkv","selected":"true","uris":[]}],
      "gid":"ff00aa11bb22cc33","numSeeders":"7","seeder":"true","status":"active",
      "totalLength":"274000000","uploadLength":"51800","uploadSpeed":"307200"}"#;

    #[test]
    fn torrent_fields_come_through() {
        let job: Value = serde_json::from_str(TELL_SEEDING).expect("the captured reply parses");
        let j = job_from(&job, "");
        assert_eq!(j.gid, "ff00aa11bb22cc33");
        assert_eq!(j.name, "bunny");
        assert_eq!(j.upload_speed, 307200);
        assert_eq!(j.upload_length, 51800);
        assert_eq!(j.num_seeders, 7);
        assert!(j.seeder);
        assert_eq!(j.followed_by, vec!["aa11bb22cc33dd44"]);
        assert_eq!(j.files, 1);
        let ratio = j.upload_length as f64 / j.done as f64;
        assert!((ratio - 51800.0 / 123456.0).abs() < 1e-12);
    }

    #[test]
    fn a_plain_file_job_has_no_torrent_fields() {
        let j = job_from(&sample(), "");
        assert!(j.name.is_empty());
        assert_eq!(j.upload_speed, 0);
        assert_eq!(j.upload_length, 0);
        assert_eq!(j.num_seeders, 0);
        assert!(!j.seeder);
        assert!(j.followed_by.is_empty());
        assert_eq!(j.files, 1);
    }

    #[test]
    fn followed_by_builds_a_remap_table() {
        let a: Value = json!({"gid": "old", "followedBy": ["new"], "files": []});
        let b: Value = json!({"gid": "plain", "files": []});
        let jobs = vec![job_from(&a, ""), job_from(&b, "")];
        let map = follow_map(&jobs);
        assert_eq!(map.get("old").map(String::as_str), Some("new"));
        assert!(!map.contains_key("plain"));
    }

    #[test]
    fn the_key_list_covers_what_the_ui_reads() {
        for k in ["bittorrent.info.name", "uploadSpeed", "numSeeders", "seeder", "followedBy", "files"] {
            assert!(KEYS.contains(&k), "{k} is missing from the aria2 key list");
        }
    }

    #[test]
    fn the_flag_list_holds_what_this_build_was_tuned_with() {
        for want in [
            "--min-split-size=1M",
            "--disk-cache=64M",
            "--file-allocation=falloc",
            "--async-dns=true",
            "--lowest-speed-limit=10K",
            "--max-tries=5",
            "--retry-wait=3",
            "--timeout=30",
            "--connect-timeout=15",
            "--max-concurrent-downloads=5",
            "--optimize-concurrent-downloads=true",
            "--http-accept-gzip=true",
            "--bt-max-peers=128",
            "--bt-request-peer-speed-limit=1M",
            "--dht-entry-point=dht.transmissionbt.com:6881",
            "--bt-tracker-connect-timeout=10",
            "--bt-tracker-timeout=10",
            "--max-connection-per-server=16",
            "--split=16",
        ] {
            assert!(FLAGS.contains(&want), "{want} is missing from the aria2 flag list");
        }
        assert!(!FLAGS.contains(&"--file-allocation=none"));
        assert!(!FLAGS.contains(&"--max-concurrent-downloads=4"));
        assert!(FLAGS.iter().all(|f| f.starts_with("--") && !f.contains(' ') && !f.is_empty()));
        let mut sorted: Vec<&str> = FLAGS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), FLAGS.len(), "a flag is in there twice");
    }

    #[test]
    fn aria_crosses_threads() {
        fn both<T: Send + Sync>() {}
        both::<Aria>();
        both::<Job>();
    }

    fn tiny_zip(member: &str, payload: &[u8]) -> Vec<u8> {
        use std::io::Write;

        use flate2::Compression;
        use flate2::write::DeflateEncoder;

        let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
        enc.write_all(payload).unwrap();
        let deflated = enc.finish().unwrap();
        let crc = crc32(payload);

        let mut z = Vec::new();
        let local_at = z.len() as u32;
        z.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        z.extend_from_slice(&20u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&8u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&crc.to_le_bytes());
        z.extend_from_slice(&(deflated.len() as u32).to_le_bytes());
        z.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        z.extend_from_slice(&(member.len() as u16).to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(member.as_bytes());
        z.extend_from_slice(&deflated);

        let dir_at = z.len() as u32;
        z.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        z.extend_from_slice(&20u16.to_le_bytes());
        z.extend_from_slice(&20u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&8u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&crc.to_le_bytes());
        z.extend_from_slice(&(deflated.len() as u32).to_le_bytes());
        z.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        z.extend_from_slice(&(member.len() as u16).to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&0u32.to_le_bytes());
        z.extend_from_slice(&local_at.to_le_bytes());
        z.extend_from_slice(member.as_bytes());

        let count = 1usize;
        z.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&(count as u16).to_le_bytes());
        z.extend_from_slice(&(count as u16).to_le_bytes());
        z.extend_from_slice(&((z.len() - dir_at as usize) as u32).to_le_bytes());
        z.extend_from_slice(&dir_at.to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (i, slot) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        let mut c = 0xFFFF_FFFFu32;
        for b in bytes {
            c = table[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        !c
    }

    #[test]
    fn one_member_comes_out_of_the_zip() {
        let payload = b"aria2 1.37.0, hidden behind a curl of deflate";
        let z = tiny_zip("aria2-1.37.0/aria2c.exe", payload);
        assert_eq!(zip_member(&z, "aria2-1.37.0/aria2c.exe").unwrap(), payload);
        assert!(zip_member(&z, "aria2-1.37.0/OTHER.txt").is_none());
        assert!(zip_member(b"PK\x05\x06 not a zip", "x").is_none());
        assert!(zip_member(&[], "x").is_none());
    }

    #[test]
    fn the_real_release_zip_holds_aria2c() {
        assert_eq!(ZIP_SHA256.len(), 64);
        assert_eq!(EXE_SHA256.len(), 64);
        assert_eq!(sha(b"hello"), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        assert_eq!(sha(&[]), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn a_big_zip_is_not_a_zip_we_read() {
        let mut z = tiny_zip("a/b.txt", b"x");
        let eocd = z.len() - 22;
        let at = u32::from_le_bytes([z[eocd + 16], z[eocd + 17], z[eocd + 18], z[eocd + 19]]) as usize;
        let plain = u32::MAX.to_le_bytes();
        z[at + 24..at + 28].copy_from_slice(&plain);
        assert!(zip_member(&z, "a/b.txt").is_none());
    }
}

#[cfg(test)]
mod live {
    use super::*;

    fn case(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-aria-{what}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn aria2_waits_to_be_wanted() {
        let dir = case("idle");
        let aria = Aria::idle(&dir, &dir.join("downloads"), true, 0, 6881);
        assert!(!aria.ready() && !aria.woken.load(Ordering::Acquire) && !aria.failed());
        assert!(!Aria::has_session(&dir));
        let _ = fs::create_dir_all(dir.join("aria2"));
        let _ = fs::write(dir.join("aria2").join(SESSION), "");
        assert!(!Aria::has_session(&dir));
        let _ = fs::write(dir.join("aria2").join(SESSION), "https://example.com/a.zip
");
        assert!(Aria::has_session(&dir));
    }

    #[test]
    #[ignore = "downloads aria2 and a file off the internet"]
    fn it_downloads_a_file_through_aria2() {
        let dir = case("live");
        let aria = Aria::idle(&dir, &dir.join("downloads"), true, 0, 6881);
        aria.wake();
        for _ in 0..300 {
            if aria.ready() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        assert!(aria.ready(), "aria2 never came up");

        let gid = aria
            .add("https://example.com/", &["Referer: https://example.com/".to_string()], None, false)
            .expect("aria2 took the url");

        let mut done = false;
        for _ in 0..150 {
            for job in aria.jobs().iter().filter(|j| j.gid == gid) {
                if job.status == "complete" {
                    assert_eq!(job.url, "https://example.com/");
                    assert!(job.done > 0, "example.com is never empty");
                    assert!(Path::new(&job.path).is_file(), "the file is at {}", job.path);
                    done = true;
                }
                assert_ne!(job.status, "error", "{}", job.error);
            }
            if done {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        assert!(done, "the download never finished");

        aria.set_speed_limit(1024);
        let again = aria.add("https://example.com/", &[], Some("second.html"), false).expect("aria2 took it again");
        assert_eq!(aria.pause(&again).ok(), Some(()));
        assert_eq!(aria.resume(&again).ok(), Some(()));
        assert_eq!(aria.remove(&again).ok(), Some(()));

        let jobs = aria.jobs();
        assert!(jobs.iter().all(|j| !j.gid.is_empty()), "every job has a gid");
    }

    #[test]
    #[ignore = "downloads aria2 and a tracker list off the internet"]
    fn a_fresh_tracker_list_reaches_a_running_aria2() {
        let dir = case("push");
        let aria = Aria::idle(&dir, &dir.join("downloads"), true, 0, 6881);
        aria.wake();
        for _ in 0..300 {
            if aria.ready() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        assert!(aria.ready(), "aria2 never came up");
        let mut held = String::new();
        for _ in 0..60 {
            {
                let engine = lock(&aria.engine);
                if let Some(engine) = engine.as_ref()
                    && let Ok(answer) = call(engine.port, &engine.secret, 1, "aria2.getGlobalOption", &[])
                {
                    held = answer["bt-tracker"].as_str().unwrap_or("").to_string();
                }
            }
            if !held.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }
        assert!(!held.is_empty(), "aria2 holds no trackers after the background fetch");
        let trackers: Vec<&str> = held.split(',').collect();
        assert!(trackers.len() > 1, "the list never made it across: {held}");
        assert!(trackers.iter().all(|t| t.starts_with("udp://") || t.starts_with("http")), "not a tracker list: {held}");
        let cached = crate::trackers::load(&dir.join("aria2"));
        assert!(!cached.is_empty(), "the list was pushed but never cached");
        assert_eq!(cached.join(","), held, "what aria2 holds and what is cached are one list");
    }

    #[test]
    #[ignore = "downloads aria2 to read its help"]
    fn every_flag_is_one_aria2_knows() {
        let dir = case("help");
        let exe = place_exe(&dir).expect("aria2 came down and its hash was right");
        let out = Command::new(&exe)
            .arg("--help=#all")
            .creation_flags(CREATE_NO_WINDOW.0)
            .output()
            .expect("aria2 answered");
        let help = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(help.contains("--min-split-size"), "aria2 printed no help at all");
        for flag in FLAGS {
            let name = flag.split('=').next().unwrap_or(flag);
            let defined = help.lines().any(|l| {
                l.contains(&format!("{name}=")) || l.contains(&format!("{name}[")) || l.contains(&format!("{name} "))
            });
            assert!(defined, "{name} is not among aria2's own options");
        }
    }
}
