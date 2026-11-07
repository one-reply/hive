//! Bounded local document IO. Call these blocking operations off the UI thread.
mod access;
pub use access::Access;

use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub mod decorations;
pub mod images;
mod language;
mod listing;
mod mutations;
#[cfg(feature = "desktop")]
pub mod tree;
pub use mutations::{
    create_directory, create_file, delete_entry, entry_path, move_entry, name_taken, relocated_path,
};
pub mod recovery;
#[cfg(feature = "desktop")]
pub mod search;
pub use language::{editor_language, is_dockerfile};
pub use listing::{Entry, FileIndex, list_directory, list_files};

/// Largest document accepted by the editor.
pub const MAX_FILE_BYTES: usize = 1024 * 1024;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// A file operation that failed without consuming the editor's draft.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// Recursive removal stopped after deleting some entries.
    #[error(
        "Could not completely delete {path}: {source}. Some entries may already have been deleted; refresh to inspect the remaining files."
    )]
    PartialDelete {
        /// Requested root of the removal.
        path: PathBuf,
        /// First filesystem error encountered.
        #[source]
        source: Box<FileError>,
    },
    /// Filesystem failure with the operation and affected path.
    #[error("Cannot {operation} {path}: {source}")]
    Io {
        /// Attempted operation.
        operation: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Operating system error.
        #[source]
        source: std::io::Error,
    },
    /// An unsupported document or path.
    #[error("Cannot edit {path}: {reason}")]
    Unsupported {
        /// Requested path.
        path: PathBuf,
        /// User-facing explanation.
        reason: &'static str,
    },
    /// Another writer changed the document since it was read.
    #[error("{0} changed on disk. Compare the versions before saving.")]
    Conflict(PathBuf),
    /// File discovery could not finish.
    #[error("Cannot list files in {path}: {message}")]
    Listing {
        /// Worktree being searched.
        path: PathBuf,
        /// Command or traversal failure.
        message: String,
    },
    /// Recovery data could not be decoded safely.
    #[error("Invalid recovery record {path}: {message}")]
    Recovery {
        /// Recovery record on disk.
        path: PathBuf,
        /// Validation failure.
        message: String,
    },
}

pub(super) fn io_error(operation: &'static str, path: &Path, source: std::io::Error) -> FileError {
    FileError::Io {
        operation,
        path: path.into(),
        source,
    }
}

pub(super) fn unsupported(path: &Path, reason: &'static str) -> FileError {
    FileError::Unsupported {
        path: path.into(),
        reason,
    }
}

/// Exact bytes of the last disk version, including BOM and line endings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(deserialize_with = "snapshot_raw")]
    raw: String,
}

fn snapshot_raw<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let raw = String::deserialize(deserializer)?;
    Snapshot::parse(Path::new("snapshot"), raw.as_bytes())
        .map(|snapshot| snapshot.raw)
        .map_err(serde::de::Error::custom)
}

impl Snapshot {
    pub(crate) fn bytes(&self) -> &[u8] {
        self.raw.as_bytes()
    }

    /// Parse bounded UTF-8 source bytes, preserving their exact disk representation.
    pub fn parse(path: &Path, bytes: &[u8]) -> Result<Self, FileError> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(unsupported(
                path,
                "files larger than 1 MiB are not supported in Code",
            ));
        }
        if bytes.contains(&0) {
            return Err(unsupported(path, "binary files are not supported"));
        }
        let raw = std::str::from_utf8(bytes)
            .map_err(|_| unsupported(path, "the file is not UTF-8"))?
            .to_owned();
        Ok(Self { raw })
    }

    /// Text presented to the editor; uniform CRLF documents use LF internally.
    pub fn text(&self) -> String {
        let text = self.raw.strip_prefix('\u{feff}').unwrap_or(&self.raw);
        if self.is_crlf() {
            text.replace("\r\n", "\n")
        } else {
            text.into()
        }
    }

    fn is_crlf(&self) -> bool {
        self.raw.contains("\r\n") && !self.raw.replace("\r\n", "").contains('\n')
    }

    /// Encode editor text using this document's original BOM and newline convention.
    pub fn encode(&self, text: &str) -> String {
        let mut encoded = if self.is_crlf() {
            text.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            text.to_owned()
        };
        if self.raw.starts_with('\u{feff}') {
            encoded.insert(0, '\u{feff}');
        }
        encoded
    }

    /// Whether the editor contains the saved version, including after Undo.
    pub fn matches(&self, text: &str) -> bool {
        self.encode(text) == self.raw
    }
}

