//! Machine-readable worktree status, including independent index and working tree changes.
use super::{GitError, READ_LIMIT, bytes, conflicts::Conflict, numstat};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A path's state on one side of the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

/// Added and removed text lines on one side of a file change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineStats {
    /// Number of added lines.
    pub added: usize,
    /// Number of removed lines.
    pub removed: usize,
}

/// One literal repository-relative path returned by Git.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// Current repository-relative path, never a display-escaped path.
    pub path: String,
    /// Previous path for renames and copies.
    pub old_path: Option<String>,
    /// Change already in the index.
    pub staged: Option<Change>,
    /// Change outside the index.
    pub unstaged: Option<Change>,
    /// Submodule changes cannot be staged by text hunk.
    pub submodule: bool,
    /// How the two sides disagree, for an unmerged path.
    #[serde(default)]
    pub conflict: Option<Conflict>,
    /// HEAD-to-index or immutable review line counts; None when binary or unavailable.
    #[serde(default)]
    pub staged_stats: Option<LineStats>,
    /// Index-to-worktree line counts (all lines for an untracked file).
    #[serde(default)]
    pub unstaged_stats: Option<LineStats>,
}

/// Snapshot of HEAD, upstream and the working tree.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// None for an unborn repository.
    pub oid: Option<String>,
    /// None for detached HEAD.
    pub branch: Option<String>,
    /// Configured upstream, independent of the review base.
    pub upstream: Option<String>,
    /// Commits present only locally, when known.
    pub ahead: usize,
    /// Commits present only in the locally known upstream, when known.
    pub behind: usize,
    /// The configured upstream has no remote-tracking ref: the branch was deleted on the
    /// remote and pruned here, or never existed there, as after cloning an empty repository.
    /// Only ever set together with `upstream`; `ahead` and `behind` are then unknown.
    #[serde(default)]
    pub upstream_gone: bool,
    /// All changed and untracked files whose names are UTF-8.
    pub files: Vec<FileChange>,
    /// Changed files whose names are not UTF-8, as a Linux host may hold. They are left out
    /// of `files`: a lossy name would point every operation at another file.
    #[serde(default)]
    pub unreadable_paths: usize,
}

impl Status {
    /// Whether there are no staged, unstaged or untracked changes, readable or not.
    pub fn is_clean(&self) -> bool {
        self.files.is_empty() && self.unreadable_paths == 0
    }
    /// The upstream the branch can be compared and synced with: the configured one, unless
    /// its remote-tracking ref is gone. A branch without it has to be published first.
    pub fn tracked_upstream(&self) -> Option<&str> {
        self.upstream.as_deref().filter(|_| !self.upstream_gone)
    }
    /// Whether the index has unresolved conflicts.
    pub fn has_conflicts(&self) -> bool {
        self.files
            .iter()
            .any(|f| f.staged == Some(Change::Conflicted))
    }
}

/// Blocking, non-locking status read. Unknown porcelain headers are ignored.
pub fn read(host: &Host, cwd: &Path) -> Result<Status, GitError> {
    parse(&bytes(
        host,
        cwd,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
        None,
        READ_LIMIT,
        false,
    )?)
}

/// Blocking: read status with line counts for display, leaving mutation guards inexpensive.
/// Unavailable counts are logged and omitted without hiding the file list.
pub fn read_with_stats(host: &Host, cwd: &Path) -> Result<Status, GitError> {
    let mut status = read(host, cwd)?;
    numstat::fill_status(host, cwd, &mut status);
    Ok(status)
}

fn change(code: u8) -> Result<Option<Change>, GitError> {
    Ok(match code {
        b'.' => None,
        b'M' => Some(Change::Modified),
        b'A' => Some(Change::Added),
        b'D' => Some(Change::Deleted),
        b'R' => Some(Change::Renamed),
        b'C' => Some(Change::Copied),
        b'T' => Some(Change::TypeChanged),
        b'U' => Some(Change::Conflicted),
        _ => return Err(unknown()),
    })
}

fn unknown() -> GitError {
    GitError::Invalid("Unknown Git status; refresh or inspect it in the terminal.".into())
}

