//! Operations that talk to a remote: fetch, fast-forward pull, push, sync and force push.
//!
//! A push always names its destination. Nothing here rewrites a remote branch except
//! [`force_push`], which is bound to the remote commit the user confirmed.
use super::Synced;
use crate::command::Cancellation;
use crate::git::{GitError, branches, history, rebase, run, run_stoppable, status};
use crate::host::Host;
use std::path::Path;

/// How many of the commits a force push discards its confirmation lists by subject.
const PREVIEW_COMMITS: usize = 8;

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

/// Updates the remote-tracking refs of `remote`, which must be a configured remote.
pub(in crate::git) fn fetch_remote(
    host: &Host,
    cwd: &Path,
    remote: &str,
    stop: &Cancellation,
) -> Result<(), GitError> {
    remote_exists(host, cwd, remote)?;
    run_stoppable(
        host,
        cwd,
        &["-c", "credential.interactive=false", "fetch", remote],
        stop,
    )
    .map(drop)
}

pub(super) fn fetch(
    host: &Host,
    cwd: &Path,
    remote: &str,
    stop: &Cancellation,
) -> Result<Synced, GitError> {
    fetch_remote(host, cwd, remote, stop)?;
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

pub(super) fn pull(host: &Host, cwd: &Path, stop: &Cancellation) -> Result<Synced, GitError> {
    branches::require_clean(host, cwd)?;
    let before = status::read(host, cwd)?;
    if before.upstream.is_none() {
        return Err(GitError::Invalid(
            "Publish this branch or configure an upstream before pulling.".into(),
        ));
    }
    run_stoppable(
        host,
        cwd,
        &[
            "-c",
            "credential.interactive=false",
            "pull",
            "--no-rebase",
            "--ff-only",
        ],
        stop,
    )?;
    // Fast-forward only, so the new commits are exactly those between the old and new HEAD.
    let range = match before.oid {
        Some(oid) => format!("{oid}..HEAD"),
        None => "HEAD".into(),
    };
    Ok(Synced::Pulled {
        commits: history::count(host, cwd, &[&range])?,
    })
}

pub(super) fn push_branch(
    host: &Host,
    cwd: &Path,
    remote: &str,
    publish: bool,
    stop: &Cancellation,
) -> Result<Synced, GitError> {
    let destination = push_destination(host, cwd, remote, publish)?;
    let ahead = status::read(host, cwd)?.ahead;
    push(host, cwd, remote, publish, &destination, stop)?;
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

/// The remote ref a push of the current branch to `remote` would update.
pub(in crate::git) fn push_destination(
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

pub(super) fn push(
    host: &Host,
    cwd: &Path,
    remote: &str,
    publish: bool,
    destination: &str,
    stop: &Cancellation,
) -> Result<(), GitError> {
    let refspec = format!("HEAD:{destination}");
    let mut args = vec!["-c", "credential.interactive=false", "push"];
    if publish {
        args.push("--set-upstream");
    }
    args.extend([remote, &refspec]);
    run_stoppable(host, cwd, &args, stop).map(drop)
}

/// The upstream ref on `remote` of a branch that has commits and is published there; what a
/// sync or a force push needs before it contacts the remote.
pub(super) fn upstream_destination(
    host: &Host,
    cwd: &Path,
    remote: &str,
) -> Result<String, GitError> {
    let status = status::read(host, cwd)?;
    if status.oid.is_none() {
        return Err(GitError::Invalid("Create the first commit first.".into()));
    }
    if status.branch.is_some() && status.upstream.is_none() {
        return Err(GitError::Invalid("Publish this branch first.".into()));
    }
    push_destination(host, cwd, remote, false)
}

/// Fetch `remote`, replay the local commits on top of the upstream, then push them.
///
/// `None` when the replay stopped on a conflict: nothing is pushed, and the unfinished rebase
/// is the user's to continue or abort.
pub(super) fn sync(
    host: &Host,
    cwd: &Path,
    remote: &str,
    stop: &Cancellation,
) -> Result<Option<Synced>, GitError> {
    let destination = upstream_destination(host, cwd, remote)?;
    branches::require_clean(host, cwd)?;
    fetch_remote(host, cwd, remote, stop)?;
    let upstream = upstream_commit(host, cwd)?;
    let fetched = status::read(host, cwd)?;
    if fetched.behind > 0 {
        if fetched.ahead > 0 {
            refuse_rewritten(host, cwd, &fetched, &upstream)?;
        }
        if rebase::onto(host, cwd, &upstream, stop)? == rebase::Outcome::Paused {
            return Ok(None);
        }
    }
    let pushed = status::read(host, cwd)?.ahead;
    if pushed > 0 {
        push(host, cwd, remote, false, &destination, stop)?;
    }
    Ok(Some(Synced::PulledAndPushed {
        remote: remote.into(),
        pulled: fetched.behind,
        pushed,
    }))
}

/// The commit the branch's upstream points at, as last fetched.
fn upstream_commit(host: &Host, cwd: &Path) -> Result<String, GitError> {
    match run(
        host,
        cwd,
        &["rev-parse", "--verify", "--quiet", "@{upstream}^{commit}"],
    ) {
        Ok(oid) => Ok(oid),
        Err(GitError::Failed { .. }) => Err(GitError::Invalid(
            "The upstream branch no longer exists on the remote. Publish this branch again.".into(),
        )),
        Err(error) => Err(error),
    }
}

/// A branch whose reflog holds the upstream's commit once pointed at it and has since been
/// rebased, amended or reset. Replaying it on that commit would bring back, as new commits,
/// whatever the rewrite moved; only a force push reconciles the two.
fn refuse_rewritten(
    host: &Host,
    cwd: &Path,
    status: &status::Status,
    upstream: &str,
) -> Result<(), GitError> {
    let Some(branch) = &status.branch else {
        return Ok(());
    };
    let reference = format!("refs/heads/{branch}");
    // An expired or disabled reflog proves nothing, and the replay goes ahead.
    let visited = match run(host, cwd, &["rev-list", "-g", &reference, "--"]) {
        Ok(visited) => visited,
        Err(GitError::Failed { .. }) => return Ok(()),
        Err(error) => return Err(error),
    };
    if !visited.lines().any(|oid| oid == upstream) {
        return Ok(());
    }
    let name = status.upstream.as_deref().unwrap_or("the remote branch");
    Err(GitError::Invalid(format!(
        "{branch} was rewritten after it was pushed, so replaying it on {name} would duplicate commits. Use Force Push to replace {name}, or reconcile the two in the terminal."
    )))
}

/// What a force push would replace, read for its confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForcePushPreview {
    /// Display name of the remote branch, such as `origin/feature`.
    pub upstream: String,
    /// The commit the remote branch was at when last fetched; the push is bound to it.
    pub expected: String,
    /// Abbreviated id and subject of the newest commits the push discards.
    pub discarded: Vec<String>,
    /// How many commits the push discards in total.
    pub total: usize,
}

/// Blocking: reads the remote commits a force push of the current branch would discard.
pub fn force_push_preview(
    host: &Host,
    cwd: &Path,
    remote: &str,
) -> Result<ForcePushPreview, GitError> {
    force_destination(host, cwd, remote)?;
    let expected = upstream_commit(host, cwd)?;
    let range = format!("HEAD..{expected}");
    let limit = format!("--max-count={PREVIEW_COMMITS}");
    let discarded = run(host, cwd, &["log", "--format=%h %s", &limit, &range, "--"])?
        .lines()
        .map(String::from)
        .collect();
    Ok(ForcePushPreview {
        upstream: run(host, cwd, &["rev-parse", "--abbrev-ref", "@{upstream}"])?,
        total: history::count(host, cwd, &[&range])?,
        expected,
        discarded,
    })
}

/// The remote ref a force push may replace: the one named like the current branch. A branch
/// that tracks another name, such as a topic started from `origin/develop`, must never
/// overwrite that branch.
fn force_destination(host: &Host, cwd: &Path, remote: &str) -> Result<String, GitError> {
    let destination = upstream_destination(host, cwd, remote)?;
    let branch = status::read(host, cwd)?.branch.unwrap_or_default();
    if destination != format!("refs/heads/{branch}") {
        return Err(GitError::Invalid(format!(
            "Force push only replaces a remote branch of the same name; {branch} tracks {remote}/{}.",
            branches::short_name(&destination)
        )));
    }
    Ok(destination)
}

/// Replace the remote branch with the local one, unless the remote has moved off `expected`,
/// the commit the user confirmed discarding from.
pub(super) fn force_push(
    host: &Host,
    cwd: &Path,
    remote: &str,
    expected: &str,
    stop: &Cancellation,
) -> Result<Synced, GitError> {
    if expected.is_empty() || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GitError::Invalid(
            "Review the remote branch again before force pushing.".into(),
        ));
    }
    let destination = force_destination(host, cwd, remote)?;
    let lease = format!("--force-with-lease={destination}:{expected}");
    let refspec = format!("HEAD:{destination}");
    let pushed = run_stoppable(
        host,
        cwd,
        &[
            "-c",
            "credential.interactive=false",
            "push",
            &lease,
            remote,
            &refspec,
        ],
        stop,
    );
    match pushed {
        Ok(_) => Ok(Synced::ForcePushed {
            remote: remote.into(),
        }),
        // Git's name for a lease the remote no longer satisfies.
        Err(GitError::Failed { stderr, .. }) if stderr.contains("stale info") => {
            Err(GitError::Invalid(format!(
                "{remote}/{} changed since you confirmed. Fetch and review it again.",
                branches::short_name(&destination)
            )))
        }
        Err(error) => Err(error),
    }
}
