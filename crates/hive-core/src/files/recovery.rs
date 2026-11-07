//! Private, per-document recovery records; source files are never changed here.
use super::{FileError, MAX_FILE_BYTES, Snapshot, atomic_write, io_error};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const MAX_RECORD_BYTES: usize = MAX_FILE_BYTES * 32;

/// The context and text needed to restore an unsaved document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Draft {
    /// Host owning the document.
    #[serde(default)]
    pub host: Host,
    /// Registered project path.
    #[serde(with = "crate::path_encoding::persisted")]
    pub project: PathBuf,
    /// Worktree or ordinary project directory.
    #[serde(with = "crate::path_encoding::persisted")]
    pub root: PathBuf,
    /// Worktree-relative document path.
    #[serde(with = "crate::path_encoding::persisted")]
    pub path: PathBuf,
    /// Last disk version accepted by the user.
    pub base: Snapshot,
    /// Current editor text, with no implicit save to the source file.
    pub text: String,
}

impl Draft {
    /// Stable within this build; records are discovered by contents on startup.
    pub fn key(&self) -> String {
        key(&self.host, &self.root, &self.path)
    }
}

/// Storage key only; document identity is always verified from the record itself.
pub fn key(host: &Host, root: &Path, path: &Path) -> String {
    let mut hash = DefaultHasher::new();
    (host, root, path).hash(&mut hash);
    format!("{:016x}", hash.finish())
}

/// A restored record retains its filename so upgrades can remove older key formats.
#[derive(Clone)]
pub struct Record {
    /// Original recovery file name without an extension.
    pub key: String,
    /// Validated recovery contents.
    pub draft: Draft,
}

/// Read valid records independently; damaged records are preserved and reported.
pub fn load(dir: &Path) -> (Vec<Record>, Vec<String>) {
    let mut records = Vec::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (records, errors),
        Err(e) => {
            return (
                records,
                vec![io_error("read recovery directory", dir, e).to_string()],
            );
        }
    };
    for entry in entries {
        match entry {
            Ok(entry) if entry.path().extension().is_some_and(|ext| ext == "json") => {
                match read_record(&entry.path()) {
                    Ok(draft) => records.push(Record {
                        key: entry
                            .path()
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        draft,
                    }),
                    Err(error) => errors.push(error.to_string()),
                }
            }
            Ok(_) => {}
            Err(error) => errors.push(io_error("read recovery record", dir, error).to_string()),
        }
    }
    (records, errors)
}

fn read_record(path: &Path) -> Result<Draft, FileError> {
    let invalid = |message: &str| FileError::Recovery {
        path: path.into(),
        message: message.into(),
    };
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error("inspect recovery", path, e))?;
    if !metadata.is_file() || metadata.len() > MAX_RECORD_BYTES as u64 {
        return Err(invalid("unsupported record type or size"));
    }
    let mut bytes = Vec::new();
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .and_then(|file| {
            file.take((MAX_RECORD_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|e| io_error("read recovery", path, e))?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(invalid("record is too large"));
    }
    let draft: Draft = serde_json::from_slice(&bytes).map_err(|e| invalid(&e.to_string()))?;
    validate(&draft, path)?;
    Ok(draft)
}

fn validate(draft: &Draft, path: &Path) -> Result<(), FileError> {
    if !draft.root.is_absolute()
        || !draft.project.is_absolute()
        || draft.path.as_os_str().is_empty()
        || draft
            .path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(name) if name != ".git"))
        || draft.base.raw.len() > MAX_FILE_BYTES
        || draft.text.len() > MAX_FILE_BYTES * 4
        || draft.text.contains('\0')
        || draft.base.raw.contains('\0')
    {
        return Err(FileError::Recovery {
            path: path.into(),
            message: "invalid document path or contents (draft limit 4 MiB)".into(),
        });
    }
    Ok(())
}

/// Atomically replace a private recovery record, checking hash collisions before writing.
pub fn write(dir: &Path, key: &str, draft: &Draft) -> Result<(), FileError> {
    validate(draft, dir)?;
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(FileError::Recovery {
            path: dir.into(),
            message: "invalid recovery key".into(),
        });
    }
    fs::create_dir_all(dir).map_err(|e| io_error("create recovery directory", dir, e))?;
    if !fs::symlink_metadata(dir)
        .map_err(|e| io_error("inspect recovery directory", dir, e))?
        .is_dir()
    {
        return Err(FileError::Recovery {
            path: dir.into(),
            message: "recovery directory must not be a symlink".into(),
        });
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| io_error("protect recovery directory", dir, e))?;
    let path = dir.join(format!("{key}.json"));
    if path.exists() {
        let old = read_record(&path)?;
        if (old.host, old.root, old.path)
            != (draft.host.clone(), draft.root.clone(), draft.path.clone())
        {
            return Err(FileError::Recovery {
                path,
                message: "document identity collision; original retained".into(),
            });
        }
    }
    let bytes = serde_json::to_vec(draft).map_err(|e| FileError::Recovery {
        path: path.clone(),
        message: e.to_string(),
    })?;
    atomic_write(&path, &bytes, None, || Ok(()))
}

/// Delete a saved or explicitly discarded draft; a missing record is already resolved.
pub fn remove(dir: &Path, key: &str) -> Result<(), FileError> {
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(FileError::Recovery {
            path: dir.into(),
            message: "invalid recovery key".into(),
        });
    }
    let path = dir.join(format!("{key}.json"));
    match fs::remove_file(&path) {
        Ok(()) => fs::File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| io_error("sync recovery directory", dir, e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error("remove recovery record", &path, e)),
    }
}
