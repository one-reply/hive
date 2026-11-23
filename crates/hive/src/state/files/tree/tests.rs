use super::{Files, SearchMode};
use gpui::{AppContext as _, Entity, TestAppContext};
use gpui_kit as gpui;
use hive_core::files::Entry;
use hive_core::files::tree::{Contents, NodeKind};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

fn navigator(cx: &mut TestAppContext) -> Entity<Files> {
    let listing = BTreeMap::from([(
        PathBuf::new(),
        vec![Entry {
            path: "lib.rs".into(),
            directory: false,
            unsupported: false,
        }],
    )]);
    cx.new(|cx| Files::listed_for_test(listing, HashSet::new(), cx))
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
fn a_reveal_whose_row_never_appears_does_not_rebuild_the_tree_on_every_poll(
    cx: &mut TestAppContext,
) {
    let files = navigator(cx);
    files.update(cx, |files, cx| {
        files.reveal("gone/missing.rs".into(), cx);
        let generation = files.tree_generation();
        for _ in 0..3 {
            let unchanged = files.entries.clone();
            files.apply_listing(Ok((unchanged, None, None)), cx);
        }
        assert_eq!(files.tree_generation(), generation);
        assert_eq!(files.reveal_request(), Some(Path::new("gone/missing.rs")));

        // The listing of its folder is news, and the rebuild it brings has the row.
        let mut listing = files.entries.clone();
        let entry = |path: &str, directory| Entry {
            path: path.into(),
            directory,
            unsupported: false,
        };
        listing
            .get_mut(Path::new(""))
            .unwrap()
            .push(entry("gone", true));
        listing.insert("gone".into(), vec![entry("gone/missing.rs", false)]);
        files.apply_listing(Ok((listing, None, None)), cx);
        assert_eq!(files.tree_generation(), generation + 1);
        let gone = files
            .nodes()
            .iter()
            .find(|node| node.path == Path::new("gone"));
        assert!(
            matches!(
                gone.map(|node| &node.kind),
                Some(NodeKind::Directory { contents: Contents::Children(children), .. })
                    if children[0].path == Path::new("gone/missing.rs")
            ),
            "{gone:?}"
        );
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
