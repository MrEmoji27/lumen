//! Real site icons, by host, cached on disk. The redesign's tiles, pinned tabs,
//! History rows and Control table draw from here, so a site the person visited
//! has its icon the next time, loaded or not.
//!
//! Two doors fill the cache: a tab's favicon arrives as PNG (`remember`, free
//! from a visit already paid for), and `fetch` asks the site itself for hosts
//! that have none — no third-party icon service ever sees a host name.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::store::data_dir;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapEncoderNoCache, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeMedianCut,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IStream,
    STREAM_SEEK_SET, StructuredStorage::CreateStreamOnHGlobal,
};

/// One icon, as big as a tile ever needs. Bigger is a picture, not an icon.
const MAX_ICON: usize = 256 * 1024;
/// The cache never grows past this many files; the oldest go first.
const MAX_FILES: usize = 2000;
/// One host gets this long to answer, page and icon together.
const TIMEOUT: Duration = Duration::from_secs(5);
/// A host that failed once is left alone for a day.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);
/// Where failures are remembered, so a dead host is not hammered.
const FAILED: &str = "failed.json";
/// How much of a page is read looking for the `<link>` tags, which live in the head.
const MAX_HTML: u64 = 512 * 1024;
/// A browser-ish agent line: some sites serve their icon only to one.
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Lumen/0.5";
/// What every PNG on disk starts with.
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

fn dir() -> PathBuf {
    data_dir().join("favicons")
}

/// The site's icon as PNG bytes (32x32 or larger, square), from the cache, or None.
#[allow(dead_code)] // the integrator draws these later
pub fn get(host: &str) -> Option<Vec<u8>> {
    get_in(&dir(), host)
}

/// Put an icon in the cache (atomic write). Called when a tab's favicon PNG
/// arrives, so visited sites fill the cache for free.
pub fn remember(host: &str, png: &[u8]) {
    let d = dir();
    if remember_in(&d, host, png) {
        prune(&d, MAX_FILES);
    }
}

/// Icons for hosts that have none, on a background thread. The request goes to
/// the site itself: `https://<host>/` and its `<link rel=icon>`, else its
/// `/favicon.ico`. No third-party icon service is ever asked.
#[allow(dead_code)] // the integrator calls this once it has a list of hosts
pub fn fetch(hosts: Vec<String>) {
    let d = dir();
    let _ = thread::Builder::new()
        .name("lumen-favicons".into())
        .spawn(move || run_in(&d, hosts));
}

// ----------------------------------------------------------------- cache

fn get_in(dir: &Path, host: &str) -> Option<Vec<u8>> {
    let host = clean_host(host);
    if host.is_empty() {
        return None;
    }
    let bytes = fs::read(dir.join(format!("{host}.png"))).ok()?;
    // The cache holds icons only; a torn or swapped file is no icon.
    if bytes.len() > MAX_ICON || !bytes.starts_with(PNG_MAGIC) {
        return None;
    }
    Some(bytes)
}

/// One icon on disk, written tmp + rename so a crash never leaves half a file.
/// False when there is nothing to write: a bad host, or a picture, not a PNG.
fn remember_in(dir: &Path, host: &str, png: &[u8]) -> bool {
    let host = clean_host(host);
    if host.is_empty() || png.len() > MAX_ICON || !png.starts_with(PNG_MAGIC) {
        return false;
    }
    // The same bytes again are no change: no write, no prune.
    if get_in(dir, &host).as_deref() == Some(png) {
        return false;
    }
    if fs::create_dir_all(dir).is_err() {
        return false;
    }
    let path = dir.join(format!("{host}.png"));
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, png).is_ok() && fs::rename(&tmp, &path).is_ok()
}

/// The cache holds at most `cap` icons; the oldest go first.
fn prune(dir: &Path, cap: usize) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut icons: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            (path.extension().is_some_and(|x| x == "png")).then(|| {
                let m = e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                (m, path)
            })
        })
        .collect();
    if icons.len() <= cap {
        return;
    }
    icons.sort(); // oldest first
    let excess = icons.len() - cap;
    for (_, path) in icons.into_iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

