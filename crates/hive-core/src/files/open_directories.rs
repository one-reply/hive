//! Which directories a file tree reads again on refresh: the ones it shows open.
//!
//! The navigator remembers every folder the user opened, including folders inside a folder
//! that was closed later and folders a branch switch removed. Reading all of them on every
//! poll grows without bound over a session, while the tree can only show the open folders
//! whose parents are open and present.

use super::Entry;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// What reading the open directories found.
#[derive(Debug, PartialEq, Eq)]
pub struct Listed<E> {
    /// Listed directories keyed by worktree-relative path; `""` is the root.
    pub directories: BTreeMap<PathBuf, Vec<Entry>>,
    /// Why open directories could not be read; each of them is listed as empty.
    pub failures: Vec<E>,
}

/// Reads the root, then every directory in `expanded` that the tree shows: one its parent's
/// listing from this same read still holds as a directory, parents first. A folder inside a
/// closed one or one gone from disk is not read, so it costs nothing and reports no error,
/// and stays open for when it shows again. Fails only when the root cannot be read.
pub fn read<E>(
    expanded: &HashSet<PathBuf>,
    mut read_directory: impl FnMut(&Path) -> Result<Vec<Entry>, E>,
) -> Result<Listed<E>, E> {
    let root = read_directory(Path::new(""))?;
    let mut directories = BTreeMap::from([(PathBuf::new(), root)]);
    let mut failures = Vec::new();
    let mut open: Vec<&PathBuf> = expanded
        .iter()
        .filter(|path| !path.as_os_str().is_empty())
        .collect();
    // Paths compare by component, so a parent sorts before everything inside it: each folder
    // is checked against a listing from this read, in the same order on every refresh.
    open.sort_unstable();
    for path in open {
        if !is_listed_directory(&directories, path) {
            continue;
        }
        match read_directory(path) {
            Ok(entries) => {
                directories.insert(path.clone(), entries);
            }
            Err(error) => {
                directories.insert(path.clone(), Vec::new());
                failures.push(error);
            }
        }
    }
    Ok(Listed {
        directories,
        failures,
    })
}

