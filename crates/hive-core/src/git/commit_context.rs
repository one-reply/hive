//! Staged-only context with a content identity, including binary index entries.
use super::{GitError, READ_LIMIT, bytes, status};
use crate::{commit_message::PROMPT_LIMIT, host::Host};
use std::path::Path;

/// A consistent snapshot of the staged changes supplied to a commit generator.
pub struct Context {
    identity: Identity,
    branch: String,
    files: String,
    patch: String,
}

#[derive(PartialEq, Eq)]
struct Identity {
    head: Option<String>,
    branch: Option<String>,
    index: Vec<u8>,
    changes: Vec<(String, Option<String>, status::Change)>,
}

fn identity(host: &Host, cwd: &Path, status: &status::Status) -> Result<Identity, GitError> {
    Ok(Identity {
        head: status.oid.clone(),
        branch: status.branch.clone(),
        index: bytes(
            host,
            cwd,
            &["ls-files", "--stage", "-z"],
            None,
            READ_LIMIT,
            false,
        )?,
        changes: status
            .files
            .iter()
            .filter_map(|f| {
                f.staged
                    .map(|change| (f.path.clone(), f.old_path.clone(), change))
            })
            .collect(),
    })
}

impl Context {
    /// Reads only staged content; rejects conflicts, empty index diffs and concurrent changes.
    pub fn read(host: &Host, cwd: &Path) -> Result<Self, GitError> {
        let status = status::read(host, cwd)?;
        if status.has_conflicts() {
            return Err(GitError::Invalid(
                "Resolve conflicts before generating a commit message.".into(),
            ));
        }
        let files: Vec<_> = status.files.iter().filter(|f| f.staged.is_some()).collect();
        if files.is_empty() {
            return Err(GitError::Invalid(
                "Stage changes before generating a commit message.".into(),
            ));
        }
        let identity = identity(host, cwd, &status)?;
        let files = files
            .iter()
            .map(|f| {
                let previous = f
                    .old_path
                    .as_ref()
                    .map(|p| format!("{p:?} → "))
                    .unwrap_or_default();
                format!(
                    "{:?} {previous}{:?}",
                    f.staged.expect("filtered staged entry"),
                    f.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let raw = bytes(
            host,
            cwd,
            &[
                "diff",
                "--cached",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--patch",
                "--no-renames",
            ],
            None,
            PROMPT_LIMIT,
            false,
        )?;
        let patch = String::from_utf8(raw).map_err(|_| {
            GitError::Invalid(
                "The staged patch is not UTF-8. Generate this message manually.".into(),
            )
        })?;
        let context = Self {
            branch: status.branch.unwrap_or_else(|| "detached HEAD".into()),
            identity,
            files,
            patch,
        };
        if !context.is_current(host, cwd)? {
            return Err(GitError::Invalid(
                "Staged changes changed while reading. Try again.".into(),
            ));
        }
        Ok(context)
    }
    /// Current branch or an explicit detached HEAD label.
    pub fn branch(&self) -> &str {
        &self.branch
    }
    /// Staged filenames, quoted to preserve unusual characters, with change types.
    pub fn files(&self) -> &str {
        &self.files
    }
    /// Exact textual staged diff; binary payloads are omitted by Git.
    pub fn patch(&self) -> &str {
        &self.patch
    }
    /// Checks HEAD, branch and all index object IDs; working-tree edits do not invalidate it.
    pub fn is_current(&self, host: &Host, cwd: &Path) -> Result<bool, GitError> {
        let status = status::read(host, cwd)?;
        Ok(!status.has_conflicts() && self.identity == identity(host, cwd, &status)?)
    }
}
