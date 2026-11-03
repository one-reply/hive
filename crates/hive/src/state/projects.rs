use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui_kit::*;
use hive_core::projects::{self, Project, Worktree};
use hive_core::worktree::{self, Removal, WorktreeError};

use super::store::Store;

/// What the sidebar has selected: a project and one of its worktrees, by path, so the
/// selection survives rescans that add or remove worktrees. `worktree` is `None` only for
/// a project that is not a git repository (the folder itself is selected).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub project: PathBuf,
    pub worktree: Option<PathBuf>,
}

/// Scanned projects (git worktrees etc.), the current selection and worktree lifecycle.
/// Rescans whenever the [`Store`]'s project list changes and after creating/removing a worktree.
pub struct Projects {
    pub items: Vec<Project>,
    selected: Option<Selection>,
    /// Worktree paths with an open terminal: the sidebar lights their status dot.
    active: HashSet<PathBuf>,
    /// One entry per worktree being created, holding its project's path.
    creating: Vec<PathBuf>,
    store: Entity<Store>,
    _store_observer: Subscription,
}

impl Projects {
    pub fn new(store: &Entity<Store>, cx: &mut Context<Self>) -> Self {
        let observer = cx.observe(store, |this, _, cx| this.rescan(cx).detach());
        let mut this = Self {
            items: vec![],
            selected: None,
            active: HashSet::new(),
            creating: Vec::new(),
            store: store.clone(),
            _store_observer: observer,
        };
        this.rescan(cx).detach();
        this
    }

    pub fn select(&mut self, project: &Path, worktree: Option<&Path>, cx: &mut Context<Self>) {
        self.selected = Some(Selection {
            project: project.to_path_buf(),
            worktree: worktree.map(Path::to_path_buf),
        });
        cx.notify();
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
            let Ok(rescan) = this.update(cx, |this, cx| {
                if let Some(ix) = this.creating.iter().position(|path| *path == project.path) {
                    this.creating.remove(ix);
                }
                this.rescan(cx)
            }) else {
                return created.map(drop);
            };
            rescan.await;
            let created = created?;
            this.update(cx, |this, cx| {
                this.select(&project.path, Some(&created.path), cx)
            })
            .ok();
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

    fn project(&self, path: &Path) -> Option<&Project> {
        self.items.iter().find(|p| p.path == path)
    }

    fn rescan(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let refs = self.store.read(cx).state.projects.clone();
        cx.spawn(async move |this, cx| {
            let items = cx
                .background_executor()
                .spawn(async move { refs.iter().map(projects::scan).collect::<Vec<_>>() })
                .await;
            this.update(cx, |this, cx| {
                this.items = items;
                this.selected = this
                    .selected
                    .take()
                    .and_then(|selection| this.revalidate(selection));
                cx.notify();
            })
            .ok();
        })
    }

    /// Keeps a selection whose worktree still exists; a removed worktree falls back to the
    /// project's primary one.
    fn revalidate(&self, selection: Selection) -> Option<Selection> {
        let project = self.project(&selection.project)?;
        let exists = |path: &PathBuf| project.worktrees.iter().any(|w| &w.path == path);
        let worktree = match selection.worktree {
            Some(path) if exists(&path) => Some(path),
            _ => project
                .worktrees
                .first()
                .map(|primary| primary.path.clone()),
        };
        Some(Selection {
            worktree,
            ..selection
        })
    }
}
