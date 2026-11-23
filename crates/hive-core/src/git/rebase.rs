//! Replaying the current branch on top of another commit: the start of the operation that
//! [`conflicts`] continues and aborts.
use super::changes::{self, Synced};
use super::{GitError, branches, conflicts, history, run, run_stoppable, status};
use crate::command::Cancellation;
use crate::host::Host;
use std::path::Path;

/// How a replay left the worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Every commit was replayed.
    Finished,
    /// Git stopped on a conflict; the rebase is unfinished.
    Paused,
}

/// Blocking: replays the commits of the current branch that `commit` lacks on top of it.
///
/// Refused before Git changes anything when the worktree is not clean, HEAD is unborn or
/// detached, the two share no history, or the commits to replay include a merge, which a
/// rebase would silently flatten. The flags pin the behavior whatever `rebase.*` and `pull.*`
/// say in the user's configuration: no stash, no squashing, no other branch moved.
pub(super) fn onto(
    host: &Host,
    cwd: &Path,
    commit: &str,
    stop: &Cancellation,
) -> Result<Outcome, GitError> {
    require_branch(host, cwd)?;
    let target = branches::resolve(host, cwd, commit)?;
    match run(host, cwd, &["merge-base", &target, "HEAD"]) {
        Ok(_) => {}
        Err(GitError::Failed { code: Some(1), .. }) => {
            return Err(GitError::Invalid(
                "This branch shares no history with the commit to rebase onto.".into(),
            ));
        }
        Err(error) => return Err(error),
    }
    if history::count(host, cwd, &["--merges", &format!("{target}..HEAD")])? > 0 {
        return Err(GitError::Invalid(
            "This branch contains merge commits, which a rebase would flatten. Rebase it in the terminal.".into(),
        ));
    }
    let replayed = run_stoppable(
        host,
        cwd,
        &[
            "-c",
            "rebase.updateRefs=false",
            "rebase",
            "--merge",
            "--no-autostash",
            "--no-autosquash",
            "--no-fork-point",
            &target,
        ],
        stop,
    );
    Ok(if conflicts::paused(host, cwd, replayed)? {
        Outcome::Paused
    } else {
        Outcome::Finished
    })
}

/// Rebase the current branch onto the branch `reference` names, fetching its remote first
/// when it is a remote-tracking branch.
///
/// `None` when the replay stopped on a conflict.
pub(super) fn onto_branch(
    host: &Host,
    cwd: &Path,
    reference: &str,
    stop: &Cancellation,
) -> Result<Option<Synced>, GitError> {
    let base = branches::list(host, cwd)?
        .into_iter()
        .find(|branch| branch.reference == reference)
        .ok_or_else(|| GitError::Invalid("Select an existing branch to rebase onto.".into()))?;
    // Nothing is fetched for a worktree the rebase would refuse anyway.
    let before = require_branch(host, cwd)?;
    if before
        .branch
        .as_ref()
        .is_some_and(|branch| format!("refs/heads/{branch}") == base.reference)
    {
        return Err(GitError::Invalid(format!(
            "{} is the current branch.",
            base.name
        )));
    }
    if base.remote {
        let remote = remote_of(host, cwd, &base.reference)?;
        changes::fetch_remote(host, cwd, &remote, stop)?;
    }
    if onto(host, cwd, &base.reference, stop)? == Outcome::Paused {
        return Ok(None);
    }
    let after = status::read(host, cwd)?;
    Ok(Some(Synced::Rebased {
        onto: base.name,
        changed: after.oid != before.oid,
        needs_force_push: after.tracked_upstream().is_some() && after.behind > 0,
    }))
}

/// The status of a branch a replay may move: a clean worktree with no unfinished operation,
/// whose HEAD is a branch with at least one commit.
fn require_branch(host: &Host, cwd: &Path) -> Result<status::Status, GitError> {
    branches::require_clean(host, cwd)?;
    let head = status::read(host, cwd)?;
    if head.oid.is_none() {
        return Err(GitError::Invalid(
            "Create the first commit before rebasing.".into(),
        ));
    }
    if head.branch.is_none() {
        return Err(GitError::Invalid(
            "Check out a branch before rebasing.".into(),
        ));
    }
    Ok(head)
}

/// The configured remote a remote-tracking ref belongs to. A remote's name may itself
/// contain a slash, so the ref is matched against the configured names, longest first.
fn remote_of(host: &Host, cwd: &Path, reference: &str) -> Result<String, GitError> {
    let tracked = reference.strip_prefix("refs/remotes/").unwrap_or(reference);
    changes::remotes(host, cwd)?
        .into_iter()
        .filter(|remote| {
            tracked
                .strip_prefix(remote.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
        })
        .max_by_key(String::len)
        .ok_or_else(|| GitError::Invalid(format!("No configured remote owns {tracked}.")))
}
