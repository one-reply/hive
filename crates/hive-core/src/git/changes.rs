//! Explicit index, commit, remote and rebase operations. No implicit staging or automatic
//! stash; the one force push is bound to the commits the user confirmed.
use super::{GitError, READ_LIMIT, branches, bytes_mutation, conflicts, diff, rebase, status};
use crate::command::Cancellation;
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

mod discard;
mod remote;
pub use discard::Discard;
pub use remote::{ForcePushPreview, force_push_preview, remotes, upstream_remote};
pub(super) use remote::{fetch_remote, push_destination, remote_tip};

/// A user-requested Git mutation, scoped by the caller to one host and worktree.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    /// Stage the listed literal paths, including deletions.
    Stage(Vec<String>),
    /// Restore the listed index paths to HEAD, preserving working files.
    Unstage(Vec<String>),
    /// Apply the confirmed operation to these exact working-tree paths.
    Discard { kind: Discard, paths: Vec<String> },
    /// Commit only the index, honoring hooks and signing configuration.
    Commit(String),
    /// Commit the index, then push; a push failure leaves the local commit intact.
    CommitAndPush {
        message: String,
        remote: String,
        publish: bool,
    },
    /// Switch to an existing local branch with a clean checkout.
    Switch(String),
    /// Create a branch from a base; tracking is explicit.
    CreateBranch {
        name: String,
        base: String,
        track: bool,
    },
    /// Delete a local branch that no worktree has checked out; `force` discards unmerged
    /// commits, which the caller must have confirmed separately.
    DeleteBranch { name: String, force: bool },
    /// Fetch an explicitly selected remote.
    Fetch(String),
    /// Fast-forward the current branch from its configured upstream.
    Pull,
    /// Push to the configured upstream or explicitly publish to a remote.
    Push { remote: String, publish: bool },
    /// Fetch the upstream's remote, replay the local commits on top of the upstream, then
    /// push them; a conflict leaves the rebase unfinished and nothing pushed.
    Sync { remote: String },
    /// Commit the index, then sync; a failure after the commit leaves it intact.
    CommitAndSync { message: String, remote: String },
    /// Replace the remote branch of the same name with `head`, unless the remote has moved
    /// off `expected` or the local branch off `head`: the commits [`ForcePushPreview`] read,
    /// whose replacement the caller must have confirmed.
    ForcePush {
        remote: String,
        expected: String,
        head: String,
    },
    /// Replay the current branch on top of the branch `onto` names by its full ref, fetching
    /// its remote first; the caller must have confirmed rewriting the branch.
    Rebase { onto: String },
    /// Resolve one unmerged path with a whole side, replacing the working file; the caller
    /// must have confirmed the loss of its content.
    ResolveWith { path: String, side: conflicts::Side },
    /// Abandon the unfinished merge, rebase, cherry-pick or revert.
    AbortOperation,
    /// Conclude the unfinished operation once every conflict is resolved.
    ContinueOperation,
}

impl Action {
    /// Whether [`execute`] can end this action early: it runs hooks or talks to a remote, so
    /// it may take minutes. A checkout counts: it runs `post-checkout` and content filters
    /// such as LFS, which download. The rest are local and finish at once.
    pub fn is_stoppable(&self) -> bool {
        matches!(
            self,
            Action::Commit(_)
                | Action::CommitAndPush { .. }
                | Action::Switch(_)
                | Action::CreateBranch { .. }
                | Action::AbortOperation
                | Action::Fetch(_)
                | Action::Pull
                | Action::Push { .. }
                | Action::Sync { .. }
                | Action::CommitAndSync { .. }
                | Action::ForcePush { .. }
                | Action::Rebase { .. }
                | Action::ContinueOperation
        )
    }
}

