//! File groups are independent of the tree's single keyboard cursor.
use super::*;
use hive_core::projects::Target;
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Copy)]
pub(super) enum Gesture {
    Replace,
    Toggle,
    Range,
    ExtendRange,
}

impl Gesture {
    pub(super) fn from_modifiers(shift: bool, command: bool) -> Self {
        match (shift, command) {
            (false, false) => Self::Replace,
            (false, true) => Self::Toggle,
            (true, false) => Self::Range,
            (true, true) => Self::ExtendRange,
        }
    }
}

#[derive(Default)]
pub(super) struct Selection {
    target: Option<Target>,
    paths: BTreeSet<PathBuf>,
    /// One ordered snapshot shared by all selected drag sources, not copied per visible row.
    ordered: Arc<[PathBuf]>,
    anchor: Option<PathBuf>,
    cursor: Option<PathBuf>,
}

impl Selection {
    pub(super) fn contains(&self, path: &Path) -> bool {
        self.paths.contains(path)
    }

    pub(super) fn count(&self) -> usize {
        self.paths.len()
    }

    pub(super) fn drag_paths(&self, path: &Path) -> Arc<[PathBuf]> {
        if self.contains(path) {
            self.ordered.clone()
        } else {
            Arc::from([path.to_path_buf()])
        }
    }

    fn click(&mut self, path: &Path, gesture: Gesture, visible: &[PathBuf]) {
        self.cursor = Some(path.to_path_buf());
        let Some(end) = visible.iter().position(|candidate| candidate == path) else {
            self.paths.clear();
            self.anchor = None;
            self.order(visible);
            return;
        };
        match gesture {
            Gesture::Replace => {
                self.paths = BTreeSet::from([path.to_path_buf()]);
                self.anchor = Some(path.to_path_buf());
            }
            Gesture::Toggle => {
                if !self.paths.remove(path) {
                    self.paths.insert(path.to_path_buf());
                }
                self.anchor = Some(path.to_path_buf());
            }
            Gesture::Range | Gesture::ExtendRange => {
                let start = self
                    .anchor
                    .as_ref()
                    .and_then(|anchor| visible.iter().position(|path| path == anchor))
                    .unwrap_or(end);
                if matches!(gesture, Gesture::Range) {
                    self.paths.clear();
                }
                self.paths
                    .extend(visible[start.min(end)..=start.max(end)].iter().cloned());
                self.anchor.get_or_insert_with(|| path.to_path_buf());
            }
        }
        self.order(visible);
        if matches!(gesture, Gesture::Toggle) && !self.contains(path) {
            self.cursor = self.ordered.last().cloned();
        }
    }

    fn sync(
        &mut self,
        target: Option<&Target>,
        visible: &[PathBuf],
        cursor: Option<PathBuf>,
    ) -> bool {
        let previous = self.ordered.clone();
        if self.target.as_ref() != target {
            *self = Self {
                target: target.cloned(),
                ..Default::default()
            };
        }
        if self.cursor != cursor {
            if let Some(path) = &cursor {
                self.click(path, Gesture::Replace, visible);
            }
            self.cursor = cursor;
        }
        let available: BTreeSet<_> = visible.iter().collect();
        self.paths.retain(|path| available.contains(path));
        if self
            .anchor
            .as_ref()
            .is_some_and(|path| !available.contains(path))
        {
            self.anchor = None;
        }
        self.order(visible);
        self.ordered != previous
    }

    fn order(&mut self, visible: &[PathBuf]) {
        self.ordered = visible
            .iter()
            .filter(|path| self.paths.contains(*path))
            .cloned()
            .collect();
    }
}

impl FilesPanel {
    fn selectable_paths(&self, tree: &TreeState, cx: &App) -> Vec<PathBuf> {
        let files = self.files.read(cx);
        let visible: Vec<_> = (0..)
            .map_while(|ix| tree.entry(ix))
            .filter(|entry| !entry.is_folder() && !entry.is_disabled())
            .map(|entry| PathBuf::from(entry.item().id.as_ref()))
            .collect();
        // Read each sibling list once, rather than scanning it for every visible file.
        let directories: BTreeSet<_> = visible.iter().filter_map(|path| path.parent()).collect();
        let supported: BTreeSet<_> = directories
            .into_iter()
            .filter_map(|directory| files.listing(directory))
            .flatten()
            .filter(|entry| !entry.unsupported)
            .map(|entry| entry.path.as_path())
            .collect();
        visible
            .into_iter()
            .filter(|path| !files.missing.contains(path) && supported.contains(path.as_path()))
            .collect()
    }

    pub(super) fn sync_selection(&mut self, tree: &TreeState, cx: &App) -> bool {
        let visible = self.selectable_paths(tree, cx);
        let cursor = tree
            .selected_item()
            .map(|item| PathBuf::from(item.id.as_ref()));
        let target = self.files.read(cx).target.clone();
        self.selection.sync(target.as_ref(), &visible, cursor)
    }

