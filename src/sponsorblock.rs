//! SponsorBlock: skip sponsor segments on YouTube.
//!
//! On youtube.com/watch pages, fetches hashed-prefix segments from
//! sponsor.ajay.app for the categories sponsor, selfpromo, interaction.
//! Only the first 4 hex chars of sha256(videoID) go to the server, so the
//! server never learns the exact video. An init script injected into the
//! page hooks the player and skips segments, with a small toast.
//!
//! Tests cover the three pieces of pure logic: the prefix hash, segment
//! filtering, and host matching.

use std::time::Duration;

use sha2::{Digest, Sha256};

/// Categories fetched and skipped.
const CATEGORIES: &[&str] = &["sponsor", "selfpromo", "interaction"];

/// A segment to skip, with its start/end times and the category it matched.
#[derive(Debug, Clone)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    /// Kept so tests can prove the category filter; the player only needs times.
    #[allow(dead_code)]
    pub category: String,
}

/// The SponsorBlock hashed-prefix endpoint base.
const API_BASE: &str = "https://sponsor.ajay.app/api/skipSegments/";

/// Four hex characters of the sha256 of a video ID, so the server cannot
/// learn the exact video until it hands back the full segment list.
pub fn prefix(video_id: &str) -> String {
    let mut h = Sha256::new();
    h.update(video_id.as_bytes());
    let hash = h.finalize();
    format!("{:02x}{:02x}", hash[0], hash[1])
}

