//! The file tree as the navigator shows it: listed entries filtered by the user's settings,
//! folders first, with deleted-but-tracked paths synthesised so a Git deletion stays visible.
//! Pure over the listed directories, so it is testable without a window.

use super::Entry;
use super::decorations::Decoration;
use crate::git::status::Change;
use crate::settings::FilesSettings;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// What a tree is built from.
pub struct Source<'a> {
    /// Listed directories keyed by worktree-relative path; `""` is the root.
    pub entries: &'a BTreeMap<PathBuf, Vec<Entry>>,
    /// Directories the user has opened.
    pub expanded: &'a HashSet<PathBuf>,
    /// Paths Git ignores.
    pub ignored: &'a HashSet<PathBuf>,
    /// Git status per path.
    pub decorations: &'a BTreeMap<PathBuf, Decoration>,
    /// Visibility preferences.
    pub settings: &'a FilesSettings,
    /// When set, only directories are listed, excluding this path and everything inside it:
    /// the picker for moving it somewhere else.
    pub destination_for: Option<&'a Path>,
}

/// One row with its children, in display order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Worktree-relative path; also the row's identity.
    pub path: PathBuf,
    /// A file, or a directory with what it shows when open.
    pub kind: NodeKind,
}

/// What a row is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory {
        /// Whether the user has opened it.
        expanded: bool,
        contents: Contents,
    },
}

/// What an opened directory shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Contents {
    Children(Vec<Node>),
    /// Listed and found empty.
    Empty,
    /// Not listed yet.
    Loading,
}

/// A built tree and the deleted paths it synthesised.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub nodes: Vec<Node>,
    /// Tracked paths that are gone from disk but still shown because Git reports the deletion.
    pub missing: HashSet<PathBuf>,
}

impl Node {
    /// The file or folder name shown on the row.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

/// Builds the tree the navigator shows.
pub fn build(source: &Source) -> Tree {
    let mut missing = HashSet::new();
    let synthesised;
    let entries = if source.settings.git_decorations && source.destination_for.is_none() {
        synthesised = with_deleted(source.entries, source.decorations, &mut missing);
        &synthesised
    } else {
        source.entries
    };
    Tree {
        nodes: children(source, Path::new(""), entries),
        missing,
    }
}

/// A copy of `entries` with every Git-deleted path (and the folders leading to it) added,
/// so the row can still be opened in a diff; those paths are collected in `missing`.
fn with_deleted(
    entries: &BTreeMap<PathBuf, Vec<Entry>>,
    decorations: &BTreeMap<PathBuf, Decoration>,
    missing: &mut HashSet<PathBuf>,
) -> BTreeMap<PathBuf, Vec<Entry>> {
    let mut entries = entries.clone();
    for (path, decoration) in decorations {
        if decoration.staged != Some(Change::Deleted)
            && decoration.unstaged != Some(Change::Deleted)
        {
            continue;
        }
        if listed(&entries, path) {
            continue;
        }
        missing.insert(path.clone());
        let mut child = path.clone();
        let mut directory = false;
        while let Some(parent) = child.parent() {
            let siblings = entries.entry(parent.into()).or_default();
            if !siblings.iter().any(|entry| entry.path == child) {
                siblings.push(Entry {
                    path: child.clone(),
                    directory,
                    unsupported: false,
                });
            }
            child = parent.into();
            directory = true;
            if child.as_os_str().is_empty() {
                break;
            }
        }
    }
    entries
}

fn listed(entries: &BTreeMap<PathBuf, Vec<Entry>>, path: &Path) -> bool {
    entries
        .get(path.parent().unwrap_or(Path::new("")))
        .is_some_and(|siblings| siblings.iter().any(|entry| entry.path == path))
}

fn children(
    source: &Source,
    directory: &Path,
    entries: &BTreeMap<PathBuf, Vec<Entry>>,
) -> Vec<Node> {
    let mut listed: Vec<&Entry> = entries
        .get(directory)
        .into_iter()
        .flatten()
        .filter(|entry| shown(source, entry))
        .collect();
    listed.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.path.cmp(&b.path))
    });
    listed
        .into_iter()
        .map(|entry| Node {
            path: entry.path.clone(),
            kind: if entry.directory {
                let nodes = children(source, &entry.path, entries);
                NodeKind::Directory {
                    expanded: source.expanded.contains(&entry.path),
                    contents: if !nodes.is_empty() {
                        Contents::Children(nodes)
                    } else if entries.contains_key(&entry.path) {
                        Contents::Empty
                    } else {
                        Contents::Loading
                    },
                }
            } else {
                NodeKind::File
            },
        })
        .collect()
}

