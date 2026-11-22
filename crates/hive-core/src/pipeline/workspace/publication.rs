//! Verification of the exact remote branch that will be supplied to the hosting CLI.
use super::*;
use crate::git::pull_request::{self, PublishedBranch};
use crate::pipeline::{Phase, Run};

/// Recheck the registered checkout, reviewed tree and live remote ref immediately before delivery.
pub fn publication(run: &Run, index: usize) -> Result<PublishedBranch, GitError> {
    let (url, branch, commit) = published_remote(run, index)?;
    Ok(PublishedBranch {
        repository: pull_request::hosted_repository(&Host::Local, &url)?,
        branch,
        commit,
    })
}

pub(super) fn published_remote(
    run: &Run,
    index: usize,
) -> Result<(String, String, String), GitError> {
    let lane = run
        .lanes()
        .get(index)
        .filter(|l| run.phase() == Phase::Accepted && !l.assignment.is_empty())
        .ok_or_else(|| GitError::Invalid("Accept this lane before opening a request.".into()))?;
    verify(run.id(), lane)?;
    let cwd = &lane.worktree;
    let name = branch(run.id(), lane);
    let reference = format!("refs/heads/{name}");
    let head = git::branches::resolve(&Host::Local, cwd, &reference)?;
    if !git::status::read(&Host::Local, cwd)?.is_clean() {
        return Err(GitError::Invalid(
            "Commit the accepted changes in the worktree first.".into(),
        ));
    }
    let actual = git::run(
        &Host::Local,
        cwd,
        &["rev-parse", "--verify", &format!("{head}^{{tree}}")],
    )?;
    let matches = match run.review_trees().get(index) {
        Some(expected) => expected == &actual,
        None => {
            run.review_revision()
                == Some(revision(run.id(), run.lanes(), &Cancellation::default())?.as_str())
        }
    };
    if !matches {
        return Err(GitError::Invalid(
            "The committed files differ from the accepted review. Inspect the worktree.".into(),
        ));
    }
    let upstream = git::run(
        &Host::Local,
        cwd,
        &[
            "for-each-ref",
            "--format=%(upstream:remotename)",
            &reference,
        ],
    )?;
    let remote = if upstream.is_empty() {
        "origin"
    } else {
        &upstream
    };
    let url = git::run(&Host::Local, cwd, &["remote", "get-url", "--", remote])?;
    if !pull_request::is_published(&Host::Local, cwd, remote, &reference, &head)? {
        return Err(GitError::Invalid(
            "Push the accepted commit to this branch before opening its request.".into(),
        ));
    }
    Ok((url, name, head))
}