/// What a remote operation or a rebase changed, for a short confirmation once it finishes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Synced {
    /// Fetched `remote`; `incoming` counts upstream commits not yet in HEAD, known only
    /// when the current branch tracks that remote.
    Fetched {
        remote: String,
        incoming: Option<usize>,
    },
    /// Fast-forwarded HEAD by this many commits; zero means it was already up to date.
    Pulled { commits: usize },
    /// Sent this many commits to the branch's upstream on `remote`.
    Pushed { remote: String, commits: usize },
    /// Created the branch on `remote` and started tracking it.
    Published { remote: String },
    /// Synced with the upstream on `remote`: `pulled` commits came in and the local ones
    /// were replayed on top of them, then `pushed` commits went out.
    PulledAndPushed {
        remote: String,
        pulled: usize,
        pushed: usize,
    },
    /// Replaced the branch on `remote` with the local one.
    ForcePushed { remote: String },
    /// Replayed the branch on top of `onto`; unchanged when it already contained it.
    Rebased {
        onto: String,
        changed: bool,
        /// The upstream still holds the commits as they were before the replay.
        needs_force_push: bool,
    },
}

impl fmt::Display for Synced {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fetched {
                remote,
                incoming: Some(0),
            } => write!(f, "Fetched {remote} · up to date"),
            Self::Fetched {
                remote,
                incoming: Some(count),
            } => write!(f, "Fetched {remote} · {} to pull", commits(*count)),
            Self::Fetched {
                remote,
                incoming: None,
            } => write!(f, "Fetched {remote}"),
            Self::Pulled { commits: 0 } => f.write_str("Already up to date"),
            Self::Pulled { commits: count } => write!(f, "Pulled {}", commits(*count)),
            Self::Pushed { remote, commits: 0 } => write!(f, "Pushed to {remote}"),
            Self::Pushed {
                remote,
                commits: count,
            } => write!(f, "Pushed {} to {remote}", commits(*count)),
            Self::Published { remote } => write!(f, "Published to {remote}"),
            Self::PulledAndPushed {
                remote,
                pulled: 0,
                pushed: 0,
            } => write!(f, "Already in sync with {remote}"),
            Self::PulledAndPushed {
                pulled, pushed: 0, ..
            } => write!(f, "Pulled {}", commits(*pulled)),
            Self::PulledAndPushed {
                remote,
                pulled: 0,
                pushed,
            } => write!(f, "Pushed {} to {remote}", commits(*pushed)),
            Self::PulledAndPushed {
                remote,
                pulled,
                pushed,
            } => write!(
                f,
                "Pulled {}, pushed {pushed} to {remote}",
                commits(*pulled)
            ),
            Self::ForcePushed { remote } => write!(f, "Force-pushed to {remote}"),
            Self::Rebased {
                onto,
                changed: false,
                ..
            } => write!(f, "Already up to date with {onto}"),
            Self::Rebased {
                onto,
                needs_force_push: false,
                ..
            } => write!(f, "Rebased onto {onto}"),
            Self::Rebased { onto, .. } => {
                write!(f, "Rebased onto {onto} · Force Push to update the remote")
            }
        }
    }
}

fn commits(count: usize) -> String {
    match count {
        1 => "1 commit".into(),
        count => format!("{count} commits"),
    }
}

pub(super) fn paths(
    host: &Host,
    cwd: &Path,
    args: &[&str],
    paths: &[String],
) -> Result<(), GitError> {
    if paths.is_empty() {
        return Ok(());
    }
    for path in paths {
        diff::validate_path(path)?;
    }
    let mut command = vec!["--literal-pathspecs"];
    command.extend_from_slice(args);
    // NUL stdin avoids shell expansion and ARG_MAX for a large Stage All.
    command.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
    let input: Vec<u8> = paths
        .iter()
        .flat_map(|p| p.bytes().chain(std::iter::once(0)))
        .collect();
    bytes_mutation(
        host,
        cwd,
        &command,
        Some(&input),
        READ_LIMIT,
        false,
        &Cancellation::default(),
    )
    .map(drop)
}

