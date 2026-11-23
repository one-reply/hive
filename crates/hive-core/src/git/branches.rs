//! Branch discovery and conservative checkout operations.
use super::{GitError, READ_LIMIT, bytes, conflicts, run, status};
use crate::command::Cancellation;
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
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

impl Branch {
    /// The name Git's branch commands take for a local branch. The display name differs next
    /// to a tag of the same name, where Git shortens `refs/heads/develop` to `heads/develop`.
    pub fn local_name(&self) -> Option<&str> {
        if self.remote {
            return None;
        }
        self.reference.strip_prefix("refs/heads/")
    }
}

/// One record per ref: four fields, each ended by a NUL, then the line feed Git adds. The path
/// of a checkout may hold a line feed itself, so only the NUL ends a field.
const LIST_FORMAT: &str = "--format=%(refname)%00%(refname:short)%00%(worktreepath)%00%(symref)%00";

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
            LIST_FORMAT,
            "refs/heads",
            "refs/remotes",
        ],
        None,
        READ_LIMIT,
        false,
    )?;
    parse_list(&raw)
}

/// Reads `for-each-ref` output in [`LIST_FORMAT`]. A ref name is text; where a branch is
/// checked out is whatever bytes the file system holds, and costs the branch nothing.
fn parse_list(raw: &[u8]) -> Result<Vec<Branch>, GitError> {
    let mut fields: Vec<&[u8]> = raw.split(|byte| *byte == 0).collect();
    // Every field ends with a NUL, so what follows the last one is only Git's line feed.
    let tail = fields.pop().unwrap_or_default();
    let (records, incomplete) = fields.as_chunks::<4>();
    if !matches!(tail, b"" | b"\n") || !incomplete.is_empty() {
        return Err(GitError::Invalid("Malformed branch list".into()));
    }
    let mut branches = Vec::with_capacity(records.len());
    for &[reference, name, worktree, symref] in records {
        // The line feed that ended the record before this one.
        let reference = reference.strip_prefix(b"\n").unwrap_or(reference);
        // A branch whose name is not UTF-8 cannot be named back to Git; it is left out, not
        // the whole list.
        let (Ok(reference), Ok(name)) = (std::str::from_utf8(reference), std::str::from_utf8(name))
        else {
            continue;
        };
        if !symref.is_empty() {
            continue;
        }
        branches.push(Branch {
            reference: reference.into(),
            name: name.into(),
            remote: reference.starts_with("refs/remotes/"),
            worktree: (!worktree.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(worktree))),
        });
    }
    Ok(branches)
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

/// Where the default base is looked for when `origin/HEAD` does not name it, in this order.
const BASE_FALLBACKS: [&str; 4] = [
    "refs/remotes/origin/main",
    "refs/remotes/origin/master",
    "refs/heads/main",
    "refs/heads/master",
];

/// The branch work is compared with and new work starts from unless the user picks another:
/// the remote's default branch (`origin/HEAD`), else the first of `origin/main`,
/// `origin/master`, `main` and `master`. Always a full ref, so a tag of the same name cannot
/// stand in. `None` when the repository has none of them.
///
/// The one rule for the review base and for a new worktree's start. No fetch is performed.
/// A candidate counts when it is a branch that names a commit: one that is missing, or that
/// was pointed at another kind of object, is passed over for the next. A Git that could not
/// answer is an error, so nothing is silently based on the wrong commit.
pub fn default_base(host: &Host, cwd: &Path) -> Result<Option<String>, GitError> {
    let mut candidates = Vec::with_capacity(BASE_FALLBACKS.len() + 1);
    match run(
        host,
        cwd,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) {
        Ok(reference) if is_branch(&reference) => candidates.push(reference),
        // No link, or a link to something that is not a branch.
        Ok(_) | Err(GitError::Failed { code: Some(1), .. }) => {}
        Err(error) => return Err(error),
    }
    candidates.extend(BASE_FALLBACKS.map(String::from));
    let usable = commit_branches(host, cwd, &candidates)?;
    Ok(candidates
        .into_iter()
        .find(|candidate| usable.contains(candidate)))
}

/// Whether a full ref is a local or remote-tracking branch, the two kinds [`list`] reads.
fn is_branch(reference: &str) -> bool {
    reference.starts_with("refs/heads/") || reference.starts_with("refs/remotes/")
}

