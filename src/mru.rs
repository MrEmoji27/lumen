//! Where the person was: tab ids in the order they were last active.

use std::collections::VecDeque;

const CAP: usize = 64;

#[derive(Default)]
pub struct Mru {
    ids: VecDeque<u64>,
}

impl Mru {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn touch(&mut self, id: u64) {
        if let Some(at) = self.ids.iter().position(|&x| x == id) {
            self.ids.remove(at);
        }
        self.ids.push_front(id);
        self.ids.truncate(CAP);
    }

    pub fn remove(&mut self, id: u64) {
        self.ids.retain(|&x| x != id);
    }

    pub fn back(&self, closing: u64, alive: impl Fn(u64) -> bool) -> Option<u64> {
        self.ids.iter().copied().find(|&id| id != closing && alive(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        m.touch(1);
        assert_eq!(order(&m), vec![1, 3, 2]);
        assert_eq!(m.back(3, |_| true), Some(1));
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
        m.remove(99);
        assert_eq!(order(&m), vec![3, 1]);
    }

    #[test]
    fn a_tab_that_is_gone_is_skipped() {
        let mut m = Mru::new();
        m.touch(1);
        m.touch(2);
        m.touch(3);
        assert_eq!(m.back(3, |id| id != 2), Some(1), "2 is gone, 1 is next in line");
        assert_eq!(m.back(3, |_| false), None);
        assert_eq!(m.back(3, |_| true), Some(2));
    }

    #[test]
    fn a_pinned_tab_is_chosen_only_when_it_really_was_the_last_one_used() {
        let mut m = Mru::new();
        m.touch(1);
        m.touch(7);
        m.touch(4);
        m.touch(9);
        assert_eq!(m.back(9, |_| true), Some(4), "a tab used after the pin wins");
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
        assert!(!order(&m).contains(&36));
        assert!(order(&m).contains(&37));
        m.touch(37);
        assert_eq!(m.ids.len(), CAP);
    }
}
