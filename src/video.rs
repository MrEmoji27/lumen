//! Save the video on a page.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

use sha2::{Digest, Sha256};

const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/yt-dlp.exe";
const YTDLP_SHA256: &str = "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a";
const FFMPEG_URL: &str = "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip";
const FFMPEG_ZIP_SHA256: &str = "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba";
const FFMPEG_MEMBER: &str = "ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe";
const FFMPEG_SHA_FILE: &str = "ffmpeg.sha256";
const MAX_FETCH: u64 = 160 << 20;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone)]
pub enum Up {
    Note(String),
    Progress { done: u64, total: u64, speed: u64, file: String },
    Done(String),
    Failed(String),
}

pub type Slot = Arc<Mutex<Option<Child>>>;

pub fn grab(tools: &Path, out: &Path, url: &str, ua: &str, slot: &Slot, sponsorblock: bool, send: impl Fn(Up)) {
    let _ = fs::create_dir_all(tools);
    let exe = tools.join("yt-dlp.exe");
    if !verified(&exe, YTDLP_SHA256) {
        send(Up::Note("getting yt-dlp (once)".into()));
        if let Err(e) = fetch_verified(YTDLP_URL, YTDLP_SHA256, &exe) {
            send(Up::Failed(format!("yt-dlp: {e}")));
            return;
        }
    }
    let ffmpeg = tools.join("ffmpeg.exe");
    if !ffmpeg.exists() {
        send(Up::Note("getting FFmpeg (once, 115 MB)".into()));
        if let Err(e) = place_ffmpeg(tools) {
            send(Up::Failed(format!("FFmpeg: {e}")));
            return;
        }
    }
    send(Up::Note("finding the video".into()));
    let mut cmd = Command::new(&exe);
    cmd.args(["--no-playlist", "--newline", "--no-colors", "--progress", "--no-simulate", "--encoding", "utf-8"])
        .arg("--ffmpeg-location")
        .arg(tools)
        .args(["-f", "bv*+ba/b", "-S", "res:1080,ext:mp4:m4a", "--merge-output-format", "mp4"])
        .arg("-P")
        .arg(out)
        .args(["-o", "%(title).150B [%(id)s].%(ext)s"])
        .args(["--progress-template", "download:LUMEN|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.filename)s"])
        .args(["--print", "after_move:LUMENFILE|%(filepath)s"])
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    if !ua.is_empty() {
        cmd.arg("--user-agent").arg(ua);
    }
    if sponsorblock {
        cmd.args(["--sponsorblock-remove", "sponsor"]);
    }
    if has_node() {
        cmd.args(["--js-runtimes", "node"]);
    }
    cmd.arg("--").arg(url);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            send(Up::Failed(format!("yt-dlp did not start: {e}")));
            return;
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    let err_thread = stderr.map(|e| {
        std::thread::spawn(move || {
            let mut last = String::new();
            for line in BufReader::new(e).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line).trim().to_string();
                if line.starts_with("ERROR") {
                    last = line;
                }
            }
            last
        })
    });
    let mut file = None;
    if let Some(o) = stdout {
        for line in BufReader::new(o).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line).trim().to_string();
            match parse(&line) {
                Some(Line::Progress { done, total, speed, file }) => send(Up::Progress { done, total, speed, file }),
                Some(Line::File(f)) => file = Some(f),
                None => {}
            }
        }
    }
    let child = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    let status = child.map(|mut c| c.wait());
    let err = err_thread.and_then(|t| t.join().ok()).unwrap_or_default();
    match (file, status) {
        (Some(f), Some(Ok(s))) if s.success() => send(Up::Done(f)),
        (_, None) => {}
        _ => send(Up::Failed(if err.is_empty() { "yt-dlp gave up".into() } else { short(&err) })),
    }
}

