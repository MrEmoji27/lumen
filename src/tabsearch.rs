//! Finding an open tab by a few typed letters.

pub struct Entry<'a> {
    pub title: &'a str,
    pub url: &'a str,
}

fn host(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let h = rest.split(['/', '?', '#']).next().unwrap_or("");
    let h = h.rsplit('@').next().unwrap_or(h).to_ascii_lowercase();
    h.strip_prefix("www.").map(str::to_string).unwrap_or(h)
}

fn word_score(w: &str, title: &str, host: &str, url: &str) -> u32 {
    if title.starts_with(w) {
        100
    } else if title.split(|c: char| !c.is_alphanumeric()).any(|t| t.starts_with(w)) {
        80
    } else if host.starts_with(w) {
        70
    } else if title.contains(w) {
        50
    } else if host.contains(w) {
        40
    } else if url.contains(w) {
        20
    } else {
        0
    }
}

pub fn score(query: &str, e: &Entry) -> Option<u32> {
    let title = e.title.to_lowercase();
    let url = e.url.to_lowercase();
    let host = host(e.url);
    let mut total = 0;
    let mut any = false;
    for w in query.split_whitespace() {
        any = true;
        let s = word_score(&w.to_lowercase(), &title, &host, &url);
        if s == 0 {
            return None;
        }
        total += s;
    }
    Some(if any { total } else { 1 })
}

pub fn rank(query: &str, entries: &[Entry]) -> Vec<usize> {
    let mut hits: Vec<(usize, u32)> = entries.iter().enumerate().filter_map(|(n, e)| score(query, e).map(|s| (n, s))).collect();
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    hits.into_iter().map(|(n, _)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e<'a>(title: &'a str, url: &'a str) -> Entry<'a> {
        Entry { title, url }
    }

    #[test]
    fn an_empty_query_lists_every_tab_in_order() {
        let tabs = [e("B", "https://b.test/"), e("A", "https://a.test/"), e("C", "")];
        assert_eq!(rank("", &tabs), vec![0, 1, 2]);
        assert_eq!(rank("   ", &tabs), vec![0, 1, 2]);
    }

    #[test]
    fn only_tabs_with_every_word_stay() {
        let tabs = [
            e("The Rust Programming Language", "https://doc.rust-lang.org/book/"),
            e("Rust (programming language) - Wikipedia", "https://en.wikipedia.org/wiki/Rust"),
            e("Hacker News", "https://news.ycombinator.com/"),
        ];
        assert_eq!(rank("rust", &tabs).len(), 2);
        assert_eq!(rank("rust wiki", &tabs), vec![1]);
        assert!(rank("rust zebra", &tabs).is_empty());
        assert_eq!(rank("YCOMBINATOR", &tabs), vec![2]);
    }

    #[test]
    fn a_title_that_starts_with_the_word_beats_one_that_only_contains_it() {
        let tabs = [
            e("Notes on mailing lists", "https://notes.test/"),
            e("Mail - Inbox", "https://mail.test/"),
            e("Fastmail settings", "https://fastmail.test/"),
        ];
        assert_eq!(rank("mail", &tabs), vec![1, 0, 2]);
    }

    #[test]
    fn the_host_counts_when_the_title_says_nothing() {
        let tabs = [e("Untitled", "https://www.example.org/x"), e("Something", "https://other.test/example")];
        assert_eq!(rank("example", &tabs), vec![0, 1]);
    }

    #[test]
    fn equal_scores_keep_the_sidebars_order() {
        let tabs = [e("Docs one", "https://a.test/"), e("Docs two", "https://b.test/"), e("Docs three", "https://c.test/")];
        assert_eq!(rank("docs", &tabs), vec![0, 1, 2]);
    }
}
