use gpui_kit::*;
use hive_core::projects::{self, Project, Worktree};
use hive_core::state::ProjectRef;

use super::store::Store;

/// Scanned projects (git worktrees etc.) plus the current selection.
/// Rescans whenever the [`Store`]'s project list changes.
pub struct Projects {
    pub items: Vec<Project>,
    /// (project index, worktree index)
    pub selected: Option<(usize, Option<usize>)>,
    _store_observer: Subscription,
}

impl Projects {
    pub fn new(store: &Entity<Store>, cx: &mut Context<Self>) -> Self {
        let observer = cx.observe(store, |this, store, cx| {
            this.rescan(store.read(cx).state.projects.clone(), cx);
        });
        let mut this = Self { items: vec![], selected: None, _store_observer: observer };
        this.rescan(store.read(cx).state.projects.clone(), cx);
        this
    }

    pub fn select(&mut self, selection: (usize, Option<usize>), cx: &mut Context<Self>) {
        self.selected = Some(selection);
        cx.notify();
    }

    pub fn selection(&self) -> Option<(&Project, Option<&Worktree>)> {
        let (i, wt) = self.selected?;
        let project = self.items.get(i)?;
        Some((project, wt.and_then(|j| project.worktrees.get(j))))
    }

    fn rescan(&mut self, refs: Vec<ProjectRef>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let items = cx
                .background_executor()
                .spawn(async move { refs.iter().map(projects::scan).collect::<Vec<_>>() })
                .await;
            let _ = this.update(cx, |projects, cx| {
                projects.items = items;
                projects.selected = projects.selected.filter(|(i, wt)| {
                    projects.items.get(*i).is_some_and(|p| wt.is_none_or(|j| j < p.worktrees.len()))
                });
                cx.notify();
            });
        })
        .detach();
    }
}
