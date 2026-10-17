//! Reading the bookmarks other browsers keep, so they can be brought here once.
//!
//! Chrome, Edge, Brave and Opera GX are all Chromium: each keeps one JSON file named
//! `Bookmarks` in its profile folder. We only ever read it. Nothing here writes to, locks
//! or moves another browser's files.

use std::path::PathBuf;

use serde_json::Value;

use crate::library::Incoming;

/// One browser whose file was found, and what is in it.
#[derive(Clone, Debug)]
pub struct Found {
    pub browser: &'static str,
    pub items: Vec<Incoming>,
}

/// Where each browser keeps its file on Windows, given the two folders everything hangs off.
/// Only the default profile: that is where nearly everyone's bookmarks are.
pub fn candidates(local: &std::path::Path, roaming: &std::path::Path) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("Chrome", local.join(r"Google\Chrome\User Data\Default\Bookmarks")),
        ("Edge", local.join(r"Microsoft\Edge\User Data\Default\Bookmarks")),
        ("Brave", local.join(r"BraveSoftware\Brave-Browser\User Data\Default\Bookmarks")),
        ("Opera GX", roaming.join(r"Opera Software\Opera GX Stable\Bookmarks")),
    ]
}

/// Every browser on this PC with a readable file and at least one bookmark.
pub fn scan() -> Vec<Found> {
    let (Some(local), Some(roaming)) = (dirs::data_local_dir(), dirs::data_dir()) else { return Vec::new() };
    candidates(&local, &roaming)
        .into_iter()
        .filter_map(|(browser, path)| {
            let text = std::fs::read_to_string(path).ok()?;
            let items = parse_chromium(&text, browser);
            (!items.is_empty()).then_some(Found { browser, items })
        })
        .collect()
}

/// The roots Chromium keeps, as they are named in the file. Their own names (the bar, "Other
/// bookmarks") say nothing worth keeping, so they do not become folders.
const ROOTS: [&str; 3] = ["bookmark_bar", "other", "synced"];

/// A Chromium `Bookmarks` file as bookmarks in folders one level deep. Everything that sat
/// loose in a root goes in a folder named for the browser; a folder inside goes in
/// "Browser / Folder", and a folder inside that in "Browser / Folder / Inner". A bad file is
/// no bookmarks. Only web addresses come over: bookmarklets and `chrome://` pages do not.
pub fn parse_chromium(json: &str, browser: &str) -> Vec<Incoming> {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    let mut out = Vec::new();
    for root in ROOTS {
        if let Some(children) = v["roots"][root]["children"].as_array() {
            walk(children, &mut vec![browser.to_string()], &mut out);
        }
    }
    out
}

fn walk(nodes: &[Value], path: &mut Vec<String>, out: &mut Vec<Incoming>) {
    for n in nodes {
        match n["type"].as_str() {
            Some("url") => {
                let url = n["url"].as_str().unwrap_or("").trim();
                if url.starts_with("http://") || url.starts_with("https://") {
                    out.push(Incoming { folder: path.join(" / "), title: n["name"].as_str().unwrap_or("").trim().to_string(), url: url.to_string() });
                }
            }
            Some("folder") => {
                let name = n["name"].as_str().unwrap_or("").trim();
                let name = if name.is_empty() { "Folder" } else { name };
                path.push(name.to_string());
                if let Some(kids) = n["children"].as_array() {
                    walk(kids, path, out);
                }
                path.pop();
            }
            _ => {}
        }
    }
}

