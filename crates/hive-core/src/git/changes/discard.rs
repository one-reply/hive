//! Guard destructive working-tree operations against changes to the confirmed selection.
use super::paths;
use crate::git::{GitError, READ_LIMIT, bytes, diff, run_mutation, status, text};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

/// The destructive operation confirmed for a set of working-tree files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Discard {
    /// Restore the working copy from the index, preserving staged edits.
    Restore,
    /// Permanently delete untracked files.
    Delete,
}

impl Discard {
    /// The operation a changed file currently permits.
    pub fn for_file(file: &status::FileChange) -> Option<Self> {
        if file.submodule || file.path.ends_with('/') {
            return None;
        }
        match file.unstaged? {
            status::Change::Conflicted => None,
            status::Change::Untracked => Some(Self::Delete),
            _ => Some(Self::Restore),
        }
    }

    pub(super) fn execute(
        self,
        host: &Host,
        cwd: &Path,
        selected: &[String],
    ) -> Result<(), GitError> {
        if selected.is_empty() {
            return Ok(());
        }
        let state = status::read(host, cwd)?;
        let eligible: HashSet<_> = state
            .files
            .iter()
            .filter(|file| Self::for_file(file) == Some(self))
            .map(|file| file.path.as_str())
            .collect();
        for path in selected {
            diff::validate_path(path)?;
            if !eligible.contains(path.as_str()) {
                return Err(GitError::Invalid(format!(
                    "The status of {path} changed. Review the file list and try again."
                )));
            }
        }
        match self {
            Self::Restore => {
                // Git restore would remove an untracked file or directory blocking checkout,
                // including ignored files that do not appear in the Changes panel.
                let blockers = bytes(
                    host,
                    cwd,
                    &["ls-files", "--killed", "-z"],
                    None,
                    READ_LIMIT,
                    false,
                )?;
                for blocker in blockers
                    .split(|&byte| byte == 0)
                    .filter(|path| !path.is_empty())
                {
                    let blocker = Path::new(text(blocker)?);
                    if selected
                        .iter()
                        .map(Path::new)
                        .any(|path| blocker.starts_with(path) || path.starts_with(blocker))
                    {
                        return Err(GitError::Invalid(format!(
                            "Move {} before restoring changes.",
                            blocker.display()
                        )));
                    }
                }
                paths(host, cwd, &["restore", "--worktree"], selected)
            }
            Self::Delete => delete_untracked(host, cwd, selected),
        }
    }
}

pub(super) fn delete_untracked(
    host: &Host,
    cwd: &Path,
    selected: &[String],
) -> Result<(), GitError> {
    // clean only accepts argv. Keep each invocation small enough for SSH quoting.
    for batch in selected.chunks(8) {
        // An explicit clean pathspec also removes directories. Exclude the directory form
        // in case another process replaces a confirmed file before Git reaches this batch.
        let specs: Vec<_> = batch
            .iter()
            .flat_map(|path| {
                [
                    format!(":(literal){path}"),
                    format!(":(exclude,literal){path}/"),
                ]
            })
            .collect();
        let mut args = vec!["--no-literal-pathspecs", "clean", "-f", "-q", "--"];
        args.extend(specs.iter().map(String::as_str));
        run_mutation(host, cwd, &args)?;
    }
    Ok(())
}
