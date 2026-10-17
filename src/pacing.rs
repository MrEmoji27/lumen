//! How often the event loop may wake itself, and how fast a page may change
//! what the chrome shows. Pure clock math: nothing here touches a window.

use std::time::{Duration, Instant};

/// The soonest the loop may ask to be woken again. A deadline already in the past
/// (a bug somewhere that forgot to move it) would otherwise be "wake now" forever:
/// a spin. With this floor the worst such bug costs 20 wakes a second, not 20 000.
pub const MIN_WAKE_GAP: Duration = Duration::from_millis(50);

/// A page may change its title this often and no faster. The chrome shows the
/// last one when the gap is over.
pub const TITLE_GAP: Duration = Duration::from_millis(250);

/// The deadline to hand the loop: `at`, but never sooner than `MIN_WAKE_GAP` from now.
pub fn clamp_wake(now: Instant, at: Instant) -> Instant {
    at.max(now + MIN_WAKE_GAP)
}

/// Lets one value through at most once per `gap` and keeps the newest of the
/// rest, so a page that changes something a hundred times a second is shown
/// at most four times, and always ends on what it last said.
pub struct Throttle<T> {
    /// When a value last got through.
    last: Option<Instant>,
    /// The newest value that had to wait.
    held: Option<T>,
}

impl<T> Default for Throttle<T> {
    fn default() -> Self {
        Throttle { last: None, held: None }
    }
}

impl<T> Throttle<T> {
    /// A new value. Some(v) means show it now; None means it waits for `due`.
    pub fn offer(&mut self, now: Instant, gap: Duration, v: T) -> Option<T> {
        if self.last.is_none_or(|l| now.saturating_duration_since(l) >= gap) {
            self.last = Some(now);
            self.held = None;
            Some(v)
        } else {
            self.held = Some(v);
            None
        }
    }

    /// When the held value may go. None when nothing waits.
    pub fn due(&self, gap: Duration) -> Option<Instant> {
        self.held.as_ref()?;
        self.last.map(|l| l + gap)
    }

    /// The held value, if its time has come.
    pub fn release(&mut self, now: Instant, gap: Duration) -> Option<T> {
        if self.due(gap).is_some_and(|d| d <= now) {
            self.last = Some(now);
            return self.held.take();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_past_deadline_cannot_spin_the_loop() {
        let now = Instant::now();
        let past = now - Duration::from_secs(5);
        assert_eq!(clamp_wake(now, past), now + MIN_WAKE_GAP);
        assert_eq!(clamp_wake(now, now), now + MIN_WAKE_GAP);
        // Even if the loop asks again at once, the next ask is still ahead of it.
        let later = now + Duration::from_millis(1);
        assert!(clamp_wake(later, past) > later);
    }

    #[test]
    fn a_far_deadline_is_left_alone() {
        let now = Instant::now();
        let at = now + Duration::from_secs(30);
        assert_eq!(clamp_wake(now, at), at);
    }

    #[test]
    fn throttle_passes_the_first_and_holds_the_burst() {
        let gap = Duration::from_millis(250);
        let t0 = Instant::now();
        let mut t = Throttle::default();
        assert_eq!(t.offer(t0, gap, "a"), Some("a"));
        assert_eq!(t.offer(t0 + Duration::from_millis(10), gap, "b"), None);
        assert_eq!(t.offer(t0 + Duration::from_millis(20), gap, "c"), None);
        // Nothing comes out early, and the newest is what comes out.
        assert_eq!(t.release(t0 + Duration::from_millis(100), gap), None);
        assert_eq!(t.due(gap), Some(t0 + gap));
        assert_eq!(t.release(t0 + gap, gap), Some("c"));
        assert_eq!(t.due(gap), None);
    }

    #[test]
    fn a_hundred_a_second_is_four_a_second() {
        let gap = Duration::from_millis(250);
        let t0 = Instant::now();
        let mut t = Throttle::default();
        let mut shown = 0;
        for ms in 0..1000u64 {
            let now = t0 + Duration::from_millis(ms);
            if t.offer(now, gap, ms).is_some() || t.release(now, gap).is_some() {
                shown += 1;
            }
        }
        assert!(shown <= 5, "{shown} shown");
    }

    #[test]
    fn a_slow_page_is_never_held() {
        let gap = Duration::from_millis(250);
        let t0 = Instant::now();
        let mut t = Throttle::default();
        assert_eq!(t.offer(t0, gap, 1), Some(1));
        assert_eq!(t.offer(t0 + Duration::from_secs(1), gap, 2), Some(2));
        assert_eq!(t.due(gap), None);
    }
}
