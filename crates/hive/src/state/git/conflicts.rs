//! Unmerged files of a worktree: opening their resolver and drafting them for an agent.
use super::*;
use hive_core::git::conflicts::{Conflict, Operation};

/// One unmerged file to resolve in a center tab.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ConflictRequest {
    pub target: Target,
    pub path: String,
}

/// What the cached `git status` says about a file a resolver tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConflictState {
    /// The cached status belongs to another worktree or has not loaded yet.
    Unknown,
    Unmerged(Conflict),
    /// The index no longer lists the file as unmerged.
    Resolved,
}

/// Whether a refresh should look for an unfinished operation.
///
/// The probe costs several processes (and ssh round trips), so a poll skips it unless an
/// operation could have begun, and once one is known every poll checks whether it has ended.
pub(super) fn inspects_operation(
    force: bool,
    previous: Option<&Status>,
    status: &Status,
    known: Option<Operation>,
) -> bool {
    force || known.is_some() || previous.is_none_or(|previous| may_have_begun(previous, status))
}

/// Whether the statuses differ in what starting an operation changes: HEAD, the branch or
/// which files are changed and how. Line counts are left out, because an agent editing a
/// file changes them on every poll.
fn may_have_begun(previous: &Status, status: &Status) -> bool {
    let shape = |file: &FileChange| (file.staged, file.unstaged, file.conflict);
    previous.oid != status.oid
        || previous.branch != status.branch
        || previous.files.len() != status.files.len()
        || previous
            .files
            .iter()
            .zip(&status.files)
            .any(|(before, after)| before.path != after.path || shape(before) != shape(after))
}

impl Git {
    /// The unfinished merge, rebase, cherry-pick or revert of the active worktree.
    pub(crate) fn operation(&self) -> Option<Operation> {
        self.operation
    }

    /// Unmerged paths of the active worktree, in status order.
    pub(crate) fn conflicted_paths(&self) -> Vec<String> {
        self.status
            .iter()
            .flat_map(|status| &status.files)
            .filter(|file| file.conflict.is_some())
            .map(|file| file.path.clone())
            .collect()
    }

    pub(crate) fn conflict_state(&self, request: &ConflictRequest) -> ConflictState {
        let Some(status) = self
            .status
            .as_ref()
            .filter(|_| self.target.as_ref() == Some(&request.target))
        else {
            return ConflictState::Unknown;
        };
        status
            .files
            .iter()
            .find(|file| file.path == request.path)
            .and_then(|file| file.conflict)
            .map_or(ConflictState::Resolved, ConflictState::Unmerged)
    }

    pub(crate) fn open_conflict(&self, path: &str, cx: &mut Context<Self>) {
        if let Some(target) = &self.target {
            cx.emit(GitEvent::OpenConflict(ConflictRequest {
                target: target.clone(),
                path: path.to_owned(),
            }));
        }
    }

    /// Drafts a request to resolve `paths` for the worktree's default agent, without sending it.
    pub(crate) fn prepare_conflict_draft(
        &self,
        target: Target,
        paths: &[String],
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        let agent = crate::app::AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .default_cli_agent();
        let operation = self
            .operation
            .filter(|_| self.target.as_ref() == Some(&target));
        let prompt = hive_core::agent::draft::conflicts(&target.path, paths, operation);
        cx.emit(GitEvent::AgentDraft(target, agent, prompt));
    }
}

#[cfg(test)]
mod tests {
    use super::{ConflictRequest, ConflictState, inspects_operation};
    use crate::state::{git::Git, projects::Projects, store::Store};
    use gpui_kit as gpui;
    use gpui_kit::{AppContext, TestAppContext};
    use hive_core::git::conflicts::{Conflict, Operation};
    use hive_core::git::status::{Change, FileChange, Status};
    use hive_core::{host::Host, projects::Target, state::StateError};

    #[test]
    fn polls_probe_for_an_operation_only_when_one_could_have_begun() {
        use hive_core::git::status::LineStats;
        let file = |staged, unstaged, added| FileChange {
            path: "a".into(),
            old_path: None,
            staged,
            unstaged,
            submodule: false,
            conflict: None,
            staged_stats: None,
            unstaged_stats: Some(LineStats { added, removed: 0 }),
        };
        let status = |files: Vec<FileChange>| Status {
            oid: Some("abc".into()),
            branch: Some("main".into()),
            files,
            ..Status::default()
        };
        let editing = status(vec![file(None, Some(Change::Modified), 1)]);
        let probes = |previous: &Status, next: &Status| {
            inspects_operation(false, Some(previous), next, None)
        };
        assert!(!probes(&editing, &editing));
        // An agent typing changes line counts on every poll; that starts no operation.
        let more_lines = status(vec![file(None, Some(Change::Modified), 40)]);
        assert!(!probes(&editing, &more_lines));
        // A merge without a commit stages files; a rebase moves HEAD or detaches it.
        let staged = status(vec![file(Some(Change::Modified), None, 1)]);
        assert!(probes(&editing, &staged));
        assert!(probes(&editing, &status(vec![])));
        let moved = Status {
            oid: Some("def".into()),
            ..editing.clone()
        };
        assert!(probes(&editing, &moved));
        let detached = Status {
            branch: None,
            ..editing.clone()
        };
        assert!(probes(&editing, &detached));
        // The first read, a forced refresh and a known operation always probe.
        assert!(inspects_operation(false, None, &editing, None));
        assert!(inspects_operation(true, Some(&editing), &editing, None));
        assert!(inspects_operation(
            false,
            Some(&editing),
            &editing,
            Some(Operation::Rebase)
        ));
    }

    #[gpui::test]
    fn conflict_state_follows_the_cached_status_of_its_own_worktree(cx: &mut TestAppContext) {
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
        let target = |path: &str| Target {
            host: Host::Local,
            project: "/unused".into(),
            path: path.into(),
        };
        let request = |path: &str| ConflictRequest {
            target: target("/unused"),
            path: path.into(),
        };
        let file = |path: &str, conflict: Option<Conflict>| FileChange {
            path: path.into(),
            old_path: None,
            staged: Some(if conflict.is_some() {
                Change::Conflicted
            } else {
                Change::Modified
            }),
            unstaged: None,
            submodule: false,
            conflict,
            staged_stats: None,
            unstaged_stats: None,
        };
        git.update(cx, |git, _| {
            assert_eq!(git.conflict_state(&request("a")), ConflictState::Unknown);
            git.target = Some(target("/unused"));
            assert_eq!(git.conflict_state(&request("a")), ConflictState::Unknown);
            git.status = Some(Status {
                files: vec![
                    file("a", Some(Conflict::DeletedByThem)),
                    file("b", None),
                    file("c", Some(Conflict::BothModified)),
                ],
                ..Status::default()
            });
            assert_eq!(
                git.conflict_state(&request("a")),
                ConflictState::Unmerged(Conflict::DeletedByThem)
            );
            assert_eq!(git.conflict_state(&request("b")), ConflictState::Resolved);
            assert_eq!(
                git.conflict_state(&request("gone")),
                ConflictState::Resolved
            );
            assert_eq!(git.conflicted_paths(), ["a", "c"]);
            // Another worktree's status says nothing about this tab's file.
            git.target = Some(target("/elsewhere"));
            assert_eq!(git.conflict_state(&request("a")), ConflictState::Unknown);
        });
    }
}
