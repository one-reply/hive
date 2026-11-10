//! Staged-only context with a content identity, including binary index entries.
mod staged_patch;

use super::{GitError, READ_LIMIT, bytes, status};
use crate::host::Host;
use staged_patch::StagedPatch;
use std::{borrow::Cow, path::Path};

const STAGED_DIFF: [&str; 8] = [
    "diff",
    "--cached",
    "--no-ext-diff",
    "--no-textconv",
    "--no-color",
    "--no-renames",
    // Wide enough that Git never abbreviates a path; the change graph stays short.
    "--stat=1000",
    "--stat-graph-width=10",
];

/// A consistent snapshot of the staged changes supplied to a commit generator.
pub struct Context {
    identity: Identity,
    branch: String,
    files: String,
    patch: StagedPatch,
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
        let patch = staged_patch(host, cwd)?;
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
    /// Exact textual staged diff when it fits `budget` bytes; binary payloads are omitted by
    /// Git. A larger diff becomes its diffstat, the complete file patches that fit and the names
    /// of the files left out, so no file is ever shown in part.
    pub fn patch_within(&self, budget: usize) -> Cow<'_, str> {
        self.patch.within(budget)
    }
    /// Checks HEAD, branch and all index object IDs; working-tree edits do not invalidate it.
    pub fn is_current(&self, host: &Host, cwd: &Path) -> Result<bool, GitError> {
        let status = status::read(host, cwd)?;
        Ok(!status.has_conflicts() && self.identity == identity(host, cwd, &status)?)
    }
}

fn staged_patch(host: &Host, cwd: &Path) -> Result<StagedPatch, GitError> {
    let mut args = STAGED_DIFF.to_vec();
    args.push("--patch");
    match bytes(host, cwd, &args, None, READ_LIMIT, false) {
        Ok(raw) => Ok(StagedPatch::from_stat_and_patch(utf8(raw)?)),
        // The diffstat stays small however large the patch is, so it still describes the change.
        Err(GitError::TooLarge(_)) => Ok(StagedPatch::from_stat(utf8(bytes(
            host,
            cwd,
            &STAGED_DIFF,
            None,
            READ_LIMIT,
            false,
        )?)?)),
        Err(error) => Err(error),
    }
}

fn utf8(raw: Vec<u8>) -> Result<String, GitError> {
    String::from_utf8(raw).map_err(|_| {
        GitError::Invalid("The staged patch is not UTF-8. Generate this message manually.".into())
    })
}
