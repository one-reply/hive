//! Bounded, NUL-delimited line counts, independent of the full patch preview.
use super::status::{Change, LineStats, Status};
use super::{GitError, READ_LIMIT, bytes, diff, script, text};
use crate::host::Host;
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Bytes of untracked files read locally per status read, which runs every two seconds
/// while the Git panel is open. 2 000 files of 20 KiB fit; a 300 MB log, which took 105 ms
/// to count on every refresh, does not, and simply shows no count.
const UNTRACKED_BYTES: u64 = 64 * 1024 * 1024;
/// Untracked files counted by Git itself per status read: those with a custom diff driver
/// and, on an SSH host, all of them. They are counted in one shell, never one connection
/// each, but still one `git` process per file: 199 files took 1.4 s locally, so beyond this
/// many the rest show no count.
const GIT_COUNTED: usize = 100;
/// `git diff --no-index` compares exactly two paths and exits 1 when they differ.
const UNTRACKED_SCRIPT: &str = "for f do git diff --no-index --numstat -z --no-ext-diff --no-textconv -- /dev/null \"$f\"; s=$?; [ \"$s\" -le 1 ] || exit \"$s\"; done";

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
    fill_untracked(host, cwd, status, UNTRACKED_BYTES);
}

/// Counts the lines of untracked files: locally by reading at most `budget` bytes, and by
/// Git in one batch where that cannot be done, such as on an SSH host.
fn fill_untracked(host: &Host, cwd: &Path, status: &mut Status, mut budget: u64) {
    let settings = untracked_settings(host, cwd, status);
    let mut by_git = Vec::new();
    for (index, file) in status.files.iter_mut().enumerate() {
        if file.unstaged != Some(Change::Untracked) {
            continue;
        }
        if let Some((attributes, threshold)) = &settings
            && let Some(attribute) = attributes.get(&file.path)
        {
            match local_untracked_counts(cwd, &file.path, attribute, *threshold, &mut budget) {
                Ok(counts) => {
                    file.unstaged_stats = counts;
                    continue;
                }
                Err(error) => {
                    tracing::debug!(%error, path = %file.path, "using Git fallback for untracked counts")
                }
            }
        }
        if diff::validate_path(&file.path).is_ok() && by_git.len() < GIT_COUNTED {
            by_git.push(index);
        }
    }
    if by_git.is_empty() {
        return;
    }
    let paths: Vec<&str> = by_git
        .iter()
        .map(|&index| status.files[index].path.as_str())
        .collect();
    let counts =
        match script(host, cwd, UNTRACKED_SCRIPT, &paths, READ_LIMIT).and_then(|raw| parse(&raw)) {
            Ok(counts) => counts,
            Err(error) => {
                tracing::warn!(%error, files = paths.len(), "Untracked line counts unavailable");
                return;
            }
        };
    for index in by_git {
        let file = &mut status.files[index];
        file.unstaged_stats = counts.get(&file.path).copied();
    }
}

fn untracked_settings(
    host: &Host,
    cwd: &Path,
    status: &Status,
) -> Option<(HashMap<String, String>, u64)> {
    if !host.is_local() {
        return None;
    }
    let mut paths = Vec::new();
    for file in &status.files {
        if file.unstaged == Some(Change::Untracked) {
            paths.extend_from_slice(file.path.as_bytes());
            paths.push(0);
        }
    }
    if paths.is_empty() {
        return None;
    }
    let result = bytes(
        host,
        cwd,
        &["check-attr", "-z", "--stdin", "diff"],
        Some(&paths),
        READ_LIMIT,
        false,
    )
    .and_then(|raw| {
        let fields: Vec<_> = raw
            .strip_suffix(b"\0")
            .ok_or_else(malformed)?
            .split(|b| *b == 0)
            .collect();
        let (records, remainder) = fields.as_chunks::<3>();
        if !remainder.is_empty() {
            return Err(malformed());
        }
        records
            .iter()
            .map(|fields| {
                if fields[1] != b"diff" {
                    return Err(malformed());
                }
                Ok((text(fields[0])?.to_owned(), text(fields[2])?.to_owned()))
            })
            .collect::<Result<HashMap<_, _>, GitError>>()
    })
    .and_then(|attributes| {
        let raw = bytes(
            host,
            cwd,
            &["config", "--type=int", "--get", "core.bigFileThreshold"],
            None,
            READ_LIMIT,
            true,
        )?;
        let threshold = if raw.is_empty() {
            512 * 1024 * 1024
        } else {
            text(&raw)?
                .trim()
                .parse()
                .map_err(|_| GitError::Invalid("Invalid core.bigFileThreshold".into()))?
        };
        Ok((attributes, threshold))
    });
    match result {
        Ok(attributes) => Some(attributes),
        Err(error) => {
            tracing::warn!(%error, "could not batch untracked attributes; using Git fallback");
            None
        }
    }
}