/// Those of `references` that exist, name a commit and are not aliases of another ref: what
/// [`list`] would show of them. Asked of Git for these refs alone, so that nothing else in
/// the repository (a branch nobody can name, where a worktree lives) decides the answer.
fn commit_branches(
    host: &Host,
    cwd: &Path,
    references: &[String],
) -> Result<Vec<String>, GitError> {
    let mut args = vec![
        "for-each-ref",
        "--format=%(refname)%00%(objecttype)%00%(symref)",
    ];
    args.extend(references.iter().map(String::as_str));
    let listed = run(host, cwd, &args)?;
    // A pattern also matches the refs below it (`refs/heads/main/x`): only a commit under
    // the exact name is kept, and the caller looks its candidates up by that name.
    Ok(listed
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            match (fields.next()?, fields.next()?, fields.next()?) {
                (reference, "commit", "") => Some(reference.to_owned()),
                _ => None,
            }
        })
        .collect())
}

/// Where a new branch starts unless the user picks otherwise: the git-flow integration branch
/// (`develop`, else `origin/develop`), else the review base, else `None` for HEAD.
pub fn default_start<'a>(branches: &'a [Branch], base: Option<&str>) -> Option<&'a Branch> {
    first_existing(
        branches,
        &["refs/heads/develop", "refs/remotes/origin/develop"],
    )
    .or_else(|| branches.iter().find(|b| Some(b.reference.as_str()) == base))
}

/// The first of `references` that is a known branch. A well-known branch is found by its full
/// ref, never by display name: next to a tag of the same name Git shortens `refs/heads/develop`
/// to `heads/develop`.
fn first_existing<'a>(branches: &'a [Branch], references: &[&str]) -> Option<&'a Branch> {
    references
        .iter()
        .find_map(|reference| branches.iter().find(|b| b.reference == *reference))
}

/// Refuse checkout, pull and rebase while changes or an unfinished operation exist.
pub fn require_clean(host: &Host, cwd: &Path) -> Result<(), GitError> {
    if !status::read(host, cwd)?.is_clean() {
        return Err(GitError::Invalid("Commit your staged, unstaged and untracked changes before switching branches, pulling or rebasing.".into()));
    }
    if conflicts::operation(host, cwd)?.is_some() || conflicts::has_sequence(host, cwd)? {
        return Err(GitError::Invalid(
            "Finish or abort the current merge, rebase or cherry-pick first.".into(),
        ));
    }
    Ok(())
}

/// Switch an existing local branch. Git enforces worktree occupancy again at execution.
/// `stop` ends the checkout, which runs `post-checkout` and content filters.
pub fn switch(host: &Host, cwd: &Path, name: &str, stop: &Cancellation) -> Result<(), GitError> {
    validate_name(host, cwd, name)?;
    require_clean(host, cwd)?;
    super::run_stoppable(host, cwd, &["switch", "--no-guess", name], stop).map(drop)
}

/// Create and switch to a branch, optionally tracking a selected remote ref. `stop` ends the
/// checkout, as for [`switch`].
pub fn create(
    host: &Host,
    cwd: &Path,
    name: &str,
    base: &str,
    track: bool,
    stop: &Cancellation,
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
    let args = if track {
        ["switch", "--track", "-c", name, base]
    } else {
        ["switch", "--no-track", "-c", name, &oid]
    };
    super::run_stoppable(host, cwd, &args, stop).map(drop)
}

/// Whether every commit of the local branch `name` is reachable from HEAD, as `git branch -d`
/// requires.
pub fn is_merged(host: &Host, cwd: &Path, name: &str) -> Result<bool, GitError> {
    validate_name(host, cwd, name)?;
    // A bare name would resolve to a tag of the same name before the branch.
    let reference = format!("refs/heads/{name}");
    match run(
        host,
        cwd,
        &["merge-base", "--is-ancestor", &reference, "HEAD"],
    ) {
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
        .find(|b| b.local_name() == Some(name))
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

/// Whether `name` is a branch Git accepts, decided without running Git so a form can check
/// it on every keystroke. Follows `git check-ref-format --branch` for literal names.
pub fn is_valid_name(name: &str) -> bool {
    const FORBIDDEN: &[char] = &[' ', '~', '^', ':', '?', '*', '[', '\\'];
    !name.is_empty()
        && name != "@"
        && !name.starts_with('-')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name
            .chars()
            .any(|c| c.is_ascii_control() || FORBIDDEN.contains(&c))
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

fn validate_name(host: &Host, cwd: &Path, name: &str) -> Result<(), GitError> {
    if name.starts_with('-') || name.contains("@{") || name.is_empty() {
        return Err(GitError::Invalid("Enter a literal Git branch name.".into()));
    }
    run(host, cwd, &["check-ref-format", "--branch", name]).map(drop)
}

#[cfg(test)]
mod tests;
