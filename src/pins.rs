//! Pinned tabs that stay.
//!
//! A pin is a place you keep. Closing a pinned tab (x, Ctrl+W, middle click) does not
//! throw the place away: the page is unloaded, like a discarded tab, and the pin stays in
//! the row, dimmed. A click loads its pinned URL again. Only "Unpin" removes a pin.
//!
//! The pinned URL is kept apart from the current one, so a pin that wandered off can go
//! back to where it was pinned. This file is the rules, as plain functions on plain
//! values. The window, the views and the session live in main.rs.

use crate::same_page;

/// What closing a tab does.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Closing {
    /// The tab goes.
    Remove,
    /// The page goes, the pin stays.
    Unload,
}

/// Closing a pinned tab unloads it. Everything else is removed. A tab that leaves no
/// trace (private, agent) is never kept, pinned or not, and a pin with nothing in it has
/// nothing to come back to.
pub fn closing(pinned: bool, off_record: bool, pin_url: &str, url: &str) -> Closing {
    if pinned && !off_record && !(pin_url.is_empty() && url.is_empty()) { Closing::Unload } else { Closing::Remove }
}

/// The URL a pin loads when it is clicked: where it was pinned, or, for a pin made on
/// a blank tab that has since gone somewhere, the page it is on.
pub fn target(pin_url: &str, url: &str) -> String {
    if pin_url.is_empty() { url.to_string() } else { pin_url.to_string() }
}

/// The pinned URL to carry after a restore. A session written before pins remembered
/// their page has none; the page the pin was on is the best answer.
pub fn restored_pin_url(pinned: bool, saved_pin: &str, url: &str) -> String {
    if !pinned {
        String::new()
    } else {
        target(saved_pin, url)
    }
}

/// The tab menu offers "Back to pinned page" when a pin is somewhere else.
pub fn offers_back(pinned: bool, pin_url: &str, url: &str) -> bool {
    pinned && !pin_url.is_empty() && !url.is_empty() && !same_page(pin_url, url)
}

/// What the next-tab choice needs to know about a tab.
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub pinned: bool,
    /// It has a page. An unloaded pin is not a place to go after closing something.
    pub loaded: bool,
}

/// Where to go after unloading the active tab at `i`: the next tab, else the one before,
/// skipping pins that are unloaded (landing on one would load it, the opposite of closing).
/// None: there is nowhere, and a fresh tab takes its place.
pub fn next_after_close(slots: &[Slot], i: usize) -> Option<usize> {
    let ok = |s: &Slot| !(s.pinned && !s.loaded);
    let after = (i + 1..slots.len()).find(|&n| ok(&slots[n]));
    after.or_else(|| (0..i.min(slots.len())).rev().find(|&n| ok(&slots[n])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(pinned: bool, loaded: bool) -> Slot {
        Slot { pinned, loaded }
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
        // A pin made on a blank tab that has since loaded something does.
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
        // Same page, give or take a slash and case: nothing to go back to.
        assert!(!offers_back(true, "https://a.test/", "https://A.test"));
        assert!(!offers_back(false, "https://a.test/", "https://b.test/"));
        // Unloaded or blank, or an old pin without a remembered page.
        assert!(!offers_back(true, "https://a.test/", ""));
        assert!(!offers_back(true, "", "https://b.test/"));
    }

    #[test]
    fn an_old_session_pin_remembers_the_page_it_was_on() {
        assert_eq!(restored_pin_url(true, "", "https://a.test/x"), "https://a.test/x");
        assert_eq!(restored_pin_url(true, "https://a.test/", "https://a.test/x"), "https://a.test/");
        // A tab that is not pinned carries no pin, whatever the file says.
        assert_eq!(restored_pin_url(false, "https://a.test/", "https://a.test/x"), "");
    }

    #[test]
    fn closing_the_active_tab_moves_to_the_next_one() {
        // [pin(live)=active, pin(off), tab, tab]
        let v = [slot(true, true), slot(true, false), slot(false, true), slot(false, true)];
        // The unloaded pin next door is skipped: it would load.
        assert_eq!(next_after_close(&v, 0), Some(2));
    }

    #[test]
    fn at_the_end_it_goes_back_to_a_live_tab() {
        let v = [slot(false, true), slot(true, false), slot(true, true)];
        assert_eq!(next_after_close(&v, 2), Some(0));
    }

    #[test]
    fn with_nowhere_to_go_a_fresh_tab_is_wanted() {
        assert_eq!(next_after_close(&[slot(true, true)], 0), None);
        assert_eq!(next_after_close(&[slot(true, true), slot(true, false)], 0), None);
        assert_eq!(next_after_close(&[], 0), None);
    }
}
