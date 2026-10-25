//! The changelog, built into the exe, as data the About window can draw.

pub const TEXT: &str = include_str!("../docs/changelog.md");

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: String,
    pub date: String,
    pub intro: Vec<String>,
    pub items: Vec<String>,
}

pub fn parse(md: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    let mut at: Option<usize> = None;
    let mut open: Option<bool> = None;
    for raw in md.lines() {
        let line = raw.trim_end();
        if let Some(heading) = line.strip_prefix("## ") {
            at = version_of(heading).map(|(version, date)| {
                out.push(Release { version, date, intro: Vec::new(), items: Vec::new() });
                out.len() - 1
            });
            open = None;
            continue;
        }
        let Some(release) = at.and_then(|i| out.get_mut(i)) else { continue };
        if line.trim().is_empty() {
            open = None;
            continue;
        }
        if let Some(text) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            release.items.push(text.to_string());
            open = Some(true);
        } else if let Some(text) = line.strip_prefix("###") {
            release.items.push(text.trim_start_matches('#').trim().to_string());
            open = Some(true);
        } else if line.starts_with("  ") {
            let text = line.trim();
            match open {
                Some(true) => joined(release.items.last_mut(), text),
                Some(false) => joined(release.intro.last_mut(), text),
                None if release.items.is_empty() => {
                    release.intro.push(text.to_string());
                    open = Some(false);
                }
                None => {}
            }
        } else if release.items.is_empty() {
            match open {
                Some(false) => joined(release.intro.last_mut(), line),
                _ => {
                    release.intro.push(line.to_string());
                    open = Some(false);
                }
            }
        }
    }
    for release in &mut out {
        release.intro = release.intro.iter().map(|s| plain(s)).collect();
        release.items = release.items.iter().map(|s| plain(s)).collect();
    }
    out
}

pub fn releases() -> Vec<Release> {
    parse(TEXT)
}

pub fn plain(s: &str) -> String {
    unlink(s).chars().filter(|c| !matches!(c, '*' | '`')).collect()
}

fn version_of(heading: &str) -> Option<(String, String)> {
    let (version, date) = heading.split_once(" — ").or_else(|| heading.split_once(" - ")).unwrap_or((heading, ""));
    let version = version.trim();
    let looks_like_version =
        !version.is_empty() && version.starts_with(|c: char| c.is_ascii_digit()) && !version.contains(char::is_whitespace);
    looks_like_version.then(|| (version.to_string(), date.trim().to_string()))
}

fn joined(last: Option<&mut String>, text: &str) {
    if let Some(last) = last {
        if !last.is_empty() {
            last.push(' ');
        }
        last.push_str(text);
    }
}

fn unlink(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        match link_at(&chars, i) {
            Some((text_end, url_end)) => {
                out.extend(chars[i + 1..text_end].iter().copied());
                i = url_end + 1;
            }
            None => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

fn link_at(chars: &[char], i: usize) -> Option<(usize, usize)> {
    if chars.get(i) != Some(&'[') {
        return None;
    }
    let text_end = i + 1 + chars[i + 1..].iter().position(|&c| c == ']')?;
    if chars.get(text_end + 1) != Some(&'(') {
        return None;
    }
    let url_end = text_end + 2 + chars[text_end + 2..].iter().position(|&c| c == ')')?;
    Some((text_end, url_end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_file_parses_into_releases() {
        let all = releases();
        assert!(all.len() >= 10, "{} releases", all.len());
        assert_eq!(all[0].version, env!("LUMEN_VERSION"));
        assert!(!all[0].date.is_empty(), "the newest release says when");
        assert!(!all[0].items.is_empty(), "and what changed");
        let last = all.last().unwrap();
        assert_eq!(last.version, "0.1.0");
        assert_eq!(last.date, "");
        assert_eq!(last.intro, vec!["Everything before numbering started."]);
        for release in &all {
            assert!(!release.version.contains('*'));
            for text in release.intro.iter().chain(&release.items) {
                assert!(!text.contains('*') && !text.contains('`'), "{text}");
            }
        }
    }

    #[test]
    fn a_sample_comes_out_exactly() {
        let md = "\
# Changelog

The preamble belongs to no release.

## Unreleased

- this is skipped, and so is the paragraph after it

## 2.0.0 — 2026-01-02

One **intro** paragraph,
joined from two lines.

- A bullet that
  runs on.
- A [link](https://example.com/a) and `code`.
- Plain and unchanged: docs/design/x.html

### Notes

- After the sub-heading.

## 1.0.0

- The oldest.
";
        let got = parse(md);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(
            got[0],
            Release {
                version: "2.0.0".into(),
                date: "2026-01-02".into(),
                intro: vec!["One intro paragraph, joined from two lines.".into()],
                items: vec![
                    "A bullet that runs on.".into(),
                    "A link and code.".into(),
                    "Plain and unchanged: docs/design/x.html".into(),
                    "Notes".into(),
                    "After the sub-heading.".into(),
                ],
            }
        );
        assert_eq!(got[1].version, "1.0.0");
        assert_eq!(got[1].date, "");
        assert!(got[1].intro.is_empty());
        assert_eq!(got[1].items, vec!["The oldest."]);
    }

    #[test]
    fn a_heading_that_is_not_a_version_is_skipped() {
        let md = "## 1.0.0 - 2025-12-31\n\n- kept.\n\n## Unreleased\n\n- dropped.\n\nAnd this paragraph too.\n\n## 0.9.0\n\n- also kept.\n";
        let got = parse(md);
        assert_eq!(got.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["1.0.0", "0.9.0"]);
        assert_eq!(got[0].date, "2025-12-31", "a plain dash with spaces works too");
        assert_eq!(got[0].items, vec!["kept."]);
        assert!(got[0].intro.is_empty());
        assert_eq!(got[1].items, vec!["also kept."]);
    }

    #[test]
    fn marks_come_out_and_nothing_else_changes() {
        assert_eq!(plain("**bold** and *em* and `code`"), "bold and em and code");
        assert_eq!(plain("see [the list](https://example.com/x) now"), "see the list now");
        assert_eq!(plain("docs/design/lumen-ui.html"), "docs/design/lumen-ui.html");
        assert_eq!(plain("0.6.2 — 2026-10-18"), "0.6.2 — 2026-10-18");
    }
}
