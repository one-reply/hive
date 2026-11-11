use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui_kit::*;
use hive_core::host::Host;
use hive_core::projects::{self, Project, Target, Worktree};
use hive_core::worktree::{self, Removal, WorktreeError};

use super::store::Store;
use hive_core::agent::launch::Launch;

pub(crate) use hive_core::state::workspace::Selection;

/// Scanned projects (git worktrees etc.), the current selection and worktree lifecycle.
/// Rescans whenever the [`Store`]'s project list changes and after creating/removing a worktree.
pub struct Projects {
    pub(crate) errors: HashMap<(Host, PathBuf), String>,
    pub items: Vec<Project>,
    selected: Option<Selection>,
    pending_selection: Option<Selection>,
    scan_generation: u64,
    /// Worktree paths with an open terminal: the sidebar lights their status dot.
    active: HashSet<(Host, PathBuf)>,
    /// One entry per worktree being created, holding its project's path.
    creating: Vec<(Host, PathBuf)>,
    store: Entity<Store>,
    _store_observer: Subscription,
}

impl Projects {
    pub fn new(store: &Entity<Store>, cx: &mut Context<Self>) -> Self {
        let mut previous = store.read(cx).state.projects.clone();
        let observer = cx.observe(store, move |this, store, cx| {
            let current = &store.read(cx).state.projects;
            if current.len() != previous.len()
                || current
                    .iter()
                    .zip(&previous)
                    .any(|(a, b)| !a.has_same_scan_settings(b))
            {
                previous = current.clone();
                this.rescan(cx).detach();
            }
        });
        let mut this = Self {
            errors: HashMap::new(),
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

    pub fn select(
        &mut self,
        host: &Host,
        project: &Path,
        worktree: Option<&Path>,
        cx: &mut Context<Self>,
    ) {
        if self.is_selected(host, project, worktree) {
            return;
        }
        self.pending_selection = None;
        self.selected = Some(Selection {
            host: host.clone(),
            project: project.to_path_buf(),
            worktree: worktree.map(Path::to_path_buf),
        });
        self.save_selection(cx);
        cx.notify();
    }

    /// Keeps the current worktree when active; otherwise selects the project's primary worktree.
    pub(crate) fn select_project(&mut self, host: &Host, path: &Path, cx: &mut Context<Self>) {
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| selected.host == *host && selected.project == path)
        {
            return;
        }
        let Some(project) = self.project(host, path) else {
            return;
        };
        let worktree = project
            .worktrees
            .first()
            .map(|worktree| worktree.path.clone());
        self.select(host, path, worktree.as_deref(), cx);
    }

    fn save_selection(&self, cx: &mut Context<Self>) {
        self.store.update(cx, |store, cx| {
            store.set_selection(self.selected.clone(), cx)
        });
    }

    pub fn selection(&self) -> Option<(&Project, Option<&Worktree>)> {
        let selected = self.selected.as_ref()?;
        let project = self.project(&selected.host, &selected.project)?;
        let worktree = selected
            .worktree
            .as_ref()
            .and_then(|path| project.worktrees.iter().find(|w| &w.path == path));
        Some((project, worktree))
    }

    /// What the selection points at, by the one rule in [`Target::of`].
    pub fn target(&self) -> Option<Target> {
        let selected = self.selected.as_ref()?;
        let project = self.project(&selected.host, &selected.project)?;
        Some(Target {
            host: project.host.clone(),
            project: project.path.clone(),
            path: selected
                .worktree
                .clone()
                .unwrap_or_else(|| project.path.clone()),
        })
    }

    pub fn is_selected(&self, host: &Host, project: &Path, worktree: Option<&Path>) -> bool {
        self.selected.as_ref().is_some_and(|s| {
            s.host == *host && s.project == project && s.worktree.as_deref() == worktree
        })
    }

    pub fn is_active(&self, host: &Host, worktree: &Path) -> bool {
        self.active
            .contains(&(host.clone(), worktree.to_path_buf()))
    }

    /// Replaces the set of worktrees that have an open terminal.
    pub fn set_active(&mut self, paths: HashSet<(Host, PathBuf)>, cx: &mut Context<Self>) {
        if self.active != paths {
            self.active = paths;
            cx.notify();
        }
    }

    pub fn creating_count(&self, host: &Host, project: &Path) -> usize {
        self.creating
            .iter()
            .filter(|(h, path)| h == host && path == project)
            .count()
    }

    pub(crate) fn bases(
        &self,
        host: &Host,
        project: &Path,
        cx: &App,
    ) -> Task<Result<Vec<String>, String>> {
        let project = self.project(host, project).cloned();
        cx.background_executor().spawn(async move {
            let project = project.ok_or("Project is no longer available")?;
            hive_core::git::branches::list(&project.host, &project.path)
                .map(|branches| branches.into_iter().map(|branch| branch.name).collect())
                .map_err(|error| error.to_string())
        })
    }

    /// Creates a worktree (generated name when `name` is blank), rescans and returns its path.
    pub fn create_worktree(
        &mut self,
        host: &Host,
        project: &Path,
        name: String,
        base: String,
        launch: Option<Launch>,
        cx: &mut Context<Self>,
    ) -> Task<Result<PathBuf, String>> {
        let Some(project) = self.project(host, project).cloned() else {
            return Task::ready(Err("Project is no longer available".into()));
        };
        self.creating
            .push((project.host.clone(), project.path.clone()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let created = cx
                .background_executor()
                .spawn({
                    let project = project.clone();
                    let launch = launch.clone();
                    async move {
                        if let Some(launch) = launch {
                            launch
                                .check_executable(&project.host)
                                .map_err(|error| error.to_string())?;
                        }
                        worktree::create_from(&project, Some(&name), Some(&base))
                            .map_err(|error| error.to_string())
                    }
                })
                .await;
            let rescan = this.update(cx, |this, cx| {
                if let Some(ix) = this
                    .creating
                    .iter()
                    .position(|(h, path)| *h == project.host && *path == project.path)
                {
                    this.creating.remove(ix);
                }
                this.rescan(cx)
            });
            if let Ok(rescan) = rescan {
                rescan.await;
            }
            created.map(|worktree| worktree.path)
        })
    }

    /// Removes a worktree and its folder (see [`worktree::remove`]), then rescans.
    pub fn delete_worktree(
        &mut self,
        host: &Host,
        project: &Path,
        worktree: &Path,
        removal: Removal,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), WorktreeError>> {
        let Some(project) = self.project(host, project).cloned() else {
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
            let result = cx
                .background_executor()
                .spawn(async move { worktree::remove(&project, &worktree, removal) })
                .await;
            // An SSH failure may arrive after removal; inspect before offering another action.
            if let Ok(rescan) = this.update(cx, |this, cx| this.rescan(cx)) {
                rescan.await;
            }
            result
        })
    }

