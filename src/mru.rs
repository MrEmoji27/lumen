//! Where the person was: tab ids in the order they were last active.
//!
//! Closing a tab should go back to the tab the person was on before it, like
//! every other browser, not to whichever tab the row happens to put next. The
//! list is the whole rule, kept as plain values; the window and the tabs live
//! in main.rs.

use std::collections::VecDeque;

/// Ids remembered, newest first. Past this the oldest fall off the end; only
/// 64 switches without a close can push one out.
const CAP: usize = 64;

#[derive(Default)]
pub struct Mru {
    ids: VecDeque<u64>,
}

impl Mru {
    pub fn new() -> Self {
        Self::default()
    }

    /// A tab became the active one: it is now the newest. Touching a tab
    /// already here moves it to the front.
    pub fn touch(&mut self, id: u64) {
        if let Some(at) = self.ids.iter().position(|&x| x == id) {
            self.ids.remove(at);
        }
        self.ids.push_front(id);
        self.ids.truncate(CAP);
    }

    /// A tab is gone: forget it, wherever it sits.
    pub fn remove(&mut self, id: u64) {
        self.ids.retain(|&x| x != id);
    }

    /// The most recent tab that still exists (`alive`), other than the one
    /// being closed. None: nothing to go back to, and the caller decides.
    pub fn back(&self, closing: u64, alive: impl Fn(u64) -> bool) -> Option<u64> {
        self.ids.iter().copied().find(|&id| id != closing && alive(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole list, newest first: what the tests measure.
    fn order(m: &Mru) -> Vec<u64> {
        m.ids.iter().copied().collect()
    }

    #[test]
    fn touch_puts_the_newest_first() {
        let mut m = Mru::new();
        m.touch(1);
        m.touch(2);
        m.touch(3);
        assert_eq!(order(&m), vec![3, 2, 1]);
        assert_eq!(m.back(3, |_| true), Some(2));
        // Going back to a tab makes it the newest: the one after it comes next.
        m.touch(1);
        assert_eq!(order(&m), vec![1, 3, 2]);
        assert_eq!(m.back(3, |_| true), Some(1));
        // Touching the same tab again changes nothing.
        m.touch(1);
        assert_eq!(order(&m), vec![1, 3, 2]);
    }

    #[test]
    fn a_closed_tab_is_forgotten() {
        let mut m = Mru::new();
        m.touch(1);
        m.touch(2);
        m.touch(3);
        m.remove(2);
        assert_eq!(order(&m), vec![3, 1]);
        assert_eq!(m.back(3, |_| true), Some(1));
        // Removing an id that is not there is not a problem.
        m.remove(99);
        assert_eq!(order(&m), vec![3, 1]);
    }

    /// A tab can leave without a `remove` (another workspace, a discarded page
    /// that never came back). `alive` is what answers for it.
    #[test]
    fn a_tab_that_is_gone_is_skipped() {
        let mut m = Mru::new();
        m.touch(1);
        m.touch(2);
        m.touch(3);
        assert_eq!(m.back(3, |id| id != 2), Some(1), "2 is gone, 1 is next in line");
        // Nothing alive: no answer, and the caller falls back.
        assert_eq!(m.back(3, |_| false), None);
        // The closing tab is never the answer, even if it still looks alive.
        assert_eq!(m.back(3, |_| true), Some(2));
    }

    /// A pin is an ordinary id here: never preferred, never punished. It is
    /// the answer only when it really was the last one used.
    #[test]
    fn a_pinned_tab_is_chosen_only_when_it_really_was_the_last_one_used() {
        let mut m = Mru::new();
        m.touch(1);
        m.touch(7); // the pin
        m.touch(4);
        m.touch(9);
        assert_eq!(m.back(9, |_| true), Some(4), "a tab used after the pin wins");
        // The pin was the last one used after all.
        m.touch(7);
        assert_eq!(m.back(9, |_| true), Some(7));
    }

    #[test]
    fn the_list_stops_at_sixty_four() {
        let mut m = Mru::new();
        for id in 1..=100 {
            m.touch(id);
        }
        assert_eq!(m.ids.len(), CAP);
        assert_eq!(m.back(100, |_| true), Some(99));
        // The oldest fell off the end; without a close this needs 64 switches.
        assert!(!order(&m).contains(&36));
        assert!(order(&m).contains(&37));
        // Touching an id already here at the cap stays at the cap.
        m.touch(37);
        assert_eq!(m.ids.len(), CAP);
    }
}
