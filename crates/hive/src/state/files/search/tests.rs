use super::{Loaded, Search, load};
use hive_core::{
    files::tree::{self, Source},
    host::Host,
};
use hive_core::{
    files::{self, FileIndex, search},
    projects::Target,
    settings::FilesSettings,
};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(Target);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hive-file-search-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(Target {
            project: path.clone(),
            path,
            host: Host::Local,
        })
    }
    fn write(&self, path: &str) {
        let path = self.0.path.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "sample").unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0.path).unwrap();
    }
}
fn index(paths: &[&str]) -> Arc<FileIndex> {
    Arc::new(FileIndex {
        paths: paths.iter().map(PathBuf::from).collect(),
        truncated: false,
    })
}
fn loaded(paths: &[&str], query: &str) -> Loaded {
    let index = index(paths);
    Loaded {
        matches: search::find(&index.paths, query, true),
        index,
        entries: BTreeMap::new(),
        error: None,
    }
}

#[test]
fn rapid_typing_reuses_index_but_never_installs_an_old_query() {
    let mut search = Search::default();
    search.set_query("a".into());
    let generation = search.generation;
    search.running = true;
    search.set_query("b".into());
    assert!(search.accepts(generation));
    assert!(search.complete("a", false, Ok(loaded(&["a.rs", "b.rs"], "a"))));
    assert!(search.index.is_some());
    assert!(search.matches.directories().is_empty());
    assert!(!search.complete("b", false, Ok(loaded(&["a.rs", "b.rs"], "b"))));
    assert_eq!(
        search.matches.highlights(Path::new("b.rs")),
        std::slice::from_ref(&(0..1))
    );
    assert!(search.matches.highlights(Path::new("a.rs")).is_empty());
}

#[test]
fn clearing_or_changing_worktree_invalidates_in_flight_results_and_cached_entries() {
    let mut search = Search::default();
    search.set_query("a".into());
    let generation = search.generation;
    search.complete("a", false, Ok(loaded(&["src/a.rs"], "a")));
    search.collapsed.insert("src".into());
    search.set_query(String::new());
    assert!(!search.accepts(generation));
    assert!(search.index.is_none());
    assert!(search.entries.is_empty());
    assert!(search.collapsed.is_empty());
    search.set_query("a".into());
    assert!(!search.accepts(generation));
    let generation = search.generation;
    search.reset();
    search.set_query("a".into());
    assert!(!search.accepts(generation));
}

#[test]
fn visibility_change_refilters_without_publishing_the_old_results() {
    let mut search = Search::default();
    search.set_query("a".into());
    assert!(search.complete("a", true, Ok(loaded(&[".hidden/a.rs"], "a"))));
    assert!(search.index.is_some());
    assert!(search.matches.directories().is_empty());
}

#[test]
fn refreshing_keeps_query_and_collapsed_folders_but_invalidates_old_snapshot() {
    let mut search = Search::default();
    search.set_query("main".into());
    let generation = search.generation;
    search.complete("main", false, Ok(loaded(&["src/main.rs"], "main")));
    search.collapsed.insert("src".into());
    search.refresh();
    assert_eq!(search.query, "main");
    assert!(search.collapsed.contains(Path::new("src")));
    assert!(search.index.is_none());
    assert!(!search.accepts(generation));
}

#[test]
fn indexing_failure_stops_loading_and_is_cleared_by_the_next_query() {
    let mut search = Search::default();
    search.set_query("a".into());
    search.running = true;
    assert!(!search.complete("a", false, Err("Cannot list files".into())));
    assert!(!search.running);
    assert_eq!(search.error.as_deref(), Some("Cannot list files"));
    search.set_query("b".into());
    assert!(search.error.is_none());
}

#[test]
fn query_in_a_new_worktree_waits_for_the_old_worktrees_mutation() {
    let mut search = Search::default();
    search.set_query("old".into());
    search.reset();
    search.set_query("new".into());
    assert!(!search.can_start(true));
    assert!(search.can_start(false));
    search.running = true;
    assert!(!search.can_start(false));
}

