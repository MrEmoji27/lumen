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
/// when a key that activates something was pressed, and when a script opened a
/// window.
///
/// The click is caught in the capture phase, on the window, so it is seen
/// before the page's own handlers run: a page that opens a window on the first
/// click has already said what that click was on. The report is a message to
/// the host, and the host reads it when the window is asked for. Measured on
/// 2026-10-19: the message arrives first, because the page sends it before the
/// browser asks for the window and the host reads the queue in the order it
/// was filled.
///
/// Only a real click or key counts (`isTrusted`): a page can dispatch a click
/// of its own, and that must not look like the person's. Measured on 2026-10-25:
/// a click the person made is reported before the navigation it starts.
///
/// The secret is kept out of the page's reach: the message is built in one go,
/// with no prototype the page can see, and the sender is Lumen's own, bound
/// before the page had a say in it.
pub fn script(token: &str) -> String {
    const SCRIPT: &str = r#"try{(()=>{const W=window.chrome&&window.chrome.webview;if(!W)return;
const T='__TOKEN__';
const P=W.postMessage.bind(W);
const say=(k,link,url)=>{try{P({__proto__:null,t:T,lumen:k,link:link,url:url})}catch(_){}};
const opens=t=>!!t&&t!=='_self'&&t!=='_top'&&t!=='_parent';
const onClick=e=>{if(!e.isTrusted)return;let el=e.target,link=0,url='';
 while(el&&el.nodeType===1){const tag=el.tagName;
  if(tag==='A'||tag==='AREA'){if(!url)url=el.href?String(el.href):'';if(opens(el.getAttribute('target')))link=1;break}
  if(tag==='FORM'){if(opens(el.getAttribute('target')))link=1;break}
  if(tag==='BUTTON'||tag==='INPUT'){if(opens(el.getAttribute('formtarget')))link=1;
   else if(el.form&&opens(el.form.getAttribute('target')))link=1;break}
  el=el.parentElement}
 say('click',link,url)};
window.addEventListener('click',onClick,true);
window.addEventListener('auxclick',onClick,true);
const onKey=e=>{if(!e.isTrusted)return;if(e.key==='Enter'||e.key===' '||e.key==='Spacebar')say('key',0,'')};
window.addEventListener('keydown',onKey,true);
const O=window.open;
if(typeof O==='function')window.open=function(u){say('open',0,String(u||''));return O.apply(this,arguments)};
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
#[derive(Clone, Debug, Default)]
pub struct Page {
    /// The last click was on a link that opens a window by itself.
    pub link: bool,
    /// The address of the link that was clicked, when the click was on one.
    pub url: String,
    /// When the click was. `None` until the person clicks.
    pub at: Option<Instant>,
    /// When a key that activates something (Enter, Space) was pressed.
    pub keyed: Option<Instant>,
    /// A script opened a window, and when: the page did it, not the person.
    pub opened: Option<(String, Instant)>,
}

/// Whether the magnet is the one the person clicked: the page's last click
/// was on a link with this address.
///
/// WebView2's own `IsUserInitiated` says true for a magnet navigation whether
/// a click asked for it or the page did it on its own (measured on 2026-10-19),
/// so the page's report of the click is what tells the two apart.
pub fn clicked_magnet(page: &Page, url: &str) -> bool {
    !page.url.is_empty() && page.url == url && page.at.is_some_and(|t| t.elapsed() <= CLICK_WINDOW)
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
    let Some(at) = page.at else { return false };
    if at.elapsed() > CLICK_WINDOW {
        return false;
    }
    // A script that opened this very window after the click did: the page did it.
    if let Some((url, opened)) = &page.opened
        && *opened >= at
        && crate::privacy_stats::host(url) == target
    {
        return false;
    }
    // The link must go where the window went.
    let host = crate::privacy_stats::host(&page.url);
    host.is_empty() || host == target
}

/// How long a page has to have been loaded before a navigation of its own is
/// treated as the page sending the person away rather than its own routing.
const REDIRECT_GRACE: Duration = Duration::from_secs(1);

/// What the redirect rule is right now: the setting, and the sites a page may
/// send the person away from.
#[derive(Clone, Debug, Default)]
pub struct RedirectPolicy {
    /// The person's "Block redirects" setting.
    pub block: bool,
    /// Hosts that may redirect: an entry covers its subdomains.
    pub allowed: Vec<String>,
}

/// The facts about one top-level navigation, for the redirect rule.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    /// A click, a key, a back or a forward asked for this navigation.
    pub gesture: bool,
    /// The site answered with a redirect of its own: a login, a trailing slash.
    pub redirected: bool,
    /// The page the person is on, and the address it is going to.
    pub from: String,
    pub to: String,
    /// How long the page being left has been loaded.
    pub loaded: Duration,
    /// The tab belongs to an agent.
    pub agent: bool,
}

