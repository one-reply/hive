use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui_kit::*;
use hive_core::projects::{self, Project, Worktree};
use hive_core::worktree::{self, Removal, WorktreeError};

use super::store::Store;

pub(crate) use hive_core::state::workspace::Selection;

/// Scanned projects (git worktrees etc.), the current selection and worktree lifecycle.
/// Rescans whenever the [`Store`]'s project list changes and after creating/removing a worktree.
pub struct Projects {
    pub items: Vec<Project>,
    selected: Option<Selection>,
    pending_selection: Option<Selection>,
    scan_generation: u64,
    /// Worktree paths with an open terminal: the sidebar lights their status dot.
    active: HashSet<PathBuf>,
    /// One entry per worktree being created, holding its project's path.
    creating: Vec<PathBuf>,
    store: Entity<Store>,
    _store_observer: Subscription,
}

impl Projects {
    pub fn new(store: &Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut previous = store.read(cx).state.projects.clone();
        let observer = cx.observe(store, move |this, store, cx| {
            let current = &store.read(cx).state.projects;
            if *current != previous {
                previous = current.clone();
                this.rescan(cx).detach();
            }
        });
        let mut this = Self {
            items: vec![],
            selected: None,
            pending_selection: store.read(cx).state.workspace.selection.clone(),
            scan_generation: 0,
            active: HashSet::new(),
            creating: Vec::new(),
            store: store.clone(),
            _store_observer: observer,
        };
        this.rescan(cx).detach();
        this
    }

    pub fn select(&mut self, project: &Path, worktree: Option<&Path>, cx: &mut Context<Self>) {
        if self.is_selected(project, worktree) {
            return;
        }
        self.pending_selection = None;
        self.selected = Some(Selection {
            project: project.to_path_buf(),
            worktree: worktree.map(Path::to_path_buf),
        });
        self.save_selection(cx);
        cx.notify();
    }

    fn save_selection(&self, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            store.set_selection(self.selected.clone(), cx)
        });
    }

    pub fn selection(&self) -> Option<(&Project, Option<&Worktree>)> {
        let selected = self.selected.as_ref()?;
        let project = self.project(&selected.project)?;
        let worktree = selected
            .worktree
            .as_ref()
            .and_then(|path| project.worktrees.iter().find(|w| &w.path == path));
        Some((project, worktree))
    }

    pub fn is_selected(&self, project: &Path, worktree: Option<&Path>) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|s| s.project == project && s.worktree.as_deref() == worktree)
    }

    pub fn is_active(&self, worktree: &Path) -> bool {
        self.active.contains(worktree)
    }

    /// Replaces the set of worktrees that have an open terminal.
    pub fn set_active(&mut self, paths: HashSet<PathBuf>, cx: &mut Context<Self>) {
        if self.active != paths {
            self.active = paths;
            cx.notify();
        }
    }

    pub fn creating_count(&self, project: &Path) -> usize {
        self.creating.iter().filter(|path| *path == project).count()
    }

    /// Creates a worktree (generated name when `name` is blank), rescans and selects it.
    pub fn create_worktree(
        &mut self,
        project: &Path,
        name: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), WorktreeError>> {
        let Some(project) = self.project(project).cloned() else {
            return Task::ready(Ok(()));
        };
        self.creating.push(project.path.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let created = cx
                .background_executor()
                .spawn({
                    let project = project.clone();
                    async move { worktree::create(&project, Some(&name)) }
                })
                .await;
            let rescan = this.update(cx, |this, cx| {
                if let Some(ix) = this.creating.iter().position(|path| *path == project.path) {
                    this.creating.remove(ix);
                }
                if let Ok(created) = &created {
                    this.pending_selection = Some(Selection {
                        project: project.path.clone(),
                        worktree: Some(created.path.clone()),
                    });
                }
                this.rescan(cx)
            });
            if let Ok(rescan) = rescan {
                rescan.await;
            }
            created?;
            Ok(())
        })
    }

    /// Removes a worktree and its folder (see [`worktree::remove`]), then rescans.
    pub fn delete_worktree(
        &mut self,
        project: &Path,
        worktree: &Path,
        removal: Removal,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), WorktreeError>> {
        let Some(project) = self.project(project).cloned() else {
            return Task::ready(Ok(()));
        };
        let Some(worktree) = project
            .worktrees
            .iter()
            .find(|w| w.path == worktree)
            .cloned()
        else {
            return Task::ready(Ok(()));
        };
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { worktree::remove(&project, &worktree, removal) })
                .await?;
            if let Ok(rescan) = this.update(cx, |this, cx| this.rescan(cx)) {
                rescan.await;
            }
            Ok(())
        })
    }

    pub(crate) fn project(&self, path: &Path) -> Option<&Project> {
        self.items.iter().find(|p| p.path == path)
    }

    pub(crate) fn rescan(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.scan_generation += 1;
        let generation = self.scan_generation;
        let refs = self.store.read(cx).state.projects.clone();
        cx.spawn(async move |this, cx| {
            let items = cx
                .background_executor()
                .spawn(async move { refs.iter().map(projects::scan).collect::<Vec<_>>() })
                .await;
            this.update(cx, |this, cx| {
                if apply_scan(
                    this.scan_generation,
                    generation,
                    &mut this.items,
                    &mut this.selected,
                    &mut this.pending_selection,
                    items,
                ) {
                    this.save_selection(cx);
                    cx.notify();
                }
            })
            .ok();
        })
    }
}

