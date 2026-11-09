//! Active worktree source control. All Git IO runs off the foreground thread.
use super::projects::Projects;
use gpui_kit::*;
use hive_core::git::{
    self,
    branches::Branch,
    changes::Action,
    diff::{DiffSource, FileDiff},
    history::Commit,
    status::{FileChange, Status},
};
use hive_core::projects::Target;
use std::collections::HashMap;
use std::time::Duration;

mod generation;
mod mutations;
mod remote;
mod review;
mod verification;

pub(crate) use remote::RemoteOperation;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DiffRequest {
    pub target: Target,
    pub path: String,
    pub old_path: Option<String>,
    pub source: DiffSource,
    pub whole_file_only: bool,
}

pub(crate) enum GitEvent {
    OpenDiff(DiffRequest),
    Activate(Target),
    Terminal(Target),
    AgentDraft(Target, hive_core::agent::AgentKind, String),
    /// A status read finished; `changed` when the status differs from the last one or the
    /// refresh was forced, so listeners that only follow the status can skip the rest.
    Refreshed {
        target: Target,
        changed: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Changes,
    Compare,
    History,
}

/// What the cached `git status` says about one file, as far as the editor toolbar cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileChanges {
    /// The cached status belongs to another worktree or has not loaded yet.
    Unknown,
    Clean,
    /// Changed in exactly one of the index and the working tree.
    One,
    /// Both staged and unstaged changes, so the viewer can choose a side.
    StagedAndUnstaged,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Repository {
    #[default]
    Unknown,
    Available,
    Absent,
}

pub(crate) struct Git {
    repository: Repository,
    pub target: Option<Target>,
    pub status: Option<Status>,
    pub branches: Vec<Branch>,
    pub remotes: Vec<String>,
    pub remote: Option<String>,
    pub upstream_remote: Option<String>,
    pub base: Option<String>,
    pub section: Section,
    pub commits: Vec<Commit>,
    pub history_more: bool,
    pub files: Vec<FileChange>,
    pub selected_commit: Option<String>,
    pub loading: bool,
    pub review_loading: bool,
    pub error: Option<String>,
    pub panel_visible: bool,
    pub files_visible: bool,
    pub diff_visible: bool,
    pub window_active: bool,
    drafts: HashMap<Target, String>,
    review_notes: HashMap<Target, Vec<git::review_notes::Note>>,
    failures: HashMap<Target, String>,
    operations: HashMap<Target, Task<()>>,
    busy: std::collections::HashSet<Target>,
    unverified: std::collections::HashSet<Target>,
    inspected: std::collections::HashSet<Target>,
    interrupted_actions: HashMap<Target, Option<Action>>,
    remote_operations: HashMap<Target, RemoteOperation>,
    synced: Option<(Target, git::changes::Synced)>,
    generations: HashMap<Target, std::sync::Arc<hive_core::commit_message::cli::Cancellation>>,
    visible_diffs: HashMap<EntityId, Target>,
    projects: Entity<Projects>,
    query_generation: u64,
    pending_refresh: Option<bool>,
    refresh_generation: u64,
    _subscriptions: Vec<Subscription>,
    _poll: Option<Task<()>>,
    _refresh: Option<Task<()>>,
    _review: Option<Task<()>>,
    _synced_timeout: Option<Task<()>>,
}

impl EventEmitter<GitEvent> for Git {}

impl Git {
    /// `path` is relative to `target`, matching the paths `git status` reports.
    pub(crate) fn file_changes(&self, target: &Target, path: &std::path::Path) -> FileChanges {
        let Some(status) = self
            .status
            .as_ref()
            .filter(|_| self.target.as_ref() == Some(target))
        else {
            return FileChanges::Unknown;
        };
        let path = path.to_string_lossy();
        match status.files.iter().find(|file| file.path == path) {
            None => FileChanges::Clean,
            Some(file) if file.staged.is_some() && file.unstaged.is_some() => {
                FileChanges::StagedAndUnstaged
            }
            Some(_) => FileChanges::One,
        }
    }