/// Whether Lumen stops this navigation: a page sending the person to another
/// site on its own, more than a moment after it loaded.
///
/// `block` is the person's "Block redirects" setting, `allowed` the sites that
/// may redirect. A click, a key, a back, a forward, a server redirect and a
/// site's own routing while the page loads all go through.
pub fn blocks_redirect(step: &Step, block: bool, allowed: &[String]) -> bool {
    if !block || step.agent || step.gesture || step.redirected {
        return false;
    }
    // Real pages only, in and out: never file:, lumen: or about:.
    if !web_page(&step.from) || !web_page(&step.to) {
        return false;
    }
    let (from, to) = (crate::privacy_stats::host(&step.from), crate::privacy_stats::host(&step.to));
    if same_site(&from, &to) || trusted_site(allowed, &from) {
        return false;
    }
    step.loaded > REDIRECT_GRACE
}

/// Whether an address is a real web page: http or https, and nothing else.
pub fn web_page(url: &str) -> bool {
    let b = url.as_bytes();
    b.len() >= 7 && (b[..7].eq_ignore_ascii_case(b"http://") || (b.len() >= 8 && b[..8].eq_ignore_ascii_case(b"https://")))
}

/// How long after a click or a key the person still counts as the one who asked
/// for a navigation. Chromium gives a script five seconds of the person's
/// activation; this is the same five.
const NAV_ASK: Duration = Duration::from_secs(5);

