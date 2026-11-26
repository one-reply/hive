//! Selecting a worktree brings back the tab it showed last. A tab belongs to the worktree it
//! works in, so the record holds even when the sidebar selection follows the tab a moment later.
use super::*;
use gpui_kit::component::dock::PaneRef;

/// The tab each worktree of one center showed last. Lives only for the session.
#[derive(Default)]
pub(super) struct WorktreeTabs {
    last: HashMap<PathBuf, PanelId>,
}

impl WorktreeTabs {
    /// `tab`, working in `worktree`, is the one on screen.
    fn remember(&mut self, worktree: PathBuf, tab: PanelId) {
        self.last.insert(worktree, tab);
    }

    /// The tab `worktree` showed last, while `is_open` says the dock still holds it.
    fn recall(&self, worktree: &Path, is_open: impl Fn(PanelId) -> bool) -> Option<PanelId> {
        self.last.get(worktree).copied().filter(|tab| is_open(*tab))
    }

    /// Forgets the tabs `is_open` says were closed.
    pub(super) fn retain_open(&mut self, is_open: impl Fn(PanelId) -> bool) {
        self.last.retain(|_, tab| is_open(*tab));
    }
}

impl ProjectCenter {
    /// The tab on screen in the split that last had the keyboard; the first split's while none
    /// has had it yet.
    pub(super) fn shown_tab(&self, cx: &App) -> Option<PanelId> {
        let dock = self.dock.read(cx);
        let tree = dock.layout(DockPlacement::Center)?;
        let group = dock
            .zoomed_group()
            .or_else(|| self.focused.and_then(|id| tree.find_panel_node(id)))
            .or_else(|| tree.find_panel_node(tree.panels().next()?))?;
        let PaneRef::Tabs { panels, active_ix } = tree.find_node(group)?.kind() else {
            return None;
        };
        panels.get(active_ix).copied()
    }

    /// Records the tab on screen for the worktree it works in. Called on every layout change of
    /// the center on screen, which is how the dock reports a tab coming on screen.
    pub(in crate::ui::workspace) fn remember_shown_tab(&mut self, window: &Window, cx: &App) {
        self.note_focus(window, cx);
        let Some(tab) = self.shown_tab(cx) else {
            return;
        };
        if let Some(worktree) = self.worktree_of(tab, cx) {
            self.worktree_tabs.remember(worktree, tab);
        }
    }

    /// Shows the tab `worktree` showed last, with the keyboard in it, so coming back to a
    /// worktree, from another project too, finds it as it was left. Without such a tab, shows
    /// the worktree's terminal like [`Self::show_terminal`].
    pub(in crate::ui::workspace) fn show_worktree(
        &mut self,
        worktree: &Path,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.prune(cx);
        let dock = self.dock.read(cx);
        let recalled = self
            .worktree_tabs
            .recall(worktree, |tab| dock.panel(tab).is_some())
            .and_then(|tab| Some((tab, dock.panel(tab)?.focus_handle(cx))));
        let Some((tab, focus)) = recalled else {
            self.show_terminal(worktree, window, cx);
            return;
        };
        // Selecting the tab the dock already displays is a no-op there, focus included: a
        // project shown again still displays its tab, only the keyboard went elsewhere.
        self.dock
            .update(cx, |dock, cx| dock.select_panel(tab, window, cx));
        focus.focus(window, cx);
    }

    /// The worktree tab `tab` works in: where a terminal started, or the checkout of a diff,
    /// a conflict or a file.
    fn worktree_of(&self, tab: PanelId, cx: &App) -> Option<PathBuf> {
        let is_tab = |id: EntityId| PanelId::from(id) == tab;
        if let Some(panel) = self.terminals.iter().find(|p| is_tab(p.entity_id())) {
            return panel.read(cx).cwd(cx);
        }
        if let Some(panel) = self.diffs.iter().find(|p| is_tab(p.entity_id())) {
            return Some(panel.read(cx).review.read(cx).request.target.path.clone());
        }
        if let Some(panel) = self.conflicts.iter().find(|p| is_tab(p.entity_id())) {
            return Some(panel.read(cx).conflict.read(cx).request.target.path.clone());
        }
        let panel = self.editors.iter().find(|p| is_tab(p.entity_id()))?;
        Some(panel.read(cx).document.read(cx).target.path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{PanelId, WorktreeTabs};
    use std::path::Path;

    const ALL_OPEN: fn(PanelId) -> bool = |_| true;

    #[test]
    fn returning_to_a_worktree_brings_back_the_tab_it_showed_last() {
        let [terminal_a, conflict_a, terminal_b] = [1, 2, 3].map(PanelId::from_u64);
        let mut tabs = WorktreeTabs::default();
        tabs.remember("/a".into(), terminal_a);
        tabs.remember("/a".into(), conflict_a);
        // Another worktree's terminal comes on screen; `/a` keeps its own record.
        tabs.remember("/b".into(), terminal_b);
        assert_eq!(tabs.recall(Path::new("/a"), ALL_OPEN), Some(conflict_a));
        assert_eq!(tabs.recall(Path::new("/b"), ALL_OPEN), Some(terminal_b));
    }

    #[test]
    fn a_worktree_never_shown_has_no_tab_to_bring_back() {
        let mut tabs = WorktreeTabs::default();
        tabs.remember("/a".into(), PanelId::from_u64(1));
        assert_eq!(tabs.recall(Path::new("/b"), ALL_OPEN), None);
    }

    #[test]
    fn a_closed_tab_is_not_brought_back() {
        let (conflict, terminal) = (PanelId::from_u64(1), PanelId::from_u64(2));
        let mut tabs = WorktreeTabs::default();
        tabs.remember("/a".into(), conflict);
        tabs.remember("/b".into(), terminal);
        let is_open = |tab: PanelId| tab != conflict;
        // Closed but not pruned yet: the dock is the authority.
        assert_eq!(tabs.recall(Path::new("/a"), is_open), None);
        tabs.retain_open(is_open);
        assert_eq!(tabs.recall(Path::new("/a"), ALL_OPEN), None);
        assert_eq!(tabs.recall(Path::new("/b"), ALL_OPEN), Some(terminal));
    }
}
