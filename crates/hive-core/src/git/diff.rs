//! Exact Git patches for display and hunk staging. Display text is never reconstructed into patches.
use super::{GitError, bytes, history, text};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

/// Maximum textual patch retained for a single file.
pub const PREVIEW_LIMIT: usize = 1024 * 1024;

/// The two versions being reviewed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiffSource {
    /// Index versus working tree.
    WorkingTree,
    /// HEAD versus index.
    Staged,
    /// A commit versus its first parent.
    Commit(String),
    /// Common ancestor with a chosen base versus HEAD.
    Compare(String),
    /// An untracked file versus an empty file.
    Untracked,
}

/// A complete text hunk retaining its exact patch bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// First original-side line (zero for an insertion before the file).
    pub old_line: usize,
    /// First modified-side line (zero for a deletion of the file).
    pub new_line: usize,
    patch: String,
}

/// A bounded patch, its text blocks and whether partial staging is supported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    patch: String,
    hunks: Vec<Hunk>,
    partial_staging: bool,
}

impl FileDiff {
    /// Original unified patch, including all headers and newline markers.
    pub fn patch(&self) -> &str {
        &self.patch
    }
    /// Text blocks used for navigation, including read-only reviews.
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }
    /// Whether exact blocks may be applied independently of file metadata.
    pub fn can_stage_hunks(&self) -> bool {
        self.partial_staging && !self.hunks.is_empty()
    }
}

pub(super) fn validate_path(path: &str) -> Result<(), GitError> {
    if path.is_empty()
        || path.contains('\0')
        || Path::new(path)
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(GitError::Invalid(
            "Git paths must stay inside the selected worktree.".into(),
        ));
    }
    Ok(())
}

/// Blocking: read one file's patch, with literal paths and external diff tools disabled.
pub fn read(
    host: &Host,
    cwd: &Path,
    path: &str,
    source: &DiffSource,
    old_path: Option<&str>,
) -> Result<FileDiff, GitError> {
    validate_path(path)?;
    if let Some(old) = old_path {
        validate_path(old)?;
    }
    let mut args = match source {
        DiffSource::WorkingTree => vec!["diff".into()],
        DiffSource::Staged => vec!["diff".into(), "--cached".into()],
        DiffSource::Commit(oid) => history::commit_args(host, cwd, oid)?,
        DiffSource::Compare(base) => history::compare_args(host, cwd, base)?,
        DiffSource::Untracked => vec!["diff".into(), "--no-index".into()],
    };
    args.extend(
        [
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            if old_path.is_some() {
                "--find-renames"
            } else {
                "--no-renames"
            },
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--unified=3",
            "--",
        ]
        .map(String::from),
    );
    if *source == DiffSource::Untracked {
        args.push("/dev/null".into());
    }
    if let Some(old) = old_path {
        args.push(old.into());
    }
    args.push(path.into());
    args.insert(0, "--literal-pathspecs".into());
    let raw = bytes(
        host,
        cwd,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
        PREVIEW_LIMIT,
        *source == DiffSource::Untracked,
    )?;
    parse(text(&raw)?.to_string())
}

fn parse(patch: String) -> Result<FileDiff, GitError> {
    let mut offsets = Vec::new();
    let mut position = 0;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            let mut fields = line.split_whitespace();
            fields.next();
            let number = |field: Option<&str>| {
                field
                    .and_then(|f| f.get(1..))
                    .and_then(|f| f.split(',').next())
                    .and_then(|f| f.parse().ok())
                    .ok_or_else(|| GitError::Invalid("Malformed diff hunk".into()))
            };
            offsets.push((position, number(fields.next())?, number(fields.next())?));
        }
        position += line.len();
    }
    let first = offsets.first().map(|v| v.0).unwrap_or(patch.len());
    let header = &patch[..first];
    let regular = !header.lines().any(|line| {
        [
            "old mode",
            "new mode",
            "rename from",
            "copy from",
            "Binary files",
            "GIT binary patch",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
            || (line.starts_with("index ")
                || line.starts_with("new file mode ")
                || line.starts_with("deleted file mode "))
                && (line.ends_with(" 120000") || line.ends_with(" 160000"))
    });
    let hunks = offsets
        .iter()
        .enumerate()
        .map(|(index, &(start, old_line, new_line))| {
            let end = offsets.get(index + 1).map(|v| v.0).unwrap_or(patch.len());
            Hunk {
                old_line,
                new_line,
                patch: format!("{header}{}", &patch[start..end]),
            }
        })
        .collect();
    Ok(FileDiff {
        patch,
        hunks,
        partial_staging: regular,
    })
}

/// Stage or unstage one exact hunk only if the reviewed patch is still current.
pub fn apply_hunk(
    host: &Host,
    cwd: &Path,
    path: &str,
    source: &DiffSource,
    reviewed: &FileDiff,
    index: usize,
) -> Result<(), GitError> {
    if !matches!(source, DiffSource::WorkingTree | DiffSource::Staged) {
        return Err(GitError::Invalid("This comparison is read-only.".into()));
    }
    if read(host, cwd, path, source, None)? != *reviewed {
        return Err(GitError::Invalid(
            "This file changed since review. Refresh and review the updated hunk.".into(),
        ));
    }
    if !reviewed.can_stage_hunks() {
        return Err(GitError::Invalid(
            "Stage metadata and non-regular files as a whole file.".into(),
        ));
    }
    let hunk = reviewed
        .hunks
        .get(index)
        .ok_or_else(|| GitError::Invalid("This hunk is no longer available.".into()))?;
    let mut args = vec!["apply", "--cached", "--whitespace=nowarn"];
    if *source == DiffSource::Staged {
        args.push("--reverse");
    }
    args.push("-");
    bytes(
        host,
        cwd,
        &args,
        Some(hunk.patch.as_bytes()),
        PREVIEW_LIMIT,
        false,
    )
    .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_and_hashes_do_not_disable_partial_staging() {
        let patch = "diff --git a/160000 b/160000\nindex 120000a..1234567 100644\n--- a/160000\n+++ b/160000\n@@ -1 +1 @@\n-old\n+new\n";
        assert!(parse(patch.into()).unwrap().can_stage_hunks());
        assert!(
            !parse(patch.replace("100644", "120000"))
                .unwrap()
                .can_stage_hunks()
        );
    }

    #[test]
    fn new_regular_files_support_review_and_staging() {
        let patch = "diff --git a/a b/a\nnew file mode 100644\nindex 0000000..1234567\n--- /dev/null\n+++ b/a\n@@ -0,0 +1 @@\n+new\n";
        let diff = parse(patch.into()).unwrap();
        assert_eq!(diff.hunks().len(), 1);
        assert!(diff.can_stage_hunks());
    }
}
