//! Pop-up protection: what a page may open, and which sites the person trusts.

use std::time::{Duration, Instant};

pub fn token() -> String {
    let mut b = [0u8; 16];
    let ok = unsafe {
        use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
        BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG).is_ok()
    };
    if !ok {
        b = [0x5a; 16];
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

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

#[derive(Clone, Debug, Default)]
pub struct Policy {
    pub block: bool,
    pub trusted: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ask {
    pub gesture: bool,
    pub link: bool,
    pub opener: String,
    pub target: String,
    pub popup_rule: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
}

pub fn decide(ask: &Ask, block: bool, trusted: &[String]) -> Verdict {
    if ask.popup_rule {
        return Verdict::Deny;
    }
    if !block {
        return Verdict::Allow;
    }
    if !ask.gesture {
        return Verdict::Deny;
    }
    if trusted_site(trusted, &ask.opener) {
        return Verdict::Allow;
    }
    if ask.link {
        return Verdict::Allow;
    }
    if same_site(&ask.opener, &ask.target) {
        return Verdict::Allow;
    }
    Verdict::Deny
}

pub fn trusted_site(list: &[String], host: &str) -> bool {
    let h = crate::blocker::site_key(host);
    !h.is_empty() && list.iter().any(|e| under(&h, e))
}

pub fn same_site(a: &str, b: &str) -> bool {
    let a = crate::privacy_stats::registrable(&crate::blocker::site_key(a));
    let b = crate::privacy_stats::registrable(&crate::blocker::site_key(b));
    !a.is_empty() && a == b
}

fn under(host: &str, site: &str) -> bool {
    host == site || (host.len() > site.len() && host.ends_with(site) && host.as_bytes()[host.len() - site.len() - 1] == b'.')
}

const CLICK_WINDOW: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub link: bool,
    pub url: String,
    pub at: Option<Instant>,
    pub keyed: Option<Instant>,
    pub opened: Option<(String, Instant)>,
}

pub fn clicked_magnet(page: &Page, url: &str) -> bool {
    !page.url.is_empty() && page.url == url && page.at.is_some_and(|t| t.elapsed() <= CLICK_WINDOW)
}

pub fn link_click(page: &Page, target: &str) -> bool {
    if !page.link {
        return false;
    }
    let Some(at) = page.at else { return false };
    if at.elapsed() > CLICK_WINDOW {
        return false;
    }
    if let Some((url, opened)) = &page.opened
        && *opened >= at
        && crate::privacy_stats::host(url) == target
    {
        return false;
    }
    let host = crate::privacy_stats::host(&page.url);
    host.is_empty() || host == target
}

const REDIRECT_GRACE: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Default)]
pub struct RedirectPolicy {
    pub block: bool,
    pub allowed: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub gesture: bool,
    pub redirected: bool,
    pub from: String,
    pub to: String,
    pub loaded: Duration,
    pub agent: bool,
}

pub fn blocks_redirect(step: &Step, block: bool, allowed: &[String]) -> bool {
    if !block || step.agent || step.gesture || step.redirected {
        return false;
    }
    if !web_page(&step.from) || !web_page(&step.to) {
        return false;
    }
    let (from, to) = (crate::privacy_stats::host(&step.from), crate::privacy_stats::host(&step.to));
    if same_site(&from, &to) || trusted_site(allowed, &from) {
        return false;
    }
    step.loaded > REDIRECT_GRACE
}

pub fn web_page(url: &str) -> bool {
    let b = url.as_bytes();
    b.len() >= 7 && (b[..7].eq_ignore_ascii_case(b"http://") || (b.len() >= 8 && b[..8].eq_ignore_ascii_case(b"https://")))
}

const NAV_ASK: Duration = Duration::from_secs(5);

pub fn asked_nav(page: &Page, to: &str) -> bool {
    if page.keyed.is_some_and(|t| t.elapsed() <= NAV_ASK) {
        return true;
    }
    if !page.at.is_some_and(|t| t.elapsed() <= NAV_ASK) {
        return false;
    }
    if page.url.is_empty() {
        return true;
    }
    same_site(&crate::privacy_stats::host(&page.url), &crate::privacy_stats::host(to))
}

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

    fn trusted() -> Vec<String> {
        vec!["example.com".into()]
    }

    #[test]
    fn a_click_with_no_link_behind_it_is_a_pop_under() {
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
        a.link = true;
        assert_eq!(decide(&a, true, &[]), Verdict::Deny);
    }

    #[test]
    fn a_window_to_the_same_site_keeps_the_old_rule() {
        let a = ask("news.example.com", "shop.example.com");
        assert_eq!(decide(&a, true, &[]), Verdict::Allow);
        let a = ask("news.example.com", "a.b.example.com");
        assert_eq!(decide(&a, true, &[]), Verdict::Allow);
    }

    #[test]
    fn a_trusted_site_keeps_the_old_rule() {
        let a = ask("example.com", "accounts.google.com");
        assert_eq!(decide(&a, true, &trusted()), Verdict::Allow);
        let a = ask("a.example.com", "accounts.google.com");
        assert_eq!(decide(&a, true, &trusted()), Verdict::Allow);
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
        let mut a = ask("example.com", "ads.example.net");
        a.popup_rule = true;
        assert_eq!(decide(&a, true, &trusted()), Verdict::Deny);
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
            opened: Some(("https://other.example.org/ad".into(), now + Duration::from_millis(1))),
        };
        assert!(!link_click(&page, "other.example.org"));
    }

    #[test]
    fn a_script_that_opened_something_else_leaves_the_link_alone() {
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
        let mut s = step("https://login.example/a", "https://accounts.example.org/in");
        s.redirected = true;
        assert!(!blocks_redirect(&s, true, &[]));
    }

    #[test]
    fn the_same_site_is_not_another_site() {
        assert!(!blocks_redirect(&step("https://news.example.com/a", "https://shop.example.com/b"), true, &[]));
        assert!(!blocks_redirect(&step("https://example.com/a", "https://www.example.com/b"), true, &[]));
    }

    #[test]
    fn routing_while_the_page_loads_is_left_alone() {
        let mut s = step("https://news.example/a", "https://cdn.example.net/b");
        s.loaded = Duration::from_millis(400);
        assert!(!blocks_redirect(&s, true, &[]));
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
        assert!(!clicked_magnet(&page, "magnet:?xt=urn:btih:0000"));
        let old = Page { link: false, url: m.into(), at: Some(Instant::now() - Duration::from_secs(60)), keyed: None, opened: None };
        assert!(!clicked_magnet(&old, m));
        assert!(!clicked_magnet(&Page::default(), m));
    }

    fn person(url: &str, ago: Duration) -> Page {
        Page { link: false, url: url.into(), at: Some(Instant::now() - ago), keyed: None, opened: None }
    }

    #[test]
    fn a_click_on_a_link_asks_for_where_the_link_goes() {
        let p = person("https://other.example.org/x", Duration::from_millis(200));
        assert!(asked_nav(&p, "https://other.example.org/y"));
        assert!(!asked_nav(&p, "https://ads.example.net/x"));
    }

    #[test]
    fn a_click_on_a_button_lets_the_page_choose() {
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
        assert!(!asked_nav(&person("https://other.example.org/x", Duration::from_secs(60)), "https://other.example.org/y"));
    }
}
