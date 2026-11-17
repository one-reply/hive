//! The right sidebar's two tabs, and what the sidebar keeps for the worktrees that are not
//! shown: which tab each had open and which of their Files panels are still needed.
use crate::{app::AppState, state::worktree_memory::WorktreeMemory};
use gpui_kit::App;
use hive_core::projects::Target;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RightTab {
    Git,
    Files,
}

impl RightTab {
    /// The tab the persisted `files_selected` setting stands for.
    pub(super) fn from_files_selected(files_selected: bool) -> Self {
        if files_selected {
            Self::Files
        } else {
            Self::Git
        }
    }

    /// The tab that is open now; a worktree seen for the first time starts with it.
    pub(super) fn selected(cx: &App) -> Self {
        Self::from_files_selected(
            AppState::get(cx)
                .store
                .read(cx)
                .state
                .settings
                .files_selected,
        )
    }

    pub(super) fn is_files(self) -> bool {
        self == Self::Files
    }
}

/// The tab each worktree had open while another one is shown.
#[derive(Default)]
pub(super) struct RightTabs {
    /// `None` is a worktree that has not been shown yet.
    parked: WorktreeMemory<Option<RightTab>>,
}

impl RightTabs {
    /// The tab to show once the selection has moved from `from` to `to` with `shown` open:
    /// the one `to` was left with, or `shown` on its first visit.
    pub(super) fn follow(
        &mut self,
        from: Option<&Target>,
        to: Option<&Target>,
        shown: RightTab,
    ) -> RightTab {
        self.parked.switch(from, to, Some(shown)).unwrap_or(shown)
    }
}

/// Whether the Files panel made for `panel` is still needed: its worktree is the selected
/// one or still exists. The panel shown while nothing is selected (`None`) always is.
pub(super) fn keeps_panel(
    panel: Option<&Target>,
    selected: Option<&Target>,
    exists: impl FnOnce(&Target) -> bool,
) -> bool {
    panel == selected || panel.is_none_or(exists)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::host::Host;

    fn worktree(path: &str) -> Target {
        Target {
            host: Host::Local,
            project: "/project".into(),
            path: path.into(),
        }
    }

    #[test]
    fn files_selected_maps_to_the_files_tab() {
        assert_eq!(RightTab::from_files_selected(true), RightTab::Files);
        assert_eq!(RightTab::from_files_selected(false), RightTab::Git);
        assert!(RightTab::Files.is_files());
        assert!(!RightTab::Git.is_files());
    }

    #[test]
    fn a_worktree_seen_for_the_first_time_keeps_the_open_tab() {
        let (a, b) = (worktree("/a"), worktree("/b"));
        let mut tabs = RightTabs::default();
        assert_eq!(
            tabs.follow(Some(&a), Some(&b), RightTab::Files),
            RightTab::Files
        );
        assert_eq!(tabs.follow(None, Some(&a), RightTab::Git), RightTab::Files);
    }

    #[test]
    fn each_worktree_returns_to_the_tab_it_was_left_with() {
        let (a, b) = (worktree("/a"), worktree("/b"));
        let mut tabs = RightTabs::default();
        // Files in `a`; `b` opens with Files too and is switched to Git.
        tabs.follow(Some(&a), Some(&b), RightTab::Files);
        assert_eq!(
            tabs.follow(Some(&b), Some(&a), RightTab::Git),
            RightTab::Files
        );
        assert_eq!(
            tabs.follow(Some(&a), Some(&b), RightTab::Files),
            RightTab::Git
        );
    }

    #[test]
    fn leaving_every_worktree_keeps_the_open_tab_and_remembers_the_one_left() {
        let a = worktree("/a");
        let mut tabs = RightTabs::default();
        assert_eq!(
            tabs.follow(Some(&a), None, RightTab::Files),
            RightTab::Files
        );
        // With nothing selected the user moved to Git; `a` still opens with Files.
        assert_eq!(tabs.follow(None, Some(&a), RightTab::Git), RightTab::Files);
    }

    #[test]
    fn a_panel_lives_while_its_worktree_is_selected_or_exists() {
        let (a, b) = (worktree("/a"), worktree("/b"));
        assert!(keeps_panel(Some(&a), Some(&b), |_| true));
        assert!(!keeps_panel(Some(&a), Some(&b), |_| false));
        // The selected worktree keeps its panel even before the project list knows it.
        assert!(keeps_panel(Some(&a), Some(&a), |_| false));
    }

    #[test]
    fn the_panel_without_a_worktree_is_never_dropped() {
        let a = worktree("/a");
        assert!(keeps_panel(None, Some(&a), |_| false));
        assert!(keeps_panel(None, None, |_| false));
    }
}
