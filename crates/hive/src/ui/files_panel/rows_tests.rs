//! When the panel reads every row of its tree. The tree notifies for each frame of a row's
//! hover, chevron and accordion animation, far more often than its rows or cursor change.
use super::{FilesPanel, selection::Gesture, tree};
use crate::state::files::Files;
use gpui_kit as gpui;
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext, point, px, size,
};
use hive_core::files::Entry;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FOLDERS: usize = 2;
const FILES: usize = 3;
/// One frame of a 60 Hz display.
const FRAME: Duration = Duration::from_micros(16_667);

fn folder(index: usize) -> PathBuf {
    format!("module_{index}").into()
}

fn file(folder_index: usize, index: usize) -> PathBuf {
    folder(folder_index).join(format!("file_{index}.rs"))
}

/// A panel over two open folders of three files each, drawn in a window.
fn panel(cx: &mut TestAppContext) -> (Entity<FilesPanel>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    let mut entries = BTreeMap::new();
    let mut root = Vec::new();
    for index in 0..FOLDERS {
        root.push(Entry {
            path: folder(index),
            directory: true,
            unsupported: false,
        });
        let children = (0..FILES)
            .map(|child| Entry {
                path: file(index, child),
                directory: false,
                unsupported: false,
            })
            .collect();
        entries.insert(folder(index), children);
    }
    entries.insert(PathBuf::new(), root);
    let expanded: HashSet<PathBuf> = (0..FOLDERS).map(folder).collect();
    let mut panel = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let files = cx.new(|cx| Files::listed_for_test(entries, expanded, cx));
        let view = cx.new(|cx| FilesPanel::new(files, window, cx));
        panel = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.simulate_resize(size(px(320.), px(600.)));
    frame(cx);
    (panel.unwrap(), cx)
}

fn frame(cx: &mut VisualTestContext) {
    cx.executor().advance_clock(FRAME);
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
}

fn row_scans(panel: &Entity<FilesPanel>, cx: &mut VisualTestContext) -> usize {
    cx.read(|cx| panel.read(cx).row_scans)
}

fn row_count(panel: &Entity<FilesPanel>, cx: &mut VisualTestContext) -> usize {
    cx.read(|cx| {
        let tree = panel.read(cx).tree.read(cx);
        (0..).take_while(|ix| tree.entry(*ix).is_some()).count()
    })
}

fn is_selected(panel: &Entity<FilesPanel>, path: &Path, cx: &mut VisualTestContext) -> bool {
    cx.read(|cx| panel.read(cx).selection.contains(path))
}

fn focus_tree(panel: &Entity<FilesPanel>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let tree = panel.read(cx).tree.clone();
        tree.update(cx, |tree, cx| tree.focus(window, cx));
    });
    frame(cx);
}

/// Puts the tree's cursor on `path`, as an arrow key or the library's own click does.
fn move_cursor(panel: &Entity<FilesPanel>, path: &Path, cx: &mut VisualTestContext) {
    let id = tree::key(path);
    cx.update(|_, cx| {
        let tree = panel.read(cx).tree.clone();
        tree.update(cx, |tree, cx| {
            let index = tree.index_of(&id);
            assert!(index.is_some(), "{path:?} is not a row");
            tree.set_selected_index(index, cx);
        })
    });
    cx.run_until_parked();
}

#[gpui::test]
fn hovering_rows_reads_none_of_them(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    assert_eq!(row_count(&panel, cx), FOLDERS * (FILES + 1));
    let before = row_scans(&panel, cx);

    // Half a second of the pointer travelling down the panel, a few frames per row.
    for step in 0..30 {
        let y = 60. + 8. * step as f32;
        cx.simulate_mouse_move(point(px(120.), px(y)), None, Modifiers::default());
        frame(cx);
    }
    // And a notification that changes nothing, such as a right click's.
    cx.update(|_, cx| {
        let tree = panel.read(cx).tree.clone();
        tree.update(cx, |_, cx| cx.notify());
    });
    cx.run_until_parked();

    assert_eq!(row_scans(&panel, cx), before);
}

#[gpui::test]
fn a_cursor_the_tree_moved_brings_the_selection_along_once(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    let before = row_scans(&panel, cx);

    move_cursor(&panel, &file(0, 1), cx);
    assert_eq!(row_scans(&panel, cx), before + 1);
    assert!(is_selected(&panel, &file(0, 1), cx));

    // The arrow keys move the tree's own cursor; the panel learns of it from the tree.
    focus_tree(&panel, cx);
    cx.simulate_keystrokes("down");
    cx.run_until_parked();
    assert_eq!(row_scans(&panel, cx), before + 2);
    assert!(is_selected(&panel, &file(0, 2), cx));
    assert!(!is_selected(&panel, &file(0, 1), cx));
    assert_eq!(cx.read(|cx| panel.read(cx).selected(cx)), Some(file(0, 2)));
}

#[gpui::test]
fn a_folder_closed_from_the_keyboard_is_read_again(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    move_cursor(&panel, &folder(0), cx);
    focus_tree(&panel, cx);
    let before = row_scans(&panel, cx);

    cx.simulate_keystrokes("left");
    cx.run_until_parked();

    assert_eq!(row_count(&panel, cx), FOLDERS * (FILES + 1) - FILES);
    assert_eq!(row_scans(&panel, cx), before + 1);
}

#[gpui::test]
fn rows_that_leave_the_tree_leave_the_selected_group(cx: &mut TestAppContext) {
    let (panel, cx) = panel(cx);
    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| {
            panel.select_row(&file(1, 1), Gesture::Replace, window, cx);
            panel.select_row(&file(0, 0), Gesture::Toggle, window, cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| panel.read(cx).selection.count()), 2);
    let before = row_scans(&panel, cx);

    // The model closes the second folder; the cursor stays on its row in the first one,
    // so only the changed rows can tell the panel.
    cx.update(|_, cx| {
        let files = panel.read(cx).files.clone();
        files.update(cx, |files, cx| files.toggle_directory(folder(1), cx));
    });
    cx.run_until_parked();

    assert_eq!(row_count(&panel, cx), FOLDERS * (FILES + 1) - FILES);
    assert_eq!(row_scans(&panel, cx), before + 1);
    assert!(is_selected(&panel, &file(0, 0), cx));
    assert!(!is_selected(&panel, &file(1, 1), cx));
}