fn parse(raw: &[u8]) -> Result<Status, GitError> {
    let mut result = Status::default();
    let mut compared = false;
    let mut records = raw.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix(b"# ") {
            compared |= read_header(&mut result, &String::from_utf8_lossy(header))?;
            continue;
        }
        let kind = record[0];
        if kind == b'!' {
            continue;
        }
        // Only a path can hold bytes that are not UTF-8: that one file is counted and left
        // out, so it hides neither the rest of the list nor the fact that the tree is dirty.
        let line = std::str::from_utf8(record);
        let old_path = if kind == b'2' {
            Some(std::str::from_utf8(records.next().ok_or_else(malformed)?))
        } else {
            None
        };
        let (Ok(line), None | Some(Ok(_))) = (line, &old_path) else {
            if !matches!(kind, b'1' | b'2' | b'u' | b'?') {
                return Err(malformed());
            }
            result.unreadable_paths += 1;
            continue;
        };
        let old_path = old_path.and_then(Result::ok).map(String::from);
        if let Some(path) = line.strip_prefix("? ") {
            result.files.push(FileChange {
                path: path.into(),
                old_path: None,
                staged: None,
                unstaged: Some(Change::Untracked),
                submodule: false,
                conflict: None,
                staged_stats: None,
                unstaged_stats: None,
            });
            continue;
        }
        let count = match kind {
            b'1' => 9,
            b'2' => 10,
            b'u' => 11,
            _ => return Err(malformed()),
        };
        let fields: Vec<_> = line.splitn(count, ' ').collect();
        if fields.len() != count || fields[1].len() != 2 {
            return Err(malformed());
        }
        let xy = fields[1].as_bytes();
        let conflict = if kind == b'u' {
            Some(Conflict::from_code([xy[0], xy[1]]).ok_or_else(unknown)?)
        } else {
            None
        };
        result.files.push(FileChange {
            path: fields[count - 1].into(),
            old_path,
            staged: if conflict.is_some() {
                Some(Change::Conflicted)
            } else {
                change(xy[0])?
            },
            unstaged: if conflict.is_some() {
                Some(Change::Conflicted)
            } else {
                change(xy[1])?
            },
            submodule: fields[2].starts_with('S'),
            conflict,
            staged_stats: None,
            unstaged_stats: None,
        });
    }
    // Git names the configured upstream even when its remote-tracking ref does not exist,
    // and then has nothing to count the commits against.
    result.upstream_gone = result.upstream.is_some() && !compared;
    Ok(result)
}

/// One `# branch.*` header; `true` for the one that compares the branch with its upstream.
/// A branch name that is not UTF-8 is shown lossily: it is only displayed, and a push of it
/// is refused by Git rather than sent elsewhere.
fn read_header(result: &mut Status, header: &str) -> Result<bool, GitError> {
    if let Some(v) = header.strip_prefix("branch.oid ") {
        result.oid = (v != "(initial)").then(|| v.into());
    }
    if let Some(v) = header.strip_prefix("branch.head ") {
        result.branch = (v != "(detached)").then(|| v.into());
    }
    if let Some(v) = header.strip_prefix("branch.upstream ") {
        result.upstream = Some(v.into());
    }
    if let Some(v) = header.strip_prefix("branch.ab ") {
        let Some((ahead, behind)) = v.split_once(' ') else {
            return Err(malformed());
        };
        result.ahead = ahead
            .strip_prefix('+')
            .and_then(|v| v.parse().ok())
            .ok_or_else(malformed)?;
        result.behind = behind
            .strip_prefix('-')
            .and_then(|v| v.parse().ok())
            .ok_or_else(malformed)?;
        return Ok(true);
    }
    Ok(false)
}