/// The words the page says after an import.
pub fn summary(browser: &str, added: usize, skipped: usize) -> String {
    match (added, skipped) {
        (0, 0) => format!("{browser} had no bookmarks to bring over"),
        (0, s) => format!("Nothing new from {browser}: all {s} were already here"),
        (a, 0) => format!("Imported {a} from {browser}"),
        (a, s) => format!("Imported {a} from {browser}, skipped {s} already here"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "checksum": "x", "version": 1,
      "roots": {
        "bookmark_bar": { "type": "folder", "name": "Bookmarks bar", "children": [
          { "type": "url", "name": "Rust", "url": "https://www.rust-lang.org/" },
          { "type": "folder", "name": "Dev", "children": [
            { "type": "url", "name": "Docs", "url": "https://doc.rust-lang.org/" },
            { "type": "folder", "name": "Crates", "children": [
              { "type": "url", "name": "crates.io", "url": "https://crates.io/" }
            ]}
          ]},
          { "type": "url", "name": "A bookmarklet", "url": "javascript:alert(1)" },
          { "type": "url", "name": "Settings", "url": "chrome://settings" }
        ]},
        "other": { "type": "folder", "name": "Other bookmarks", "children": [
          { "type": "url", "name": "  Padded  ", "url": " http://example.com/ " },
          { "type": "folder", "name": "", "children": [
            { "type": "url", "name": "Nameless folder", "url": "https://nameless.test/" }
          ]}
        ]},
        "synced": { "type": "folder", "name": "Mobile bookmarks", "children": [] }
      }
    }"#;

    fn row(folder: &str, title: &str, url: &str) -> Incoming {
        Incoming { folder: folder.into(), title: title.into(), url: url.into() }
    }

    #[test]
    fn nested_folders_flatten_to_named_paths() {
        let got = parse_chromium(FIXTURE, "Chrome");
        assert_eq!(
            got,
            vec![
                row("Chrome", "Rust", "https://www.rust-lang.org/"),
                row("Chrome / Dev", "Docs", "https://doc.rust-lang.org/"),
                row("Chrome / Dev / Crates", "crates.io", "https://crates.io/"),
                row("Chrome", "Padded", "http://example.com/"),
                row("Chrome / Folder", "Nameless folder", "https://nameless.test/"),
            ]
        );
    }

    #[test]
    fn bookmarklets_and_browser_pages_stay_behind() {
        let got = parse_chromium(FIXTURE, "Edge");
        assert!(got.iter().all(|b| b.url.starts_with("http")));
        assert!(!got.iter().any(|b| b.title == "A bookmarklet" || b.title == "Settings"));
    }

    #[test]
    fn a_broken_or_empty_file_is_no_bookmarks() {
        assert!(parse_chromium("not json", "Chrome").is_empty());
        assert!(parse_chromium("{}", "Chrome").is_empty());
        assert!(parse_chromium(r#"{"roots":{"bookmark_bar":{"children":[]}}}"#, "Chrome").is_empty());
    }

    #[test]
    fn each_browser_has_its_own_standard_path() {
        let c = candidates(std::path::Path::new(r"C:\L"), std::path::Path::new(r"C:\R"));
        let names: Vec<&str> = c.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["Chrome", "Edge", "Brave", "Opera GX"]);
        assert!(c[3].1.to_string_lossy().ends_with(r"Opera Software\Opera GX Stable\Bookmarks"));
        assert!(c[3].1.starts_with(r"C:\R"), "Opera GX lives under %APPDATA%");
        assert!(c[0].1.starts_with(r"C:\L"), "Chrome lives under %LOCALAPPDATA%");
    }

    /// Importing the fixture twice into a library adds each bookmark once.
    #[test]
    fn importing_twice_adds_once() {
        let dir = std::env::temp_dir().join(format!("lumen-bookmarks-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut lib = crate::library::Library::load(&dir);
        let items = parse_chromium(FIXTURE, "Chrome");
        assert_eq!(lib.import(&items), (5, 0));
        assert_eq!(lib.import(&items), (0, 5));
        assert_eq!(lib.folders(), ["Chrome", "Chrome / Dev", "Chrome / Dev / Crates", "Chrome / Folder"]);
    }

    #[test]
    fn the_summary_says_what_happened() {
        assert_eq!(summary("Chrome", 12, 0), "Imported 12 from Chrome");
        assert_eq!(summary("Edge", 3, 2), "Imported 3 from Edge, skipped 2 already here");
        assert_eq!(summary("Brave", 0, 4), "Nothing new from Brave: all 4 were already here");
    }
}
