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
use hive_core::host::Host;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

mod generation;
mod mutations;
mod review;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Target {
    pub project: PathBuf,
    pub path: PathBuf,
    pub host: Host,
}

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
    Refreshed(Target),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Changes,
    Compare,
    History,
}

pub(crate) struct Git {
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
    pub diff_visible: bool,
    pub window_active: bool,
    drafts: HashMap<Target, String>,
    failures: HashMap<Target, String>,
    operations: HashMap<Target, Task<()>>,
    busy: std::collections::HashSet<Target>,
    generations: HashMap<Target, std::sync::Arc<hive_core::commit_message::cli::Cancellation>>,
    visible_diffs: HashMap<EntityId, Target>,
    projects: Entity<Projects>,
    query_generation: u64,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
    _refresh: Option<Task<()>>,
    _review: Option<Task<()>>,
}

impl EventEmitter<GitEvent> for Git {}

impl Git {
    pub fn new(projects: Entity<Projects>, cx: &mut Context<Self>) -> Self {
        let observer = cx.observe(&projects, |this, _, cx| this.sync_target(cx));
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                if this
                    .update(cx, |this, cx| {
                        if this.window_active && (this.panel_visible || this.diff_visible) {
                            this.refresh(false, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
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
            diff_visible: false,
            window_active: true,
            drafts: HashMap::new(),
            failures: HashMap::new(),
            operations: HashMap::new(),
            busy: Default::default(),
            generations: HashMap::new(),
            visible_diffs: Default::default(),
            projects,
            query_generation: 0,
            _subscriptions: vec![observer],
            _poll: poll,
            _refresh: None,
            _review: None,
        }
    }

    fn sync_target(&mut self, cx: &mut Context<Self>) {
        let target = self.projects.read(cx).selection().and_then(|(p, w)| {
            w.map(|w| Target {
                project: p.path.clone(),
                path: w.path.clone(),
                host: p.host.clone(),
            })
        });
        if target == self.target {
            return;
        }
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
        self._review = None;
        self.loading = false;
        self.review_loading = false;
        self.query_generation += 1;
        self.error = self
            .target
            .as_ref()
            .and_then(|t| self.failures.get(t).cloned());
        self.refresh(true, cx);
        cx.notify();
    }

    pub fn is_busy(&self) -> bool {
        self.target.as_ref().is_some_and(|t| self.is_busy_at(t))
    }
    pub(crate) fn is_busy_at(&self, target: &Target) -> bool {
        self.busy.contains(target)
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
    pub fn terminal(&self, cx: &mut Context<Self>) {
        if let Some(t) = &self.target {
            tracing::debug!(path = %t.path.display(), "source control: open terminal");
            cx.emit(GitEvent::Terminal(t.clone()));
        }
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
        if self.loading || self.is_busy() {
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
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let query_target = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let t = &query_target;
                    let status = git::status::read_with_stats(&t.host, &t.path)?;
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
                if this.target.as_ref() != Some(&target) {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((status, metadata)) => {
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
                        cx.emit(GitEvent::Refreshed(target.clone()));
                        if branch_changed {
                            this.projects.update(cx, |p, cx| p.rescan(cx)).detach();
                        }
                        if head_changed || force {
                            this.load_review(false, cx);
                        }
                        if changed || force || head_changed {
                            cx.notify();
                        }
                    }
                    Err(error) => this.report(&target, error, cx),
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
        if let Some(target) = &self.target {
            self.failures.remove(target);
        }
        if self.error.take().is_some() {
            cx.notify();
        }
    }
}
