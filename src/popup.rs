//! Pop-up protection: what a page may open, and which sites the person trusts.
//!
//! The decision is a plain function of what the page did and what the person
//! chose, so it can be tested without a browser. The engine gathers the facts
//! and asks; the chrome says what came of it.

use std::time::{Duration, Instant};

/// The secret that tells Lumen's own page reports from a page's forged ones.
/// The script keeps it in a closure the page cannot reach, so a page cannot
/// claim it clicked something.
pub fn token() -> String {
    let mut b = [0u8; 16];
    let ok = unsafe {
        use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
        BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG).is_ok()
    };
    if !ok {
        // The system's numbers are not coming: a token of our own is still
        // better than none, and the page cannot read this one either.
        b = [0x5a; 16];
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The script Lumen puts in every page: it says what the last click was on,
/// and when a script opened a window.
///
/// The click is caught in the capture phase, so it is seen before the page's
/// own handlers run: a page that opens a window on the first click has already
/// said what that click was on. The report is a message to the host, and the
/// host reads it when the window is asked for. Measured on 2026-10-19: the
/// message arrives first, because the page sends it before the browser asks
/// for the window and the host reads the queue in the order it was filled.
pub fn script(token: &str) -> String {
    const SCRIPT: &str = r#"try{(()=>{const W=window.chrome&&window.chrome.webview;if(!W)return;
const T='__TOKEN__';
const say=m=>{m.t=T;try{W.postMessage(m)}catch(_){}};
const opens=t=>!!t&&t!=='_self'&&t!=='_top'&&t!=='_parent';
const onClick=e=>{let el=e.target,link=0,url='';
 while(el&&el.nodeType===1){const tag=el.tagName;
  if(tag==='A'||tag==='AREA'){if(!url)url=el.href?String(el.href):'';if(opens(el.getAttribute('target')))link=1;break}
  if(tag==='FORM'){if(opens(el.getAttribute('target')))link=1;break}
  if(tag==='BUTTON'||tag==='INPUT'){if(opens(el.getAttribute('formtarget')))link=1;
   else if(el.form&&opens(el.form.getAttribute('target')))link=1;break}
  el=el.parentElement}
 say({lumen:'click',link:link,url:url})};
document.addEventListener('click',onClick,true);
document.addEventListener('auxclick',onClick,true);
const O=window.open;
if(typeof O==='function')window.open=function(u){say({lumen:'open',url:String(u||'')});return O.apply(this,arguments)};
})()}catch(e){}"#;
    SCRIPT.replace("__TOKEN__", token)
}

/// What the pop-up rules are right now: the setting, and the trusted sites.
#[derive(Clone, Debug, Default)]
pub struct Policy {
    /// The person's "Block pop-ups" setting.
    pub block: bool,
    /// Hosts whose pop-ups are allowed. An entry covers its subdomains.
    pub trusted: Vec<String>,
}

/// What a page asked for, and what is known about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ask {
    /// A click or a key asked for this window.
    pub gesture: bool,
    /// The click was on a link that opens a window by itself.
    pub link: bool,
    /// The host of the page that asked.
    pub opener: String,
    /// The host the new window would open.
    pub target: String,
    /// A `$popup` rule in the filter lists names the target host.
    pub popup_rule: bool,
}

/// What Lumen does with a new-window request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Let it open.
    Allow,
    /// Stop it.
    Deny,
}

/// The answer to one new-window request.
///
/// `block` is the person's "Block pop-ups" setting, `trusted` the sites whose
/// pop-ups are allowed. A `$popup` rule has the last word: it says no whatever
/// the rest says.
pub fn decide(ask: &Ask, block: bool, trusted: &[String]) -> Verdict {
    // The filter lists' own word about this host: no, whatever else is true.
    if ask.popup_rule {
        return Verdict::Deny;
    }
    // Pop-ups turned off: only the rule above still says no.
    if !block {
        return Verdict::Allow;
    }
    // No click, no key: a pop-under, not a window the person asked for.
    if !ask.gesture {
        return Verdict::Deny;
    }
    // A site the person trusts keeps the old rule: a click may open a window.
    if trusted_site(trusted, &ask.opener) {
        return Verdict::Allow;
    }
    // A click on a real link is a window the person asked for.
    if ask.link {
        return Verdict::Allow;
    }
    // A window to another site, with no link behind it: the click-hijack.
    if same_site(&ask.opener, &ask.target) {
        return Verdict::Allow;
    }
    Verdict::Deny
}

