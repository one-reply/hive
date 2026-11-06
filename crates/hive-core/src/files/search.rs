//! Substring matching and tree projection for the Files navigator, without UI or I/O.

use super::tree::{Contents, Node, NodeKind};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

/// Matching files and their ancestors, with byte ranges in each displayed basename.
#[derive(Default)]
pub struct Matches {
    rows: BTreeMap<PathBuf, Vec<Range<usize>>>,
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
        self.rows.get(path).map(Vec::as_slice).unwrap_or_default()
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
    }
}

/// Finds case-insensitive substrings of relative paths. Hidden ancestors hide a whole path.
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
        if !folded.contains(&query) {
            continue;
        }
        // Lowercasing can expand a character (İ → i + combining dot). Keep offsets
        // in the original label, never use lowercase byte offsets to slice UTF-8.
        let original: Vec<_> = text
            .char_indices()
            .flat_map(|(offset, character)| {
                let bytes = character.to_lowercase().map(char::len_utf8).sum();
                std::iter::repeat_n(offset..offset + character.len_utf8(), bytes)
            })
            .collect();
        let ranges: Vec<_> = folded
            .match_indices(&query)
            .map(|(start, value)| original[start].start..original[start + value.len() - 1].end)
            .collect();
        for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
            let name = ancestor.file_name().unwrap_or_default().to_string_lossy();
            let end = ancestor.to_string_lossy().len();
            let start = end - name.len();
            let highlights = matches.rows.entry(ancestor.into()).or_default();
            for range in &ranges {
                let left = range.start.max(start);
                let right = range.end.min(end);
                if left < right {
                    highlights.push(left - start..right - start);
                }
            }
        }
    }
    for ranges in matches.rows.values_mut() {
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

fn lowercase(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        files::{
            Entry,
            tree::{self, Source},
        },
        settings::FilesSettings,
    };

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