fn shown(source: &Source, entry: &Entry) -> bool {
    if let Some(moving) = source.destination_for {
        return entry.directory && !entry.path.starts_with(moving);
    }
    let hidden = entry
        .path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'));
    (source.settings.show_hidden || !hidden)
        && (source.settings.show_ignored || !source.ignored.contains(&entry.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, directory: bool) -> Entry {
        Entry {
            path: PathBuf::from(path),
            directory,
            unsupported: false,
        }
    }

    fn listing() -> BTreeMap<PathBuf, Vec<Entry>> {
        BTreeMap::from([
            (
                PathBuf::new(),
                vec![
                    entry("readme.md", false),
                    entry("src", true),
                    entry(".git", true),
                    entry("target", true),
                    entry("empty", true),
                ],
            ),
            (PathBuf::from("src"), vec![entry("src/main.rs", false)]),
            (PathBuf::from("empty"), vec![]),
        ])
    }

    fn build_with(
        entries: &BTreeMap<PathBuf, Vec<Entry>>,
        settings: &FilesSettings,
        expanded: &HashSet<PathBuf>,
        decorations: &BTreeMap<PathBuf, Decoration>,
        destination_for: Option<&Path>,
    ) -> Tree {
        build(&Source {
            entries,
            expanded,
            ignored: &HashSet::from([PathBuf::from("target")]),
            decorations,
            settings,
            destination_for,
        })
    }

    fn paths(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|n| n.path.to_str().unwrap()).collect()
    }

    #[test]
    fn folders_first_then_by_name_with_hidden_and_ignored_filtered_by_settings() {
        let entries = listing();
        let mut settings = FilesSettings::default();
        let all = build_with(&entries, &settings, &HashSet::new(), &BTreeMap::new(), None);
        assert_eq!(
            paths(&all.nodes),
            [".git", "empty", "src", "target", "readme.md"]
        );
        settings.show_hidden = false;
        settings.show_ignored = false;
        let filtered = build_with(&entries, &settings, &HashSet::new(), &BTreeMap::new(), None);
        assert_eq!(paths(&filtered.nodes), ["empty", "src", "readme.md"]);
    }

    #[test]
    fn directories_report_children_empty_or_loading_and_their_expansion() {
        let entries = listing();
        let expanded = HashSet::from([PathBuf::from("src")]);
        let tree = build_with(
            &entries,
            &FilesSettings::default(),
            &expanded,
            &BTreeMap::new(),
            None,
        );
        let kind = |name: &str| {
            tree.nodes
                .iter()
                .find(|n| n.path == Path::new(name))
                .map(|n| n.kind.clone())
                .unwrap()
        };
        assert!(matches!(
            kind("src"),
            NodeKind::Directory { expanded: true, contents: Contents::Children(children) }
                if paths(&children) == ["src/main.rs"]
        ));
        assert!(matches!(
            kind("empty"),
            NodeKind::Directory {
                expanded: false,
                contents: Contents::Empty
            }
        ));
        assert!(matches!(
            kind("target"),
            NodeKind::Directory {
                expanded: false,
                contents: Contents::Loading
            }
        ));
        assert_eq!(kind("readme.md"), NodeKind::File);
    }

    #[test]
    fn git_deleted_paths_are_synthesised_with_their_folders_and_marked_missing() {
        let entries = listing();
        let deleted = Decoration {
            staged: None,
            unstaged: Some(Change::Deleted),
            descendants: false,
            conflicted: false,
        };
        let decorations = BTreeMap::from([
            (PathBuf::from("docs/old/guide.md"), deleted.clone()),
            (PathBuf::from("src/main.rs"), deleted),
        ]);
        let tree = build_with(
            &entries,
            &FilesSettings::default(),
            &HashSet::new(),
            &decorations,
            None,
        );
        assert_eq!(
            tree.missing,
            HashSet::from([PathBuf::from("docs/old/guide.md")]),
            "a listed file is never missing"
        );
        let docs = tree
            .nodes
            .iter()
            .find(|n| n.path == Path::new("docs"))
            .unwrap();
        let NodeKind::Directory {
            contents: Contents::Children(old),
            ..
        } = &docs.kind
        else {
            panic!("docs must be a folder with children");
        };
        assert_eq!(paths(old), ["docs/old"]);
        let mut settings = FilesSettings::default();
        settings.git_decorations = false;
        let plain = build_with(&entries, &settings, &HashSet::new(), &decorations, None);
        assert!(plain.missing.is_empty());
        assert!(!paths(&plain.nodes).contains(&"docs"));
    }

    #[test]
    fn destination_picker_lists_only_folders_outside_the_moved_path() {
        let mut entries = listing();
        entries.insert(
            PathBuf::from("src"),
            vec![entry("src/main.rs", false), entry("src/bin", true)],
        );
        let tree = build_with(
            &entries,
            &FilesSettings::default(),
            &HashSet::new(),
            &BTreeMap::new(),
            Some(Path::new("src")),
        );
        assert_eq!(paths(&tree.nodes), [".git", "empty", "target"]);
    }
}