/// Execute one mutation synchronously. Call only on a background executor.
///
/// Remote operations and a finished rebase report what they changed; local mutations and a
/// replay that stopped on a conflict return `None`.
///
/// `stop` ends the commands that run hooks or talk to a remote (commit, checkout, fetch, pull,
/// push, sync, rebase, and aborting or continuing an operation). A command ended midway
/// reports an unknown outcome; one the stop reached before it started reports that it changed
/// nothing. The quick local mutations ignore the stop.
pub fn execute(
    host: &Host,
    cwd: &Path,
    action: &Action,
    stop: &Cancellation,
) -> Result<Option<Synced>, GitError> {
    let local = match action {
        Action::Fetch(remote) => return remote::fetch(host, cwd, remote, stop).map(Some),
        Action::Pull => return remote::pull(host, cwd, stop).map(Some),
        Action::Push { remote, publish } => {
            return remote::push_branch(host, cwd, remote, *publish, stop).map(Some);
        }
        Action::Sync { remote } => return remote::sync(host, cwd, remote, stop),
        Action::CommitAndSync { message, remote } => {
            // Reject what the sync would refuse before creating a local commit.
            remote::upstream_route(host, cwd, remote)?;
            require_fully_staged(host, cwd)?;
            commit(host, cwd, message, stop)?;
            return remote::sync(host, cwd, remote, stop).map_err(|source| {
                GitError::SyncAfterCommit {
                    source: Box::new(source),
                }
            });
        }
        Action::ForcePush {
            remote,
            expected,
            head,
        } => {
            return remote::force_push(host, cwd, remote, expected, head, stop).map(Some);
        }
        Action::Rebase { onto } => return rebase::onto_branch(host, cwd, onto, stop),
        Action::Stage(selected) => paths(host, cwd, &["add", "-A"], selected),
        Action::Unstage(selected) => {
            if status::read(host, cwd)?.oid.is_some() {
                paths(host, cwd, &["restore", "--staged"], selected)
            } else {
                paths(
                    host,
                    cwd,
                    &["rm", "--cached", "--ignore-unmatch", "-r"],
                    selected,
                )
            }
        }
        Action::Discard { kind, paths } => kind.execute(host, cwd, paths),
        Action::Commit(message) => commit(host, cwd, message, stop),
        Action::CommitAndPush {
            message,
            remote,
            publish,
        } => {
            // Reject an unavailable destination before creating a local commit.
            let (branch, destination) = remote::push_route(host, cwd, remote, *publish)?;
            commit(host, cwd, message, stop)?;
            remote::push(host, cwd, remote, *publish, &branch, &destination, stop).map_err(
                |source| GitError::PushAfterCommit {
                    source: Box::new(source),
                },
            )
        }
        Action::Switch(name) => branches::switch(host, cwd, name, stop),
        Action::DeleteBranch { name, force } => branches::delete(host, cwd, name, *force),
        Action::CreateBranch { name, base, track } => {
            branches::create(host, cwd, name, base, *track, stop)
        }
        Action::ResolveWith { path, side } => conflicts::resolve_with(host, cwd, path, *side),
        Action::AbortOperation => conflicts::abort(host, cwd, stop),
        Action::ContinueOperation => conflicts::continue_operation(host, cwd, stop),
    };
    local.map(|()| None)
}

fn commit(host: &Host, cwd: &Path, message: &str, stop: &Cancellation) -> Result<(), GitError> {
    if message.trim().is_empty() {
        return Err(GitError::Invalid("Enter a commit message.".into()));
    }
    if status::read(host, cwd)?.has_conflicts() {
        return Err(GitError::Invalid(
            "Resolve conflicts before committing.".into(),
        ));
    }
    bytes_mutation(
        host,
        cwd,
        &["commit", "--file=-"],
        Some(message.as_bytes()),
        READ_LIMIT,
        false,
        stop,
    )
    .map(drop)
}

/// A sync rebases, which needs a clean worktree: everything changed has to be in the commit.
fn require_fully_staged(host: &Host, cwd: &Path) -> Result<(), GitError> {
    let status = status::read(host, cwd)?;
    if status.files.iter().any(|file| file.unstaged.is_some()) {
        return Err(GitError::Invalid(
            "Stage or discard the remaining changes first: syncing needs a clean worktree after the commit.".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