/// Reject paths outside a worktree, Git internals, symlinks and remote hosts.
pub fn resolve(host: &Host, root: &Path, relative: &Path) -> Result<PathBuf, FileError> {
    if host != &Host::Local {
        return Err(unsupported(root, "remote editing is not supported yet"));
    }
    if !root.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(name) if name != ".git"))
    {
        return Err(unsupported(
            relative,
            "the path must stay inside the selected worktree",
        ));
    }
    let mut path = fs::canonicalize(root).map_err(|e| io_error("resolve worktree", root, e))?;
    for part in relative.components() {
        path.push(part);
        let metadata = fs::symlink_metadata(&path).map_err(|e| io_error("inspect", &path, e))?;
        if metadata.file_type().is_symlink() {
            return Err(unsupported(&path, "symbolic links are not editable"));
        }
    }
    Ok(path)
}

/// Read a regular UTF-8 document without lossy decoding or unbounded allocation.
pub fn read(host: &Host, root: &Path, relative: &Path) -> Result<Snapshot, FileError> {
    if !host.is_local() {
        return crate::remote::read(
            host,
            root,
            relative,
            &crate::command::Cancellation::default(),
        );
    }
    let path = resolve(host, root, relative)?;
    read_path(&path)
}

fn read_path(path: &Path) -> Result<Snapshot, FileError> {
    let bytes = read_bytes(
        path,
        MAX_FILE_BYTES,
        "files larger than 1 MiB are not supported",
    )?;
    Snapshot::parse(path, &bytes)
}

fn read_bytes(
    path: &Path,
    limit: usize,
    limit_message: &'static str,
) -> Result<Vec<u8>, FileError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error("inspect", path, e))?;
    if !metadata.is_file() {
        return Err(unsupported(path, "only regular files are supported"));
    }
    if metadata.len() > limit as u64 {
        return Err(unsupported(path, limit_message));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| io_error("open", path, e))?;
    if !file
        .metadata()
        .map_err(|e| io_error("inspect", path, e))?
        .is_file()
    {
        return Err(unsupported(path, "only regular files are supported"));
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error("read", path, e))?;
    if bytes.len() > limit {
        return Err(unsupported(path, limit_message));
    }
    Ok(bytes)
}

/// Save only if the on-disk version still equals `expected`; never recreate a missing file.
pub fn save(
    host: &Host,
    root: &Path,
    relative: &Path,
    expected: &Snapshot,
    text: &str,
) -> Result<Snapshot, FileError> {
    if !host.is_local() {
        return crate::remote::save(
            host,
            root,
            relative,
            expected,
            text,
            &crate::command::Cancellation::default(),
        );
    }
    let path = resolve(host, root, relative)?;
    let raw = expected.encode(text);
    if raw.len() > MAX_FILE_BYTES || raw.contains('\0') {
        return Err(unsupported(
            &path,
            "the edited text exceeds the document limits",
        ));
    }
    if read_path(&path)? != *expected {
        return Err(FileError::Conflict(path));
    }
    let metadata = fs::metadata(&path).map_err(|e| io_error("read permissions", &path, e))?;
    if metadata.permissions().readonly() {
        return Err(unsupported(&path, "the file is read-only"));
    }
    atomic_write(&path, raw.as_bytes(), Some(metadata.permissions()), || {
        if resolve(host, root, relative)? != path || read_path(&path)? != *expected {
            return Err(FileError::Conflict(path.clone()));
        }
        Ok(())
    })?;
    Ok(Snapshot { raw })
}

pub(super) fn atomic_write(
    path: &Path,
    bytes: &[u8],
    permissions: Option<fs::Permissions>,
    check: impl FnOnce() -> Result<(), FileError>,
) -> Result<(), FileError> {
    let dir = path
        .parent()
        .ok_or_else(|| unsupported(path, "missing parent directory"))?;
    let (temporary, mut file) = loop {
        let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let temporary = dir.join(format!(".hive-write-{}-{sequence}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error("create temporary file", &temporary, error)),
        }
    };
    let result = (|| {
        file.write_all(bytes)
            .map_err(|e| io_error("write", &temporary, e))?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)
                .map_err(|e| io_error("preserve permissions", &temporary, e))?;
        }
        file.sync_all()
            .map_err(|e| io_error("sync", &temporary, e))?;
        // shortcut: external CLI writers do not share a lock; the final check narrows, but
        // cannot eliminate, the race before rename. Upgrade with a cooperating filesystem service.
        check()?;
        fs::rename(&temporary, path).map_err(|e| io_error("replace", path, e))?;
        File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| io_error("sync directory", dir, e))
    })();
    if result.is_err()
        && let Err(error) = fs::remove_file(&temporary)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(?temporary, %error, "could not remove temporary document");
    }
    result
}

#[cfg(test)]
mod tests;
