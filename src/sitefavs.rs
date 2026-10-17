//! A favicon for each site, kept across runs, so the new tab page's tiles have their icons
//! the moment it opens. The icons are the ones pages already hand us for their tabs (WebView2's
//! favicon event): nothing is fetched for this. `favicons.json` in the data folder, host to
//! `data:image/png` URI.

use std::collections::BTreeMap;

use crate::speeddial::host_of;

const FILE: &str = "favicons.json";
/// Sites remembered. Past this a new site is not added; a known one is still refreshed.
const MAX_SITES: usize = 300;
/// A favicon bigger than this (as the data URI) is a picture, not an icon.
const MAX_BYTES: usize = 48_000;

pub type Favs = BTreeMap<String, String>;

pub fn load() -> Favs {
    std::fs::read_to_string(crate::store::data_dir().join(FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(favs: &Favs) {
    let path = crate::store::data_dir().join(FILE);
    let tmp = path.with_extension("tmp");
    if let Ok(json) = serde_json::to_string(favs)
        && std::fs::write(&tmp, json).is_ok()
    {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Remember `data_uri` as the icon of the site `url` is on. True when something changed.
pub fn remember(favs: &mut Favs, url: &str, data_uri: &str) -> bool {
    let host = host_of(url);
    if host.is_empty() || !data_uri.starts_with("data:image/") || data_uri.len() > MAX_BYTES {
        return false;
    }
    if favs.get(&host).is_some_and(|old| old == data_uri) || (!favs.contains_key(&host) && favs.len() >= MAX_SITES) {
        return false;
    }
    favs.insert(host, data_uri.to_string());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICON: &str = "data:image/png;base64,AAAA";

    #[test]
    fn a_site_has_one_icon_whatever_the_page() {
        let mut f = Favs::new();
        assert!(remember(&mut f, "https://www.github.com/rust-lang/rust", ICON));
        assert_eq!(f.get("github.com").map(String::as_str), Some(ICON));
        // The same icon again is no change; a different page of the site is the same site.
        assert!(!remember(&mut f, "https://github.com/", ICON));
        assert!(remember(&mut f, "https://github.com/", "data:image/png;base64,BBBB"));
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn only_small_images_are_kept() {
        let mut f = Favs::new();
        assert!(!remember(&mut f, "https://a.test/", "not a data uri"));
        assert!(!remember(&mut f, "https://a.test/", &format!("data:image/png;base64,{}", "A".repeat(MAX_BYTES))));
        assert!(!remember(&mut f, "about:blank", ICON));
        assert!(f.is_empty());
    }

    #[test]
    fn a_full_cache_takes_no_new_site_but_refreshes_old_ones() {
        let mut f = Favs::new();
        for n in 0..MAX_SITES {
            assert!(remember(&mut f, &format!("https://s{n}.test/"), ICON));
        }
        assert!(!remember(&mut f, "https://new.test/", ICON));
        assert!(remember(&mut f, "https://s0.test/", "data:image/png;base64,CCCC"));
        assert_eq!(f.len(), MAX_SITES);
    }
}