#[test]
fn result_metadata_keeps_nonmatching_siblings_and_marks_symlinks_unsupported() {
    let fixture = Fixture::new();
    fixture.write("src/main.rs");
    fixture.write("src/other.rs");
    std::os::unix::fs::symlink("main.rs", fixture.0.path.join("src/link.rs")).unwrap();
    let loaded = load(
        &fixture.0,
        "main",
        true,
        Some(index(&["src/main.rs"])),
        &BTreeMap::new(),
    )
    .unwrap();
    let siblings = &loaded.entries[Path::new("src")];
    assert_eq!(siblings.len(), 3);
    assert!(
        siblings
            .iter()
            .find(|entry| entry.path == Path::new("src/link.rs"))
            .unwrap()
            .unsupported
    );
    assert!(files::name_taken(
        siblings.iter().map(|entry| entry.path.as_path()),
        "OTHER.rs",
        None
    ));
}

#[test]
fn missing_directory_returns_partial_results_and_an_error() {
    let fixture = Fixture::new();
    fixture.write("good/main.rs");
    let loaded = load(
        &fixture.0,
        "main",
        true,
        Some(index(&["good/main.rs", "gone/main.rs"])),
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(loaded.entries.contains_key(Path::new("good")));
    assert!(
        loaded
            .error
            .as_ref()
            .is_some_and(|error| error.contains("gone"))
    );
    let missing_root = Target {
        path: fixture.0.path.join("absent"),
        ..fixture.0.clone()
    };
    assert!(load(&missing_root, "main", true, None, &BTreeMap::new()).is_err());
}

#[test]
fn refreshed_index_follows_rename_and_delete() {
    let fixture = Fixture::new();
    fixture.write("src/old.rs");
    let first = load(&fixture.0, ".rs", true, None, &BTreeMap::new()).unwrap();
    assert!(first.index.paths.contains(&"src/old.rs".into()));
    fs::rename(
        fixture.0.path.join("src/old.rs"),
        fixture.0.path.join("src/new.rs"),
    )
    .unwrap();
    let renamed = load(&fixture.0, ".rs", true, None, &BTreeMap::new()).unwrap();
    assert_eq!(renamed.index.paths, [PathBuf::from("src/new.rs")]);
    fs::remove_file(fixture.0.path.join("src/new.rs")).unwrap();
    let deleted = load(&fixture.0, ".rs", true, None, &BTreeMap::new()).unwrap();
    assert!(deleted.index.paths.is_empty());
}

#[test]
fn large_directory_search_keeps_all_matching_rows() {
    let fixture = Fixture::new();
    for number in 0..2000 {
        fixture.write(&format!("large/file-{number:04}.txt"));
    }
    let loaded = load(&fixture.0, "file-1", true, None, &BTreeMap::new()).unwrap();
    assert_eq!(loaded.index.paths.len(), 2000);
    let mut tree = tree::build(&Source {
        entries: &loaded.entries,
        expanded: &HashSet::new(),
        ignored: &HashSet::new(),
        decorations: &BTreeMap::new(),
        settings: &FilesSettings::default(),
        destination_for: None,
    });
    loaded.matches.filter(&mut tree.nodes, &HashSet::new());
    let hive_core::files::tree::NodeKind::Directory {
        contents: hive_core::files::tree::Contents::Children(children),
        ..
    } = &tree.nodes[0].kind
    else {
        panic!("folder expected")
    };
    assert_eq!(children.len(), 1000);
}

#[test]
fn search_expansion_does_not_change_the_ordinary_tree() {
    let fixture = Fixture::new();
    fixture.write("src/deep/main.rs");
    let loaded = load(
        &fixture.0,
        "main",
        true,
        Some(index(&["src/deep/main.rs"])),
        &BTreeMap::new(),
    )
    .unwrap();
    let expanded = HashSet::from([PathBuf::from("src")]);
    let source = Source {
        entries: &loaded.entries,
        expanded: &expanded,
        ignored: &HashSet::new(),
        decorations: &BTreeMap::new(),
        settings: &FilesSettings::default(),
        destination_for: None,
    };
    let before = tree::build(&source);
    let mut filtered = tree::build(&source);
    loaded.matches.filter(&mut filtered.nodes, &HashSet::new());
    assert_ne!(filtered, before);
    let mut search = Search::default();
    search.set_query("main".into());
    search.collapsed.insert("src".into());
    search.set_query(String::new());
    assert_eq!(tree::build(&source), before);
}
