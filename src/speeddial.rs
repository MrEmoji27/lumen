//! The new tab page's speed dial: tiles you keep, in a grid, and what to suggest when you keep none.

use crate::store::Shortcut;

pub const MAX_TILES: usize = 16;
const MIN_COLS: usize = 4;
const MAX_COLS: usize = 6;
const MIN_CELL: f32 = 88.0;
const MAX_CELL: f32 = 112.0;
const GAP: f32 = 12.0;
pub const ROW_H: f32 = 104.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub cols: usize,
    pub cell_w: f32,
    pub n: usize,
    pub x0: f32,
}

impl Grid {
    pub fn rows(&self) -> usize {
        self.n.div_ceil(self.cols.max(1))
    }

    pub fn height(&self) -> f32 {
        self.rows() as f32 * ROW_H
    }

    pub fn origin(&self, i: usize) -> (f32, f32) {
        let (row, col) = (i / self.cols, i % self.cols);
        let in_row = if row + 1 == self.rows() { self.n - row * self.cols } else { self.cols };
        let row_w = in_row as f32 * self.cell_w + (in_row.saturating_sub(1)) as f32 * GAP;
        let full_w = self.cols as f32 * self.cell_w + (self.cols - 1) as f32 * GAP;
        let lead = self.x0 + (full_w - row_w) / 2.0;
        (lead + col as f32 * (self.cell_w + GAP), row as f32 * ROW_H)
    }

    pub fn index_at(&self, x: f32, y: f32) -> Option<usize> {
        if y < 0.0 {
            return None;
        }
        (0..self.n).find(|&i| {
            let (cx, cy) = self.origin(i);
            x >= cx && x < cx + self.cell_w && y >= cy && y < cy + ROW_H
        })
    }
}

pub fn grid(width: f32, n: usize) -> Grid {
    let fit = ((width + GAP) / (MIN_CELL + GAP)).floor() as usize;
    let cols = fit.clamp(MIN_COLS, MAX_COLS);
    let cell_w = ((width - GAP * (cols as f32 - 1.0)) / cols as f32).clamp(56.0, MAX_CELL);
    Grid { cols, cell_w, n, x0: 0.0 }
}

pub fn grid_in(left: f32, width: f32, n: usize) -> Grid {
    let g = grid(width, n);
    let full_w = g.cols as f32 * g.cell_w + (g.cols - 1) as f32 * GAP;
    Grid { x0: left + (width - full_w).max(0.0) / 2.0, ..g }
}

pub fn clean_url(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return None;
    }
    let url = if t.contains("://") { t.to_string() } else { format!("https://{t}") };
    let host = host_of(&url);
    if !(url.starts_with("http://") || url.starts_with("https://")) || host.is_empty() || !host.contains('.') && host != "localhost" && !host.starts_with("localhost:") {
        return None;
    }
    Some(url)
}

pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or("");
    let h = rest.split(['/', '?', '#']).next().unwrap_or("");
    let h = h.rsplit('@').next().unwrap_or(h);
    h.strip_prefix("www.").unwrap_or(h).to_lowercase()
}

pub fn label(sc: &Shortcut) -> String {
    if sc.title.trim().is_empty() { host_of(&sc.url) } else { sc.title.trim().to_string() }
}

pub fn add(tiles: &mut Vec<Shortcut>, title: &str, url: &str) -> bool {
    let Some(url) = clean_url(url) else { return false };
    if tiles.len() >= MAX_TILES || tiles.iter().any(|t| same_site_page(&t.url, &url)) {
        return false;
    }
    tiles.push(Shortcut { title: title.trim().to_string(), url });
    true
}

pub fn edit(tiles: &mut [Shortcut], i: usize, title: &str, url: &str) -> bool {
    let Some(url) = clean_url(url) else { return false };
    if i >= tiles.len() || tiles.iter().enumerate().any(|(n, t)| n != i && same_site_page(&t.url, &url)) {
        return false;
    }
    tiles[i] = Shortcut { title: title.trim().to_string(), url };
    true
}

pub fn reorder(tiles: &mut Vec<Shortcut>, from: usize, to: usize) {
    if from >= tiles.len() || to >= tiles.len() || from == to {
        return;
    }
    let t = tiles.remove(from);
    tiles.insert(to, t);
}

