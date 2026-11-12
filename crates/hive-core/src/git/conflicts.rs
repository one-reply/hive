//! Unmerged paths and the operation that left them: whole-file resolution, abort and continue.
use super::{GitError, READ_TIMEOUT, changes, run, run_mutation, run_stoppable, status};
use crate::command::{self, Cancellation};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One line per probed path; the bound only guards against a misbehaving remote shell.
const INSPECT_OUTPUT_LIMIT: usize = 64 * 1024;
/// Prints `y` or `n` for each argument, so one remote round trip probes every marker.
const EXISTS_SCRIPT: &str = r#"for path do if [ -e "$path" ]; then echo y; else echo n; fi; done"#;

/// A side of an unmerged path, in Git's own terms.
///
/// During a rebase or cherry-pick `Ours` is the branch being built on and `Theirs` the commit
/// being replayed, which is why the UI calls them current and incoming.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Index stage 2: the checked-out side.
    Ours,
    /// Index stage 3: the side being merged or replayed.
    Theirs,
}

impl Side {
    fn checkout_flag(self) -> &'static str {
        match self {
            Self::Ours => "--ours",
            Self::Theirs => "--theirs",
        }
    }
}

/// How the two sides of an unmerged path disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Conflict {
    /// Both sides changed the file.
    BothModified,
    /// Both sides added a file at this path.
    BothAdded,
    /// Both sides deleted the file; typically one half of a rename conflict.
    BothDeleted,
    /// Only our side has the file.
    AddedByUs,
    /// Only their side has the file.
    AddedByThem,
    /// We deleted the file they changed.
    DeletedByUs,
    /// They deleted the file we changed.
    DeletedByThem,
}

impl Conflict {
    /// The conflict named by the XY code of a porcelain `u` record.
    pub(super) fn from_code(code: [u8; 2]) -> Option<Self> {
        Some(match &code {
            b"UU" => Self::BothModified,
            b"AA" => Self::BothAdded,
            b"DD" => Self::BothDeleted,
            b"AU" => Self::AddedByUs,
            b"UA" => Self::AddedByThem,
            b"DU" => Self::DeletedByUs,
            b"UD" => Self::DeletedByThem,
            _ => return None,
        })
    }

    /// Whether `side` has a version of the file; taking a side without one deletes the file.
    pub fn has(self, side: Side) -> bool {
        match self {
            Self::BothModified | Self::BothAdded => true,
            Self::BothDeleted => false,
            Self::AddedByUs | Self::DeletedByThem => side == Side::Ours,
            Self::AddedByThem | Self::DeletedByUs => side == Side::Theirs,
        }
    }
}

/// An unfinished Git operation that stops for conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl Operation {
    /// The Git subcommand that accepts `--abort` and `--continue` for this operation.
    fn command(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }
}

/// Blocking: the operation the worktree is in the middle of, if any.
///
/// A rebase wins over the cherry-pick and merge markers it leaves while replaying a commit,
/// as in Git's own status.
pub fn operation(host: &Host, cwd: &Path) -> Result<Option<Operation>, GitError> {
    if git_paths_exist(host, cwd, &["rebase-merge", "rebase-apply"])?.contains(&true) {
        return Ok(Some(Operation::Rebase));
    }
    let heads = [
        ("MERGE_HEAD", Operation::Merge),
        ("CHERRY_PICK_HEAD", Operation::CherryPick),
        ("REVERT_HEAD", Operation::Revert),
    ];
    for (head, operation) in heads {
        if head_exists(host, cwd, head)? {
            return Ok(Some(operation));
        }
    }
    Ok(None)
}

/// Blocking: whether a multi-commit cherry-pick or revert is paused between commits, when
/// none of the markers [`operation`] reads exist.
pub(super) fn has_sequence(host: &Host, cwd: &Path) -> Result<bool, GitError> {
    Ok(git_paths_exist(host, cwd, &["sequencer"])?.contains(&true))
}

