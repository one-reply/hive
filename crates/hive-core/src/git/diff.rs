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
    /// The hunks' lines in patch order, numbered on the sides they belong to.
    pub fn body_lines(&self) -> Vec<BodyLine<'_>> {
        let mut lines = Vec::new();
        // The current hunk's index and the next line number on each side.
        let mut hunk: Option<(usize, usize, usize)> = None;
        for line in self.patch.lines() {
            if let Some((old_line, new_line)) = hunk_start(line) {
                let index = hunk.map_or(0, |(index, ..)| index + 1);
                hunk = Some((index, old_line, new_line));
                continue;
            }
            let Some((index, old_line, new_line)) = hunk.as_mut() else {
                continue;
            };
            // The markers are ASCII, so the text after them starts on a char boundary.
            let (original, modified, text) = match line.as_bytes().first() {
                Some(b' ') => (Some(*old_line), Some(*new_line), &line[1..]),
                Some(b'-') => (Some(*old_line), None, &line[1..]),
                Some(b'+') => (None, Some(*new_line), &line[1..]),
                // "\ No newline at end of file" belongs to neither side.
                _ => continue,
            };
            *old_line += usize::from(original.is_some());
            *new_line += usize::from(modified.is_some());
            lines.push(BodyLine {
                hunk: *index,
                original,
                modified,
                text,
            });
        }
        lines
    }
}

/// One line of a hunk, with its one-based number on each side that has it: a context line
/// on both, a removed line on the original side only, an added line on the modified side only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyLine<'a> {
    /// Index of the line's hunk in [`FileDiff::hunks`].
    pub hunk: usize,
    /// Number in the original file.
    pub original: Option<usize>,
    /// Number in the modified file.
    pub modified: Option<usize>,
    /// The line without its marker and line ending.
    pub text: &'a str,
}

/// The first original and modified lines named by a `@@ -a,b +c,d @@` hunk header.
fn hunk_start(line: &str) -> Option<(usize, usize)> {
    let mut fields = line.strip_prefix("@@ ")?.split_whitespace();
    let mut number =
        || -> Option<usize> { fields.next()?.get(1..)?.split(',').next()?.parse().ok() };
    Some((number()?, number()?))
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
    let args = match source {
        DiffSource::WorkingTree => vec!["diff".into()],
        DiffSource::Staged => vec!["diff".into(), "--cached".into()],
        DiffSource::Commit(oid) => history::commit_args(host, cwd, oid)?,
        DiffSource::Compare(base) => history::compare_args(host, cwd, base)?,
        DiffSource::Untracked => vec!["diff".into(), "--no-index".into()],
    };
    read_patch(
        host,
        cwd,
        path,
        old_path,
        args,
        *source == DiffSource::Untracked,
    )
}

pub(crate) fn read_patch(
    host: &Host,
    cwd: &Path,
    path: &str,
    old_path: Option<&str>,
    mut args: Vec<String>,
    untracked: bool,
) -> Result<FileDiff, GitError> {
    validate_path(path)?;
    if let Some(old) = old_path {
        validate_path(old)?;
    }
    args.extend(
        [
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--full-index",
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
    if untracked {
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
        untracked,
    )?;
    parse(text(&raw)?.to_string())
}

pub(crate) fn parse(patch: String) -> Result<FileDiff, GitError> {
    let mut offsets = Vec::new();
    let mut position = 0;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            let (old_line, new_line) =
                hunk_start(line).ok_or_else(|| GitError::Invalid("Malformed diff hunk".into()))?;
            offsets.push((position, old_line, new_line));
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
    super::bytes_mutation(
        host,
        cwd,
        &args,
        Some(hunk.patch.as_bytes()),
        PREVIEW_LIMIT,
        false,
        &crate::command::Cancellation::default(),
    )
    .map(drop)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reviewed_revision_changes_when_complete_object_changes_outside_visible_hunks() {
        use super::*;
        use crate::{git::reviewed::Revision, host::Host};
        let patch = format!(
            "diff --git a/a b/a\nindex {}..{} 100644\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
            "a".repeat(40),
            "b".repeat(40)
        );
        let changed = patch.replace(&"b".repeat(40), &"c".repeat(40));
        let revision = |patch| {
            Revision::from_diff(
                &Host::Local,
                Path::new("/work"),
                "a",
                &DiffSource::WorkingTree,
                &parse(patch).unwrap(),
            )
            .unwrap()
        };
        assert_ne!(revision(patch), revision(changed));
    }
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
    fn body_lines_number_each_side_and_skip_headers_and_newline_markers() {
        let diff = parse(include_str!("fixtures/diff-three-hunks.patch").into()).unwrap();
        let lines = diff.body_lines();
        let line = |text: &str| *lines.iter().find(|line| line.text == text).unwrap();
        let body = |hunk, original, modified, text| BodyLine {
            hunk,
            original,
            modified,
            text,
        };
        assert_eq!(lines.len(), 7 + 7 + 12);
        assert_eq!(line("line 1"), body(0, Some(1), Some(1), "line 1"));
        assert_eq!(line("line 3"), body(0, Some(3), None, "line 3"));
        assert_eq!(
            line("line 3 changed"),
            body(0, None, Some(3), "line 3 changed")
        );
        assert_eq!(line("line 15"), body(1, Some(15), None, "line 15"));
        assert_eq!(line("line 16"), body(1, Some(16), Some(15), "line 16"));
        assert_eq!(line("added two"), body(2, None, Some(26), "added two"));
        assert_eq!(line("tail"), body(2, Some(31), None, "tail"));
        assert_eq!(
            line("tail changed"),
            body(2, None, Some(32), "tail changed")
        );
        assert!(!lines.iter().any(|line| line.text.contains("No newline")));
    }

    #[test]
    fn new_regular_files_support_review_and_staging() {
        let patch = "diff --git a/a b/a\nnew file mode 100644\nindex 0000000..1234567\n--- /dev/null\n+++ b/a\n@@ -0,0 +1 @@\n+new\n";
        let diff = parse(patch.into()).unwrap();
        assert_eq!(diff.hunks().len(), 1);
        assert!(diff.can_stage_hunks());
    }
}
