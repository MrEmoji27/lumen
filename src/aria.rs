//! Downloads run on many wires at once, so aria2 runs them.
//!
//! aria2 is a shell of a thing: we hand it a URL, it splits the file into
//! pieces, opens sixteen connections, keeps the map on disk, and resumes
//! tomorrow where it stopped tonight. We never speak to it but for JSON-RPC
//! over localhost. No UI lives here.
//!
//! The binary is not ours, so it gets no trust for free. It arrives in a zip
//! whose hash we pinned before we ever ran it, and only `aria2c.exe` leaves
//! that zip. If the bytes are wrong, the file is deleted and we stay silent.
//! `ready()` is false and Lumen keeps using its own downloader.

use std::{
    collections::HashMap,
    fs,
    io::Read,
    net::TcpListener,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::trackers;

/// Where the zip comes from. One line, pinned to a release.
const ZIP_URL: &str =
    "https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-win-64bit-build1.zip";
/// Hash of that zip, computed here before a single byte of it was ever run.
const ZIP_SHA256: &str = "67d015301eef0b612191212d564c5bb0a14b5b9c4796b76454276a4d28d9b288";
/// Hash of `aria2c.exe` as it sits inside that zip. A second lock on the same door.
const EXE_SHA256: &str = "be2099c214f63a3cb4954b09a0becd6e2e34660b886d4c898d260febfe9d70c2";
/// The one member we take out of the archive.
const MEMBER: &str = "aria2-1.37.0-win-64bit-build1/aria2c.exe";
const EXE: &str = "aria2c.exe";
const ZIP: &str = "aria2.zip";
const SESSION: &str = "session.txt";
/// aria2c.exe is 5.6 MB. Anything larger was never the file we asked for.
const MAX_EXE: usize = 64 << 20;
/// The release zip is 2.4 MB. Same rule, same reason.
const MAX_ZIP: usize = 32 << 20;
/// Getting the zip is a one time thing; asking aria2 questions is not.
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const RPC_TIMEOUT: Duration = Duration::from_secs(3);
/// How long aria2 gets to open its port after it is spawned.
const BOOT_TRIES: u32 = 50;
const BOOT_SLEEP: Duration = Duration::from_millis(100);

/// What aria2 is told at startup, every time. Every line here was read in
/// `aria2c.exe --help=#all` for 1.37.0: a flag the binary does not know is
/// dropped without a word, and the tuning would then be a lie.
const FLAGS: &[&str] = &[
    "--enable-rpc",
    // A downloader that listens on the network is a hole.
    "--rpc-listen-all=false",
    "--continue=true",
    // Sixteen wires into one file, in pieces of a megabyte and up: small
    // pieces keep every wire busy, 1M keeps the bookkeeping sane.
    "--max-connection-per-server=16",
    "--split=16",
    "--min-split-size=1M",
    // falloc lays a big file down on NTFS at once. `none` would grow it in
    // pieces and pay a seek per write.
    "--file-allocation=falloc",
    "--auto-file-renaming=true",
    // Five at once, and aria2 slides the count toward whatever moves.
    "--max-concurrent-downloads=5",
    "--optimize-concurrent-downloads=true",
    // 64 MB of finished pieces held in RAM: fewer, larger writes out, and a
    // hash check never has to read the disk back.
    "--disk-cache=64M",
    // HTTP: DNS off the blocking path, gzip when the server offers it.
    "--async-dns=true",
    "--http-accept-gzip=true",
    // A dead server gets 15 seconds to connect and 30 to move, then five
    // tries three seconds apart before the download is called failed.
    "--connect-timeout=15",
    "--timeout=30",
    "--max-tries=5",
    "--retry-wait=3",
    // Hang up on a crawl. aria2's own help says this one never touches
    // BitTorrent, so a torrent that is only waiting for peers is left alone.
    "--lowest-speed-limit=10K",
    "--save-session-interval=10",
    "--console-log-level=warn",
    "--summary-interval=0",
    // BitTorrent: magnet links, DHT, local peer discovery, follow a
    // finished .torrent into the torrent it names.
    "--enable-dht=true",
    "--bt-enable-lpd=true",
    // These two ports belong to the upnp worker. It rewrites them; we do not.
    "--follow-torrent=true",
    "--bt-save-metadata=true",
    // A torrent a PAGE starts must never join the swarm on its own: born
    // paused, waiting for the person to press Resume. Person-started torrents
    // are resumed by the main loop once their metadata is in.
    "--pause-metadata=true",
    "--seed-ratio=1.0",
    // The swarm: 128 peers at a time, four times aria2's own default of 55.
    // 0 means no cap, and an uncapped swarm is how a laptop cooks itself.
    // Under 1M/s overall, aria2 goes looking for more of them.
    "--bt-max-peers=128",
    "--bt-request-peer-speed-limit=1M",
    // The DHT has to shake hands with one live node before it finds the rest.
    "--dht-entry-point=dht.transmissionbt.com:6881",
    // A tracker that never answers must not hold the swarm hostage for
    // aria2's default of a minute.
    "--bt-tracker-connect-timeout=10",
    "--bt-tracker-timeout=10",
];

/// One download, as the UI needs to see it.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Job {
    pub gid: String,
    pub url: String,
    /// Full file path. Empty until aria2 has decided on one.
    pub path: String,
    pub total: u64,
    pub done: u64,
    /// Bytes per second, right now.
    pub speed: u64,
    pub connections: u32,
    /// "active" | "waiting" | "paused" | "complete" | "error" | "removed"
    pub status: &'static str,
    pub error: String,
    /// Torrent name from bittorrent.info.name. Empty for plain files.
    pub name: String,
    /// Bytes per second out, right now.
    pub upload_speed: u64,
    pub upload_length: u64,
    pub num_seeders: u64,
    /// True when the download is complete and seeding.
    pub seeder: bool,
    /// Gids aria2 spawned for this one (a .torrent file becomes a torrent).
    pub followed_by: Vec<String>,
    /// Number of files (a multi-file torrent opens its folder, not the first file).
    pub files: usize,
}

