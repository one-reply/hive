//! Branch discovery and conservative checkout operations.
use super::{GitError, READ_LIMIT, bytes, conflicts, run, status, text};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A local or remote-tracking branch with its worktree occupancy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    /// Full ref name, safe to resolve independently of display names.
    pub reference: String,
    /// Short display name.
    pub name: String,
    /// Whether this is a remote-tracking ref.
    pub remote: bool,
    /// Checkout already using this local branch.
    pub worktree: Option<PathBuf>,
}

/// Blocking: lists local and remote branches, most recently committed first (name as the
/// tie-breaker), excluding remote HEAD aliases.
pub fn list(host: &Host, cwd: &Path) -> Result<Vec<Branch>, GitError> {
    // Git treats the last `--sort` as the primary key.
    let raw = bytes(
        host,
        cwd,
        &[
            "for-each-ref",
            "--sort=refname",
            "--sort=-committerdate",
            "--format=%(refname)%00%(refname:short)%00%(worktreepath)%00%(symref)",
            "refs/heads",
            "refs/remotes",
        ],
        None,
        READ_LIMIT,
        false,
    )?;
    text(&raw)?
        .lines()
        .map(|line| {
            let fields: Vec<_> = line.split('\0').collect();
            if fields.len() != 4 {
                return Err(GitError::Invalid("Malformed branch list".into()));
            }
            if !fields[3].is_empty() {
                return Ok(None);
            }
            Ok(Some(Branch {
                reference: fields[0].into(),
                name: fields[1].into(),
                remote: fields[0].starts_with("refs/remotes/"),
                worktree: (!fields[2].is_empty()).then(|| PathBuf::from(fields[2])),
            }))
        })
        .filter_map(|r| r.transpose())
        .collect()
}

/// Display name of a full branch ref (`refs/remotes/origin/main` → `origin/main`); other
/// refs and plain names are returned unchanged.
pub fn short_name(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
}

/// Resolve a user-selected ref as a commit, never as an option or path.
pub fn resolve(host: &Host, cwd: &Path, reference: &str) -> Result<String, GitError> {
    run(
        host,
        cwd,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
}

/// Preferred review base; no implicit fetch is performed.
pub fn default_base(
    host: &Host,
    cwd: &Path,
    branches: &[Branch],
) -> Result<Option<String>, GitError> {
    match run(
        host,
        cwd,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) {
        Ok(reference) if branches.iter().any(|b| b.reference == reference) => {
            return Ok(Some(reference));
        }
        Ok(_) | Err(GitError::Failed { code: Some(1), .. }) => {}
        Err(error) => return Err(error),
    }
    Ok(["origin/main", "origin/master", "main", "master"]
        .into_iter()
        .find_map(|name| {
            branches
                .iter()
                .find(|b| b.name == name)
                .map(|b| b.reference.clone())
        }))
}

/// Refuse checkout and pull while changes or an unfinished operation exist.
pub fn require_clean(host: &Host, cwd: &Path) -> Result<(), GitError> {
    if !status::read(host, cwd)?.is_clean() {
        return Err(GitError::Invalid("Commit your staged, unstaged and untracked changes before switching branches or pulling.".into()));
    }
    if conflicts::operation(host, cwd)?.is_some() || conflicts::has_sequence(host, cwd)? {
        return Err(GitError::Invalid(
            "Finish or abort the current merge, rebase or cherry-pick first.".into(),
        ));
    }
    Ok(())
}

/// Switch an existing local branch. Git enforces worktree occupancy again at execution.
pub fn switch(host: &Host, cwd: &Path, name: &str) -> Result<(), GitError> {
    validate_name(host, cwd, name)?;
    require_clean(host, cwd)?;
    super::run_mutation(host, cwd, &["switch", "--no-guess", name]).map(drop)
}

/// Create and switch to a branch, optionally tracking a selected remote ref.
pub fn create(
    host: &Host,
    cwd: &Path,
    name: &str,
    base: &str,
    track: bool,
) -> Result<(), GitError> {
    validate_name(host, cwd, name)?;
    let oid = resolve(host, cwd, base)?;
    if track
        && !list(host, cwd)?
            .iter()
            .any(|b| b.remote && b.reference == base)
    {
        return Err(GitError::Invalid(
            "Select an existing remote branch to track.".into(),
        ));
    }
    require_clean(host, cwd)?;
    if track {
        super::run_mutation(host, cwd, &["switch", "--track", "-c", name, base]).map(drop)
    } else {
        super::run_mutation(host, cwd, &["switch", "--no-track", "-c", name, &oid]).map(drop)
    }
}

/// Whether every commit of `name` is reachable from HEAD, as `git branch -d` requires.
pub fn is_merged(host: &Host, cwd: &Path, name: &str) -> Result<bool, GitError> {
    validate_name(host, cwd, name)?;
    match run(host, cwd, &["merge-base", "--is-ancestor", name, "HEAD"]) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Delete a local branch. Without `force` Git's safe delete refuses unmerged work; a branch
/// checked out in any worktree is refused up front with a clear message instead of Git's.
pub fn delete(host: &Host, cwd: &Path, name: &str, force: bool) -> Result<(), GitError> {
    validate_name(host, cwd, name)?;
    let branch = list(host, cwd)?
        .into_iter()
        .find(|b| !b.remote && b.name == name)
        .ok_or_else(|| GitError::Invalid(format!("Branch {name} no longer exists.")))?;
    if let Some(worktree) = branch.worktree {
        return Err(GitError::Invalid(format!(
            "{name} is checked out in {}. Switch that worktree away first.",
            worktree.display()
        )));
    }
    let flag = if force { "-D" } else { "-d" };
    super::run_mutation(host, cwd, &["branch", flag, "--", name]).map(drop)
}

fn validate_name(host: &Host, cwd: &Path, name: &str) -> Result<(), GitError> {
    if name.starts_with('-') || name.contains("@{") || name.is_empty() {
        return Err(GitError::Invalid("Enter a literal Git branch name.".into()));
    }
    run(host, cwd, &["check-ref-format", "--branch", name]).map(drop)
}

#[cfg(test)]
mod tests {
    use super::short_name;

    #[test]
    fn short_name_strips_branch_namespaces_only() {
        assert_eq!(short_name("refs/remotes/origin/develop"), "origin/develop");
        assert_eq!(short_name("refs/heads/feature/x"), "feature/x");
        assert_eq!(short_name("refs/tags/v1"), "refs/tags/v1");
        assert_eq!(short_name("main"), "main");
        assert_eq!(short_name(""), "");
    }
}
