use super::{Files, SearchMode, content_search, search};
use gpui::{AppContext as _, Entity, TestAppContext};
use gpui_kit as gpui;
use hive_core::{files::Entry, settings::FilesSettings};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

fn navigator(cx: &mut TestAppContext) -> Entity<Files> {
    cx.new(|_| Files {
        target: None,
        error: None,
        visible: false,
        window_active: true,
        entries: BTreeMap::from([(
            PathBuf::new(),
            vec![Entry {
                path: "lib.rs".into(),
                directory: false,
                unsupported: false,
            }],
        )]),
        expanded: HashSet::new(),
        nodes: Vec::new(),
        tree_generation: 0,
        loading: false,
        busy: false,
        settings: FilesSettings::default(),
        decorations: BTreeMap::new(),
        ignored: HashSet::new(),
        dirty: HashSet::new(),
        missing: HashSet::new(),
        active: None,
        recent: Vec::new(),
        destination_for: None,
        pending_reveal: None,
        pending_refresh: false,
        search: search::Search::default(),
        search_mode: SearchMode::Names,
        content: content_search::ContentSearch::default(),
        generation: 0,
        _operation: None,
        _read: None,
        _poll: None,
        _subscriptions: Vec::new(),
    })
}

#[gpui::test]
fn explicit_reveal_clears_a_filename_filter_and_exposes_the_requested_row(cx: &mut TestAppContext) {
    let files = navigator(cx);
    files.update(cx, |files, cx| {
        files.set_search_text("preview".into(), cx);
        assert!(files.nodes().is_empty());
        files.reveal_in_tree("lib.rs".into(), cx);
        assert_eq!(files.search_text(), "");
        assert_eq!(files.search_mode(), SearchMode::Names);
        assert_eq!(files.reveal_request(), Some(Path::new("lib.rs")));
        assert_eq!(files.nodes().len(), 1);
        assert_eq!(files.nodes()[0].path, PathBuf::from("lib.rs"));
    });
}

#[gpui::test]
fn explicit_reveal_leaves_content_search_without_transferring_its_query(cx: &mut TestAppContext) {
    let files = navigator(cx);
    files.update(cx, |files, cx| {
        files.set_search_mode(SearchMode::Content, cx);
        files.set_search_text("needle".into(), cx);
        files.reveal_in_tree("lib.rs".into(), cx);
        assert_eq!(files.search_mode(), SearchMode::Names);
        assert!(files.search_text().is_empty());
        assert_eq!(files.nodes().len(), 1);
    });
}

#[gpui::test]
fn automatic_reveal_preserves_the_filename_filter(cx: &mut TestAppContext) {
    let files = navigator(cx);
    files.update(cx, |files, cx| {
        files.set_search_text("preview".into(), cx);
        files.reveal("lib.rs".into(), cx);
        assert_eq!(files.search_text(), "preview");
        assert!(files.nodes().is_empty());
    });
}

#[gpui::test]
fn only_a_visible_navigator_polls(cx: &mut TestAppContext) {
    let files = navigator(cx);
    files.update(cx, |files, cx| {
        assert!(files._poll.is_none());
        files.set_visible(true, cx);
        assert!(files._poll.is_some());
        files.set_visible(false, cx);
        assert!(files._poll.is_none());
    });
}

#[gpui::test]
fn returning_to_a_navigator_drops_the_snapshot_of_its_filename_search(cx: &mut TestAppContext) {
    let files = navigator(cx);
    files.update(cx, |files, cx| {
        files.set_visible(true, cx);
        files.set_search_text("lib".into(), cx);
        files.search.entries = std::sync::Arc::new(files.entries.clone());
        // Still on screen: choosing the tab again keeps the snapshot.
        files.set_visible(true, cx);
        assert!(!files.search.entries.is_empty());

        // Files may have changed while another worktree was shown.
        files.set_visible(false, cx);
        files.set_visible(true, cx);
        assert!(files.search.entries.is_empty());
        assert_eq!(files.search_text(), "lib");
    });
}