/// The cache key: lowercased, no port, no `www.` — so `www.YouTube.com:443`
/// and `youtube.com` are one file. Tolerates a URL being passed in.
fn clean_host(host: &str) -> String {
    let h = host.trim();
    let h = h.split("://").nth(1).unwrap_or(h);
    let h = h.split(['/', '?', '#']).next().unwrap_or(h);
    let h = h.rsplit('@').next().unwrap_or(h);
    let h = h.split(':').next().unwrap_or(h); // no port
    let h = h.trim_end_matches('.').to_ascii_lowercase();
    let h = h.strip_prefix("www.").unwrap_or(&h);
    // A cache key is a file name: only host characters get in, and nothing
    // that is all dots ("..").
    if h.is_empty() || h.chars().all(|c| c == '.') {
        return String::new();
    }
    if !h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
        return String::new();
    }
    h.to_string()
}

// ------------------------------------------------------------------ fetch

/// The work `fetch` does, blocking. One thread, batches of four hosts at a time.
fn run_in(dir: &Path, hosts: Vec<String>) {
    let _ = fs::create_dir_all(dir);
    let mut failed = load_failed(dir);
    let now = secs_now();
    // Failures older than a day are retried; the stale lines go with the save.
    failed.retain(|_, at| now.saturating_sub(*at) < DAY.as_secs());
    let mut todo: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for h in hosts {
        let h = clean_host(&h);
        if h.is_empty() || !seen.insert(h.clone()) {
            continue;
        }
        if get_in(dir, &h).is_some() || failed.contains_key(&h) {
            continue;
        }
        todo.push(h);
    }
    let mut misses: Vec<String> = Vec::new();
    for batch in todo.chunks(4) {
        let hits: Vec<String> = thread::scope(|s| {
            let workers: Vec<_> = batch.iter().map(|h| s.spawn(move || fetch_one(dir, h))).collect();
            workers.into_iter().filter_map(|w| w.join().ok().flatten()).collect()
        });
        misses.extend(hits);
    }
    if !misses.is_empty() {
        let now = secs_now();
        for h in &misses {
            failed.insert(h.clone(), now);
        }
        save_failed(dir, &failed);
    }
    prune(dir, MAX_FILES);
}

/// One host, one try: Some(host) when nothing could be remembered for it.
fn fetch_one(dir: &Path, host: &str) -> Option<String> {
    // WIC wants COM set up on the thread that uses it.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let missed = !one_host(dir, host);
    if com.is_ok() {
        unsafe {
            let _ = CoUninitialize();
        }
    }
    missed.then(|| host.to_string())
}

/// One host's icon onto disk. True when it landed.
fn one_host(dir: &Path, host: &str) -> bool {
    // One budget for the page and the icon together, as the brief counts it.
    let deadline = Instant::now() + TIMEOUT;
    let html = get_url(&format!("https://{host}/"), deadline, MAX_HTML).unwrap_or_default();
    let html = String::from_utf8_lossy(&html);
    // The page's own icon link, else the old faithful /favicon.ico.
    let url = pick_icon(&html)
        .and_then(|href| icon_url(host, &href))
        .unwrap_or_else(|| format!("https://{host}/favicon.ico"));
    let Some(bytes) = get_url(&url, deadline, MAX_ICON as u64) else { return false };
    let Some(png) = to_png(&bytes) else { return false };
    remember_in(dir, host, &png)
}

/// One GET, capped, with whatever is left of the deadline. A non-200 is a miss.
fn get_url(url: &str, deadline: Instant, limit: u64) -> Option<Vec<u8>> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return None;
    }
    let config = ureq::config::Config::builder()
        .timeout_global(Some(left))
        // A missing /favicon.ico is a normal miss, not a crash.
        .http_status_as_error(false)
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let reply = agent.get(url).header("User-Agent", UA).call().ok()?;
    if reply.status().as_u16() != 200 {
        return None;
    }
    let body = reply.into_body().into_with_config().limit(limit).read_to_vec().ok()?;
    (!body.is_empty()).then_some(body)
}

