//! Explicit index, commit and remote operations. No force, implicit staging or automatic stash.
use super::{GitError, READ_LIMIT, branches, bytes, diff, run, status};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A user-requested Git mutation, scoped by the caller to one host and worktree.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    /// Stage the listed literal paths, including deletions.
    Stage(Vec<String>),
    /// Restore the listed index paths to HEAD, preserving working files.
    Unstage(Vec<String>),
    /// Restore one tracked working file from its index version.
    Discard(String),
    /// Commit only the index, honoring hooks and signing configuration.
    Commit(String),
    /// Switch to an existing local branch with a clean checkout.
    Switch(String),
    /// Create a branch from a base; tracking is explicit.
    CreateBranch {
        name: String,
        base: String,
        track: bool,
    },
    /// Fetch an explicitly selected remote.
    Fetch(String),
    /// Fast-forward the current branch from its configured upstream.
    Pull,
    /// Push to the configured upstream or explicitly publish to a remote.
    Push { remote: String, publish: bool },
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

fn paths(host: &Host, cwd: &Path, args: &[&str], paths: &[String]) -> Result<(), GitError> {
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
    bytes(host, cwd, &command, Some(&input), READ_LIMIT, false).map(drop)
}

/// Execute one mutation synchronously. Call only on a background executor.
pub fn execute(host: &Host, cwd: &Path, action: &Action) -> Result<(), GitError> {
    match action {
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
            if matches!(
                file.unstaged,
                None | Some(status::Change::Untracked | status::Change::Conflicted)
            ) || file.submodule
            {
                return Err(GitError::Invalid(
                    "Only unstaged tracked files can be discarded here.".into(),
                ));
            }
            paths(
                host,
                cwd,
                &["restore", "--worktree"],
                std::slice::from_ref(path),
            )
        }
        Action::Commit(message) => {
            if message.trim().is_empty() {
                return Err(GitError::Invalid("Enter a commit message.".into()));
            }
            if status::read(host, cwd)?.has_conflicts() {
                return Err(GitError::Invalid(
                    "Resolve conflicts in the terminal before committing.".into(),
                ));
            }
            bytes(
                host,
                cwd,
                &["commit", "--file=-"],
                Some(message.as_bytes()),
                READ_LIMIT,
                false,
            )
            .map(drop)
        }
        Action::Switch(name) => branches::switch(host, cwd, name),
        Action::CreateBranch { name, base, track } => {
            branches::create(host, cwd, name, base, *track)
        }
        Action::Fetch(remote) => {
            remote_exists(host, cwd, remote)?;
            run(
                host,
                cwd,
                &["-c", "credential.interactive=false", "fetch", remote],
            )
            .map(drop)
        }
        Action::Pull => {
            branches::require_clean(host, cwd)?;
            if status::read(host, cwd)?.upstream.is_none() {
                return Err(GitError::Invalid(
                    "Publish this branch or configure an upstream before pulling.".into(),
                ));
            }
            run(
                host,
                cwd,
                &[
                    "-c",
                    "credential.interactive=false",
                    "pull",
                    "--no-rebase",
                    "--ff-only",
                ],
            )
            .map(drop)
        }
        Action::Push { remote, publish } => {
            remote_exists(host, cwd, remote)?;
            let state = status::read(host, cwd)?;
            let branch = state.branch.ok_or_else(|| {
                GitError::Invalid("Create a branch before pushing detached HEAD.".into())
            })?;
            let destination = if *publish {
                format!("refs/heads/{branch}")
            } else {
                let configured = upstream_remote(host, cwd, &branch)?;
                if configured.as_deref() != Some(remote) {
                    return Err(GitError::Invalid(
                        "Push must use this branch's configured upstream remote.".into(),
                    ));
                }
                run(
                    host,
                    cwd,
                    &["config", "--get", &format!("branch.{branch}.merge")],
                )?
            };
            let refspec = format!("HEAD:{destination}");
            let mut args = vec!["-c", "credential.interactive=false", "push"];
            if *publish {
                args.push("--set-upstream");
            }
            args.extend([remote.as_str(), refspec.as_str()]);
            run(host, cwd, &args).map(drop)
        }
    }
}

#[cfg(test)]
mod tests;