/// `Ok(None)` for a binary or oversized file, and for one that would overrun what is left of
/// `budget`, which reading it then uses up.
fn local_untracked_counts(
    cwd: &Path,
    path: &str,
    attribute: &str,
    binary_threshold: u64,
    budget: &mut u64,
) -> std::io::Result<Option<LineStats>> {
    diff::validate_path(path).map_err(std::io::Error::other)?;
    if !matches!(attribute, "unspecified" | "set" | "unset") {
        return Err(std::io::Error::other("custom diff driver requires Git"));
    }
    let path = cwd.join(path);
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(path)?;
        return count_added_lines(target.as_os_str().as_bytes(), false);
    }
    if !metadata.is_file() {
        return Ok(None);
    }
    if attribute == "unset" {
        return Ok(None);
    }
    // O_NONBLOCK also makes a concurrent replacement with a FIFO safe to open.
    let fd = rustix::fs::open(
        &path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    let file = std::fs::File::from(fd);
    let metadata = file.metadata()?;
    if !metadata.is_file() || (attribute != "set" && metadata.len() > binary_threshold) {
        return Ok(None);
    }
    let Some(left) = budget.checked_sub(metadata.len()) else {
        return Ok(None);
    };
    *budget = left;
    // A file that grows while it is read is counted only up to the size the budget allowed.
    count_added_lines(file.take(metadata.len()), attribute != "set")
}

fn count_added_lines(
    mut input: impl Read,
    detect_binary: bool,
) -> std::io::Result<Option<LineStats>> {
    // Git's automatic binary detection checks the first 8000 bytes.
    const BINARY_PREFIX: usize = 8000;
    let mut buffer = [0; 8192];
    let (mut bytes, mut lines, mut last) = (0usize, 0usize, None);
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let prefix = count.min(BINARY_PREFIX.saturating_sub(bytes));
        if detect_binary && buffer[..prefix].contains(&0) {
            return Ok(None);
        }
        bytes += count;
        lines += buffer[..count]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count();
        last = Some(buffer[count - 1]);
    }
    Ok(Some(LineStats {
        added: lines + usize::from(last.is_some_and(|byte| byte != b'\n')),
        removed: 0,
    }))
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
    fn local_settings_enable_the_batch_path_and_parse_git_size_units() {
        let root = std::env::temp_dir().join(format!("hive-numstat-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        super::super::run(&Host::Local, &root, &["init", "-q"]).unwrap();
        super::super::run(
            &Host::Local,
            &root,
            &["config", "core.bigFileThreshold", "512m"],
        )
        .unwrap();
        std::fs::write(root.join("new.txt"), "one\nlast").unwrap();
        let status = super::super::status::read(&Host::Local, &root).unwrap();
        let (attributes, threshold) = untracked_settings(&Host::Local, &root, &status).unwrap();
        assert_eq!(threshold, 512 * 1024 * 1024);
        assert_eq!(attributes["new.txt"], "unspecified");
        let mut budget = 100;
        assert_eq!(
            local_untracked_counts(
                &root,
                "new.txt",
                &attributes["new.txt"],
                threshold,
                &mut budget
            )
            .unwrap(),
            Some(LineStats {
                added: 2,
                removed: 0
            })
        );
        // The file's eight bytes came out of the budget; one that does not fit is not read.
        assert_eq!(budget, 92);
        let mut spent = 7;
        assert_eq!(
            local_untracked_counts(
                &root,
                "new.txt",
                &attributes["new.txt"],
                threshold,
                &mut spent
            )
            .unwrap(),
            None
        );
        assert_eq!(spent, 7);
        super::super::run(
            &Host::Local,
            &root,
            &["config", "core.bigFileThreshold", "2k"],
        )
        .unwrap();
        assert_eq!(
            untracked_settings(&Host::Local, &root, &status).unwrap().1,
            2048
        );
        std::fs::remove_dir_all(root).unwrap();
    }

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