fn same_site_page(a: &str, b: &str) -> bool {
    let key = |u: &str| u.trim().trim_end_matches('/').to_lowercase();
    key(a) == key(b)
}

#[derive(Clone, Debug)]
pub struct Seen {
    pub url: String,
}

pub fn suggest(seen: &[Seen], tiles: &[Shortcut], limit: usize) -> Vec<Shortcut> {
    let mut have: Vec<String> = tiles.iter().map(|t| host_of(&t.url)).collect();
    let mut out = Vec::new();
    for s in seen {
        if out.len() >= limit {
            break;
        }
        if !(s.url.starts_with("http://") || s.url.starts_with("https://")) {
            continue;
        }
        let host = host_of(&s.url);
        if host.is_empty() || host.starts_with("localhost") || host.starts_with("127.") || is_search(&s.url, &host) || have.contains(&host) {
            continue;
        }
        let scheme = if s.url.starts_with("http://") { "http" } else { "https" };
        have.push(host.clone());
        out.push(Shortcut { title: String::new(), url: format!("{scheme}://{host}/") });
    }
    out
}

fn is_search(url: &str, host: &str) -> bool {
    let q = url.contains("?q=") || url.contains("&q=") || url.contains("/search?");
    q && ["google.", "bing.", "duckduckgo.", "brave.", "startpage.", "yahoo."].iter().any(|s| host.contains(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(title: &str, url: &str) -> Shortcut {
        Shortcut { title: title.into(), url: url.into() }
    }
    fn seen(url: &str) -> Seen {
        Seen { url: url.into() }
    }

    #[test]
    fn four_to_six_columns_by_width() {
        assert_eq!(grid(300.0, 8).cols, 4, "never fewer than four");
        assert_eq!(grid(420.0, 8).cols, 4);
        assert_eq!(grid(520.0, 8).cols, 5);
        assert_eq!(grid(640.0, 8).cols, 6);
        assert_eq!(grid(2000.0, 8).cols, 6, "never more than six");
    }

    #[test]
    fn cells_are_as_wide_as_the_room_gives_up_to_a_limit() {
        let g = grid(1000.0, 8);
        assert_eq!(g.cell_w, MAX_CELL);
        let g = grid(400.0, 8);
        assert!((g.cell_w * 4.0 + GAP * 3.0 - 400.0).abs() < 0.01);
    }

    #[test]
    fn rows_count_rounds_up() {
        assert_eq!(grid(700.0, 6).rows(), 1);
        assert_eq!(grid(700.0, 7).rows(), 2);
        assert_eq!(grid(700.0, 17).rows(), 3);
        assert_eq!(grid(700.0, 7).height(), 2.0 * ROW_H);
    }

    #[test]
    fn the_grid_sits_in_the_middle_of_its_space() {
        let g = grid_in(100.0, 800.0, 6);
        let (first, _) = g.origin(0);
        let (last, _) = g.origin(5);
        let right = last + g.cell_w;
        assert!((first - 100.0 - (900.0 - right)).abs() < 0.01);
    }

    #[test]
    fn a_short_last_row_is_centred_under_the_full_ones() {
        let g = grid_in(0.0, 700.0, 7);
        let (full0, _) = g.origin(0);
        let (alone, y) = g.origin(6);
        assert_eq!(y, ROW_H);
        assert!(alone > full0 + g.cell_w);
    }

    #[test]
    fn a_point_finds_its_cell() {
        let g = grid_in(0.0, 700.0, 8);
        for i in 0..8 {
            let (x, y) = g.origin(i);
            assert_eq!(g.index_at(x + 3.0, y + 3.0), Some(i));
        }
        assert_eq!(g.index_at(-5.0, 10.0), None);
        assert_eq!(g.index_at(10.0, -1.0), None);
        let (x, _) = g.origin(7);
        assert_eq!(g.index_at(x + 3.0, 5.0 * ROW_H), None);
    }

    #[test]
    fn addresses_are_cleaned() {
        assert_eq!(clean_url(" example.com ").as_deref(), Some("https://example.com"));
        assert_eq!(clean_url("http://localhost:3000").as_deref(), Some("http://localhost:3000"));
        assert_eq!(clean_url("https://a.test/x?y=1").as_deref(), Some("https://a.test/x?y=1"));
        assert_eq!(clean_url("two words"), None);
        assert_eq!(clean_url(""), None);
        assert_eq!(clean_url("javascript:alert(1)"), None);
        assert_eq!(clean_url("justaword"), None);
    }

    #[test]
    fn adding_stops_at_the_cap_and_at_duplicates() {
        let mut t = vec![sc("A", "https://a.test/")];
        assert!(!add(&mut t, "A again", "https://a.test"));
        assert!(!add(&mut t, "x", "not a url at all"));
        assert!(add(&mut t, "  B ", "b.test"));
        assert_eq!(t[1], sc("B", "https://b.test"));
        let mut full: Vec<Shortcut> = (0..MAX_TILES).map(|n| sc("", &format!("https://s{n}.test/"))).collect();
        assert!(!add(&mut full, "", "https://one-more.test/"));
        assert_eq!(full.len(), MAX_TILES);
    }

    #[test]
    fn editing_cannot_collide_with_another_tile() {
        let mut t = vec![sc("A", "https://a.test/"), sc("B", "https://b.test/")];
        assert!(!edit(&mut t, 1, "B", "https://a.test"));
        assert!(edit(&mut t, 1, "Bee", "bee.test"));
        assert_eq!(t[1], sc("Bee", "https://bee.test"));
        assert!(edit(&mut t, 0, "Alpha", "https://a.test/"));
        assert!(!edit(&mut t, 9, "", "https://z.test"));
    }

    #[test]
    fn reordering_moves_one_tile() {
        let mut t: Vec<Shortcut> = ["a", "b", "c", "d"].iter().map(|n| sc(n, &format!("https://{n}.test/"))).collect();
        reorder(&mut t, 0, 2);
        assert_eq!(t.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["b", "c", "a", "d"]);
        reorder(&mut t, 3, 0);
        assert_eq!(t.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(), ["d", "b", "c", "a"]);
        reorder(&mut t, 1, 1);
        reorder(&mut t, 9, 0);
        assert_eq!(t.len(), 4);
    }

    #[test]
    fn a_tile_is_called_by_its_title_else_its_site() {
        assert_eq!(label(&sc("Mail", "https://mail.example.com/inbox")), "Mail");
        assert_eq!(label(&sc("  ", "https://www.example.com/inbox")), "example.com");
    }

    #[test]
    fn suggestions_are_the_sites_seen_most_one_each() {
        let s = [
            seen("https://music.youtube.com/watch?v=1"),
            seen("https://music.youtube.com/playlist?list=2"),
            seen("https://www.github.com/rust-lang/rust"),
            seen("https://github.com/"),
            seen("https://news.ycombinator.com/item?id=5"),
        ];
        let got = suggest(&s, &[], 10);
        let urls: Vec<&str> = got.iter().map(|g| g.url.as_str()).collect();
        assert_eq!(urls, ["https://music.youtube.com/", "https://github.com/", "https://news.ycombinator.com/"]);
    }

    #[test]
    fn suggestions_skip_tiles_you_have_searches_and_non_places() {
        let s = [
            seen("https://github.com/anthropics"),
            seen("https://duckduckgo.com/?q=rust+book"),
            seen("https://www.google.com/search?q=cats"),
            seen("file:///C:/notes.txt"),
            seen("http://localhost:3000/"),
            seen("https://docs.rs/serde"),
        ];
        let tiles = [sc("GitHub", "https://www.github.com")];
        let got = suggest(&s, &tiles, 10);
        assert_eq!(got.iter().map(|g| g.url.as_str()).collect::<Vec<_>>(), ["https://docs.rs/"]);
    }

    #[test]
    fn suggestions_stop_at_the_limit() {
        let s: Vec<Seen> = (0..20).map(|n| seen(&format!("https://site{n}.test/"))).collect();
        assert_eq!(suggest(&s, &[], 6).len(), 6);
        assert!(suggest(&s, &[], 0).is_empty());
    }
}
