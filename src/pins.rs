//! Pinned tabs that stay.

use crate::same_page;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Closing {
    Remove,
    Unload,
}

pub fn closing(pinned: bool, off_record: bool, pin_url: &str, url: &str) -> Closing {
    if pinned && !off_record && !(pin_url.is_empty() && url.is_empty()) { Closing::Unload } else { Closing::Remove }
}

pub fn target(pin_url: &str, url: &str) -> String {
    if pin_url.is_empty() { url.to_string() } else { pin_url.to_string() }
}

pub fn restored_pin_url(pinned: bool, saved_pin: &str, url: &str) -> String {
    if !pinned {
        String::new()
    } else {
        target(saved_pin, url)
    }
}

pub fn offers_back(pinned: bool, pin_url: &str, url: &str) -> bool {
    pinned && !pin_url.is_empty() && !url.is_empty() && !same_page(pin_url, url)
}

#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub pinned: bool,
    pub loaded: bool,
    pub agent: bool,
}

pub fn next_after_close(slots: &[Slot], i: usize) -> Option<usize> {
    let ok = |s: &Slot| !s.agent && !(s.pinned && !s.loaded);
    let after = (i + 1..slots.len()).find(|&n| ok(&slots[n]));
    after.or_else(|| (0..i.min(slots.len())).rev().find(|&n| ok(&slots[n])))
}

pub fn after_remove(slots: &[Slot], i: usize) -> Option<usize> {
    let near = |ok: &dyn Fn(&Slot) -> bool| {
        (i..slots.len()).find(|&n| ok(&slots[n])).or_else(|| (0..i.min(slots.len())).rev().find(|&n| ok(&slots[n])))
    };
    near(&|s| !s.agent && !s.pinned).or_else(|| near(&|s| !s.agent && s.loaded))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(pinned: bool, loaded: bool) -> Slot {
        Slot { pinned, loaded, agent: false }
    }

    #[test]
    fn a_pinned_tab_unloads_when_closed() {
        assert_eq!(closing(true, false, "https://music.youtube.com/", "https://music.youtube.com/watch?v=1"), Closing::Unload);
        assert_eq!(closing(false, false, "", "https://a.test/"), Closing::Remove);
    }

    #[test]
    fn nothing_off_the_record_is_kept() {
        assert_eq!(closing(true, true, "https://a.test/", "https://a.test/"), Closing::Remove);
    }

    #[test]
    fn an_empty_pin_has_nothing_to_come_back_to() {
        assert_eq!(closing(true, false, "", ""), Closing::Remove);
        assert_eq!(closing(true, false, "", "https://a.test/"), Closing::Unload);
        assert_eq!(target("", "https://a.test/"), "https://a.test/");
    }

    #[test]
    fn the_pin_loads_where_it_was_pinned_not_where_it_wandered() {
        assert_eq!(target("https://music.youtube.com/", "https://www.youtube.com/watch?v=1"), "https://music.youtube.com/");
    }

    #[test]
    fn back_is_offered_only_for_a_pin_that_left() {
        assert!(offers_back(true, "https://a.test/", "https://a.test/other"));
        assert!(!offers_back(true, "https://a.test/", "https://A.test"));
        assert!(!offers_back(false, "https://a.test/", "https://b.test/"));
        assert!(!offers_back(true, "https://a.test/", ""));
        assert!(!offers_back(true, "", "https://b.test/"));
    }

    #[test]
    fn an_old_session_pin_remembers_the_page_it_was_on() {
        assert_eq!(restored_pin_url(true, "", "https://a.test/x"), "https://a.test/x");
        assert_eq!(restored_pin_url(true, "https://a.test/", "https://a.test/x"), "https://a.test/");
        assert_eq!(restored_pin_url(false, "https://a.test/", "https://a.test/x"), "");
    }

    #[test]
    fn closing_the_active_tab_moves_to_the_next_one() {
        let v = [slot(true, true), slot(true, false), slot(false, true), slot(false, true)];
        assert_eq!(next_after_close(&v, 0), Some(2));
    }

    #[test]
    fn at_the_end_it_goes_back_to_a_live_tab() {
        let v = [slot(false, true), slot(true, false), slot(true, true)];
        assert_eq!(next_after_close(&v, 2), Some(0));
    }

    #[test]
    fn closing_the_start_page_never_wakes_an_unloaded_pin() {
        assert_eq!(after_remove(&[slot(true, false), slot(true, false)], 2), None);
        assert_eq!(after_remove(&[slot(true, false), slot(false, false), slot(true, true)], 3), Some(1));
        assert_eq!(after_remove(&[slot(true, false), slot(true, true)], 2), Some(1));
        assert_eq!(after_remove(&[slot(false, true), slot(false, true)], 1), Some(1));
        assert_eq!(after_remove(&[slot(false, true), slot(false, true)], 2), Some(1));
        assert_eq!(after_remove(&[Slot { pinned: false, loaded: true, agent: true }], 1), None);
    }

    #[test]
    fn with_nowhere_to_go_a_fresh_tab_is_wanted() {
        assert_eq!(next_after_close(&[slot(true, true)], 0), None);
        assert_eq!(next_after_close(&[slot(true, true), slot(true, false)], 0), None);
        assert_eq!(next_after_close(&[], 0), None);
    }
}
