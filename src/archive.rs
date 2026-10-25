//! The archive.

use std::time::{Duration, Instant};

pub const CHOICES: [(&str, &str); 4] = [("off", "Off"), ("12h", "12 hours"), ("1d", "1 day"), ("1w", "1 week")];

pub const DEFAULT: &str = "off";

pub fn after(setting: &str) -> Option<Duration> {
    match setting {
        "12h" => Some(Duration::from_secs(12 * 3600)),
        "1d" => Some(Duration::from_secs(24 * 3600)),
        "1w" => Some(Duration::from_secs(7 * 24 * 3600)),
        _ => None,
    }
}

pub fn may_archive(pinned: bool, essential: bool, on_screen: bool, sound: bool, call: bool, agent: bool, private: bool) -> bool {
    !(pinned || essential || on_screen || sound || call || agent || private)
}

pub fn idle(saved_ms: u64, since: Option<Instant>, now: Instant) -> Duration {
    let done = Duration::from_millis(saved_ms);
    match since {
        Some(at) => done + now.saturating_duration_since(at),
        None => done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pin_an_essential_and_the_tab_on_screen_stay() {
        assert!(!may_archive(true, false, false, false, false, false, false));
        assert!(!may_archive(false, true, false, false, false, false, false));
        assert!(!may_archive(false, false, true, false, false, false, false));
        assert!(!may_archive(false, false, false, true, false, false, false));
        assert!(!may_archive(false, false, false, false, true, false, false));
        assert!(!may_archive(false, false, false, false, false, true, false));
        assert!(!may_archive(false, false, false, false, false, false, true));
        assert!(may_archive(false, false, false, false, false, false, false));
    }

    #[test]
    fn the_clock_counts_only_while_lumen_is_open() {
        let now = Instant::now();
        let t = idle(60_000, Some(now - Duration::from_secs(30)), now);
        assert_eq!(t, Duration::from_secs(90));
        assert_eq!(idle(60_000, None, now), Duration::from_secs(60));
        assert_eq!(idle(0, Some(now), now), Duration::ZERO);
    }

    #[test]
    fn the_setting_says_how_long() {
        assert_eq!(after("12h"), Some(Duration::from_secs(12 * 3600)));
        assert_eq!(after("1d"), Some(Duration::from_secs(24 * 3600)));
        assert_eq!(after("1w"), Some(Duration::from_secs(7 * 24 * 3600)));
        assert_eq!(after("off"), None);
        assert_eq!(after("nonsense"), None);
    }
}