fn malformed() -> GitError {
    GitError::Invalid("Malformed Git status response; no changes were applied.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_name_that_is_not_utf8_is_counted_and_hides_neither_the_rest_nor_a_dirty_tree() {
        // A Latin-1 file name as a Linux host returns it; OIDs shortened.
        let raw = b"# branch.oid abc\0# branch.head caf\xe9\0\
1 .M N... 100644 100644 100644 abc abc caf\xe9.txt\0\
2 R. N... 100644 100644 100644 abc def R100 new.txt\0caf\xe9-old.txt\0\
? notes.txt\0? r\xe9sum\xe9.txt\0";
        let status = parse(raw).expect("one unreadable name must not fail the read");
        assert_eq!(status.branch.as_deref(), Some("caf\u{fffd}"));
        let paths: Vec<_> = status.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["notes.txt"]);
        assert_eq!(status.unreadable_paths, 3);

        let only_unreadable = parse(b"# branch.oid abc\0? caf\xe9.txt\0").unwrap();
        assert!(only_unreadable.files.is_empty());
        assert!(!only_unreadable.is_clean());
        // Other bytes outside a path are still a malformed answer.
        assert!(parse(b"\xff broken\0").is_err());
    }

    #[test]
    fn an_upstream_without_its_tracking_ref_is_reported_as_gone() {
        // Captured from Git 2.54 after `git push origin --delete topic && git fetch --prune`
        // on a branch with one unpushed commit: the upstream is named, `branch.ab` is absent.
        let gone = parse(
            b"# branch.oid 2ad67d24e51243ae72b3b196ee521ec913e978cc\0# branch.head topic\0\
# branch.upstream origin/topic\0",
        )
        .expect("porcelain");
        assert_eq!(gone.upstream.as_deref(), Some("origin/topic"));
        assert!(gone.upstream_gone);
        assert_eq!(gone.tracked_upstream(), None);
        assert_eq!((gone.ahead, gone.behind), (0, 0));

        // The same branch while the remote still had it.
        let tracked = parse(
            b"# branch.oid 2ad67d24e51243ae72b3b196ee521ec913e978cc\0# branch.head topic\0\
# branch.upstream origin/topic\0# branch.ab +1 -0\0",
        )
        .expect("porcelain");
        assert!(!tracked.upstream_gone);
        assert_eq!(tracked.tracked_upstream(), Some("origin/topic"));
        assert_eq!(tracked.ahead, 1);

        let unpublished = parse(b"# branch.oid abc\0# branch.head topic\0").expect("porcelain");
        assert!(!unpublished.upstream_gone);
        assert_eq!(unpublished.tracked_upstream(), None);
    }

    #[test]
    fn a_status_serialized_before_the_gone_upstream_was_known_still_loads() {
        let old = r#"{"oid":"abc","branch":"main","upstream":"origin/main","ahead":1,"behind":2,"files":[]}"#;
        let status: Status = serde_json::from_str(old).expect("previous shape");
        assert!(!status.upstream_gone);
        assert_eq!(status.tracked_upstream(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (1, 2));
    }

    #[test]
    fn porcelain_preserves_paths_and_both_index_sides() {
        // Captured porcelain-v2 records; OIDs shortened since they are opaque here.
        let raw = b"# branch.oid abc\0# branch.head main\0# branch.ab +2 -3\0# future.header yes\0\
1 MM N... 100644 100644 100644 abc def space name\n.txt\0\
2 R. N... 100644 100644 100644 abc def R100 renamed\0original\0? -[new].txt\0";
        let s = parse(raw).expect("porcelain");
        assert_eq!((s.ahead, s.behind), (2, 3));
        assert_eq!(s.files[0].path, "space name\n.txt");
        assert_eq!(s.files[0].staged, Some(Change::Modified));
        assert_eq!(s.files[0].unstaged, Some(Change::Modified));
        assert_eq!(s.files[1].old_path.as_deref(), Some("original"));
        assert_eq!(s.files[2].unstaged, Some(Change::Untracked));
        assert!(parse(b"1 broken\0").is_err());
    }

    #[test]
    fn unmerged_records_keep_how_the_sides_disagree() {
        // Captured from `git status --porcelain=v2 -z` during merges; OIDs shortened.
        let record = |code: &str, path: &str| {
            format!("u {code} N... 100644 100644 100644 100644 abc def 123 {path}\0")
        };
        let cases = [
            ("UU", Conflict::BothModified),
            ("AA", Conflict::BothAdded),
            ("DD", Conflict::BothDeleted),
            ("AU", Conflict::AddedByUs),
            ("UA", Conflict::AddedByThem),
            ("DU", Conflict::DeletedByUs),
            ("UD", Conflict::DeletedByThem),
        ];
        for (code, expected) in cases {
            let status = parse(record(code, "dir/a b.txt").as_bytes()).expect(code);
            let file = &status.files[0];
            assert_eq!(file.path, "dir/a b.txt");
            assert_eq!(file.conflict, Some(expected), "{code}");
            assert_eq!(file.staged, Some(Change::Conflicted));
            assert_eq!(file.unstaged, Some(Change::Conflicted));
            assert!(status.has_conflicts());
        }
        assert!(parse(record("UX", "a").as_bytes()).is_err());
        assert!(parse(b"u UU N... 100644 truncated\0").is_err());
    }

    #[test]
    fn file_change_without_conflict_field_still_loads() {
        let old =
            r#"{"path":"a","old_path":null,"staged":"Modified","unstaged":null,"submodule":false}"#;
        let file: FileChange = serde_json::from_str(old).expect("previous shape");
        assert_eq!(file.conflict, None);
        assert_eq!(file.staged, Some(Change::Modified));
    }
}