fn secs_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn load_failed(dir: &Path) -> BTreeMap<String, u64> {
    fs::read_to_string(dir.join(FAILED)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Failures to disk, tmp + rename, same as an icon.
fn save_failed(dir: &Path, failed: &BTreeMap<String, u64>) {
    let path = dir.join(FAILED);
    let tmp = path.with_extension("tmp");
    if let Ok(json) = serde_json::to_string(failed)
        && fs::write(&tmp, json).is_ok()
    {
        let _ = fs::rename(&tmp, &path);
    }
}

// ------------------------------------------------------------ link picking

/// The page's best `<link rel=icon>` href: apple-touch-icon first, then the
/// largest declared icon. SVG is left out; WIC cannot draw it.
fn pick_icon(html: &str) -> Option<String> {
    let mut apple: Option<(u32, String)> = None;
    let mut icon: Option<(u32, String)> = None;
    for tag in link_tags(html) {
        let rel = attr(&tag, "rel").to_ascii_lowercase();
        let tokens: Vec<&str> = rel.split_whitespace().collect();
        let is_apple = tokens.iter().any(|t| *t == "apple-touch-icon" || *t == "apple-touch-icon-precomposed");
        let is_icon = !is_apple && tokens.iter().any(|t| *t == "icon");
        if !is_apple && !is_icon {
            continue;
        }
        let href = attr(&tag, "href");
        let typ = attr(&tag, "type").to_ascii_lowercase();
        if href.is_empty() || typ.contains("svg") || href.to_ascii_lowercase().ends_with(".svg") {
            continue;
        }
        let px = sizes_px(&attr(&tag, "sizes"));
        let slot = if is_apple { &mut apple } else { &mut icon };
        if slot.as_ref().is_none_or(|(p, _)| px > *p) {
            *slot = Some((px, href));
        }
    }
    apple.or(icon).map(|(_, href)| href)
}

/// Every `<link …>` tag's text. The search is case-blind, the slice is the
/// page's own bytes, so an href keeps its case.
fn link_tags(html: &str) -> Vec<String> {
    let low = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(start) = low[at..].find("<link") {
        let start = at + start;
        // A '>' inside a quoted value ends the tag early; link tags rarely
        // carry one, and a miss costs one bad href, not a crash.
        let end = low[start..].find('>').map(|e| start + e).unwrap_or(html.len());
        out.push(html[start..end].to_string());
        at = end.max(start + 5);
    }
    out
}

/// One attribute's value from a tag's text, or empty. `rel='x'`, `rel=x`,
/// `REL="x"` all read. A whole word only: `data-icon=` is not `icon=`.
fn attr(tag: &str, name: &str) -> String {
    let low = tag.to_ascii_lowercase();
    let needle = format!("{name}=");
    let mut at = 0;
    while let Some(hit) = low[at..].find(&needle) {
        let hit = at + hit;
        let whole = hit == 0 || tag[..hit].chars().last().is_some_and(|c| c.is_whitespace() || c == '<');
        if whole {
            let rest = tag[hit + needle.len()..].trim_start();
            let Some(q) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                return rest.split_whitespace().next().unwrap_or("").trim_end_matches('>').to_string();
            };
            let after = &rest[q.len_utf8()..];
            let end = after.find(q).unwrap_or(after.len());
            return after[..end].to_string();
        }
        at = hit + needle.len();
    }
    String::new()
}

/// The largest edge any listed size declares: "16x16 32x32" is 32. No sizes
/// reads as 16 (the classic favicon), so a lone /favicon.ico still competes.
fn sizes_px(sizes: &str) -> u32 {
    let mut best = if sizes.trim().is_empty() { 16 } else { 0 };
    for part in sizes.split_whitespace() {
        let part = part.to_ascii_lowercase();
        let (w, h) = part.split_once('x').unwrap_or((&part, "0"));
        best = best.max(w.parse().unwrap_or(0)).max(h.parse().unwrap_or(0));
    }
    best
}

/// A link tag's href as a URL to fetch. Only http(s) goes out the door.
fn icon_url(host: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    if href.starts_with("http://") || href.starts_with("https://") {
        return Some(href.to_string());
    }
    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if href.starts_with("data:") || href.starts_with("javascript:") {
        return None;
    }
    Some(format!("https://{host}/{}", href.trim_start_matches('/')))
}

// ------------------------------------------------------------------- WIC

/// Any image Windows can decode (PNG, JPEG, GIF, BMP, ICO, TIFF…) as PNG
/// bytes. An ICO carries several sizes; the largest frame wins. A small icon
/// is scaled up once here, so the cache always holds 32 or more.
fn to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.is_empty() {
        return None;
    }
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let src = wic.CreateStream().ok()?;
        src.InitializeFromMemory(bytes).ok()?;
        let dec = wic.CreateDecoderFromStream(&src, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let count = dec.GetFrameCount().ok()?;
        let mut best = dec.GetFrame(0).ok()?;
        let (mut bw, mut bh) = (0u32, 0u32);
        best.GetSize(&mut bw, &mut bh).ok()?;
        for i in 1..count {
            if let Ok(frame) = dec.GetFrame(i) {
                let (mut w, mut h) = (0u32, 0u32);
                if frame.GetSize(&mut w, &mut h).is_ok() && w * h > bw * bh {
                    best = frame;
                    (bw, bh) = (w, h);
                }
            }
        }
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&best, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)
            .ok()?;
        if bw < 32 || bh < 32 {
            let k = 32.0 / bw.min(bh).max(1) as f32;
            let scaler = wic.CreateBitmapScaler().ok()?;
            scaler
                .Initialize(&conv, ((bw as f32 * k) as u32).max(32), ((bh as f32 * k) as u32).max(32), WICBitmapInterpolationModeFant)
                .ok()?;
            encode_png(&wic, &scaler)
        } else {
            encode_png(&wic, &conv)
        }
    }
}

