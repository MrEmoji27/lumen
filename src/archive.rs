//! The archive: tabs nobody has looked at for a long time close themselves away,
//! and can be opened again from the history page.
//!
//! The rules are here, as plain functions on plain values: the window may be
//! closed, and the clock that counts it, so the main loop only has to ask.

use std::time::{Duration, Instant};

/// The settings' choices: "off" | "12h" | "1d" | "1w".
pub const CHOICES: [(&str, &str); 4] = [("off", "Off"), ("12h", "12 hours"), ("1d", "1 day"), ("1w", "1 week")];

/// The default: twelve hours.
pub const DEFAULT: &str = "12h";

/// How long a tab may go unseen before it is archived. None: never.
pub fn after(setting: &str) -> Option<Duration> {
    match setting {
        "12h" => Some(Duration::from_secs(12 * 3600)),
        "1d" => Some(Duration::from_secs(24 * 3600)),
        "1w" => Some(Duration::from_secs(7 * 24 * 3600)),
        _ => None,
    }
}

/// May this tab be closed into the archive? A pin, an essential, the tab on
/// screen, a tab with sound or in a call, an agent's tab and a private one all
/// stay where they are.
///
/// Lumen does not know whether the person has typed into a form on a page, so
/// such a tab is archived like any other. The rule is here, so it can be
/// switched on the day Lumen learns to tell.
pub fn may_archive(pinned: bool, essential: bool, on_screen: bool, sound: bool, call: bool, agent: bool, private: bool) -> bool {
    !(pinned || essential || on_screen || sound || call || agent || private)
}

/// How long a tab has been unseen: the time it has been idle, and the stretch
/// since it was last on screen. The time Lumen was closed does not count: the
/// clock only runs while it is open, so a tab is not archived because the
/// laptop slept over the weekend.
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
        // A tab with sound, or in a call, or an agent's, or private.
        assert!(!may_archive(false, false, false, true, false, false, false));
        assert!(!may_archive(false, false, false, false, true, false, false));
        assert!(!may_archive(false, false, false, false, false, true, false));
        assert!(!may_archive(false, false, false, false, false, false, true));
        // An ordinary tab that nobody is looking at may go.
        assert!(may_archive(false, false, false, false, false, false, false));
    }

    #[test]
    fn the_clock_counts_only_while_lumen_is_open() {
        let now = Instant::now();
        // Idle time carried over from the last run, and a stretch since.
        let t = idle(60_000, Some(now - Duration::from_secs(30)), now);
        assert_eq!(t, Duration::from_secs(90));
        // A tab that is on screen has no stretch: the time is what was carried.
        assert_eq!(idle(60_000, None, now), Duration::from_secs(60));
        // The time Lumen was closed is not in there: the clock starts again.
        assert_eq!(idle(0, Some(now), now), Duration::ZERO);
    }

    #[test]
    fn the_setting_says_how_long() {
        assert_eq!(after("12h"), Some(Duration::from_secs(12 * 3600)));
        assert_eq!(after("1d"), Some(Duration::from_secs(24 * 3600)));
        assert_eq!(after("1w"), Some(Duration::from_secs(7 * 24 * 3600)));
        assert_eq!(after("off"), None);
        // Anything else is off, not a surprise.
        assert_eq!(after("nonsense"), None);
    }
}
