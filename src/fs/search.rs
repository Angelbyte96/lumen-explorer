//! Quick-jump search over the [`Index`]: what was typed is looked for in each
//! name as letters in order, not necessarily together ("down" finds
//! "Windows NT"), ignoring case and spaces. Folders visited often come
//! first, like File Pilot's; then better matches: the whole name, then its
//! start, then the letters together, then apart; among equal ones, the
//! shallowest. After them, what is in the first folder found.
//! Measured (release, a million 35-letter names): 25-40 ms a query, 76 ms
//! for one letter, which matches nearly all.

// The path box uses it from fase-9 T4 on.
#![cfg_attr(not(test), allow(dead_code))]

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::ops::Range;
use std::path::PathBuf;

use super::index::Index;

/// One result: the entry, its path and the name's letters to mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub id: u32,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Byte ranges of the name that matched; empty for an entry listed
    /// because it is in the first folder found.
    pub ranges: Vec<Range<usize>>,
}

/// Where the letters of `query` are in `name`, as byte ranges, or `None`
/// when they are not all there in order.
/// The letters together when they are, else each at its first place after
/// the one before.
#[allow(clippy::single_range_in_vec_init)] // one range, not a range of ranges
pub fn letters(name: &str, query: &str) -> Option<Vec<Range<usize>>> {
    let query: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(fold).collect();
    if query.is_empty() {
        return None;
    }
    // Letter by letter, so a range in the folded name is one in `name`.
    let name_letters: Vec<(usize, char)> = name.char_indices().map(|(at, c)| (at, fold(c))).collect();
    let end_of = |k: usize| name_letters.get(k).map_or(name.len(), |(at, _)| *at);
    let together = (0..name_letters.len())
        .find(|&s| s + query.len() <= name_letters.len() && (0..query.len()).all(|k| name_letters[s + k].1 == query[k]));
    if let Some(start) = together {
        return Some(vec![name_letters[start].0..end_of(start + query.len())]);
    }
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let mut next = 0;
    for (k, &(at, c)) in name_letters.iter().enumerate() {
        if next < query.len() && c == query[next] {
            next += 1;
            match ranges.last_mut() {
                Some(range) if range.end == at => range.end = end_of(k + 1),
                _ => ranges.push(at..end_of(k + 1)),
            }
        }
    }
    (next == query.len()).then_some(ranges)
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Whether `name` holds the letters in order: the quick test every entry
/// goes through. An ASCII query compares bytes.
fn has_letters(name: &str, query: &[char], ascii: Option<&[u8]>) -> bool {
    let mut next = 0;
    if let Some(query) = ascii {
        for byte in name.bytes() {
            if byte.to_ascii_lowercase() == query[next] {
                next += 1;
                if next == query.len() {
                    return true;
                }
            }
        }
        return false;
    }
    for c in name.chars() {
        if fold(c) == query[next] {
            next += 1;
            if next == query.len() {
                return true;
            }
        }
    }
    false
}

/// How well `name` matches: 3 the whole name, 2 its start, 1 the letters
/// together, 0 apart. `lower` is a buffer kept between calls.
fn quality(name: &str, typed: &str, compact: &str, lower: &mut String) -> u8 {
    lower.clear();
    if name.is_ascii() {
        lower.push_str(name);
        lower.make_ascii_lowercase();
    } else {
        lower.extend(name.chars().flat_map(char::to_lowercase));
    }
    if lower == typed {
        3
    } else if lower.starts_with(typed) {
        2
    } else if lower.contains(compact) {
        1
    } else {
        0
    }
}

struct Found {
    id: u32,
    visits: f64,
    quality: u8,
    depth: usize,
}

/// Better first: `Less` when `a` ranks above `b`.
fn rank(a: &Found, b: &Found) -> Ordering {
    (b.visits > 0.0)
        .cmp(&(a.visits > 0.0))
        .then(b.quality.cmp(&a.quality))
        .then(b.visits.total_cmp(&a.visits))
        .then(a.depth.cmp(&b.depth))
        .then(a.id.cmp(&b.id))
}

/// Ordered by [`rank`], so a heap's top is the worst kept.
struct Ranked(Found);

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        rank(&self.0, &other.0)
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ranked {}

/// The best `limit` hits for `query`. Hidden entries (and all inside hidden
/// folders) only when shown or when the query starts with a dot. `frecent`
/// holds visited folders and their scores.
pub fn search(index: &Index, query: &str, show_hidden: bool, frecent: &[(PathBuf, f64)], limit: usize) -> Vec<Hit> {
    let typed = query.trim().to_lowercase();
    let compact: String = typed.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() || limit == 0 {
        return Vec::new();
    }
    let letters_typed: Vec<char> = compact.chars().map(fold).collect();
    let ascii = compact.is_ascii().then_some(compact.as_bytes());
    let hidden_ok = show_hidden || typed.starts_with('.');
    // Only an entry named like a visited folder needs its path looked up.
    let frecent_names: HashSet<&str> = frecent.iter().filter_map(|(p, _)| p.file_name()?.to_str()).collect();

    // Only the best `limit` are kept while scanning: one letter matches
    // nearly every entry.
    let mut best: BinaryHeap<Ranked> = BinaryHeap::with_capacity(limit + 1);
    let mut lower = String::new();
    for id in 0..index.len() as u32 {
        // Roots ("/", "C:\") are where the search looks, not results.
        if index.parent(id).is_none() || (!hidden_ok && index.is_hidden(id)) {
            continue;
        }
        let name = index.name(id);
        if !has_letters(name, &letters_typed, ascii) {
            continue;
        }
        let visits = if !frecent_names.is_empty() && frecent_names.contains(name) {
            let path = index.path(id);
            frecent.iter().find(|(p, _)| *p == path).map_or(0.0, |(_, score)| *score)
        } else {
            0.0
        };
        let found = Found { id, visits, quality: quality(name, &typed, &compact, &mut lower), depth: index.depth(id) };
        if best.len() == limit {
            if best.peek().is_some_and(|worst| rank(&found, &worst.0).is_ge()) {
                continue;
            }
            best.pop();
        }
        best.push(Ranked(found));
    }
    let found = best.into_sorted_vec();

    let mut hits: Vec<Hit> = found
        .iter()
        .map(|Ranked(f)| Hit {
            id: f.id,
            path: index.path(f.id),
            is_dir: index.is_dir(f.id),
            ranges: letters(index.name(f.id), query).unwrap_or_default(),
        })
        .collect();
    if let Some(folder) = hits.iter().find(|h| h.is_dir).map(|h| h.id) {
        let mut inside: Vec<u32> = (0..index.len() as u32)
            .filter(|&id| index.parent(id) == Some(folder) && (hidden_ok || !index.is_hidden(id)))
            .filter(|&id| !hits.iter().any(|h| h.id == id))
            .collect();
        inside.sort_by_cached_key(|&id| (!index.is_dir(id), index.name(id).to_lowercase()));
        for id in inside.into_iter().take(limit - hits.len()) {
            hits.push(Hit { id, path: index.path(id), is_dir: index.is_dir(id), ranges: Vec::new() });
        }
    }
    hits
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    /// An index from paths below one root "/r"; folders end with a slash.
    fn index(paths: &[&str]) -> Index {
        let mut index = Index::default();
        let root = index.add(None, "/r", true);
        let mut ids: Vec<(String, u32)> = vec![(String::new(), root)];
        for path in paths {
            let is_dir = path.ends_with('/');
            let path = path.trim_end_matches('/');
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
            let parent = ids.iter().find(|(p, _)| p == parent).expect("parent listed first").1;
            let id = index.add(Some(parent), name, is_dir);
            ids.push((path.to_string(), id));
        }
        index
    }

    fn names(index: &Index, hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|h| index.name(h.id).to_string()).collect()
    }

    #[test]
    fn letters_are_found_in_order_apart_and_whatever_their_case() {
        assert_eq!(letters("Windows NT", "down"), Some(vec![3..6, 8..9]));
        assert_eq!(letters("Downloads", "DOWN"), Some(vec![0..4]));
        assert_eq!(letters("Windows NT", "win nt"), Some(vec![0..3, 8..10]), "spaces are ignored");
        assert_eq!(letters("Árbol.txt", "árb"), Some(vec![0..4]), "not only ASCII");
        assert_eq!(letters("Windows", "nwd"), None, "out of order");
        assert_eq!(letters("Windows", ""), None, "nothing typed finds nothing");
    }

    #[test]
    fn the_whole_name_then_its_start_then_together_then_apart() {
        let index = index(&["Windows NT/", "my-downloads/", "Downloads/", "down/"]);
        let hits = search(&index, "down", false, &[], 10);
        assert_eq!(names(&index, &hits), ["down", "Downloads", "my-downloads", "Windows NT"]);
        assert_eq!(hits[1].path, PathBuf::from("/r/Downloads"));
        assert!(hits[1].is_dir);
        assert_eq!(hits[3].ranges, [3..6, 8..9]);
    }

    #[test]
    fn among_equal_matches_the_shallowest_comes_first_and_a_visited_folder_before_all() {
        let index = index(&["a/", "a/b/", "a/b/Downloads/", "c/", "c/Downloads/", "Downloads.txt"]);
        let hits = search(&index, "downloads", false, &[], 10);
        assert_eq!(hits.iter().map(|h| h.path.clone()).collect::<Vec<_>>()[..3], [
            PathBuf::from("/r/c/Downloads"),
            PathBuf::from("/r/a/b/Downloads"),
            PathBuf::from("/r/Downloads.txt"),
        ]);
        let frecent = [(PathBuf::from("/r/a/b/Downloads"), 10.0)];
        let hits = search(&index, "downloads", false, &frecent, 10);
        assert_eq!(hits[0].path, PathBuf::from("/r/a/b/Downloads"));
    }

    #[test]
    fn a_visited_folder_beats_a_better_match() {
        let index = index(&["Downloads/", "down/"]);
        let frecent = [(PathBuf::from("/r/Downloads"), 10.0)];
        assert_eq!(names(&index, &search(&index, "down", false, &frecent, 10)), ["Downloads", "down"]);
    }

    #[test]
    fn hidden_entries_and_all_inside_hidden_folders_only_when_shown_or_typed_with_a_dot() {
        let index = index(&[".config/", ".config/lumen/", "lumen.txt"]);
        assert_eq!(names(&index, &search(&index, "lumen", false, &[], 10)), ["lumen.txt"]);
        assert_eq!(names(&index, &search(&index, "lumen", true, &[], 10)), ["lumen", "lumen.txt"]);
        assert_eq!(names(&index, &search(&index, ".conf", false, &[], 10)), [".config", "lumen"], "and what it holds");
    }

    #[test]
    fn what_is_in_the_first_folder_found_follows_the_matches_up_to_the_limit() {
        let index = index(&["Downloads/", "Downloads/song.mp3", "Downloads/photos/", "Windows NT/", "x/", "x/a.txt"]);
        let hits = search(&index, "down", false, &[], 10);
        assert_eq!(names(&index, &hits), ["Downloads", "Windows NT", "photos", "song.mp3"]);
        assert!(hits[2].ranges.is_empty(), "listed for its folder, not for a match");
        assert_eq!(search(&index, "down", false, &[], 3).len(), 3);
    }

    #[test]
    fn nothing_typed_or_nothing_matching_finds_nothing() {
        let index = index(&["Downloads/"]);
        assert!(search(&index, "", false, &[], 10).is_empty());
        assert!(search(&index, "  ", false, &[], 10).is_empty());
        assert!(search(&index, "zzz", false, &[], 10).is_empty());
    }

    /// A million names, timed: `cargo test --release search_is_fast -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing"]
    fn search_is_fast_on_a_million_entries() {
        let mut index = Index::default();
        let root = index.add(None, "/r", true);
        for i in 0..1_000_000u32 {
            index.add(Some(root), &format!("file number {i} of some folder.txt"), false);
        }
        for query in ["d", "down", "number 99", "zzz", "d", "d"] {
            let start = std::time::Instant::now();
            let hits = search(&index, query, false, &[], 50);
            eprintln!("{query:>10}: {} hits in {:?}", hits.len(), start.elapsed());
        }
    }
}