pub fn stop(slot: &Slot) {
    if let Some(mut c) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

#[derive(Debug, PartialEq)]
enum Line {
    Progress { done: u64, total: u64, speed: u64, file: String },
    File(String),
}

fn parse(line: &str) -> Option<Line> {
    if let Some(f) = line.strip_prefix("LUMENFILE|") {
        return Some(Line::File(f.to_string()));
    }
    let rest = line.strip_prefix("LUMEN|")?;
    let mut p = rest.splitn(5, '|');
    let mut num = || p.next().and_then(|s| s.parse::<f64>().ok()).map(|f| f.max(0.0) as u64);
    let done = num().unwrap_or(0);
    let total = num();
    let estimate = num();
    let speed = num().unwrap_or(0);
    let file = p.next().unwrap_or("").to_string();
    Some(Line::Progress { done, total: total.or(estimate).unwrap_or(0), speed, file })
}

fn short(err: &str) -> String {
    let s = err.trim_start_matches("ERROR:").trim();
    let s = match s.strip_prefix('[').and_then(|r| r.split_once("] ")) {
        Some((_, r)) => r.split_once(": ").map(|(_, m)| m).unwrap_or(r),
        None => s,
    };
    s.chars().take(120).collect()
}

fn has_node() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|s| s.success())
}

fn verified(path: &Path, want: &str) -> bool {
    crate::aria::sha_of_file(path).as_deref() == Some(want)
}

fn fetch_verified(url: &str, want: &str, dest: &Path) -> Result<(), String> {
    let part = dest.with_extension("part");
    let _ = fs::remove_file(&part);
    let got = stream_to(url, &part)?;
    if got != want {
        let _ = fs::remove_file(&part);
        return Err("wrong SHA-256, refused".into());
    }
    fs::rename(&part, dest).map_err(|e| e.to_string())
}

fn stream_to(url: &str, to: &Path) -> Result<String, String> {
    let config = ureq::config::Config::builder().timeout_global(Some(Duration::from_secs(600))).build();
    let agent = ureq::Agent::new_with_config(config);
    let reply = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut body = reply.into_body().into_with_config().limit(MAX_FETCH).reader();
    let mut f = fs::File::create(to).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        f.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn place_ffmpeg(tools: &Path) -> Result<(), String> {
    let zip = tools.join("ffmpeg.zip");
    fetch_verified(FFMPEG_URL, FFMPEG_ZIP_SHA256, &zip)?;
    let tar = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join(r"System32\tar.exe");
    let ok = Command::new(tar)
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(tools)
        .args(["--strip-components", "2", FFMPEG_MEMBER])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|s| s.success());
    let _ = fs::remove_file(&zip);
    if !(ok && tools.join("ffmpeg.exe").exists()) {
        return Err("could not unpack".into());
    }
    if let Some(sha) = crate::aria::sha_of_file(&tools.join("ffmpeg.exe")) {
        let record = tools.join(FFMPEG_SHA_FILE);
        let tmp = record.with_extension("tmp");
        if fs::write(&tmp, sha).is_ok() {
            let _ = fs::rename(&tmp, &record);
        }
    }
    Ok(())
}

pub fn ytdlp_state(tools: &Path) -> Option<bool> {
    let sha = crate::aria::sha_of_file(&tools.join("yt-dlp.exe"))?;
    Some(sha == YTDLP_SHA256)
}

pub fn ffmpeg_state(tools: &Path) -> Option<bool> {
    let sha = crate::aria::sha_of_file(&tools.join("ffmpeg.exe"))?;
    let Ok(want) = fs::read_to_string(tools.join(FFMPEG_SHA_FILE)) else { return Some(true) };
    Some(want.trim().eq_ignore_ascii_case(&sha))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines_parse() {
        assert_eq!(
            parse("LUMEN|130048|309288|NA|48727965.57|out\\Me at the zoo [x].f140.m4a"),
            Some(Line::Progress { done: 130048, total: 309288, speed: 48727965, file: "out\\Me at the zoo [x].f140.m4a".into() })
        );
        assert_eq!(
            parse("LUMEN|10|NA|2000.0|NA|a|b.mp4"),
            Some(Line::Progress { done: 10, total: 2000, speed: 0, file: "a|b.mp4".into() })
        );
        assert_eq!(parse("LUMENFILE|C:\\x\\y.mp4"), Some(Line::File("C:\\x\\y.mp4".into())));
        assert_eq!(parse("[youtube] abc: Downloading webpage"), None);
    }

    #[test]
    fn errors_are_short() {
        assert_eq!(short("ERROR: [youtube] abc123: Video unavailable. This video is private"), "Video unavailable. This video is private");
        assert_eq!(short("ERROR: Unsupported URL: https://x.com/"), "Unsupported URL: https://x.com/");
    }
}
