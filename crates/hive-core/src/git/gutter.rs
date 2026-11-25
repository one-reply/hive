//! Changed buffer lines compared with an immutable Git revision.
use super::{GitError, blame::is_oid, bytes};
use crate::{
    files::{MAX_FILE_BYTES, Snapshot},
    host::Host,
};
use similar::{DiffTag, TextDiff};
use std::{ops::Range, path::Path, time::Duration};

/// How a buffer range differs from the committed file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// New lines without corresponding committed lines.
    Added,
    /// Old lines removed at this boundary.
    Deleted,
    /// An existing block was edited.
    Modified,
}

/// A gutter mark over UTF-8 bytes in the buffer. Deletions have an empty range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Bytes in the current buffer.
    pub range: Range<usize>,
    /// Visual meaning of the mark.
    pub kind: Kind,
}

/// Reads the blob at a specific commit. An absent file has an empty baseline.
pub fn baseline(host: &Host, root: &Path, oid: &str, path: &Path) -> Result<String, GitError> {
    if !is_oid(oid) {
        return Err(GitError::Invalid("Invalid gutter revision".into()));
    }
    let name = path
        .to_str()
        .ok_or_else(|| GitError::Invalid("The Git path is not UTF-8".into()))?;
    super::diff::validate_path(name)?;
    let entry = bytes(
        host,
        root,
        &["--literal-pathspecs", "ls-tree", "-z", oid, "--", name],
        None,
        MAX_FILE_BYTES,
        false,
    )?;
    if entry.is_empty() {
        return Ok(String::new());
    }
    let metadata = entry.split(|b| *b == b'\t').next().unwrap_or_default();
    let fields: Vec<_> = std::str::from_utf8(metadata)
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let [mode, "blob", blob] = fields.as_slice() else {
        return Err(GitError::Invalid(
            "No regular Git blob for this path".into(),
        ));
    };
    if !matches!(*mode, "100644" | "100755") || !is_oid(blob) {
        return Err(GitError::Invalid(
            "No regular Git blob for this path".into(),
        ));
    }
    let raw = bytes(
        host,
        root,
        &["cat-file", "blob", blob],
        None,
        MAX_FILE_BYTES,
        false,
    )?;
    Snapshot::parse(path, &raw)
        .map(|s| s.text())
        .map_err(|e| GitError::Invalid(e.to_string()))
}

/// Computes changed blocks including unsaved text; a bounded diff can group nearby edits.
pub fn marks(before: &str, after: &str) -> Vec<Mark> {
    let starts: Vec<_> = std::iter::once(0)
        .chain(after.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    TextDiff::configure()
        .timeout(Duration::from_millis(100))
        .diff_lines(before, after)
        .ops()
        .iter()
        .filter_map(|op| {
            let kind = match op.tag() {
                DiffTag::Equal => return None,
                DiffTag::Insert => Kind::Added,
                DiffTag::Delete => Kind::Deleted,
                DiffTag::Replace => Kind::Modified,
            };
            let r = op.new_range();
            let byte = |i| *starts.get(i).unwrap_or(&after.len());
            Some(Mark {
                range: byte(r.start)..byte(r.end),
                kind,
            })
        })
        .collect()
}

/// Chooses the next or previous hunk start, wrapping at the edge.
pub fn destination(marks: &[Range<usize>], cursor: usize, forward: bool) -> Option<usize> {
    if forward {
        marks.iter().find(|m| m.start > cursor).or(marks.first())
    } else {
        marks
            .iter()
            .rev()
            .find(|m| m.start < cursor)
            .or(marks.last())
    }
    .map(|m| m.start)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn baseline_reads_literal_paths_from_the_requested_commit() {
        use crate::git::testing;
        use std::fs;
        let root = std::env::temp_dir().join(format!("hive-gutter-{}", std::process::id()));
        testing::init(&root);
        let name = "[header].tsx";
        fs::write(root.join(name), "\u{feff}alpha\r\n").unwrap();
        testing::run(&root, &["--literal-pathspecs", "add", "--", name]);
        testing::run(&root, &["commit", "-qm", "initial"]);
        let oid = testing::run(&root, &["rev-parse", "HEAD"]);
        fs::rename(root.join(name), root.join("new.tsx")).unwrap();
        assert_eq!(
            baseline(&Host::Local, &root, &oid, Path::new(name)).unwrap(),
            "alpha\n"
        );
        assert_eq!(
            baseline(&Host::Local, &root, &oid, Path::new("new.tsx")).unwrap(),
            ""
        );
        assert!(baseline(&Host::Local, &root, "--help", Path::new(name)).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unicode_change_and_eof_deletion_wrap() {
        let m = marks("α\na\nb\nc\n", "α\nA\nb\n");
        assert_eq!(
            m,
            [
                Mark {
                    range: 3..5,
                    kind: Kind::Modified
                },
                Mark {
                    range: 7..7,
                    kind: Kind::Deleted
                }
            ]
        );
        let ranges: Vec<_> = m.into_iter().map(|m| m.range).collect();
        assert_eq!(destination(&ranges, 7, true), Some(3));
        assert_eq!(destination(&ranges, 3, false), Some(7));
    }
}
