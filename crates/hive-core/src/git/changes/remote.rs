//! Operations that talk to a remote: fetch, fast-forward pull, push, sync and force push.
//!
//! A push always names its source branch and its destination, and only ever updates the
//! remote branch of the same name, as Git's own `push.default=simple` does. Nothing here
//! rewrites a remote branch except [`force_push`], which is bound to both commits the user
//! confirmed: the remote one it replaces and the local one it sends.
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
    // A branch whose upstream is gone has nothing to count incoming commits against.
    let tracks_remote = match (&status.branch, status.tracked_upstream()) {
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
    if before.upstream_gone {
        return Err(unpublished(&before));
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
    let (branch, destination) = push_route(host, cwd, remote, publish)?;
    let ahead = status::read(host, cwd)?.ahead;
    push(host, cwd, remote, publish, &branch, &destination, stop)?;
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

/// The remote ref a push of the current branch to `remote` would update: the branch it
/// creates when publishing, else whatever the branch's upstream names, of any name.
pub(in crate::git) fn push_destination(
    host: &Host,
    cwd: &Path,
    remote: &str,
    publish: bool,
) -> Result<String, GitError> {
    route(host, cwd, remote, publish).map(|(_, destination)| destination)
}

/// The current branch and the remote ref a push of it to `remote` would update.
fn route(
    host: &Host,
    cwd: &Path,
    remote: &str,
    publish: bool,
) -> Result<(String, String), GitError> {
    remote_exists(host, cwd, remote)?;
    let branch = status::read(host, cwd)?
        .branch
        .ok_or_else(|| GitError::Invalid("Create a branch before pushing detached HEAD.".into()))?;
    if publish {
        let destination = format!("refs/heads/{branch}");
        return Ok((branch, destination));
    }
    if upstream_remote(host, cwd, &branch)?.as_deref() != Some(remote) {
        return Err(GitError::Invalid(
            "Push must use this branch's configured upstream remote.".into(),
        ));
    }
    let destination = run(
        host,
        cwd,
        &["config", "--get", &format!("branch.{branch}.merge")],
    )?;
    Ok((branch, destination))
}

/// The branch a push from the current one sends and the remote ref it updates. A branch
/// started from `origin/develop` tracks `develop`; pushing it there would put unreviewed work
/// on the integration branch, so only a remote branch of the same name is ever updated.
pub(super) fn push_route(
    host: &Host,
    cwd: &Path,
    remote: &str,
    publish: bool,
) -> Result<(String, String), GitError> {
    let (branch, destination) = route(host, cwd, remote, publish)?;
    namesake(&branch, remote, &destination)?;
    Ok((branch, destination))
}

fn namesake(branch: &str, remote: &str, destination: &str) -> Result<(), GitError> {
    if destination.strip_prefix("refs/heads/") == Some(branch) {
        return Ok(());
    }
    Err(GitError::Invalid(format!(
        "{branch} tracks {remote}/{}: Hive pushes a branch only to the remote branch of the same name. Publish it under its own name, or push from the terminal.",
        branches::short_name(destination)
    )))
}

/// Sends `branch`, by name rather than HEAD, so a checkout that moves on while the push
/// starts cannot send another branch to `destination`.
pub(super) fn push(
    host: &Host,
    cwd: &Path,
    remote: &str,
    publish: bool,
    branch: &str,
    destination: &str,
    stop: &Cancellation,
) -> Result<(), GitError> {
    let refspec = format!("refs/heads/{branch}:{destination}");
    let mut args = vec!["-c", "credential.interactive=false", "push"];
    if publish {
        args.push("--set-upstream");
    }
    args.extend([remote, &refspec]);
    run_stoppable(host, cwd, &args, stop).map(drop)
}

/// The current branch and its upstream ref on `remote`, for a branch that has commits and is
/// published there under its own name; what a sync or a force push needs before it contacts
/// the remote.
pub(super) fn upstream_route(
    host: &Host,
    cwd: &Path,
    remote: &str,
) -> Result<(String, String), GitError> {
    let status = status::read(host, cwd)?;
    if status.oid.is_none() {
        return Err(GitError::Invalid("Create the first commit first.".into()));
    }
    if status.branch.is_some() && status.tracked_upstream().is_none() {
        return Err(unpublished(&status));
    }
    push_route(host, cwd, remote, false)
}

/// Why a branch without a usable upstream cannot be pulled, synced or force-pushed. Both
/// texts name Publish, which the Git panel offers for exactly these branches.
fn unpublished(status: &status::Status) -> GitError {
    GitError::Invalid(match &status.upstream {
        Some(upstream) if status.upstream_gone => format!(
            "{upstream} does not exist on the remote: it was deleted there, or never pushed. Publish this branch to create it."
        ),
        _ => "Publish this branch first.".into(),
    })
}

/// Blocking: the commit `reference` points at on `remote` right now, asked of the remote
/// itself rather than of the last fetch; `None` when the remote has no such ref.
pub(in crate::git) fn remote_tip(
    host: &Host,
    cwd: &Path,
    remote: &str,
    reference: &str,
) -> Result<Option<String>, GitError> {
    let listed = run(
        host,
        cwd,
        &[
            "-c",
            "credential.interactive=false",
            "ls-remote",
            "--refs",
            "--",
            remote,
            reference,
        ],
    )?;
    // The pattern also matches longer refs that end the same way, so the name is compared.
    Ok(listed
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find_map(|(oid, name)| (name == reference).then(|| oid.to_owned())))
}

/// A fetch without `--prune` keeps the remote-tracking ref of a branch deleted on the remote,
/// so only the remote itself can say the branch is gone.
///
/// The advice has to be something the Git panel allows. While the stale ref exists the panel
/// still counts the branch as published, so it offers Push only for commits the ref lacks;
/// Publish comes back once the ref is pruned, which Hive leaves to the user: a prune drops
/// every stale ref of the remote, not only this one.
fn deleted_on_remote(
    host: &Host,
    cwd: &Path,
    remote: &str,
    destination: &str,
) -> Result<GitError, GitError> {
    let name = branches::short_name(destination);
    let advice = if status::read(host, cwd)?.ahead > 0 {
        format!("Push sends {name} there again; switch to another branch if it was merged.")
    } else {
        format!(
            "{name} has no commits of its own to push: switch to another branch if it was merged. To publish it again, run `git fetch --prune {remote}` in the terminal first."
        )
    };
    Ok(GitError::Invalid(format!(
        "{remote}/{name} was deleted on the remote. {advice}"
    )))
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
    let (branch, destination) = upstream_route(host, cwd, remote)?;
    branches::require_clean(host, cwd)?;
    // Replaying onto a stale ref and pushing would bring a merged and deleted branch back.
    if remote_tip(host, cwd, remote, &destination)?.is_none() {
        return Err(deleted_on_remote(host, cwd, remote, &destination)?);
    }
    fetch_remote(host, cwd, remote, stop)?;
    let upstream = upstream_commit(host, cwd)?;
    let fetched = status::read(host, cwd)?;
    if fetched.branch.as_deref() != Some(branch.as_str()) {
        return Err(GitError::Invalid(format!(
            "The worktree left {branch} during the sync; nothing was replayed or pushed."
        )));
    }
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
        push(host, cwd, remote, false, &branch, &destination, stop)?;
    }
    Ok(Some(Synced::PulledAndPushed {
        remote: remote.into(),
        pulled: fetched.behind,
        pushed,
    }))
}

/// The commit the branch's upstream points at, as last fetched.
pub(super) fn upstream_commit(host: &Host, cwd: &Path) -> Result<String, GitError> {
    match run(
        host,
        cwd,
        &["rev-parse", "--verify", "--quiet", "@{upstream}^{commit}"],
    ) {
        Ok(oid) => Ok(oid),
        // `--quiet` answers a ref that is not there with a silent exit 1. Any other failure,
        // a dropped SSH connection among them, is Git not answering, not the ref missing.
        Err(GitError::Failed { code: Some(1), .. }) => Err(GitError::Invalid(
            "The upstream branch no longer exists on the remote. Publish this branch again.".into(),
        )),
        Err(error) => Err(error),
    }
}

/// A branch whose reflog holds the upstream's commit once pointed at it and has since been
/// rebased, amended or reset. Replaying it on that commit would bring back, as new commits,
/// whatever the rewrite moved; only a force push reconciles the two.
pub(super) fn refuse_rewritten(
    host: &Host,
    cwd: &Path,
    status: &status::Status,
    upstream: &str,
) -> Result<(), GitError> {
    let Some(branch) = &status.branch else {
        return Ok(());
    };
    let reference = format!("refs/heads/{branch}");
    // An expired or disabled reflog lists nothing: that proves nothing, and the replay goes
    // ahead. A Git that fails to answer is another matter, and stops the sync: taking the
    // failure for "never rewritten" would lift this guard whenever the connection drops.
    let visited = run(host, cwd, &["rev-list", "-g", &reference, "--"])?;
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
    /// The commit the remote branch is at, as the remote itself confirmed; the push is bound
    /// to it.
    pub expected: String,
    /// The local commit the push sends; the push is refused once the branch moves off it.
    pub head: String,
    /// Abbreviated id and subject of the newest commits the push discards.
    pub discarded: Vec<String>,
    /// How many commits the push discards in total.
    pub total: usize,
}

/// Blocking: reads the remote commits a force push of the current branch would discard.
///
/// The list is read from the last fetch, so the remote is asked first whether that fetch still
/// holds: a branch deleted there, or moved since, would make the list describe the wrong loss.
pub fn force_push_preview(
    host: &Host,
    cwd: &Path,
    remote: &str,
) -> Result<ForcePushPreview, GitError> {
    let (_, destination) = upstream_route(host, cwd, remote)?;
    let expected = upstream_commit(host, cwd)?;
    match remote_tip(host, cwd, remote, &destination)? {
        None => return Err(deleted_on_remote(host, cwd, remote, &destination)?),
        Some(tip) if tip != expected => {
            return Err(GitError::Invalid(format!(
                "{remote}/{} moved since the last fetch. Fetch, then review the force push again.",
                branches::short_name(&destination)
            )));
        }
        Some(_) => {}
    }
    let head = branches::resolve(host, cwd, "HEAD")?;
    let range = format!("{head}..{expected}");
    let limit = format!("--max-count={PREVIEW_COMMITS}");
    let discarded = run(host, cwd, &["log", "--format=%h %s", &limit, &range, "--"])?
        .lines()
        .map(String::from)
        .collect();
    Ok(ForcePushPreview {
        upstream: run(host, cwd, &["rev-parse", "--abbrev-ref", "@{upstream}"])?,
        total: history::count(host, cwd, &[&range])?,
        expected,
        head,
        discarded,
    })
}

/// Replace the remote branch with `head`, unless the remote has moved off `expected` or the
/// local branch has moved off `head`: the two commits whose replacement the user confirmed.
pub(super) fn force_push(
    host: &Host,
    cwd: &Path,
    remote: &str,
    expected: &str,
    head: &str,
    stop: &Cancellation,
) -> Result<Synced, GitError> {
    let is_commit = |oid: &str| !oid.is_empty() && oid.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !is_commit(expected) || !is_commit(head) {
        return Err(GitError::Invalid(
            "Review the remote branch again before force pushing.".into(),
        ));
    }
    let (branch, destination) = upstream_route(host, cwd, remote)?;
    // An agent that commits or resets in this worktree meanwhile would otherwise make the
    // push discard more than the confirmation listed.
    if status::read(host, cwd)?.oid.as_deref() != Some(head) {
        return Err(GitError::Invalid(format!(
            "{branch} moved since you confirmed. Review the force push again."
        )));
    }
    let lease = format!("--force-with-lease={destination}:{expected}");
    // The confirmed commit rather than HEAD, so nothing committed after the check rides along.
    let refspec = format!("{head}:{destination}");
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