    pub fn new(projects: Entity<Projects>, cx: &mut Context<Self>) -> Self {
        let observer = cx.observe(&projects, |this, _, cx| this.sync_target(cx));
        Self {
            repository: Repository::Unknown,
            target: None,
            status: None,
            branches: vec![],
            remotes: vec![],
            remote: None,
            upstream_remote: None,
            base: None,
            section: Section::Changes,
            commits: vec![],
            history_more: false,
            files: vec![],
            selected_commit: None,
            loading: false,
            review_loading: false,
            error: None,
            panel_visible: true,
            files_visible: false,
            diff_visible: false,
            window_active: true,
            drafts: HashMap::new(),
            review_notes: HashMap::new(),
            failures: HashMap::new(),
            operations: HashMap::new(),
            busy: Default::default(),
            unverified: Default::default(),
            inspected: Default::default(),
            interrupted_actions: HashMap::new(),
            remote_operations: HashMap::new(),
            synced: None,
            generations: HashMap::new(),
            visible_diffs: Default::default(),
            projects,
            query_generation: 0,
            pending_refresh: None,
            refresh_generation: 0,
            _subscriptions: vec![observer],
            _poll: None,
            _refresh: None,
            _review: None,
            _synced_timeout: None,
        }
    }

    pub(crate) fn has_no_repository(&self) -> bool {
        self.repository == Repository::Absent
    }