/// One bitmap, encoded as PNG in memory.
fn encode_png(wic: &IWICImagingFactory, source: &IWICBitmapSource) -> Option<Vec<u8>> {
    unsafe {
        let out: IStream = CreateStreamOnHGlobal(HGLOBAL::default(), true).ok()?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).ok()?;
        enc.Initialize(&out, WICBitmapEncoderNoCache).ok()?;
        let mut frame = None;
        enc.CreateNewFrame(&mut frame, std::ptr::null_mut()).ok()?;
        let frame = frame?;
        frame.Initialize(None).ok()?;
        frame.WriteSource(source, std::ptr::null()).ok()?;
        frame.Commit().ok()?;
        enc.Commit().ok()?;
        // The stream holds the PNG: back to the start, then read it all out.
        out.Seek(0, STREAM_SEEK_SET, None).ok()?;
        let mut data = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let mut n = 0u32;
            if out.Read(buf.as_mut_ptr() as _, buf.len() as u32, Some(&mut n)).is_err() || n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n as usize]);
            if data.len() > MAX_ICON {
                return None;
            }
        }
        (data.starts_with(PNG_MAGIC)).then_some(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Imaging::WICBitmapCacheOnDemand;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumen-favicon-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("temp dir");
        d
    }

    /// Bytes that pass for a PNG in the cache: the magic, then nothing that matters.
    fn fake_png() -> Vec<u8> {
        let mut v = PNG_MAGIC.to_vec();
        v.extend_from_slice(b"not really");
        v
    }

    #[test]
    fn hosts_come_out_clean() {
        assert_eq!(clean_host("WWW.Example.COM:443"), "example.com");
        assert_eq!(clean_host("https://www.Example.com/path?q=1"), "example.com");
        assert_eq!(clean_host("youtube.com."), "youtube.com");
        assert_eq!(clean_host(" user@Host.Example "), "host.example");
        assert_eq!(clean_host("a/b"), "a");
        // A space in the host itself is no host.
        assert_eq!(clean_host("bad host.com"), "");
        // Nothing that could name a file but a host.
        assert_eq!(clean_host(""), "");
        assert_eq!(clean_host("../evil"), "");
        assert_eq!(clean_host("..."), "");
    }

    #[test]
    fn the_icon_picker_prefers_apple_then_the_largest() {
        assert_eq!(pick_icon(r#"<link rel="icon" href="/a.ico">"#), Some("/a.ico".into()));
        // Largest declared sizes wins.
        let html = r#"<link rel="icon" href="/16.png" sizes="16x16"><link rel="icon" href="/32.png" sizes="32x32">"#;
        assert_eq!(pick_icon(html), Some("/32.png".into()));
        // apple-touch-icon beats any plain icon, whatever its size.
        let html = r#"<link rel="icon" href="/512.png" sizes="512x512"><link rel="apple-touch-icon" href="/180.png" sizes="180x180">"#;
        assert_eq!(pick_icon(html), Some("/180.png".into()));
        // SVG is left out; the PNG behind it still counts.
        let html = r#"<link rel="icon" type="image/svg+xml" href="/s.svg"><link rel="icon" href="/p.png">"#;
        assert_eq!(pick_icon(html), Some("/p.png".into()));
        // mask-icon is not an icon tag.
        assert_eq!(pick_icon(r##"<link rel="mask-icon" href="/m.svg" color="#000">"##), None);
        // Uppercase, single quotes, unquoted values, shortcut icon.
        assert_eq!(pick_icon(r#"<LINK REL='ICON' HREF="/x.png">"#), Some("/x.png".into()));
        assert_eq!(pick_icon("<link rel=icon href=/y.png>"), Some("/y.png".into()));
        assert_eq!(pick_icon(r#"<link rel="shortcut icon" href="/z.ico">"#), Some("/z.ico".into()));
        // data-icon= is not icon=.
        assert_eq!(pick_icon(r#"<link rel="apple-touch-icon" data-icon="q" href="/t.png">"#), Some("/t.png".into()));
        assert_eq!(pick_icon("<html><body>hi</body></html>"), None);
    }

    #[test]
    fn hrefs_become_urls_only_for_http() {
        assert_eq!(icon_url("example.com", "https://cdn.example/i.png"), Some("https://cdn.example/i.png".into()));
        assert_eq!(icon_url("example.com", "//cdn.example/i.png"), Some("https://cdn.example/i.png".into()));
        assert_eq!(icon_url("example.com", "/i.png"), Some("https://example.com/i.png".into()));
        assert_eq!(icon_url("example.com", "i.png"), Some("https://example.com/i.png".into()));
        assert_eq!(icon_url("example.com", "data:image/png;base64,AAAA"), None);
        assert_eq!(icon_url("example.com", ""), None);
    }

    /// A real 16x16 ICO (PNG-compressed, the form Windows writes since Vista)
    /// goes in; a PNG comes out, scaled up to 32.
    #[test]
    fn an_ico_decodes_to_a_png() {
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        unsafe {
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).expect("wic");
            let bmp = wic.CreateBitmap(16, 16, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand).expect("bitmap");
            let png = encode_png(&wic, &bmp).expect("png out of wic");
            let ico = wrap_in_ico(&png, 16);
            let out = to_png(&ico).expect("ico decodes");
            assert!(out.starts_with(PNG_MAGIC), "not a PNG");
            // Back through WIC once more: the size that came out is the size that survived.
            let src = wic.CreateStream().expect("stream");
            src.InitializeFromMemory(&out).expect("stream memory");
            let dec = wic.CreateDecoderFromStream(&src, std::ptr::null(), WICDecodeMetadataCacheOnDemand).expect("decoder");
            let frame = dec.GetFrame(0).expect("frame");
            let (mut w, mut h) = (0u32, 0u32);
            frame.GetSize(&mut w, &mut h).expect("size");
            assert_eq!((w, h), (32, 32), "16x16 upscaled to the tile size");
        }
        if com.is_ok() {
            unsafe {
                let _ = CoUninitialize();
            }
        }
    }

    /// An ICO file with one PNG-compressed entry: icon header, one directory
    /// entry, then the PNG bytes themselves.
    fn wrap_in_ico(png: &[u8], px: u8) -> Vec<u8> {
        let mut ico = Vec::new();
        ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]); // reserved, type=icon, one image
        ico.extend_from_slice(&[px, px, 0, 0]); // width, height, colors, reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bit count
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes()); // straight after the headers
        ico.extend_from_slice(png);
        ico
    }

    #[test]
    fn the_cache_fills_and_stays_capped() {
        let d = tmp_dir("cap");
        let png = fake_png();
        for n in 0..8 {
            assert!(remember_in(&d, &format!("s{n}.test"), &png));
        }
        // The same icon again is no change.
        assert!(!remember_in(&d, "s3.test", &png));
        prune(&d, 3);
        assert_eq!(fs::read_dir(&d).expect("dir").count(), 3);
        assert!(get_in(&d, "s0.test").is_none(), "the oldest went");
        assert!(get_in(&d, "s7.test").is_some(), "the newest stayed");
        assert!(get_in(&d, "never.test").is_none());
        // A bad host or a picture that is not a PNG never lands.
        assert!(!remember_in(&d, "", &png));
        assert!(!remember_in(&d, "ok.test", b"JFIF not png"));
        assert!(!remember_in(&d, "ok.test", &vec![0u8; MAX_ICON + 1]));
    }

    /// The real thing: the brief's two hosts, the whole fetch path, what landed.
    #[test]
    #[ignore = "hits the network; run with --ignored --nocapture"]
    fn live_fetch_lands_real_icons() {
        let d = tmp_dir("live");
        run_in(&d, vec!["example.com".into(), "github.com".into()]);
        for host in ["example.com", "github.com"] {
            match get_in(&d, host) {
                Some(png) => eprintln!("live: {host}: {} bytes, png={}", png.len(), png.starts_with(PNG_MAGIC)),
                None => eprintln!("live: {host}: nothing landed"),
            }
        }
        // example.com ships a deliberately empty icon (href=data:,) and 404s
        // /favicon.ico: nothing lands there, and that miss is correct.
        assert!(get_in(&d, "github.com").is_some(), "github.com got no icon");
    }
}
