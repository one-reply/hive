//! Explicit index, commit and remote operations. No force, implicit staging or automatic stash.
use super::{
    GitError, READ_LIMIT, branches, bytes_mutation, conflicts, diff, run, run_mutation, status,
};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

/// A user-requested Git mutation, scoped by the caller to one host and worktree.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    /// Stage the listed literal paths, including deletions.
    Stage(Vec<String>),
    /// Restore the listed index paths to HEAD, preserving working files.
    Unstage(Vec<String>),
    /// Restore one tracked working file from its index version, or delete it when untracked.
    Discard(String),
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
    /// Resolve one unmerged path with a whole side, replacing the working file; the caller
    /// must have confirmed the loss of its content.
    ResolveWith { path: String, side: conflicts::Side },
    /// Abandon the unfinished merge, rebase, cherry-pick or revert.
    AbortOperation,
    /// Conclude the unfinished operation once every conflict is resolved.
    ContinueOperation,
}

/// What a fetch, pull or push changed, for a short confirmation once it finishes.
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
        }
    }
}

fn commits(count: usize) -> String {
    match count {
        1 => "1 commit".into(),
        count => format!("{count} commits"),
    }
}

/// Configured remote names, never URLs containing credentials.
pub fn remotes(host: &Host, cwd: &Path) -> Result<Vec<String>, GitError> {
    Ok(run(host, cwd, &["remote"])?
        .lines()
        .map(String::from)
        .collect())
}

/// The remote configured for this branch, if it is a network remote.
pub fn upstream_remote(host: &Host, cwd: &Path, branch: &str) -> Result<Option<String>, GitError> {
    let value = run(
        host,
        cwd,
        &["config", "--get", &format!("branch.{branch}.remote")],
    );
    match value {
        Ok(value) => Ok((value != ".").then_some(value)),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(err) => Err(err),
    }
}

fn remote_exists(host: &Host, cwd: &Path, name: &str) -> Result<(), GitError> {
    if name.starts_with('-') || !remotes(host, cwd)?.iter().any(|r| r == name) {
        return Err(GitError::Invalid("Select a configured remote.".into()));
    }
    Ok(())
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
    bytes_mutation(host, cwd, &command, Some(&input), READ_LIMIT, false).map(drop)
}

/// Execute one mutation synchronously. Call only on a background executor.
///
/// Fetch, pull and push report what they changed; local mutations return `None`.
pub fn execute(host: &Host, cwd: &Path, action: &Action) -> Result<Option<Synced>, GitError> {
    let local = match action {
        Action::Fetch(remote) => return fetch(host, cwd, remote).map(Some),
        Action::Pull => return pull(host, cwd).map(Some),
        Action::Push { remote, publish } => {
            return push_branch(host, cwd, remote, *publish).map(Some);
        }
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
        Action::Discard(path) => {
            let state = status::read(host, cwd)?;
            let file = state
                .files
                .iter()
                .find(|f| f.path == *path)
                .ok_or_else(|| GitError::Invalid("This file no longer has changes.".into()))?;
            if matches!(file.unstaged, None | Some(status::Change::Conflicted)) || file.submodule {
                return Err(GitError::Invalid(
                    "Only unstaged files can be discarded here.".into(),
                ));
            }
            if file.unstaged == Some(status::Change::Untracked) {
                diff::validate_path(path)?;
                // `clean` has no --pathspec-from-file; status lists untracked files one by one,
                // so a single literal file path never needs `-d`.
                run_mutation(
                    host,
                    cwd,
                    &["--literal-pathspecs", "clean", "-f", "-q", "--", path],
                )
                .map(drop)
            } else {
                paths(
                    host,
                    cwd,
                    &["restore", "--worktree"],
                    std::slice::from_ref(path),
                )
            }
        }
        Action::Commit(message) => commit(host, cwd, message),
        Action::CommitAndPush {
            message,
            remote,
            publish,
        } => {
            // Reject an unavailable destination before creating a local commit.
            let destination = push_destination(host, cwd, remote, *publish)?;
            commit(host, cwd, message)?;
            push(host, cwd, remote, *publish, &destination).map_err(|source| {
                GitError::PushAfterCommit {
                    source: Box::new(source),
                }
            })
        }
        Action::Switch(name) => branches::switch(host, cwd, name),
        Action::DeleteBranch { name, force } => branches::delete(host, cwd, name, *force),
        Action::CreateBranch { name, base, track } => {
            branches::create(host, cwd, name, base, *track)
        }
        Action::ResolveWith { path, side } => conflicts::resolve_with(host, cwd, path, *side),
        Action::AbortOperation => conflicts::abort(host, cwd),
        Action::ContinueOperation => conflicts::continue_operation(host, cwd),
    };
    local.map(|()| None)
}

