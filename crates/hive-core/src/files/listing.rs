use super::{FileError, io_error, resolve};
use crate::{command, host::Host};
use std::os::unix::ffi::OsStringExt;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// Maximum number of searchable paths retained per worktree.
const INDEX_LIMIT: usize = 100_000;
const COMMAND_LIMIT: usize = 32 * 1024 * 1024;

/// One directory entry. Symlinks are listed but never followed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    /// Worktree-relative path.
    #[serde(with = "crate::path_encoding")]
    pub path: PathBuf,
    /// Whether this is a traversable directory.
    pub directory: bool,
    /// Whether opening this entry is unsupported.
    pub unsupported: bool,
}

/// A bounded file list for Quick Open.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct FileIndex {
    /// Worktree-relative paths, sorted by name.
    #[serde(with = "crate::path_encoding::many")]
    pub paths: Vec<PathBuf>,
    /// Whether more files exist beyond the index limit.
    pub truncated: bool,
}

/// Read one directory, excluding Git internals and Hive's in-flight atomic writes.
pub fn list_directory(host: &Host, root: &Path, relative: &Path) -> Result<Vec<Entry>, FileError> {
    if !host.is_local() {
        return crate::remote::list_directory(
            host,
            root,
            relative,
            &crate::command::Cancellation::default(),
        );
    }
    let path = resolve(host, root, relative)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&path).map_err(|e| io_error("list directory", &path, e))? {
        let entry = entry.map_err(|e| io_error("list directory", &path, e))?;
        let name = entry.file_name();
        if name == ".git" || name.to_string_lossy().starts_with(".hive-write-") {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|e| io_error("inspect", &entry.path(), e))?;
        entries.push(Entry {
            path: relative.join(name),
            directory: kind.is_dir(),
            unsupported: !kind.is_file() && !kind.is_dir(),
        });
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(entries)
}

/// Index tracked and non-ignored untracked files, or walk a non-Git directory.
/// Cancelling stops the Git commands or the walk with `FileError::Listing`.
pub fn list_files(
    host: &Host,
    root: &Path,
    cancellation: &command::Cancellation,
) -> Result<FileIndex, FileError> {
    if !host.is_local() {
        return crate::remote::list_files(host, root, cancellation);
    }
    resolve(host, root, Path::new(""))?;
    let probe = command::run(
        host.command("git", &["rev-parse", "--show-toplevel"], Some(root)),
        &[],
        cancellation,
        Duration::from_secs(10),
        COMMAND_LIMIT,
    )
    .map_err(|e| FileError::Listing {
        path: root.into(),
        message: e.to_string(),
    })?;
    if probe.code == Some(0) {
        let output = command::run(
            host.command(
                "git",
                &[
                    "ls-files",
                    "-z",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                ],
                Some(root),
            ),
            &[],
            cancellation,
            Duration::from_secs(15),
            COMMAND_LIMIT,
        )
        .map_err(|e| FileError::Listing {
            path: root.into(),
            message: e.to_string(),
        })?;
        if output.code != Some(0) {
            return Err(FileError::Listing {
                path: root.into(),
                message: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        let mut result = FileIndex::default();
        for bytes in output
            .stdout
            .split(|b| *b == 0)
            .filter(|bytes| !bytes.is_empty())
        {
            if result.paths.len() == INDEX_LIMIT {
                result.truncated = true;
                break;
            }
            result
                .paths
                .push(PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())));
        }
        result.paths.sort();
        result.paths.dedup();
        return Ok(result);
    }
    let mut result = FileIndex::default();
    let mut pending = vec![PathBuf::new()];
    while let Some(directory) = pending.pop() {
        if cancellation.is_cancelled() {
            return Err(FileError::Listing {
                path: root.into(),
                message: command::Error::Cancelled.to_string(),
            });
        }
        for entry in list_directory(host, root, &directory)? {
            if entry.directory {
                pending.push(entry.path);
            } else if !entry.unsupported {
                result.paths.push(entry.path);
            }
            if result.paths.len() >= INDEX_LIMIT {
                result.truncated = true;
                result.paths.sort();
                return Ok(result);
            }
        }
    }
    result.paths.sort();
    Ok(result)
}