/// Accept a complete scan atomically, before notifying views that may close project tabs.
fn apply_scan(
    current: u64,
    generation: u64,
    items: &mut Vec<Project>,
    selected: &mut Option<Selection>,
    pending: &mut Option<Selection>,
    scanned: Vec<Project>,
) -> bool {
    if generation != current {
        return false;
    }
    *items = scanned;
    *selected = pending
        .take()
        .or_else(|| selected.take())
        .and_then(|selection| {
            let project = items
                .iter()
                .find(|project| project.path == selection.project)?;
            let worktree = match selection.worktree {
                Some(path) if project.worktrees.iter().any(|w| w.path == path) => Some(path),
                _ => project
                    .worktrees
                    .first()
                    .map(|primary| primary.path.clone()),
            };
            Some(Selection {
                worktree,
                ..selection
            })
        });
    true
}

#[cfg(test)]
mod tests {
    use super::{Project, Selection, Worktree, apply_scan};
    use hive_core::{host::Host, identity::ProjectIdentity, projects::Head, state::WorktreeFilter};

    fn project() -> Project {
        Project {
            name: "project".into(),
            path: "/project".into(),
            host: Host::Local,
            filter: WorktreeFilter::All,
            identity: ProjectIdentity::default(),
            worktrees: vec!["/project", "/new-worktree"]
                .into_iter()
                .map(|path| Worktree {
                    path: path.into(),
                    head: Head::Branch("main".into()),
                })
                .collect(),
        }
    }

    #[test]
    fn delayed_scan_cannot_change_projects_selection_or_consume_created_worktree() {
        let mut items = vec![project()];
        let selected_worktree = Selection {
            project: "/project".into(),
            worktree: Some("/project".into()),
        };
        let created = Selection {
            project: "/project".into(),
            worktree: Some("/new-worktree".into()),
        };
        let mut selected = Some(selected_worktree.clone());
        let mut pending = Some(created.clone());
        assert!(!apply_scan(
            3,
            2,
            &mut items,
            &mut selected,
            &mut pending,
            vec![]
        ));
        assert_eq!(selected, Some(selected_worktree));
        assert_eq!(pending, Some(created.clone()));
        assert_eq!(items.len(), 1);
        assert!(apply_scan(
            3,
            3,
            &mut items,
            &mut selected,
            &mut pending,
            vec![project()]
        ));
        assert_eq!(selected, Some(created.clone()));
        assert!(pending.is_none());
        // The oldest scan completes last, reporting that every project was removed.
        assert!(!apply_scan(
            3,
            1,
            &mut items,
            &mut selected,
            &mut pending,
            vec![]
        ));
        assert_eq!(selected, Some(created));
        assert_eq!(items[0].worktrees.len(), 2);
    }

    #[test]
    fn saved_selection_is_restored_and_missing_worktrees_fall_back_to_primary() {
        let mut items = Vec::new();
        let mut selected = None;
        let mut pending = Some(Selection {
            project: "/project".into(),
            worktree: Some("/new-worktree".into()),
        });
        assert!(apply_scan(
            1,
            1,
            &mut items,
            &mut selected,
            &mut pending,
            vec![project()]
        ));
        assert_eq!(
            selected.as_ref().unwrap().worktree.as_deref(),
            Some(std::path::Path::new("/new-worktree"))
        );
        pending = Some(Selection {
            project: "/project".into(),
            worktree: Some("/removed".into()),
        });
        apply_scan(
            2,
            2,
            &mut items,
            &mut selected,
            &mut pending,
            vec![project()],
        );
        assert_eq!(
            selected.as_ref().unwrap().worktree.as_deref(),
            Some(std::path::Path::new("/project"))
        );
        apply_scan(3, 3, &mut items, &mut selected, &mut pending, vec![]);
        assert!(selected.is_none());
    }
}
