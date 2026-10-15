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

/// The categories, as the query parameter wants them: a JSON array.
const CATEGORIES_QUERY: &str = "%5B%22sponsor%22%2C%22selfpromo%22%2C%22interaction%22%5D";

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
/// The server returns every video sharing the hash prefix, each with its own
/// `segments` list; we keep only ours and only the categories we skip.
pub fn parse_segments(raw: &str, video_id: &str) -> Vec<Segment> {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(videos) = json.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for v in videos {
        if v["videoID"].as_str() != Some(video_id) {
            continue;
        }
        let Some(segs) = v["segments"].as_array() else {
            continue;
        };
        for s in segs {
            // intro, outro, music_offtopic and the rest are left alone.
            let category = s["category"].as_str().unwrap_or("");
            if !CATEGORIES.contains(&category) {
                continue;
            }
            let Some(times) = s["segment"].as_array() else {
                continue;
            };
            let (Some(start), Some(end)) = (times.first().and_then(|t| t.as_f64()), times.get(1).and_then(|t| t.as_f64())) else {
                continue;
            };
            out.push(Segment {
                start,
                end,
                category: category.to_string(),
            });
        }
    }
    out
}

/// Fetch segment data for a video. Empty on any failure.
pub fn fetch_segments(video_id: &str) -> Vec<Segment> {
    let url = format!("{}{}?categories={}", API_BASE, prefix(video_id), CATEGORIES_QUERY);
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
        // A known answer: sha256("dQw4w9WgXcQ") starts 5f6b. The server sees
        // these four characters and never the video ID itself.
        assert_eq!(prefix("dQw4w9WgXcQ"), "5f6b");
        assert_eq!(prefix("dQw4w9WgXcQ").len(), 4);
        assert!(prefix("dQw4w9WgXcQ").chars().all(|c| c.is_ascii_hexdigit()));
        // Different IDs -> different prefixes (overwhelmingly likely).
        assert_ne!(prefix("dQw4w9WgXcQ"), prefix("abcdefghijk"));
    }

    /// The real shape of a hashed-prefix reply, captured from the API and cut
    /// down: one object per video that shares the prefix, each with its own
    /// segments, each segment with one category name.
    const REPLY: &str = r#"[
      {"videoID":"aQp-iM0bCs4","segments":[
        {"category":"sponsor","actionType":"skip","segment":[0,31.861],"UUID":"b7","votes":0},
        {"category":"intro","actionType":"skip","segment":[31.861,40.0],"UUID":"c1","votes":3},
        {"category":"selfpromo","actionType":"skip","segment":[45.5,60.25],"UUID":"d2","votes":2},
        {"category":"interaction","actionType":"skip","segment":[70.0,74.5],"UUID":"e3","votes":1},
        {"category":"music_offtopic","actionType":"skip","segment":[80.0,90.0],"UUID":"f4","votes":0}
      ]},
      {"videoID":"thODYqd9G9U","segments":[
        {"category":"sponsor","actionType":"skip","segment":[0.332106,22.484015],"UUID":"a1","votes":1}
      ]}
    ]"#;

    #[test]
    fn segments_filter_by_category_and_video() {
        let segs = parse_segments(REPLY, "aQp-iM0bCs4");
        // Three kept (sponsor, selfpromo, interaction), intro and
        // music_offtopic dropped, and nothing from the other video.
        assert_eq!(segs.len(), 3);
        assert_eq!((segs[0].start, segs[0].end), (0.0, 31.861));
        assert_eq!(segs[0].category, "sponsor");
        assert_eq!(segs[1].category, "selfpromo");
        assert_eq!(segs[2].category, "interaction");
        // The other video in the same bucket is parsed on its own.
        assert_eq!(parse_segments(REPLY, "thODYqd9G9U").len(), 1);
        // A video with no segments of ours gets nothing.
        assert!(parse_segments(REPLY, "dQw4w9WgXcQ").is_empty());
    }

    #[test]
    fn segments_tolerate_garbage() {
        assert!(parse_segments("not json", "x").is_empty());
        assert!(parse_segments("[]", "x").is_empty());
        assert!(parse_segments(r#"[{"videoID":"x"}]"#, "x").is_empty());
        assert!(parse_segments(r#"[{"videoID":"x","segments":[{"category":"sponsor"}]}]"#, "x").is_empty());
        assert!(parse_segments(r#"[{"videoID":"x","segments":[{"segment":[1]}]}]"#, "x").is_empty());
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

/// Against the real API. `cargo test -- --ignored` when the network is there:
/// it is the only way to see that the endpoint and its shape still hold.
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore = "talks to sponsor.ajay.app"]
    fn the_api_answers_for_a_video_with_segments() {
        let segs = fetch_segments("aQp-iM0bCs4");
        println!("{segs:#?}");
        assert!(!segs.is_empty(), "the prefix hash, the URL or the reply shape changed");
        assert!(segs.iter().all(|s| CATEGORIES.contains(&s.category.as_str())));
    }
}