/// Whether the host is one of the trusted entries, or hangs under one:
/// `example.com` covers `a.example.com`, and not `notexample.com`.
pub fn trusted_site(list: &[String], host: &str) -> bool {
    let h = crate::blocker::site_key(host);
    !h.is_empty() && list.iter().any(|e| under(&h, e))
}

/// Whether two hosts belong to the same site: the same registrable domain.
pub fn same_site(a: &str, b: &str) -> bool {
    let a = crate::privacy_stats::registrable(&crate::blocker::site_key(a));
    let b = crate::privacy_stats::registrable(&crate::blocker::site_key(b));
    !a.is_empty() && a == b
}

/// `host` is `site` itself or a name hanging under it, at a dot boundary.
fn under(host: &str, site: &str) -> bool {
    host == site || (host.len() > site.len() && host.ends_with(site) && host.as_bytes()[host.len() - site.len() - 1] == b'.')
}

/// How long a click still counts as the one that asked for the window.
const CLICK_WINDOW: Duration = Duration::from_secs(10);

/// What a page did last, as far as Lumen could see it.
#[derive(Clone, Debug)]
pub struct Page {
    /// The last click was on a link that opens a window by itself.
    pub link: bool,
    /// The address of the link that was clicked, when the click was on one.
    pub url: String,
    /// When the click was.
    pub at: Instant,
    /// A script opened a window, and when: the page did it, not the person.
    pub opened: Option<(String, Instant)>,
}

impl Default for Page {
    fn default() -> Self {
        Page { link: false, url: String::new(), at: Instant::now(), opened: None }
    }
}

/// Whether the magnet is the one the person clicked: the page's last click
/// was on a link with this address.
///
/// WebView2's own `IsUserInitiated` says true for a magnet navigation whether
/// a click asked for it or the page did it on its own (measured on 2026-10-19),
/// so the page's report of the click is what tells the two apart.
pub fn clicked_magnet(page: &Page, url: &str) -> bool {
    !page.url.is_empty() && page.url == url && page.at.elapsed() <= CLICK_WINDOW
}

/// Whether the click that asked for this window was on a link that opens one.
///
/// The page says what it clicked on, and says it before the window is asked
/// for, so the answer is here by the time the question is. A click on a link
/// that goes somewhere else is the person's own doing; a click with no link
/// behind it is the page's.
pub fn link_click(page: &Page, target: &str) -> bool {
    if !page.link {
        return false;
    }
    if page.at.elapsed() > CLICK_WINDOW {
        return false;
    }
    // A script that opened this very window after the click did: the page did it.
    if let Some((url, at)) = &page.opened
        && *at >= page.at
        && crate::privacy_stats::host(url) == target
    {
        return false;
    }
    // The link must go where the window went.
    let host = crate::privacy_stats::host(&page.url);
    host.is_empty() || host == target
}