    fn start_poll(&mut self, cx: &mut Context<Self>) {
        if self._poll.is_some() || self.target.is_none() || self.has_no_repository() {
            return;
        }
        self._poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                if this
                    .update(cx, |this, cx| {
                        if this.window_active
                            && (this.panel_visible || this.diff_visible || this.files_visible)
                        {
                            this.refresh(false, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn stop_repository(&mut self, cx: &mut Context<Self>) {
        self.repository = Repository::Absent;
        self._poll = None;
        self.pending_refresh = None;
        self._review = None;
        self.query_generation += 1;
        self.review_loading = false;
        self.status = None;
        self.branches.clear();
        self.remotes.clear();
        self.remote = None;
        self.upstream_remote = None;
        self.base = None;
        self.commits.clear();
        self.history_more = false;
        self.files.clear();
        self.selected_commit = None;
        if let Some(target) = &self.target {
            self.failures.remove(target);
        }
        self.error = None;
        cx.notify();
    }

    fn sync_target(&mut self, cx: &mut Context<Self>) {
        let target = self.projects.read(cx).target();
        if target == self.target {
            return;
        }
        self._poll = None;
        self.repository = Repository::Unknown;
        self.target = target;
        self.sync_diff_visibility();
        self.status = None;
        self.branches.clear();
        self.remotes.clear();
        self.remote = None;
        self.upstream_remote = None;
        self.base = None;
        self.commits.clear();
        self.history_more = false;
        self.files.clear();
        self.selected_commit = None;
        self._refresh = None;
        self.refresh_generation += 1;
        self._review = None;
        self.loading = false;
        self.review_loading = false;
        self.query_generation += 1;
        self.pending_refresh = None;
        self.error = self
            .target
            .as_ref()
            .and_then(|t| self.failures.get(t).cloned());
        self.start_poll(cx);
        self.refresh(true, cx);
        cx.notify();
    }

    pub(crate) fn is_running(&self) -> bool {
        self.target.as_ref().is_some_and(|t| self.busy.contains(t))
    }

    pub fn is_busy(&self) -> bool {
        self.target.as_ref().is_some_and(|t| self.is_busy_at(t))
    }
    pub(crate) fn is_busy_at(&self, target: &Target) -> bool {
        self.busy.contains(target) || self.unverified.contains(target)
    }
    pub(crate) fn is_generating(&self) -> bool {
        self.target
            .as_ref()
            .is_some_and(|t| self.generations.contains_key(t))
    }
    pub fn draft(&self, target: &Target) -> &str {
        self.drafts
            .get(target)
            .map(String::as_str)
            .unwrap_or_default()
    }
    pub fn set_draft(&mut self, target: Target, message: String, cx: &mut Context<Self>) {
        if self.generations.contains_key(&target) {
            return;
        }
        if self.draft(&target) != message {
            self.drafts.insert(target, message);
            cx.notify();
        }
    }

    pub fn activate(&mut self, target: Target, cx: &mut Context<Self>) {
        cx.emit(GitEvent::Activate(target));
    }
    pub fn terminal_at(&self, target: Target, cx: &mut Context<Self>) {
        cx.emit(GitEvent::Terminal(target));
    }
    pub fn activate_path(&self, path: &std::path::Path, cx: &mut Context<Self>) {
        if let Some(project) = self
            .projects
            .read(cx)
            .items
            .iter()
            .find(|p| p.worktrees.iter().any(|w| w.path == path))
        {
            let target = Target {
                project: project.path.clone(),
                path: path.to_path_buf(),
                host: project.host.clone(),
            };
            cx.emit(GitEvent::Activate(target));
        }
    }
    pub fn set_diff_visible(&mut self, id: EntityId, target: Option<Target>) {
        if let Some(target) = target {
            self.visible_diffs.insert(id, target);
        } else {
            self.visible_diffs.remove(&id);
        }
        self.sync_diff_visibility();
    }
    fn sync_diff_visibility(&mut self) {
        self.diff_visible = self
            .visible_diffs
            .values()
            .any(|t| self.target.as_ref() == Some(t));
    }

    pub(crate) fn prepare_error_draft(&self, target: Target, error: &str, cx: &mut Context<Self>) {
        let agent = crate::app::AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .default_cli_agent();
        let prompt = hive_core::agent::draft::git_error(&target.path, error);
        cx.emit(GitEvent::AgentDraft(target, agent, prompt));
    }
    pub fn open_diff(&self, file: &FileChange, source: DiffSource, cx: &mut Context<Self>) {
        if let Some(target) = &self.target {
            cx.emit(GitEvent::OpenDiff(DiffRequest {
                target: target.clone(),
                path: file.path.clone(),
                old_path: file.old_path.clone(),
                source,
                whole_file_only: file.submodule || file.old_path.is_some(),
            }));
        }
    }

    pub fn refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.has_no_repository() {
            return;
        }
        if self.loading || self.target.as_ref().is_some_and(|t| self.busy.contains(t)) {
            self.pending_refresh = Some(self.pending_refresh.unwrap_or(false) || force);
            return;
        }
        let Some(target) = self.target.clone() else {
            return;
        };
        let old_head = self
            .status
            .as_ref()
            .map(|s| (s.oid.clone(), s.branch.clone(), s.upstream.clone()));
        self.loading = true;
        self.refresh_generation += 1;
        let generation = self.refresh_generation;
        let with_stats = self.panel_visible || self.diff_visible;
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let query_target = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let t = &query_target;
                    let status = if with_stats {
                        git::status::read_with_stats(&t.host, &t.path)?
                    } else {
                        git::status::read(&t.host, &t.path)?
                    };
                    let metadata = if force
                        || old_head
                            != Some((
                                status.oid.clone(),
                                status.branch.clone(),
                                status.upstream.clone(),
                            )) {
                        let branches = git::branches::list(&t.host, &t.path)?;
                        let base = git::branches::default_base(&t.host, &t.path, &branches)?;
                        let remotes = git::changes::remotes(&t.host, &t.path)?;
                        let upstream = status
                            .branch
                            .as_deref()
                            .map(|b| git::changes::upstream_remote(&t.host, &t.path, b))
                            .transpose()?
                            .flatten();
                        Some((branches, base, remotes, upstream))
                    } else {
                        None
                    };
                    Ok::<_, git::GitError>((status, metadata))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.target.as_ref() != Some(&target) || this.refresh_generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((status, metadata)) => {
                        this.repository = Repository::Available;
                        let head_changed = this.status.as_ref().is_none_or(|s| s.oid != status.oid);
                        let branch_changed = this
                            .status
                            .as_ref()
                            .is_some_and(|s| s.branch != status.branch);
                        let changed = this.status.as_ref() != Some(&status);
                        this.status = Some(status);
                        if let Some((branches, base, remotes, upstream)) = metadata {
                            this.branches = branches;
                            this.remotes = remotes;
                            this.upstream_remote = upstream;
                            if this.base.is_none() {
                                this.base = base;
                            }
                            if this
                                .remote
                                .as_ref()
                                .is_none_or(|r| !this.remotes.contains(r))
                            {
                                this.remote = this.upstream_remote.clone().or_else(|| {
                                    (this.remotes.len() == 1).then(|| this.remotes[0].clone())
                                });
                            }
                        }
                        cx.emit(GitEvent::Refreshed {
                            target: target.clone(),
                            changed: changed || force,
                        });
                        if branch_changed {
                            this.projects.update(cx, |p, cx| p.rescan(cx)).detach();
                        }
                        if (head_changed || force) && with_stats {
                            this.load_review(false, cx);
                        }
                        if changed || force || head_changed {
                            cx.notify();
                        }
                    }
                    Err(error) if error.is_not_repository() => this.stop_repository(cx),
                    Err(error) => this.report(&target, error, cx),
                }
                if let Some(force) = this.pending_refresh.take() {
                    this.refresh(force, cx);
                }
            });
        }));
    }

    fn report(&mut self, target: &Target, error: git::GitError, cx: &mut Context<Self>) {
        tracing::error!(path = ?target.path, %error, "source control failed");
        let message = error.to_string();
        self.failures.insert(target.clone(), message.clone());
        if self.target.as_ref() == Some(target) {
            self.error = Some(message);
            cx.notify();
        }
    }

    pub fn clear_error(&mut self, cx: &mut Context<Self>) {
        if self.needs_verification() {
            return;
        }
        if let Some(target) = &self.target {
            self.failures.remove(target);
        }
        if self.error.take().is_some() {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Git, Repository};
    use crate::state::{projects::Projects, store::Store};
    use gpui_kit as gpui;
    use gpui_kit::{AppContext, TestAppContext};
    use hive_core::{git::status::Status, host::Host, projects::Target, state::StateError};

    #[gpui::test]
    fn absent_repository_stops_queries_and_preserves_drafts(cx: &mut TestAppContext) {
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
        let projects = cx.new(|cx| Projects::new(&store, cx));
        let git = cx.new(|cx| Git::new(projects, cx));
        git.update(cx, |git, cx| {
            let target = Target {
                host: Host::Local,
                project: "/unused".into(),
                path: "/unused".into(),
            };
            git.target = Some(target.clone());
            git.repository = Repository::Available;
            git.status = Some(Status::default());
            git.error = Some("old failure".into());
            git.drafts.insert(target.clone(), "keep draft".into());
            git.pending_refresh = Some(true);
            git.start_poll(cx);
            assert!(git._poll.is_some());

            git.stop_repository(cx);
            assert!(git.has_no_repository());
            assert!(git._poll.is_none());
            assert!(git.pending_refresh.is_none());
            assert!(git.status.is_none());
            assert!(git.error.is_none());
            assert_eq!(git.draft(&target), "keep draft");
            let generation = git.refresh_generation;
            git.refresh(false, cx);
            git.refresh(true, cx);
            git.start_poll(cx);
            assert_eq!(git.refresh_generation, generation);
            assert!(git._refresh.is_none());
            assert!(git._poll.is_none());

            // Clearing the selection resets absence and leaves no polling task.
            git.sync_target(cx);
            assert!(!git.has_no_repository());
            assert!(git.target.is_none());
            assert!(git._poll.is_none());
            git.target = Some(target);
            git.start_poll(cx);
            assert!(git._poll.is_some());
            git._poll = None;
        });
    }
}