    pub(crate) fn project(&self, host: &Host, path: &Path) -> Option<&Project> {
        self.items
            .iter()
            .find(|p| p.host == *host && p.path == path)
    }

    pub(crate) fn rescan(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.scan_generation += 1;
        let generation = self.scan_generation;
        let refs = self.store.read(cx).state.projects.clone();
        cx.spawn(async move |this, cx| {
            let items = cx
                .background_executor()
                .spawn(async move {
                    refs.into_iter()
                        .map(|reference| {
                            let result =
                                projects::scan(&reference).map_err(|error| error.to_string());
                            (reference, result)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                if generation != this.scan_generation {
                    return;
                }
                let (items, errors) = merge_scans(&this.items, items);
                let retained_selection = this
                    .pending_selection
                    .as_ref()
                    .or(this.selected.as_ref())
                    .filter(|s| errors.contains_key(&(s.host.clone(), s.project.clone())))
                    .cloned();
                this.errors = errors;
                if apply_scan(
                    this.scan_generation,
                    generation,
                    &mut this.items,
                    &mut this.selected,
                    &mut this.pending_selection,
                    items,
                ) {
                    if let Some(selection) = retained_selection {
                        this.selected = Some(selection);
                    }
                    this.save_selection(cx);
                    cx.notify();
                }
            })
            .ok();
        })
    }
}

fn merge_scans(
    previous: &[Project],
    scanned: Vec<(hive_core::state::ProjectRef, Result<Project, String>)>,
) -> (Vec<Project>, HashMap<(Host, PathBuf), String>) {
    let mut errors = HashMap::new();
    let items = scanned.into_iter().map(|(reference, result)| match result {
        Ok(project) => project,
        Err(error) => {
            tracing::warn!(host = ?reference.host, path = ?reference.path, %error, "project scan failed; retaining worktrees");
            errors.insert((reference.host.clone(), reference.path.clone()), error);
            let mut project = Project::from(&reference);
            if let Some(old) = previous.iter().find(|old| old.host == reference.host && old.path == reference.path) {
                project.worktrees.clone_from(&old.worktrees);
            }
            project
        }
    }).collect();
    (items, errors)
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
            let project = items.iter().find(|project| {
                project.host == selection.host && project.path == selection.project
            })?;
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
    use gpui_kit as gpui;
    use hive_core::{host::Host, identity::ProjectIdentity, projects::Head, state::WorktreeFilter};

    #[gpui::test]
    fn project_selection_handles_hosts_folders_and_removed_projects(cx: &mut gpui::TestAppContext) {
        use crate::state::store::Store;
        use gpui_kit::AppContext as _;
        use hive_core::state::StateError;

        let store = cx.new(|cx| {
            Store::new(
                Err(StateError::Io {
                    operation: "test",
                    path: "/unused".into(),
                    source: std::io::Error::other("persistence disabled for test"),
                }),
                cx,
            )
        });
        let projects = cx.new(|cx| super::Projects::new(&store, cx));
        cx.run_until_parked();
        projects.update(cx, |projects, cx| {
            let mut remote = project();
            remote.host = Host::Ssh {
                target: "remote".into(),
            };
            remote.worktrees[0].path = "/remote-primary".into();
            let mut folder = project();
            folder.path = "/folder".into();
            folder.worktrees.clear();
            projects.items = vec![project(), remote.clone(), folder];

            projects.select_project(&Host::Local, "/project".as_ref(), cx);
            projects.select_project(&remote.host, &remote.path, cx);
            assert_eq!(projects.target().unwrap().host, remote.host);
            assert_eq!(
                projects.target().unwrap().path,
                std::path::Path::new("/remote-primary")
            );
            projects.select(
                &remote.host,
                &remote.path,
                Some("/new-worktree".as_ref()),
                cx,
            );
            projects.select_project(&remote.host, &remote.path, cx);
            projects.select_project(&Host::Local, "/removed".as_ref(), cx);
            assert!(projects.is_selected(
                &remote.host,
                &remote.path,
                Some("/new-worktree".as_ref())
            ));

            projects.select_project(&Host::Local, "/folder".as_ref(), cx);
            assert!(projects.is_selected(&Host::Local, "/folder".as_ref(), None));
            assert_eq!(
                projects.target().unwrap().path,
                std::path::Path::new("/folder")
            );
            assert_eq!(store.read(cx).state.workspace.selection, projects.selected);
        });
    }

    #[test]
    fn failed_remote_scan_keeps_only_that_hosts_worktrees() {
        let local = project();
        let mut remote = project();
        remote.host = Host::Ssh {
            target: "home".into(),
        };
        remote.worktrees[1].path = "/remote-worktree".into();
        let reference = hive_core::state::ProjectRef::new(remote.path.clone(), remote.host.clone());
        let (items, errors) = super::merge_scans(
            &[local, remote.clone()],
            vec![(reference, Err("offline".into()))],
        );
        assert_eq!(items[0].host, remote.host);
        assert_eq!(items[0].worktrees[1].path, remote.worktrees[1].path);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn selection_distinguishes_equal_paths_on_different_hosts() {
        let local = project();
        let mut remote = project();
        remote.host = Host::Ssh {
            target: "home".into(),
        };
        remote.worktrees[1].path = "/remote-worktree".into();
        let mut selected = Some(Selection {
            host: remote.host.clone(),
            project: remote.path.clone(),
            worktree: Some("/remote-worktree".into()),
        });
        let mut items = Vec::new();
        assert!(apply_scan(
            1,
            1,
            &mut items,
            &mut selected,
            &mut None,
            vec![local, remote]
        ));
        assert_eq!(
            selected.unwrap().worktree.unwrap(),
            std::path::Path::new("/remote-worktree")
        );
    }

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
            host: Host::Local,
            project: "/project".into(),
            worktree: Some("/project".into()),
        };
        let created = Selection {
            host: Host::Local,
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
            host: Host::Local,
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
            host: Host::Local,
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
