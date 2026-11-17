//! Ranked fuzzy matching and tree projection for the Files navigator, without UI or I/O.

use super::tree::{Contents, Node, NodeKind};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

/// Matching files and their ancestors, with byte ranges in each displayed basename.
#[derive(Default)]
pub struct Matches {
    rows: BTreeMap<PathBuf, Match>,
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Match {
    ranges: Vec<Range<usize>>,
    score: usize,
}

impl Matches {
    /// Directories whose full listing is needed to render and act on the results.
    pub fn directories(&self) -> BTreeSet<PathBuf> {
        self.rows
            .keys()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect()
    }

    /// Matching byte ranges in the original, unmodified row label.
    pub fn highlights(&self, path: &Path) -> &[Range<usize>] {
        self.rows
            .get(path)
            .map(|row| row.ranges.as_slice())
            .unwrap_or_default()
    }

    /// Retains matching files and their ancestors, opening all but manually closed folders.
    pub fn filter(&self, nodes: &mut Vec<Node>, collapsed: &HashSet<PathBuf>) {
        nodes.retain_mut(|node| {
            if !self.rows.contains_key(&node.path) {
                return false;
            }
            match &mut node.kind {
                NodeKind::File => true,
                NodeKind::Directory { expanded, contents } => {
                    let Contents::Children(children) = contents else {
                        return false;
                    };
                    self.filter(children, collapsed);
                    *expanded = !collapsed.contains(&node.path);
                    !children.is_empty()
                }
            }
        });
        nodes.sort_by_key(|node| {
            std::cmp::Reverse(self.rows.get(&node.path).map_or(0, |row| row.score))
        });
    }