fn head_exists(host: &Host, cwd: &Path, head: &str) -> Result<bool, GitError> {
    // These refs also resolve in linked worktrees, whose .git is a file.
    match run(host, cwd, &["rev-parse", "--verify", "--quiet", head]) {
        Ok(_) => Ok(true),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Whether each entry of the worktree's Git directory exists, in the order given.
fn git_paths_exist(host: &Host, cwd: &Path, entries: &[&str]) -> Result<Vec<bool>, GitError> {
    let mut args = vec!["rev-parse"];
    for entry in entries {
        args.extend(["--git-path", entry]);
    }
    let resolved = run(host, cwd, &args)?;
    let paths: Vec<&str> = resolved.lines().collect();
    if paths.len() != entries.len() {
        return Err(GitError::Invalid(
            "Unexpected `git rev-parse --git-path` output; refresh to retry.".into(),
        ));
    }
    match host {
        Host::Local => paths
            .iter()
            .map(|path| {
                let path = Path::new(path);
                let path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    cwd.join(path)
                };
                path.try_exists().map_err(|source| GitError::Spawn {
                    command: format!("inspect {}", path.display()),
                    host: host.clone(),
                    source,
                })
            })
            .collect(),
        Host::Ssh { .. } => remote_paths_exist(host, cwd, &paths),
    }
}

fn remote_paths_exist(host: &Host, cwd: &Path, paths: &[&str]) -> Result<Vec<bool>, GitError> {
    let command = "inspect unfinished operation".to_owned();
    let mut args = vec!["-c", EXISTS_SCRIPT, "sh"];
    args.extend_from_slice(paths);
    let output = command::run(
        host.command("sh", &args, Some(cwd)),
        &[],
        &Cancellation::default(),
        READ_TIMEOUT,
        INSPECT_OUTPUT_LIMIT,
    )
    .map_err(|source| GitError::Interrupted {
        command: command.clone(),
        host: host.clone(),
        cwd: cwd.into(),
        source,
        mutation: false,
        guidance: "Refresh to retry.",
    })?;
    let failed = |stderr: String| GitError::Failed {
        command: command.clone(),
        host: host.clone(),
        cwd: cwd.into(),
        code: output.code,
        stderr,
    };
    if output.code != Some(0) {
        return Err(failed(String::from_utf8_lossy(&output.stderr).into_owned()));
    }
    parse_existence(&output.stdout, paths.len())
        .ok_or_else(|| failed("unexpected output while inspecting the Git directory".into()))
}

/// Reads the `y`/`n` lines of [`EXISTS_SCRIPT`]; `None` unless there is exactly one per path.
fn parse_existence(stdout: &[u8], expected: usize) -> Option<Vec<bool>> {
    let answers = std::str::from_utf8(stdout)
        .ok()?
        .lines()
        .map(|line| match line {
            "y" => Some(true),
            "n" => Some(false),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    (answers.len() == expected).then_some(answers)
}

/// Resolve one unmerged path by taking a whole side, discarding the working file's content.
pub(super) fn resolve_with(
    host: &Host,
    cwd: &Path,
    path: &str,
    side: Side,
) -> Result<(), GitError> {
    let conflict = status::read(host, cwd)?
        .files
        .iter()
        .find(|file| file.path == path)
        .and_then(|file| file.conflict)
        .ok_or_else(|| GitError::Invalid("This file is no longer in conflict.".into()))?;
    let selected = [path.to_owned()];
    if !conflict.has(side) {
        return changes::paths(host, cwd, &["rm", "--quiet"], &selected);
    }
    changes::paths(host, cwd, &["checkout", side.checkout_flag()], &selected)?;
    changes::paths(host, cwd, &["add"], &selected)
}

/// Abandon the unfinished operation and restore the state it started from.
pub(super) fn abort(host: &Host, cwd: &Path) -> Result<(), GitError> {
    let operation = require_operation(host, cwd)?;
    run_mutation(host, cwd, &[operation.command(), "--abort"]).map(drop)
}

/// Conclude the unfinished operation, or advance it to its next stop.
pub(super) fn continue_operation(
    host: &Host,
    cwd: &Path,
    stop: &Cancellation,
) -> Result<(), GitError> {
    let operation = require_operation(host, cwd)?;
    if status::read(host, cwd)?.has_conflicts() {
        return Err(GitError::Invalid(
            "Resolve every conflict before continuing.".into(),
        ));
    }
    match run_stoppable(host, cwd, &[operation.command(), "--continue"], stop) {
        Ok(_) => Ok(()),
        // Git exits non-zero when the next replayed commit conflicts; that is the operation
        // moving on, and the new conflicts are what the user needs to see.
        Err(error @ GitError::Failed { .. }) => {
            if status::read(host, cwd)?.has_conflicts() {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

fn require_operation(host: &Host, cwd: &Path) -> Result<Operation, GitError> {
    operation(host, cwd)?.ok_or_else(|| {
        GitError::Invalid("No merge, rebase, cherry-pick or revert is in progress.".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taking_a_side_without_the_file_means_deleting_it() {
        use Conflict::*;
        for conflict in [BothModified, BothAdded] {
            assert!(conflict.has(Side::Ours) && conflict.has(Side::Theirs));
        }
        assert!(!BothDeleted.has(Side::Ours) && !BothDeleted.has(Side::Theirs));
        for conflict in [AddedByUs, DeletedByThem] {
            assert!(conflict.has(Side::Ours) && !conflict.has(Side::Theirs));
        }
        for conflict in [AddedByThem, DeletedByUs] {
            assert!(!conflict.has(Side::Ours) && conflict.has(Side::Theirs));
        }
    }

    #[test]
    fn remote_probe_needs_one_answer_per_path() {
        assert_eq!(parse_existence(b"y\nn\n", 2), Some(vec![true, false]));
        assert_eq!(parse_existence(b"", 0), Some(vec![]));
        // A login banner, a missing line or binary noise must not read as "absent".
        assert_eq!(parse_existence(b"y\n", 2), None);
        assert_eq!(parse_existence(b"Welcome\ny\nn\n", 2), None);
        assert_eq!(parse_existence(b"\xff\n", 1), None);
    }

    #[test]
    fn exists_script_reports_each_argument() {
        let directory = std::env::temp_dir();
        let output = std::process::Command::new("sh")
            .args(["-c", EXISTS_SCRIPT, "sh"])
            .arg(&directory)
            .arg(directory.join("hive-missing-marker"))
            .output()
            .expect("sh");
        assert_eq!(parse_existence(&output.stdout, 2), Some(vec![true, false]));
    }
}