/// Job keys worth asking aria2 for. Request only these so the poll stays small.
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

/// gid -> the job it spawned, for every job that has a followedBy.
pub fn follow_map(jobs: &[Job]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for j in jobs {
        if let Some(next) = j.followed_by.first() {
            map.insert(j.gid.clone(), next.clone());
        }
    }
    map
}

/// The poll thread's switch. A download flips it on; the thread sleeps on a
/// condvar until then, so an idle Lumen has no polling thread at all.
pub struct Poll {
    on: Mutex<bool>,
    wake: Condvar,
}

impl Poll {
    pub fn new() -> Arc<Poll> {
        Arc::new(Poll { on: Mutex::new(false), wake: Condvar::new() })
    }

    /// True: start asking aria2 about its jobs. False: go back to sleep.
    pub fn store(&self, on: bool) {
        *lock(&self.on) = on;
        self.wake.notify_all();
    }

    /// Sleep until a download has something to ask about.
    pub fn wait(&self) {
        let mut g = lock(&self.on);
        while !*g {
            g = self.wake.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// The engine, and nothing else. `start` returns before it knows anything.
pub struct Aria {
    /// The running engine. None until it arrives; none forever if it never does.
    engine: Mutex<Option<Engine>>,
    /// gid to the URL we asked for, because a stopped job forgets its own URL.
    urls: Mutex<HashMap<String, String>>,
    ids: AtomicU64,
    /// Download speed cap in bytes/s (0 = none). Held here so it survives an
    /// engine that has not booted yet, and is applied the moment it has.
    limit: AtomicU64,
}

/// The process, and how to talk to it. One per Lumen run.
struct Engine {
    child: Child,
    port: u16,
    secret: String,
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[allow(dead_code)]
impl Aria {
    /// Returns at once. The binary is fetched, checked and started on a
    /// background thread; `ready()` is false until that is done.
    pub fn start(
        data_dir: &Path,
        download_dir: &Path,
        keep_seeding: bool,
        limit_kbps: u32,
        bt_port: u16,
    ) -> Arc<Aria> {
        let aria = Arc::new(Aria {
            engine: Mutex::new(None),
            urls: Mutex::new(HashMap::new()),
            ids: AtomicU64::new(0),
            limit: AtomicU64::new(limit_kbps as u64 * 1024),
        });
        let weak = Arc::downgrade(&aria);
        let data = data_dir.to_path_buf();
        let dl = download_dir.to_path_buf();
        let _ = thread::Builder::new()
            .name("lumen-aria".into())
            .spawn(move || {
                let Some(engine) = Engine::boot(&data, &dl, keep_seeding, bt_port) else { return };
                // Lumen may have died while we were downloading. Let the
                // engine drop and the aria2 process dies with it.
                if let Some(aria) = weak.upgrade() {
                    *lock(&aria.engine) = Some(engine);
                    // The speed limit chosen before boot takes effect now.
                    aria.try_speed_limit();
                }
            });
        // Trackers are a network errand of their own: once a week, off this
        // thread, never in the way of a start.
        let late = aria.clone();
        let late_data = data_dir.to_path_buf();
        let _ = thread::Builder::new()
            .name("lumen-trackers".into())
            .spawn(move || trackers::refresh(&late_data, &late));
        aria
    }

    /// True when aria2 answers. Until then there is no downloader to talk to.
    pub fn ready(&self) -> bool {
        lock(&self.engine).is_some()
    }

    /// Queue a URL. Headers are whole lines: "Cookie: a=b", "Referer: ...".
    /// `pause` puts it in paused, for a torrent a PAGE started: it must wait
    /// for the person to press Resume. Returns the gid aria2 gave it.
    pub fn add(&self, url: &str, headers: &[String], filename: Option<&str>, pause: bool) -> Result<String, String> {
        self.add_in(url, headers, filename, pause, None)
    }

    /// Like `add`, into a folder of its own instead of the download folder. A torrent that a
    /// .torrent file fetched this way spawns inherits the folder.
    pub fn add_in(&self, url: &str, headers: &[String], filename: Option<&str>, pause: bool, dir: Option<&Path>) -> Result<String, String> {
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

    /// Queue a base64 .torrent (Open torrent…). Headers travel the same way
    /// as for a URL, for sites that need a cookie even on the torrent itself.
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

    /// Stop the pieces, keep the file.
    pub fn pause(&self, gid: &str) -> Result<(), String> {
        let (port, secret) = self.line()?;
        call(port, &secret, self.id(), "aria2.forcePause", &[json!(gid)]).map(|_| ())
    }

    /// Pick the pieces up again.
    pub fn resume(&self, gid: &str) -> Result<(), String> {
        let (port, secret) = self.line()?;
        call(port, &secret, self.id(), "aria2.unpause", &[json!(gid)]).map(|_| ())
    }

    /// Kill it and forget it. A running download only stops if forced.
    pub fn remove(&self, gid: &str) -> Result<(), String> {
        let (port, secret) = self.line()?;
        let gone = call(port, &secret, self.id(), "aria2.forceRemove", &[json!(gid)]).ok();
        let gone = call(port, &secret, self.id(), "aria2.removeDownloadResult", &[json!(gid)])
            .ok()
            .or(gone);
        lock(&self.urls).remove(gid);
        gone.map(|_| ()).ok_or_else(|| "aria2 refused to forget that gid".to_string())
    }

    /// Everything aria2 is holding: active, waiting, and recently stopped.
    /// Newest first.
    pub fn jobs(&self) -> Vec<Job> {
        let Some((port, secret)) = self.line_opt() else { return Vec::new() };
        let known = lock(&self.urls).clone();
        let mut out: Vec<Job> = Vec::new();
        let keys = json!(KEYS);
        // Active and waiting come out oldest first; stopped comes out newest first.
        read_list(port, &secret, self.id(), "aria2.tellActive", &[keys.clone()], true, &known, &mut out);
        read_list(port, &secret, self.id(), "aria2.tellWaiting", &[json!(0), json!(100), keys.clone()], true, &known, &mut out);
        read_list(port, &secret, self.id(), "aria2.tellStopped", &[json!(0), json!(100), keys], false, &known, &mut out);
        // Rebuild the URL table from what is still alive, so it cannot grow
        // without bound while a browser runs for weeks.
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

    /// Bytes per second for the whole engine. 0 means no cap. Remembered, so it
    /// survives an engine that has not booted yet and is applied when it does.
    pub fn set_speed_limit(&self, bytes_per_sec: u64) {
        self.limit.store(bytes_per_sec, Ordering::Relaxed);
        self.try_speed_limit();
    }

    /// Push the remembered limit into the engine, if it is up.
    fn try_speed_limit(&self) {
        let Some((port, secret)) = self.line_opt() else { return };
        let bytes = self.limit.load(Ordering::Relaxed);
        let value = json!({"max-overall-download-limit": bytes.to_string()});
        let _ = call(port, &secret, self.id(), "aria2.changeGlobalOption", &[value]);
    }

    /// Hand aria2 a tracker list it has not seen, without restarting it.
    /// aria2 takes `bt-tracker` as one comma separated string; it answered
    /// `OK` to that shape on 1.37.0 before this was ever wired up.
    pub fn set_trackers(&self, joined: &str) {
        if joined.is_empty() {
            return;
        }
        let Some((port, secret)) = self.line_opt() else { return };
        let value = json!({"bt-tracker": joined});
        let _ = call(port, &secret, self.id(), "aria2.changeGlobalOption", &[value]);
    }

    /// Port and secret, or an error if the engine never came up.
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

/// A poisoned lock is only poison because a thread panicked while holding it.
/// The data inside is still good.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Engine {
    /// Put a verified aria2c.exe on disk, run it, and wait for its port to
    /// answer. None means Lumen downloads files itself.
    fn boot(data_dir: &Path, download_dir: &Path, keep_seeding: bool, bt_port: u16) -> Option<Engine> {
        let dir = data_dir.join("aria2");
        fs::create_dir_all(&dir).ok()?;
        fs::create_dir_all(download_dir).ok()?;
        let exe = place_exe(&dir)?;
        let port = free_port()?;
        let secret = random_hex()?;
        let session = dir.join(SESSION);
        let mut cmd = Command::new(&exe);
        // Everything that never changes, in one place a test can read. aria2
        // drops an option it does not know without a word, so the list would
        // rot silently if nothing held it to the binary it was written for.
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
            // A torrent a PAGE starts must never join the swarm on its own:
            // born paused, waiting for the person to press Resume. Person-started
            // torrents are resumed by the main loop once their metadata is in.
            "--pause-metadata=true",
            "--seed-ratio=1.0",
        ]);
        // Stop seeding after an hour, or immediately when the user said no to sharing.
        cmd.arg(if keep_seeding { "--seed-time=60" } else { "--seed-time=0" });
        // Last week's trackers, straight off disk: reading a file is not I/O
        // worth fearing. A fresher list arrives on its own thread later and
        // is pushed in over JSON-RPC, so nothing here waits on the network.
        let trackers = trackers::startup(data_dir);
        if !trackers.is_empty() {
            cmd.arg(format!("--bt-tracker={trackers}"));
        }
        if session.exists() {
            // Last night's queue comes back.
            cmd.arg(format!("--input-file={}", session.display()));
        }
        // Lumen dies, aria2 dies. No orphans eating the wire.
        cmd.arg(format!("--stop-with-process={}", std::process::id()));
        // No console window. The user did not open a terminal.
        cmd.creation_flags(CREATE_NO_WINDOW.0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd.spawn().ok()?;
        let engine = Engine { child, port, secret };
        // The port appears a moment after the process does. Wait for it.
        for _ in 0..BOOT_TRIES {
            if call(engine.port, &engine.secret, 1, "aria2.getVersion", &[]).is_ok() {
                return Some(engine);
            }
            thread::sleep(BOOT_SLEEP);
        }
        // It never answered. Dropping it kills the process.
        None
    }
}

/// Bind a port, read it, let go. It falls to aria2 to hold on to it.
fn free_port() -> Option<u16> {
    let socket = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = socket.local_addr().ok()?.port();
    drop(socket);
    Some(port)
}

/// 32 bytes from the operating system, as hex. No seed, no clock.
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

// --------------------------------------------------------------- getting it

/// The verified aria2c.exe, on disk. None means it cannot be had, or it
/// cannot be believed.
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
        // Wrong bytes mean anything: a mirror, a proxy, a lie. Delete and refuse.
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
    // Write beside the target and move, so a half binary never sits there.
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

/// Bytes over the wire, written to disk. False if they never arrive.
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

// ------------------------------------------------------------------- the zip

/// One file out of a zip, inflated. Enough for a release archive: no zip64,
/// no encryption, no split disks.
fn zip_member(zip: &[u8], want: &str) -> Option<Vec<u8>> {
    let eocd = end_of_central_dir(zip)?;
    let count = u16::from_le_bytes([zip[eocd + 10], zip[eocd + 11]]) as usize;
    // The central directory offset is the last thing a writer knows; the
    // directory itself is read forward from there.
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

/// The entry's bytes, inflated. The local header sits before the data, so the
/// data starts after its two variable length fields.
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

/// Where the central directory starts, found from the end of the file.
fn end_of_central_dir(zip: &[u8]) -> Option<usize> {
    let last = zip.len().checked_sub(22)?;
    // Up to 64 KB of comment may follow the record.
    let from = last.saturating_sub(65_535);
    (from..=last).rev().find(|&i| sig(zip, i) == Some(0x0605_4b50))
}

fn sig(zip: &[u8], at: usize) -> Option<u32> {
    let b = zip.get(at..at + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

// ------------------------------------------------------------- json-rpc talk

/// One JSON-RPC call over HTTP. It succeeds or it says why not.
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

/// The wire form. The secret goes first in params, always.
fn request(id: u64, secret: &str, method: &str, params: &[Value]) -> Value {
    let mut all: Vec<Value> = Vec::with_capacity(params.len() + 1);
    all.push(Value::String(format!("token:{secret}")));
    all.extend_from_slice(params);
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": all})
}

/// Per download options. Header lines travel as a list; the name goes in "out".
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

/// One list call into jobs. `back_to_front` walks the reply backwards, for
/// the calls that answer oldest first.
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

/// aria2's words for a state, so far as they fit ours.
fn map_status(status: &str) -> &'static str {
    match status {
        "active" => "active",
        "waiting" => "waiting",
        "paused" => "paused",
        "complete" => "complete",
        "removed" => "removed",
        // "error", and anything we have never heard of, which is the same thing.
        _ => "error",
    }
}

/// aria2 counts in strings. Numbers with letters are still numbers.
fn num(value: &Value) -> u64 {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        .unwrap_or(0)
}

/// One tellStatus record, as a Job. `fallback` is the URL we asked for, since
/// a stopped job forgets its own.
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

    /// Caught off the wire from aria2 1.37.0, right after a small file landed.
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
        // Compared as JSON, not as text. serde_json keeps object keys in sorted or in
        // insertion order depending on its preserve_order feature, which some of the
        // dependency trees Lumen can be built with turn on. The shape is the point.
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
        // A name that is only spaces is no name at all.
        assert_eq!(uri_options(&["User-Agent: x".to_string()], Some(""), false), json!({"header": ["User-Agent: x"]}));
        // A torrent a page started goes in paused, waiting for the person.
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
        // The same shape changeGlobalOption is handed; aria2 wants strings.
        let value = json!({"max-overall-download-limit": 4096u64.to_string()});
        assert_eq!(value["max-overall-download-limit"], "4096");
        assert_eq!(value.to_string(), r#"{"max-overall-download-limit":"4096"}"#);
    }

    /// Caught off the wire from aria2 1.37.0 while a torrent was seeding.
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
        // ratio = uploadLength / completedLength.
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
        // The poll asks for exactly these; drop one and a field goes dark.
        for k in ["bittorrent.info.name", "uploadSpeed", "numSeeders", "seeder", "followedBy", "files"] {
            assert!(KEYS.contains(&k), "{k} is missing from the aria2 key list");
        }
    }

    #[test]
    fn the_flag_list_holds_what_this_build_was_tuned_with() {
        // Every flag the tuning added, plus the ones kept from before. aria2
        // drops an option it does not know without a word, so a typo here
        // would look exactly like a tuning that works.
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
        // The values these replaced must not creep back.
        assert!(!FLAGS.contains(&"--file-allocation=none"));
        assert!(!FLAGS.contains(&"--max-concurrent-downloads=4"));
        // Every entry is one well formed flag, and no flag is said twice:
        // aria2 would take the last one and nobody would notice.
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

    /// A zip with one deflated member, written by hand so the reader can be
    /// checked without the internet.
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
        z.extend_from_slice(&0u16.to_le_bytes()); // flags
        z.extend_from_slice(&8u16.to_le_bytes()); // deflate
        z.extend_from_slice(&0u16.to_le_bytes()); // time
        z.extend_from_slice(&0u16.to_le_bytes()); // date
        z.extend_from_slice(&crc.to_le_bytes());
        z.extend_from_slice(&(deflated.len() as u32).to_le_bytes());
        z.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        z.extend_from_slice(&(member.len() as u16).to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes()); // extra
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
        z.extend_from_slice(&0u32.to_le_bytes()); // attrs
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
        // A member that was never in there is nothing, not a crash.
        assert!(zip_member(&z, "aria2-1.37.0/OTHER.txt").is_none());
        assert!(zip_member(b"PK\x05\x06 not a zip", "x").is_none());
        assert!(zip_member(&[], "x").is_none());
    }

    #[test]
    fn the_real_release_zip_holds_aria2c() {
        // The hash we pinned is the hash of the file that is actually there.
        assert_eq!(ZIP_SHA256.len(), 64);
        assert_eq!(EXE_SHA256.len(), 64);
        assert_eq!(sha(b"hello"), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        assert_eq!(sha(&[]), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn a_big_zip_is_not_a_zip_we_read() {
        // The reader must not believe a size that was never checked.
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
    //! Needs the internet. Run with: cargo test aria -- --ignored

    use super::*;

    fn case(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-aria-{what}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    #[ignore = "downloads aria2 and a file off the internet"]
    fn it_downloads_a_file_through_aria2() {
        let dir = case("live");
        let aria = Aria::start(&dir, &dir.join("downloads"), true, 0, 6881);
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

    /// The runtime tracker push, end to end: a fresh list, a running aria2,
    /// and the value read back over the same JSON-RPC door Lumen uses.
    #[test]
    #[ignore = "downloads aria2 and a tracker list off the internet"]
    fn a_fresh_tracker_list_reaches_a_running_aria2() {
        let dir = case("push");
        let aria = Aria::start(&dir, &dir.join("downloads"), true, 0, 6881);
        for _ in 0..300 {
            if aria.ready() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
        }
        assert!(aria.ready(), "aria2 never came up");
        // The background thread fetches the public list on its own once it
        // sees there is no cache. It does not have to be asked.
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
        // And the same list is on disk, so the next start does not need a fetch.
        let cached = crate::trackers::load(&dir.join("aria2"));
        assert!(!cached.is_empty(), "the list was pushed but never cached");
        assert_eq!(cached.join(","), held, "what aria2 holds and what is cached are one list");
    }

    /// Hold every flag to the binary's own help. aria2 drops an option it
    /// does not know without a word, so this is the only check that says the
    /// tuning reaches the engine instead of vanishing on the way in.
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
