//! Bounded, NUL-delimited line counts, independent of the full patch preview.
use super::status::{Change, LineStats, Status};
use super::{GitError, READ_LIMIT, bytes, diff, text};
use crate::host::Host;
use std::collections::HashMap;
use std::path::Path;

pub(super) fn read(
    host: &Host,
    cwd: &Path,
    args: &[&str],
    paths: &[&str],
) -> Result<HashMap<String, LineStats>, GitError> {
    let mut command = vec!["--literal-pathspecs"];
    command.extend_from_slice(args);
    command.extend([
        "--numstat",
        "-z",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
        "--",
    ]);
    command.extend_from_slice(paths);
    parse(&bytes(
        host,
        cwd,
        &command,
        None,
        READ_LIMIT,
        args.contains(&"--no-index"),
    )?)
}

pub(super) fn fill_status(host: &Host, cwd: &Path, status: &mut Status) {
    let read_counts = |args: &[&str]| {
        read(host, cwd, args, &[]).unwrap_or_else(|error| {
            tracing::warn!(%error, path = %cwd.display(), "Git line counts unavailable");
            HashMap::new()
        })
    };
    if status.files.iter().any(|f| f.staged.is_some()) {
        let counts = read_counts(&["diff", "--cached"]);
        for file in &mut status.files {
            if file.staged != Some(Change::Conflicted) && !file.submodule {
                file.staged_stats = counts.get(&file.path).copied();
            }
        }
    }
    if status.files.iter().any(|f| {
        f.unstaged
            .is_some_and(|c| !matches!(c, Change::Untracked | Change::Conflicted))
    }) {
        let counts = read_counts(&["diff"]);
        for file in &mut status.files {
            if file.unstaged != Some(Change::Conflicted) && !file.submodule {
                file.unstaged_stats = counts.get(&file.path).copied();
            }
        }
    }
    // shortcut: one Git process per untracked file; batch if large untracked trees become a bottleneck.
    for file in &mut status.files {
        if file.unstaged != Some(Change::Untracked) {
            continue;
        }
        let counts = diff::validate_path(&file.path).and_then(|()| {
            read(
                host,
                cwd,
                &["diff", "--no-index"],
                &["/dev/null", &file.path],
            )
        });
        match counts {
            Ok(counts) => file.unstaged_stats = counts.get(&file.path).copied(),
            Err(error) => {
                tracing::warn!(%error, path = %file.path, "Untracked line counts unavailable");
            }
        }
    }
}

fn parse(raw: &[u8]) -> Result<HashMap<String, LineStats>, GitError> {
    let mut counts = HashMap::new();
    if raw.is_empty() {
        return Ok(counts);
    }
    let mut records = raw
        .strip_suffix(b"\0")
        .ok_or_else(malformed)?
        .split(|b| *b == 0);
    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, |b| *b == b'\t');
        let added = fields.next().ok_or_else(malformed)?;
        let removed = fields.next().ok_or_else(malformed)?;
        let path = fields.next().ok_or_else(malformed)?;
        let path = if path.is_empty() {
            let old_path = records.next().ok_or_else(malformed)?;
            if old_path.is_empty() {
                return Err(malformed());
            }
            records.next().ok_or_else(malformed)?
        } else {
            path
        };
        if path.is_empty() {
            return Err(malformed());
        }
        if added == b"-" && removed == b"-" {
            continue;
        }
        let number = |value| text(value)?.parse::<usize>().map_err(|_| malformed());
        counts.insert(
            text(path)?.to_string(),
            LineStats {
                added: number(added)?,
                removed: number(removed)?,
            },
        );
    }
    Ok(counts)
}

fn malformed() -> GitError {
    GitError::Invalid("Malformed Git line counts".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_numstat_handles_rename_binary_and_literal_paths() {
        // Captured from git diff --cached --numstat -z --find-renames.
        let raw = b"-\t-\tbinary.bin\0\
2\t1\t\0old\tname.txt\0new\nname.txt\0\
2\t0\tspace name.txt\0";
        let counts = parse(raw).unwrap();
        assert_eq!(counts.len(), 2);
        assert_eq!(
            counts["new\nname.txt"],
            LineStats {
                added: 2,
                removed: 1
            }
        );
        assert_eq!(
            counts["space name.txt"],
            LineStats {
                added: 2,
                removed: 0
            }
        );
        assert!(parse(b"").unwrap().is_empty());
        for raw in [&b"1\t0\t\0old\0"[..], b"-\t0\tfile\0", b"broken\0"] {
            assert!(parse(raw).is_err());
        }
    }
}