    /// Boosts equally strong matches using most-recent-first paths from this worktree.
    pub fn prefer_recent(&mut self, paths: &[PathBuf]) {
        for (index, path) in paths.iter().take(10).enumerate() {
            if !self.rows.contains_key(path) {
                continue;
            }
            let score = self.rows.get(path).map_or(0, |row| row.score) + 10 - index;
            for ancestor in path.ancestors() {
                if let Some(row) = self.rows.get_mut(ancestor) {
                    row.score = row.score.max(score);
                }
            }
        }
    }
}

/// Finds case-insensitive substrings of the path and subsequences of the file name, ranking
/// exact names, prefixes and substrings first. A query with `/` also matches as a subsequence
/// of the whole path.
/// Hidden ancestors hide a whole path.
/// An empty query has no matches: callers use the ordinary tree in that case.
pub fn find(paths: &[PathBuf], query: &str, show_hidden: bool) -> Matches {
    let query = lowercase(query);
    let mut matches = Matches::default();
    if query.is_empty() {
        return matches;
    }
    for path in paths {
        if !show_hidden
            && path
                .iter()
                .any(|part| part.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let text = path.to_string_lossy();
        let folded = lowercase(&text);
        let contiguous = folded.contains(&query);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let name_start = text.len() - name.len();
        let fuzzy = if contiguous {
            None
        } else {
            subsequence(&name, &query)
                .map(|ranges| {
                    ranges
                        .into_iter()
                        .map(|r| r.start + name_start..r.end + name_start)
                        .collect()
                })
                // Across directories only when the query names a path: the letters of a short
                // word appear, in order, somewhere along most paths of a repository.
                .or_else(|| {
                    query
                        .contains('/')
                        .then(|| subsequence(&text, &query))
                        .flatten()
                })
        };
        if !contiguous && fuzzy.is_none() {
            continue;
        }
        let name = lowercase(&name);
        let score = if name == query {
            500
        } else if name.starts_with(&query) {
            400
        } else if name.contains(&query) {
            300
        } else if contiguous {
            200
        } else {
            fuzzy
                .as_ref()
                .and_then(|ranges| ranges.first().zip(ranges.last()))
                .filter(|(first, _)| first.start >= name_start)
                .map_or(100, |(first, last)| {
                    120 + 50usize.saturating_sub(text[first.start..last.end].chars().count())
                })
        };
        let ranges: Vec<_> = if let Some(ranges) = fuzzy {
            ranges
        } else if text.is_ascii() {
            folded
                .match_indices(&query)
                .map(|(start, value)| start..start + value.len())
                .collect()
        } else {
            // Lowercasing can expand a character (İ → i + combining dot). Keep offsets
            // in the original label, never use lowercase byte offsets to slice UTF-8.
            let original: Vec<_> = text
                .char_indices()
                .flat_map(|(offset, character)| {
                    let bytes = character.to_lowercase().map(char::len_utf8).sum();
                    std::iter::repeat_n(offset..offset + character.len_utf8(), bytes)
                })
                .collect();
            folded
                .match_indices(&query)
                .map(|(start, value)| original[start].start..original[start + value.len() - 1].end)
                .collect()
        };
        for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            let name = ancestor.file_name().unwrap_or_default().to_string_lossy();
            let end = ancestor.to_string_lossy().len();
            let start = end - name.len();
            let row = matches.rows.entry(ancestor.into()).or_default();
            row.score = row.score.max(score);
            let highlights = &mut row.ranges;
            for range in &ranges {
                let left = range.start.max(start);
                let right = range.end.min(end);
                if left < right {
                    highlights.push(left - start..right - start);
                }
            }
        }
    }
    for row in matches.rows.values_mut() {
        let ranges = &mut row.ranges;
        if ranges.len() < 2 {
            continue;
        }
        ranges.sort_unstable_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in ranges.drain(..) {
            if let Some(last) = merged.last_mut()
                && range.start <= last.end
            {
                last.end = last.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        *ranges = merged;
    }
    matches
}

fn subsequence(text: &str, query: &str) -> Option<Vec<Range<usize>>> {
    let mut needle = query.chars();
    let mut next = needle.next()?;
    let mut ranges = Vec::new();
    for (offset, character) in text.char_indices() {
        for folded in character.to_lowercase() {
            if folded == next {
                ranges.push(offset..offset + character.len_utf8());
                match needle.next() {
                    Some(character) => next = character,
                    None => return Some(ranges),
                }
            }
        }
    }
    None
}

fn lowercase(text: &str) -> String {
    if text.is_ascii() {
        text.to_ascii_lowercase()
    } else {
        text.chars().flat_map(char::to_lowercase).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_filename_initials_rank_above_sparse_directory_matches() {
        let paths = vec![
            PathBuf::from("node_modules/.bin/typescript-language-server"),
            PathBuf::from("scopes.ts"),
        ];
        let found = find(&paths, "scps", true);
        assert!(found.rows[&paths[1]].score > found.rows[&paths[0]].score);
        assert!(found.highlights(Path::new("node_modules")).is_empty());
    }
    use crate::{
        files::{
            Entry,
            tree::{self, Source},
        },
        settings::FilesSettings,
    };

    #[test]
    fn fuzzy_unicode_ranges_are_valid_and_names_rank_above_path_matches() {
        let paths = vec![
            "Код/ЇжакТест.rs".into(),
            "src/it.rs".into(),
            "it/other.rs".into(),
        ];
        let matches = find(&paths, "їт", true);
        let name = "ЇжакТест.rs";
        assert_eq!(
            matches
                .highlights(&paths[0])
                .iter()
                .map(|range| &name[range.clone()])
                .collect::<Vec<_>>(),
            ["Ї", "Т"]
        );
        let mut matches = find(&paths, "it", true);
        assert!(matches.rows[&paths[1]].score > matches.rows[&paths[2]].score);
        matches.prefer_recent(&[paths[2].clone()]);
        assert!(matches.rows[&paths[1]].score > matches.rows[&paths[2]].score);
    }

    #[test]
    fn a_word_is_not_matched_letter_by_letter_across_directories() {
        let paths: Vec<PathBuf> = vec![
            "crates/hive/src/state/files.rs".into(),
            "crates/hive/src/state/tests.rs".into(),
            "crates/hive/src/terminal_session_input.rs".into(),
        ];
        // "t…e…s…t" runs through "crates/…/state" of every path; only names may be fuzzy.
        let found = find(&paths, "test", true);
        assert!(!found.rows.contains_key(&paths[0]));
        assert!(found.rows[&paths[1]].score > found.rows[&paths[2]].score);
        // A path-shaped query asks for exactly that.
        assert!(
            find(&paths, "hive/stfil", true)
                .rows
                .contains_key(&paths[0])
        );
        assert!(
            find(&paths, "state/files", true)
                .rows
                .contains_key(&paths[0])
        );
    }

    #[test]
    fn a_large_index_has_stable_results() {
        let paths: Vec<PathBuf> = (0..20_000)
            .map(|n| format!("src/group{n}/example{n}.rs").into())
            .collect();
        let a = find(&paths, "ex999", true);
        let b = find(&paths, "ex999", true);
        assert_eq!(a.rows, b.rows);
        assert!(
            a.rows
                .contains_key(&PathBuf::from("src/group999/example999.rs"))
        );
    }

    #[test]
    fn matches_names_paths_case_and_multiple_occurrences() {
        let paths = vec!["src/Main-main.rs".into(), "docs/main.md".into()];
        let matches = find(&paths, "MAIN", true);
        assert_eq!(
            matches.highlights(Path::new("src/Main-main.rs")),
            &[0..4, 5..9]
        );
        assert_eq!(
            matches.directories(),
            BTreeSet::from(["".into(), "src".into(), "docs".into()])
        );
        let matches = find(&paths, "SRC/ma", true);
        assert_eq!(
            matches.highlights(Path::new("src")),
            std::slice::from_ref(&(0..3))
        );
        assert_eq!(
            matches.highlights(Path::new("src/Main-main.rs")),
            std::slice::from_ref(&(0..2))
        );
        assert!(!matches.rows.contains_key(Path::new("docs")));
    }

    #[test]
    fn highlights_original_utf8_even_when_lowercase_expands() {
        let paths = vec!["Код/ЇЖАК-İ.rs".into()];
        let matches = find(&paths, "їжак-i", true);
        let name = "ЇЖАК-İ.rs";
        let ranges = matches.highlights(&paths[0]);
        assert_eq!(&name[ranges[0].clone()], "ЇЖАК-İ");
    }

    #[test]
    fn ascii_and_unicode_paths_keep_original_offsets_and_simple_case_folding() {
        let paths = vec!["SRC/TEST-test.rs".into(), "Код/TEST-test.rs".into()];
        let matches = find(&paths, "test", true);
        for path in &paths {
            assert_eq!(matches.highlights(path), &[0..4, 5..9]);
        }
        assert_eq!(lowercase("ΟΣ"), "οσ");
        assert_eq!(lowercase("AİΣ"), "ai\u{307}σ");
        assert!(find(&paths, "ї", true).rows.is_empty());
    }

    #[test]
    fn empty_missing_and_hidden_queries_do_not_leak_rows() {
        let paths = vec![".hidden/main.rs".into(), "src/.main.rs".into()];
        for query in ["", "absent"] {
            assert!(find(&paths, query, true).rows.is_empty());
        }
        assert!(find(&paths, "main", false).rows.is_empty());
        assert_eq!(
            find(&paths, "main", true).highlights(&paths[0]),
            std::slice::from_ref(&(0..4))
        );
    }

    #[test]
    fn projection_keeps_only_results_and_ancestors_in_tree_order() {
        let entry = |path: &str, directory| Entry {
            path: path.into(),
            directory,
            unsupported: false,
        };
        let entries = BTreeMap::from([
            (
                PathBuf::new(),
                vec![
                    entry("z.rs", false),
                    entry("src", true),
                    entry("other", true),
                ],
            ),
            (
                "src".into(),
                vec![
                    entry("src/b.rs", false),
                    entry("src/a.rs", false),
                    entry("src/no.txt", false),
                ],
            ),
            ("other".into(), vec![entry("other/no.txt", false)]),
        ]);
        let mut tree = tree::build(&Source {
            entries: &entries,
            expanded: &HashSet::new(),
            ignored: &HashSet::new(),
            decorations: &BTreeMap::new(),
            settings: &FilesSettings::default(),
            destination_for: None,
        });
        let paths = vec!["src/b.rs".into(), "src/a.rs".into(), "z.rs".into()];
        let matches = find(&paths, ".rs", true);
        matches.filter(&mut tree.nodes, &HashSet::new());
        assert_eq!(
            tree.nodes.iter().map(Node::name).collect::<Vec<_>>(),
            ["src", "z.rs"]
        );
        let NodeKind::Directory {
            expanded,
            contents: Contents::Children(children),
        } = &tree.nodes[0].kind
        else {
            panic!("directory expected")
        };
        assert!(*expanded);
        assert_eq!(
            children.iter().map(Node::name).collect::<Vec<_>>(),
            ["a.rs", "b.rs"]
        );
        matches.filter(&mut tree.nodes, &HashSet::from(["src".into()]));
        assert!(matches!(
            tree.nodes[0].kind,
            NodeKind::Directory {
                expanded: false,
                ..
            }
        ));
    }
}