/// The host a person typed: an address, or a plain site name. Empty when it is
/// not a host at all.
pub fn host_of(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        return String::new();
    }
    let h = if t.contains("://") { crate::privacy_stats::host(t) } else { t.to_string() };
    let h = crate::blocker::site_key(&h);
    if h.is_empty() || h.contains(['/', ' ', '*', '?', '#']) {
        return String::new();
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(opener: &str, target: &str) -> Ask {
        Ask { gesture: true, link: false, opener: opener.into(), target: target.into(), popup_rule: false }
    }

    /// The site's own pop-ups, as a person set it up.
    fn trusted() -> Vec<String> {
        vec!["example.com".into()]
    }

    #[test]
    fn a_click_with_no_link_behind_it_is_a_pop_under() {
        // The ad case: the page hangs a listener on the whole page and opens
        // a window of its own on the first click.
        let a = ask("news.example", "ads.example.net");
        assert_eq!(decide(&a, true, &[]), Verdict::Deny);
    }

    #[test]
    fn a_link_the_person_clicked_may_open_a_window() {
        let mut a = ask("news.example", "other.example.org");
        a.link = true;
        assert_eq!(decide(&a, true, &[]), Verdict::Allow);
    }

    #[test]
    fn no_click_at_all_is_never_a_pop_up() {
        let mut a = ask("news.example", "other.example.org");
        a.gesture = false;
        assert_eq!(decide(&a, true, &[]), Verdict::Deny);
        // Not even a link saves it: nothing was clicked.
        a.link = true;
        assert_eq!(decide(&a, true, &[]), Verdict::Deny);
    }

    #[test]
    fn a_window_to_the_same_site_keeps_the_old_rule() {
        let a = ask("news.example.com", "shop.example.com");
        assert_eq!(decide(&a, true, &[]), Verdict::Allow);
        // A sub domain of the same site is the same site.
        let a = ask("news.example.com", "a.b.example.com");
        assert_eq!(decide(&a, true, &[]), Verdict::Allow);
    }

    #[test]
    fn a_trusted_site_keeps_the_old_rule() {
        // An OAuth pop-up from a site the person trusts has to work.
        let a = ask("example.com", "accounts.google.com");
        assert_eq!(decide(&a, true, &trusted()), Verdict::Allow);
        // A sub domain of a trusted site is trusted too.
        let a = ask("a.example.com", "accounts.google.com");
        assert_eq!(decide(&a, true, &trusted()), Verdict::Allow);
        // Another site is not.
        let a = ask("notexample.com", "accounts.google.com");
        assert_eq!(decide(&a, true, &trusted()), Verdict::Deny);
    }

    #[test]
    fn a_trusted_site_still_needs_a_click() {
        let mut a = ask("example.com", "accounts.google.com");
        a.gesture = false;
        assert_eq!(decide(&a, true, &trusted()), Verdict::Deny);
    }

    #[test]
    fn the_filter_lists_have_the_last_word() {
        // A `$popup` rule says no, even on a trusted site and even with a click.
        let mut a = ask("example.com", "ads.example.net");
        a.popup_rule = true;
        assert_eq!(decide(&a, true, &trusted()), Verdict::Deny);
        // And with pop-ups turned off, the rule still says no.
        assert_eq!(decide(&a, false, &trusted()), Verdict::Deny);
    }

    #[test]
    fn with_pop_ups_off_only_the_rule_stops_them() {
        let a = ask("news.example", "ads.example.net");
        assert_eq!(decide(&a, false, &[]), Verdict::Allow);
    }

    #[test]
    fn a_trusted_site_is_the_site_or_a_name_under_it() {
        assert!(trusted_site(&trusted(), "example.com"));
        assert!(trusted_site(&trusted(), "a.b.example.com"));
        assert!(!trusted_site(&trusted(), "notexample.com"));
        assert!(!trusted_site(&trusted(), "example.com.evil.test"));
        assert!(!trusted_site(&[], "example.com"));
    }

    #[test]
    fn the_link_must_go_where_the_window_went() {
        let page = Page { link: true, url: "https://other.example.org/x".into(), at: Instant::now(), opened: None };
        assert!(link_click(&page, "other.example.org"));
        assert!(!link_click(&page, "ads.example.net"));
    }

    #[test]
    fn a_script_that_opened_the_window_is_not_the_person() {
        let now = Instant::now();
        let page = Page {
            link: true,
            url: "https://other.example.org/x".into(),
            at: now,
            // The page opened this very window itself, after the click.
            opened: Some(("https://other.example.org/ad".into(), now + Duration::from_millis(1))),
        };
        assert!(!link_click(&page, "other.example.org"));
    }

    #[test]
    fn a_script_that_opened_something_else_leaves_the_link_alone() {
        // The usual OAuth shape: the link opens a window, and the script opens
        // another one it will fill in later.
        let now = Instant::now();
        let page = Page { link: true, url: "https://other.example.org/x".into(), at: now, opened: Some((String::new(), now + Duration::from_millis(1))) };
        assert!(link_click(&page, "other.example.org"));
    }

    #[test]
    fn an_old_click_is_not_the_one_that_asked() {
        let page = Page { link: true, url: "https://other.example.org/x".into(), at: Instant::now() - Duration::from_secs(60), opened: None };
        assert!(!link_click(&page, "other.example.org"));
    }

    #[test]
    fn a_typed_site_is_a_host_and_nothing_else() {
        assert_eq!(host_of("example.com"), "example.com");
        assert_eq!(host_of("  https://www.example.com/x  "), "example.com");
        assert_eq!(host_of(""), "");
        assert_eq!(host_of("not a host"), "");
        assert_eq!(host_of("example.com/evil"), "");
    }

    #[test]
    fn a_magnet_is_the_persons_only_when_they_clicked_it() {
        let m = "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c";
        let page = Page { link: false, url: m.into(), at: Instant::now(), opened: None };
        assert!(clicked_magnet(&page, m));
        // Another magnet: the page asked for that one.
        assert!(!clicked_magnet(&page, "magnet:?xt=urn:btih:0000"));
        // A click long ago is not the one that asked.
        let old = Page { link: false, url: m.into(), at: Instant::now() - Duration::from_secs(60), opened: None };
        assert!(!clicked_magnet(&old, m));
        // No click at all.
        assert!(!clicked_magnet(&Page::default(), m));
    }
}