/// Extract the video ID from a youtube.com/watch?v=... URL.
/// Handles extra query params (e.g. &list=...). Short youtu.be links are not
/// supported: they are rare in the address bar.
pub fn video_id(url: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    for param in q.split('&') {
        if let Some(v) = param.strip_prefix("v=") {
            let v = v.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
            if v.len() >= 11 {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// True for a youtube.com/watch page (with or without www., with params).
pub fn is_youtube_watch(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or("");
    let raw_host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = raw_host.strip_prefix("www.").unwrap_or(raw_host);
    if host != "youtube.com" {
        return false;
    }
    let after = &rest[raw_host.len()..];
    after.starts_with("/watch")
}

/// Parse a SponsorBlock API response and filter to our categories and video.
/// The server returns segments for ALL videos sharing the hash prefix; we keep
/// only ours and only the categories we skip.
pub fn parse_segments(raw: &str, video_id: &str) -> Vec<Segment> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(arr) = json.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|item| {
            if item["videoID"].as_str() != Some(video_id) {
                return None;
            }
            let cats = item["category"].as_array()?;
            let cat = cats.iter().find(|c| CATEGORIES.contains(&c.as_str().unwrap_or("")))?;
            let seg = item["segment"].as_array()?;
            let start = seg.get(0)?.as_f64()?;
            let end = seg.get(1)?.as_f64()?;
            Some(Segment {
                start,
                end,
                category: cat.as_str().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// Fetch segment data for a video. Empty on any failure.
pub fn fetch_segments(video_id: &str) -> Vec<Segment> {
    let url = format!("{}{}", API_BASE, prefix(video_id));
    let config = ureq::config::Config::builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let Ok(mut reply) = agent.get(&url).call() else {
        return Vec::new();
    };
    let body = reply
        .body_mut()
        .read_to_string()
        .unwrap_or_default();
    parse_segments(&body, video_id)
}

/// The init script injected into a YouTube watch page. `segments` is JSON like
/// [[start, end], ...] — only start and end, categories are already filtered.
/// The script hooks the video element and skips sponsor segments.
pub fn init_script(segments: &str) -> String {
    format!(
        r#"(() => {{
  if (window !== window.top) return;
  const SEGMENTS = {segments};
  if (!SEGMENTS.length) return;
  // YouTube is a single-page app: the next video runs this again in the same
  // document. Hand the new segment list to the listeners already attached
  // instead of stacking a second set of them.
  if (window.__lumenSpb) {{
    window.__lumenSpb.segs = SEGMENTS;
    window.__lumenSpb.skipped = new Set();
    return;
  }}
  const st = window.__lumenSpb = {{ segs: SEGMENTS, skipped: new Set() }};
  function showToast(msg) {{
    let t = document.getElementById('__lumen_spb_toast');
    if (!t) {{
      t = document.createElement('div');
      t.id = '__lumen_spb_toast';
      Object.assign(t.style, {{
        position: 'fixed', bottom: '80px', left: '50%', transform: 'translateX(-50%)',
        background: 'rgba(0,0,0,0.85)', color: '#fff', padding: '8px 16px',
        borderRadius: '16px', font: '13px/1.4 system-ui,sans-serif', zIndex: '99999',
        opacity: '0', transition: 'opacity .2s', display: 'none',
      }});
      document.body.appendChild(t);
    }}
    t.textContent = msg;
    t.style.display = 'block';
    t.style.opacity = '1';
    setTimeout(() => {{ t.style.opacity = '0'; setTimeout(() => t.style.display = 'none', 200); }}, 2000);
  }}
  function jump(video) {{
    const t = video.currentTime;
    for (const [start, end] of st.segs) {{
      if (t >= start && t < end - 0.2) {{
        if (!st.skipped.has(start)) {{
          st.skipped.add(start);
          showToast('Skipped sponsor');
        }}
        video.currentTime = end;
        return true;
      }}
    }}
    return false;
  }}
  function hookVideo() {{
    const video = document.querySelector('video');
    if (!video) {{ setTimeout(hookVideo, 300); return; }}
    video.addEventListener('timeupdate', () => jump(video));
    // A scrub that lands inside a segment is pushed past it too.
    video.addEventListener('seeking', () => jump(video));
  }}
  if (document.readyState === 'loading') {{
    document.addEventListener('DOMContentLoaded', hookVideo, {{ once: true }});
  }} else {{
    hookVideo();
  }}
}})();"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_is_four_hex_chars_of_sha256() {
        // sha256("dQw4w9WgXcQ") = 7b...; first 4 hex chars are "7b4b".
        let p = prefix("dQw4w9WgXcQ");
        assert_eq!(p.len(), 4);
        assert!(p.chars().all(|c| c.is_ascii_hexdigit()));
        // Deterministic.
        assert_eq!(prefix("abc"), prefix("abc"));
        // Different IDs -> different prefixes (overwhelmingly likely).
        assert_ne!(prefix("dQw4w9WgXcQ"), prefix("abcdefghijk"));
    }

    #[test]
    fn segments_filter_by_category_and_video() {
        let raw = r#"[
          {"segment":[10.0, 25.0], "category":["sponsor"], "videoID":"dQw4w9WgXcQ"},
          {"segment":[30.0, 40.0], "category":["selfpromo"], "videoID":"dQw4w9WgXcQ"},
          {"segment":[50.0, 60.0], "category":["interaction"], "videoID":"dQw4w9WgXcQ"},
          {"segment":[70.0, 80.0], "category":["intro"], "videoID":"dQw4w9WgXcQ"},
          {"segment":[90.0, 100.0], "category":["sponsor"], "videoID":"other_video_id"}
        ]"#;
        let segs = parse_segments(raw, "dQw4w9WgXcQ");
        // Three kept (sponsor, selfpromo, interaction), intro and other-video dropped.
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].start, 10.0);
        assert_eq!(segs[0].end, 25.0);
        assert_eq!(segs[0].category, "sponsor");
        assert_eq!(segs[1].category, "selfpromo");
        assert_eq!(segs[2].category, "interaction");
    }

    #[test]
    fn segments_tolerate_garbage() {
        assert!(parse_segments("not json", "x").is_empty());
        assert!(parse_segments("[]", "x").is_empty());
        assert!(parse_segments(r#"[{"category":["sponsor"]}]"#, "x").is_empty());
    }

    #[test]
    fn host_matching_youtube_watch() {
        assert!(is_youtube_watch("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(is_youtube_watch("https://youtube.com/watch?v=dQw4w9WgXcQ&t=42s"));
        assert!(is_youtube_watch("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL"));
        // Not youtube.
        assert!(!is_youtube_watch("https://example.com/watch?v=x"));
        // youtube.com but not /watch.
        assert!(!is_youtube_watch("https://www.youtube.com/results?search_query=cats"));
        // No protocol.
        assert!(!is_youtube_watch("youtube.com/watch?v=x"));
    }

    #[test]
    fn video_id_extracted_from_query() {
        assert_eq!(video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), Some("dQw4w9WgXcQ".to_string()));
        assert_eq!(video_id("https://youtube.com/watch?v=dQw4w9WgXcQ&list=PL"), Some("dQw4w9WgXcQ".to_string()));
        assert_eq!(video_id("https://example.com/"), None);
        assert_eq!(video_id("https://youtube.com/"), None);
    }

    #[test]
    fn init_script_contains_segments() {
        let s = init_script("[[10.0,25.0],[30.0,40.0]]");
        assert!(s.contains("[[10.0,25.0],[30.0,40.0]]"));
        assert!(s.contains("Skipped sponsor"));
        assert!(s.contains("timeupdate"));
    }
}