fn fetch(host: &Host, cwd: &Path, remote: &str) -> Result<Synced, GitError> {
    remote_exists(host, cwd, remote)?;
    run_mutation(
        host,
        cwd,
        &["-c", "credential.interactive=false", "fetch", remote],
    )?;
    let status = status::read(host, cwd)?;
    let tracks_remote = match (&status.branch, &status.upstream) {
        (Some(branch), Some(_)) => upstream_remote(host, cwd, branch)?.as_deref() == Some(remote),
        _ => false,
    };
    Ok(Synced::Fetched {
        remote: remote.into(),
        incoming: tracks_remote.then_some(status.behind),
    })
}

fn pull(host: &Host, cwd: &Path) -> Result<Synced, GitError> {
    branches::require_clean(host, cwd)?;
    let before = status::read(host, cwd)?;
    if before.upstream.is_none() {
        return Err(GitError::Invalid(
            "Publish this branch or configure an upstream before pulling.".into(),
        ));
    }
    run_mutation(
        host,
        cwd,
        &[
            "-c",
            "credential.interactive=false",
            "pull",
            "--no-rebase",
            "--ff-only",
        ],
    )?;
    // Fast-forward only, so the new commits are exactly those between the old and new HEAD.
    let range = match before.oid {
        Some(oid) => format!("{oid}..HEAD"),
        None => "HEAD".into(),
    };
    Ok(Synced::Pulled {
        commits: count_commits(host, cwd, &range)?,
    })
}

fn push_branch(host: &Host, cwd: &Path, remote: &str, publish: bool) -> Result<Synced, GitError> {
    let destination = push_destination(host, cwd, remote, publish)?;
    let ahead = status::read(host, cwd)?.ahead;
    push(host, cwd, remote, publish, &destination)?;
    Ok(if publish {
        Synced::Published {
            remote: remote.into(),
        }
    } else {
        Synced::Pushed {
            remote: remote.into(),
            commits: ahead,
        }
    })
}

fn count_commits(host: &Host, cwd: &Path, range: &str) -> Result<usize, GitError> {
    let raw = run(host, cwd, &["rev-list", "--count", range, "--"])?;
    raw.trim()
        .parse()
        .map_err(|_| GitError::Invalid(format!("Unexpected `git rev-list --count` output: {raw}")))
}

fn commit(host: &Host, cwd: &Path, message: &str) -> Result<(), GitError> {
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
    )
    .map(drop)
}

pub(super) fn push_destination(
    host: &Host,
    cwd: &Path,
    remote: &str,
    publish: bool,
) -> Result<String, GitError> {
    remote_exists(host, cwd, remote)?;
    let branch = status::read(host, cwd)?
        .branch
        .ok_or_else(|| GitError::Invalid("Create a branch before pushing detached HEAD.".into()))?;
    if publish {
        return Ok(format!("refs/heads/{branch}"));
    }
    if upstream_remote(host, cwd, &branch)?.as_deref() != Some(remote) {
        return Err(GitError::Invalid(
            "Push must use this branch's configured upstream remote.".into(),
        ));
    }
    run(
        host,
        cwd,
        &["config", "--get", &format!("branch.{branch}.merge")],
    )
}

fn push(
    host: &Host,
    cwd: &Path,
    remote: &str,
    publish: bool,
    destination: &str,
) -> Result<(), GitError> {
    let refspec = format!("HEAD:{destination}");
    let mut args = vec!["-c", "credential.interactive=false", "push"];
    if publish {
        args.push("--set-upstream");
    }
    args.extend([remote, &refspec]);
    run_mutation(host, cwd, &args).map(drop)
}

#[cfg(test)]
mod tests;
