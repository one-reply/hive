//! Branch discovery and conservative checkout operations.
use super::{GitError, READ_LIMIT, bytes, conflicts, run, status};
use crate::command::Cancellation;
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
    // A branch whose name is not UTF-8 cannot be named back to Git; it is left out, not the
    // whole list.
    raw.split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| std::str::from_utf8(line).ok())
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

/// The branch work is compared with and new work starts from unless the user picks another:
/// the remote's default branch (`origin/HEAD`) when the branch it names exists, else the
/// first of `origin/main`, `origin/master`, `main` and `master` that does. Always a full ref,
/// so a tag of the same name cannot stand in. `None` when the repository has none of them.
///
/// The one rule for the review base and for a new worktree's start. No fetch is performed.
/// Only a missing `origin/HEAD` moves on to the fallbacks: a Git that could not answer is an
/// error, so nothing is silently based on the wrong commit.
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
    Ok(first_existing(
        branches,
        &[
            "refs/remotes/origin/main",
            "refs/remotes/origin/master",
            "refs/heads/main",
            "refs/heads/master",
        ],
    )
    .map(|b| b.reference.clone()))
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
mod tests {
    use super::{Branch, default_start, is_valid_name, short_name};

    #[test]
    fn branch_names_follow_git_ref_rules() {
        for name in [
            "develop",
            "feature/login",
            "fix-1.2",
            "user/v2_beta",
            "ünïcode",
        ] {
            assert!(is_valid_name(name), "{name}");
        }
        for name in [
            "", "@", "-flag", "a..b", "a b", "end.", "a//b", "/lead", "trail/", ".hidden",
            "x/.dot", "ref.lock", "a@{1}", "a~1", "a^", "a:b", "a?", "a*", "a[", "a\\b", "tab\t",
        ] {
            assert!(!is_valid_name(name), "{name:?}");
        }
    }

    fn branch(reference: &str) -> Branch {
        Branch {
            reference: reference.into(),
            name: short_name(reference).into(),
            remote: reference.starts_with("refs/remotes/"),
            worktree: None,
        }
    }

    fn start(references: &[&str], base: Option<&str>) -> Option<String> {
        let branches: Vec<_> = references.iter().map(|r| branch(r)).collect();
        default_start(&branches, base).map(|b| b.name.clone())
    }

    #[test]
    fn default_start_prefers_local_then_remote_develop() {
        let base = Some("refs/remotes/origin/main");
        let all = [
            "refs/remotes/origin/main",
            "refs/remotes/origin/develop",
            "refs/heads/develop",
        ];
        assert_eq!(start(&all, base).as_deref(), Some("develop"));
        assert_eq!(start(&all[..2], base).as_deref(), Some("origin/develop"));
    }

    #[test]
    fn default_start_finds_develop_that_git_displays_as_heads_develop() {
        // What `%(refname:short)` prints for the branch when a tag is also named `develop`.
        let mut local = branch("refs/heads/develop");
        local.name = "heads/develop".into();
        let branches = [branch("refs/remotes/origin/develop"), local];
        let start = default_start(&branches, Some("refs/remotes/origin/develop"));
        assert_eq!(
            start.map(|b| b.reference.as_str()),
            Some("refs/heads/develop")
        );
    }

    #[test]
    fn well_known_branches_are_found_beside_tags_of_the_same_name() {
        use super::{default_base, list};
        use crate::{git::testing, host::Host};

        let path = std::env::temp_dir().join(format!("hive-branches-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        testing::init(&path);
        for args in [
            &["commit", "--allow-empty", "-m", "fixture"][..],
            &["branch", "-m", "develop"],
            &["branch", "main"],
            &["tag", "develop"],
            &["tag", "main"],
        ] {
            testing::run(&path, args);
        }
        let branches = list(&Host::Local, &path).unwrap();
        let names: Vec<_> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"heads/develop"), "{names:?}");
        assert_eq!(
            default_start(&branches, None).map(|b| b.reference.as_str()),
            Some("refs/heads/develop")
        );
        assert_eq!(
            default_base(&Host::Local, &path, &branches)
                .unwrap()
                .as_deref(),
            Some("refs/heads/main")
        );
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn default_start_falls_back_to_review_base_then_head() {
        let branches = ["refs/heads/feature", "refs/remotes/origin/main"];
        assert_eq!(
            start(&branches, Some("refs/remotes/origin/main")).as_deref(),
            Some("origin/main")
        );
        assert_eq!(start(&branches, Some("refs/heads/gone")), None);
        assert_eq!(start(&branches, None), None);
        assert_eq!(start(&[], None), None);
    }

    #[test]
    fn short_name_strips_branch_namespaces_only() {
        assert_eq!(short_name("refs/remotes/origin/develop"), "origin/develop");
        assert_eq!(short_name("refs/heads/feature/x"), "feature/x");
        assert_eq!(short_name("refs/tags/v1"), "refs/tags/v1");
        assert_eq!(short_name("main"), "main");
        assert_eq!(short_name(""), "");
    }
}