/// Whether the person's own input asked for this navigation: a click or a key
/// in the last few seconds, as the page reported it.
///
/// WebView2's own `IsUserInitiated` says true for a page's own navigation as
/// well (measured on 2026-10-25), so it cannot tell a click from a script. A
/// click on a link must go where the link went; a click on a button, or a key,
/// may go anywhere: that is the person pressing something the page reacts to.
pub fn asked_nav(page: &Page, to: &str) -> bool {
    if page.keyed.is_some_and(|t| t.elapsed() <= NAV_ASK) {
        return true;
    }
    if !page.at.is_some_and(|t| t.elapsed() <= NAV_ASK) {
        return false;
    }
    // A click on a button, not a link: the page decides where it goes.
    if page.url.is_empty() {
        return true;
    }
    same_site(&crate::privacy_stats::host(&page.url), &crate::privacy_stats::host(to))
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
        let page = Page { link: true, url: "https://other.example.org/x".into(), at: Some(Instant::now()), keyed: None, opened: None };
        assert!(link_click(&page, "other.example.org"));
        assert!(!link_click(&page, "ads.example.net"));
    }

    #[test]
    fn a_script_that_opened_the_window_is_not_the_person() {
        let now = Instant::now();
        let page = Page {
            link: true,
            url: "https://other.example.org/x".into(),
            at: Some(now),
            keyed: None,
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
        let page = Page { link: true, url: "https://other.example.org/x".into(), at: Some(now), keyed: None, opened: Some((String::new(), now + Duration::from_millis(1))) };
        assert!(link_click(&page, "other.example.org"));
    }

    #[test]
    fn an_old_click_is_not_the_one_that_asked() {
        let page = Page { link: true, url: "https://other.example.org/x".into(), at: Some(Instant::now() - Duration::from_secs(60)), keyed: None, opened: None };
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

    /// A navigation a page starts on its own, well after it loaded.
    fn step(from: &str, to: &str) -> Step {
        Step { gesture: false, redirected: false, from: from.into(), to: to.into(), loaded: Duration::from_secs(5), agent: false }
    }

    #[test]
    fn a_page_sending_the_person_away_on_its_own_is_stopped() {
        let s = step("https://news.example/a", "https://ads.example.net/x");
        assert!(blocks_redirect(&s, true, &[]));
    }

    #[test]
    fn a_click_may_go_anywhere() {
        let mut s = step("https://news.example/a", "https://shop.example.org/x");
        s.gesture = true;
        assert!(!blocks_redirect(&s, true, &[]));
    }

    #[test]
    fn a_server_redirect_is_the_sites_own_routing() {
        // A login: the site answers with another site's page. Not a trick.
        let mut s = step("https://login.example/a", "https://accounts.example.org/in");
        s.redirected = true;
        assert!(!blocks_redirect(&s, true, &[]));
    }

    #[test]
    fn the_same_site_is_not_another_site() {
        // A sub domain, a www, another path: the same site.
        assert!(!blocks_redirect(&step("https://news.example.com/a", "https://shop.example.com/b"), true, &[]));
        assert!(!blocks_redirect(&step("https://example.com/a", "https://www.example.com/b"), true, &[]));
    }

    #[test]
    fn routing_while_the_page_loads_is_left_alone() {
        // A site's own hop in the first second is how its routing works.
        let mut s = step("https://news.example/a", "https://cdn.example.net/b");
        s.loaded = Duration::from_millis(400);
        assert!(!blocks_redirect(&s, true, &[]));
        // The moment is over: after a second it is the page's doing.
        s.loaded = Duration::from_millis(1001);
        assert!(blocks_redirect(&s, true, &[]));
    }

    #[test]
    fn an_agents_tab_is_never_redirect_blocked() {
        let mut s = step("https://news.example/a", "https://ads.example.net/x");
        s.agent = true;
        assert!(!blocks_redirect(&s, true, &[]));
    }

    #[test]
    fn only_real_pages_are_judged() {
        // A new tab going from nothing to a page, and Lumen's own pages.
        assert!(!blocks_redirect(&step("", "https://news.example/a"), true, &[]));
        assert!(!blocks_redirect(&step("lumen:start", "https://news.example/a"), true, &[]));
        assert!(!blocks_redirect(&step("https://news.example/a", "about:blank"), true, &[]));
        assert!(!blocks_redirect(&step("file:///C:/x.html", "https://news.example/a"), true, &[]));
    }

    #[test]
    fn a_site_the_person_trusts_may_redirect() {
        let allowed = vec!["example.com".into()];
        assert!(!blocks_redirect(&step("https://example.com/a", "https://ads.example.net/x"), true, &allowed));
        assert!(blocks_redirect(&step("https://other.example/a", "https://ads.example.net/x"), true, &allowed));
    }

    #[test]
    fn with_redirects_off_nothing_is_stopped() {
        assert!(!blocks_redirect(&step("https://news.example/a", "https://ads.example.net/x"), false, &[]));
    }

    #[test]
    fn a_magnet_is_the_persons_only_when_they_clicked_it() {
        let m = "magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c";
        let page = Page { link: false, url: m.into(), at: Some(Instant::now()), keyed: None, opened: None };
        assert!(clicked_magnet(&page, m));
        // Another magnet: the page asked for that one.
        assert!(!clicked_magnet(&page, "magnet:?xt=urn:btih:0000"));
        // A click long ago is not the one that asked.
        let old = Page { link: false, url: m.into(), at: Some(Instant::now() - Duration::from_secs(60)), keyed: None, opened: None };
        assert!(!clicked_magnet(&old, m));
        // No click at all.
        assert!(!clicked_magnet(&Page::default(), m));
    }

    /// What the page last reported the person doing, `ago` before now.
    fn person(url: &str, ago: Duration) -> Page {
        Page { link: false, url: url.into(), at: Some(Instant::now() - ago), keyed: None, opened: None }
    }

    #[test]
    fn a_click_on_a_link_asks_for_where_the_link_goes() {
        let p = person("https://other.example.org/x", Duration::from_millis(200));
        assert!(asked_nav(&p, "https://other.example.org/y"));
        // Another site: the page moved the person somewhere they did not click.
        assert!(!asked_nav(&p, "https://ads.example.net/x"));
    }

    #[test]
    fn a_click_on_a_button_lets_the_page_choose() {
        // Sign in with Google, a form submit: the button has no address of its own.
        let p = person("", Duration::from_millis(200));
        assert!(asked_nav(&p, "https://accounts.example.org/in"));
    }

    #[test]
    fn a_key_that_activates_something_is_the_persons() {
        let p = Page { keyed: Some(Instant::now()), ..Page::default() };
        assert!(asked_nav(&p, "https://accounts.example.org/in"));
        let old = Page { keyed: Some(Instant::now() - Duration::from_secs(60)), ..Page::default() };
        assert!(!asked_nav(&old, "https://accounts.example.org/in"));
    }

    #[test]
    fn with_no_input_the_page_asked_for_itself() {
        assert!(!asked_nav(&Page::default(), "https://ads.example.net/x"));
        // A click from a minute ago is not this navigation's.
        assert!(!asked_nav(&person("https://other.example.org/x", Duration::from_secs(60)), "https://other.example.org/y"));
    }
}