    pub(super) fn select_row(
        &mut self,
        path: &Path,
        gesture: Gesture,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tree = self.tree.clone();
        let visible = self.selectable_paths(tree.read(cx), cx);
        self.selection.click(path, gesture, &visible);
        let id: Option<SharedString> = self
            .selection
            .cursor
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned().into());
        tree.update(cx, |tree, cx| {
            tree.set_selected_index(id.and_then(|id| tree.index_of(&id)), cx);
            tree.focus(window, cx);
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{Gesture, Selection};
    use hive_core::projects::Target;
    use std::path::{Path, PathBuf};

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn target(path: &str) -> Target {
        Target {
            project: "/repo".into(),
            path: path.into(),
            host: hive_core::host::Host::Local,
        }
    }

    #[test]
    fn command_click_toggles_files_and_drag_keeps_tree_order() {
        let visible = paths(&["z.rs", "a.rs", "m.rs"]);
        let mut selection = Selection::default();
        selection.click(Path::new("m.rs"), Gesture::Replace, &visible);
        selection.click(Path::new("z.rs"), Gesture::Toggle, &visible);
        assert_eq!(
            selection.drag_paths(Path::new("m.rs")).as_ref(),
            paths(&["z.rs", "m.rs"])
        );
        assert_eq!(
            selection.drag_paths(Path::new("a.rs")).as_ref(),
            paths(&["a.rs"])
        );
        selection.click(Path::new("m.rs"), Gesture::Toggle, &visible);
        assert_eq!(selection.ordered.as_ref(), paths(&["z.rs"]));
        assert_eq!(selection.cursor.as_deref(), Some(Path::new("z.rs")));
        selection.click(Path::new("z.rs"), Gesture::Toggle, &visible);
        assert_eq!(selection.count(), 0);
        assert_eq!(selection.cursor, None);
    }

    #[test]
    fn shift_range_keeps_its_anchor_in_both_directions_and_can_be_extended() {
        let visible = paths(&["a", "b", "c", "d", "e"]);
        let mut selection = Selection::default();
        selection.click(Path::new("c"), Gesture::Replace, &visible);
        selection.click(Path::new("e"), Gesture::Range, &visible);
        assert_eq!(selection.ordered.as_ref(), paths(&["c", "d", "e"]));
        selection.click(Path::new("a"), Gesture::Range, &visible);
        assert_eq!(selection.ordered.as_ref(), paths(&["a", "b", "c"]));
        selection.click(Path::new("e"), Gesture::ExtendRange, &visible);
        assert_eq!(selection.ordered.as_ref(), visible);
        selection.click(Path::new("d"), Gesture::Replace, &visible);
        assert_eq!(selection.ordered.as_ref(), paths(&["d"]));
    }

    #[test]
    fn refresh_preserves_group_but_filtering_and_target_changes_remove_stale_paths() {
        let first = target("/repo");
        let second = target("/repo-worktrees/other");
        let visible = paths(&["a", "b", "c"]);
        let mut selection = Selection::default();
        selection.sync(Some(&first), &visible, None);
        selection.click(Path::new("a"), Gesture::Replace, &visible);
        selection.click(Path::new("c"), Gesture::Toggle, &visible);
        let snapshot = selection.drag_paths(Path::new("a"));
        assert!(!selection.sync(Some(&first), &visible, Some("c".into())));
        assert!(selection.sync(Some(&first), &paths(&["a", "b"]), None));
        assert_eq!(selection.ordered.as_ref(), paths(&["a"]));
        assert_eq!(snapshot.as_ref(), paths(&["a", "c"]));
        selection.sync(Some(&second), &visible, None);
        assert_eq!(selection.count(), 0);
        selection.click(Path::new("b"), Gesture::Range, &visible);
        assert_eq!(selection.ordered.as_ref(), paths(&["b"]));
    }

    #[test]
    fn keyboard_navigation_and_ineligible_rows_end_group_selection() {
        let target = target("/repo");
        let visible = paths(&["a", "b", "c"]);
        let mut selection = Selection::default();
        selection.sync(Some(&target), &visible, None);
        selection.click(Path::new("a"), Gesture::Replace, &visible);
        selection.click(Path::new("c"), Gesture::Toggle, &visible);
        selection.sync(Some(&target), &visible, Some("b".into()));
        assert_eq!(selection.ordered.as_ref(), paths(&["b"]));
        selection.click(Path::new("folder"), Gesture::Replace, &visible);
        assert_eq!(selection.count(), 0);
        selection.click(Path::new("a"), Gesture::Range, &visible);
        assert_eq!(selection.ordered.as_ref(), paths(&["a"]));
    }

    #[test]
    fn modifier_combinations_map_to_the_expected_selection_gesture() {
        assert!(matches!(
            Gesture::from_modifiers(false, false),
            Gesture::Replace
        ));
        assert!(matches!(
            Gesture::from_modifiers(false, true),
            Gesture::Toggle
        ));
        assert!(matches!(
            Gesture::from_modifiers(true, false),
            Gesture::Range
        ));
        assert!(matches!(
            Gesture::from_modifiers(true, true),
            Gesture::ExtendRange
        ));
    }
}