/// Whether `path` appears as a directory in its parent's listing.
fn is_listed_directory(directories: &BTreeMap<PathBuf, Vec<Entry>>, path: &Path) -> bool {
    let parent = path.parent().unwrap_or(Path::new(""));
    directories.get(parent).is_some_and(|entries| {
        entries
            .iter()
            .any(|entry| entry.directory && entry.path == path)
    })
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

    /// A worktree with `src/app`, an ignored `node_modules/pkg/lib` and a file `notes`.
    fn disk() -> BTreeMap<PathBuf, Vec<Entry>> {
        BTreeMap::from([
            (
                PathBuf::new(),
                vec![
                    entry("node_modules", true),
                    entry("src", true),
                    entry("notes", false),
                ],
            ),
            (PathBuf::from("src"), vec![entry("src/app", true)]),
            (
                PathBuf::from("src/app"),
                vec![entry("src/app/main.rs", false)],
            ),
            (
                PathBuf::from("node_modules"),
                vec![entry("node_modules/pkg", true)],
            ),
            (
                PathBuf::from("node_modules/pkg"),
                vec![entry("node_modules/pkg/lib", true)],
            ),
            (PathBuf::from("node_modules/pkg/lib"), vec![]),
        ])
    }

    fn open(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    /// Reads `expanded` from `disk`; also returns every directory that was read, in order.
    fn read_from(
        disk: &BTreeMap<PathBuf, Vec<Entry>>,
        expanded: &HashSet<PathBuf>,
    ) -> (Result<Listed<String>, String>, Vec<String>) {
        let mut reads = Vec::new();
        let result = read(expanded, |path| {
            reads.push(path.display().to_string());
            disk.get(path)
                .cloned()
                .ok_or_else(|| format!("cannot list {}", path.display()))
        });
        (result, reads)
    }

    fn keys(listed: &Listed<String>) -> Vec<&str> {
        listed
            .directories
            .keys()
            .map(|path| path.to_str().unwrap())
            .collect()
    }

    #[test]
    fn reads_the_root_and_open_folders_whose_parents_are_open() {
        let (listed, reads) = read_from(&disk(), &open(&["src", "src/app"]));
        let listed = listed.unwrap();
        assert_eq!(keys(&listed), ["", "src", "src/app"]);
        assert_eq!(reads, ["", "src", "src/app"], "parents are read first");
        assert!(listed.failures.is_empty());
    }

    #[test]
    fn a_folder_left_open_inside_a_closed_one_is_not_read() {
        // Revealing a file deep in node_modules opened every folder on the way; closing
        // node_modules must stop the reads below it, not leave them running on every poll.
        let expanded = open(&["node_modules/pkg", "node_modules/pkg/lib", "src"]);
        let (listed, reads) = read_from(&disk(), &expanded);
        assert_eq!(keys(&listed.unwrap()), ["", "src"]);
        assert_eq!(reads, ["", "src"]);
    }

    #[test]
    fn reopening_the_parent_reads_the_folders_still_open_below_it() {
        let expanded = open(&["node_modules", "node_modules/pkg", "node_modules/pkg/lib"]);
        let (listed, _) = read_from(&disk(), &expanded);
        assert_eq!(
            keys(&listed.unwrap()),
            [
                "",
                "node_modules",
                "node_modules/pkg",
                "node_modules/pkg/lib"
            ]
        );
    }

    #[test]
    fn a_folder_gone_from_disk_is_skipped_without_an_error() {
        // A branch switch removed `src/app` while it was open.
        let mut disk = disk();
        disk.insert(PathBuf::from("src"), vec![]);
        disk.remove(Path::new("src/app"));
        let (listed, reads) = read_from(&disk, &open(&["src", "src/app"]));
        let listed = listed.unwrap();
        assert_eq!(keys(&listed), ["", "src"]);
        assert!(listed.failures.is_empty());
        assert_eq!(reads, ["", "src"]);
    }

    #[test]
    fn a_folder_replaced_by_a_file_is_not_read() {
        let mut disk = disk();
        disk.insert(PathBuf::from("src"), vec![entry("src/app", false)]);
        let (listed, reads) = read_from(&disk, &open(&["src", "src/app"]));
        assert_eq!(keys(&listed.unwrap()), ["", "src"]);
        assert_eq!(reads, ["", "src"]);
    }

    #[test]
    fn an_unreadable_folder_is_listed_empty_with_its_error_and_nothing_below_it_is_read() {
        let mut disk = disk();
        disk.remove(Path::new("src"));
        let (listed, reads) = read_from(&disk, &open(&["src", "src/app"]));
        let listed = listed.unwrap();
        assert_eq!(keys(&listed), ["", "src"]);
        assert!(listed.directories[Path::new("src")].is_empty());
        assert_eq!(listed.failures, ["cannot list src"]);
        assert_eq!(reads, ["", "src"]);
    }

    #[test]
    fn an_unreadable_root_fails_the_whole_read() {
        let (listed, reads) = read_from(&BTreeMap::new(), &open(&["src"]));
        assert_eq!(listed, Err("cannot list ".to_owned()));
        assert_eq!(reads, [""]);
    }

    #[test]
    fn failures_come_in_path_order_whatever_the_set_order() {
        // An unchanged error must read the same on every poll, or each poll reports it anew.
        let disk = BTreeMap::from([(
            PathBuf::new(),
            vec![entry("a", true), entry("b", true), entry("c", true)],
        )]);
        let (listed, reads) = read_from(&disk, &open(&["c", "a", "b"]));
        assert_eq!(
            listed.unwrap().failures,
            ["cannot list a", "cannot list b", "cannot list c"]
        );
        assert_eq!(reads, ["", "a", "b", "c"]);
    }

    #[test]
    fn a_sibling_with_a_longer_name_does_not_come_between_a_folder_and_its_children() {
        // `src-old` must not be read before `src/app` is checked against `src`.
        let disk = BTreeMap::from([
            (
                PathBuf::new(),
                vec![entry("src", true), entry("src-old", true)],
            ),
            (PathBuf::from("src"), vec![entry("src/app", true)]),
            (PathBuf::from("src/app"), vec![]),
            (PathBuf::from("src-old"), vec![]),
        ]);
        let (listed, reads) = read_from(&disk, &open(&["src-old", "src/app", "src"]));
        assert_eq!(keys(&listed.unwrap()), ["", "src", "src/app", "src-old"]);
        assert_eq!(reads, ["", "src", "src/app", "src-old"]);
    }

    #[test]
    fn an_open_root_is_read_once() {
        let (listed, reads) = read_from(&disk(), &open(&[""]));
        assert_eq!(keys(&listed.unwrap()), [""]);
        assert_eq!(reads, [""]);
    }
}
