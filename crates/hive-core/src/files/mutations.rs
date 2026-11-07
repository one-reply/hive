//! File mutations are relative to opened directory handles, never symlink traversal.
use super::{FileError, io_error, unsupported};
use crate::host::Host;
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, Dir, FileType, Mode, OFlags, RenameFlags},
};
use std::os::unix::ffi::OsStrExt;
use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const MAX_DEPTH: usize = 256;

/// Turn one user-entered name into a relative entry path, without permitting traversal.
pub fn entry_path(parent: &Path, name: &str) -> Result<PathBuf, FileError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\0'])
        || name.eq_ignore_ascii_case(".git")
        || name.starts_with(".hive-write-")
    {
        return Err(unsupported(
            Path::new(name),
            "enter one file or folder name, outside Git internals",
        ));
    }
    Ok(parent.join(name))
}

/// Whether `name` collides with one of `siblings`, the paths listed in its parent folder.
///
/// Local names compare case-insensitively, as on macOS's default volumes, where `README.md` and
/// `Readme.md` are one entry. `renaming` is left out, so renaming an entry to its own name in
/// another case is not a collision. The listing is a snapshot: the file system still refuses
/// to replace an entry created since, so this only lets the panel warn while the user types.
pub fn name_taken<'a>(
    host: &Host,
    siblings: impl IntoIterator<Item = &'a Path>,
    name: &str,
    renaming: Option<&Path>,
) -> bool {
    let name = if host.is_local() {
        name.to_lowercase()
    } else {
        name.to_owned()
    };
    siblings.into_iter().any(|sibling| {
        Some(sibling) != renaming
            && sibling.file_name().is_some_and(|existing| {
                if host.is_local() {
                    existing.to_string_lossy().to_lowercase() == name
                } else {
                    existing == std::ffi::OsStr::new(&name)
                }
            })
    })
}

/// Relocate an entry or descendant, keeping a file destination free of trailing separators.
pub fn relocated_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let suffix = path.strip_prefix(from).ok()?;
    Some(if suffix.as_os_str().is_empty() {
        to.to_owned()
    } else {
        to.join(suffix)
    })
}

fn parent(host: &Host, root: &Path, path: &Path) -> Result<(OwnedFd, OsString), FileError> {
    if host != &Host::Local || !root.is_absolute() {
        return Err(unsupported(
            root,
            "file operations require an absolute local worktree",
        ));
    }
    if path.as_os_str().is_empty() || path.components().any(|part| {
        !matches!(part, Component::Normal(name) if !name.as_bytes().eq_ignore_ascii_case(b".git") && !name.as_bytes().starts_with(b".hive-write-"))
    }) {
        return Err(unsupported(path, "the path must stay inside the worktree, outside Git internals"));
    }
    let mut directory = fs::open(root, DIRECTORY_FLAGS, Mode::empty())
        .map_err(|e| io_error("open worktree", root, e.into()))?;
    let name = path
        .file_name()
        .ok_or_else(|| unsupported(path, "missing file name"))?;
    let mut current = root.to_path_buf();
    for part in path.parent().unwrap_or(Path::new("")).components() {
        current.push(part);
        directory = fs::openat(&directory, part.as_os_str(), DIRECTORY_FLAGS, Mode::empty())
            .map_err(|e| io_error("open directory", &current, e.into()))?;
    }
    Ok((directory, name.to_owned()))
}

fn kind(directory: &OwnedFd, name: &std::ffi::OsStr, path: &Path) -> Result<FileType, FileError> {
    let stat = fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|e| io_error("inspect", path, e.into()))?;
    Ok(FileType::from_raw_mode(stat.st_mode))
}

/// Create one empty file exclusively. Its parent must already exist.
pub fn create_file(host: &Host, root: &Path, path: &Path) -> Result<(), FileError> {
    if !host.is_local() {
        return crate::remote::mutate(
            host,
            root,
            path,
            crate::remote::Mutation::CreateFile,
            &crate::command::Cancellation::default(),
        );
    }
    let (directory, name) = parent(host, root, path)?;
    let file = fs::openat(
        &directory,
        &name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644),
    )
    .map_err(|e| io_error("create file", &root.join(path), e.into()))?;
    fs::fsync(&file).map_err(|e| io_error("sync file", &root.join(path), e.into()))?;
    Ok(())
}

/// Create one directory exclusively, without creating missing ancestors.
pub fn create_directory(host: &Host, root: &Path, path: &Path) -> Result<(), FileError> {
    if !host.is_local() {
        return crate::remote::mutate(
            host,
            root,
            path,
            crate::remote::Mutation::CreateDirectory,
            &crate::command::Cancellation::default(),
        );
    }
    let (directory, name) = parent(host, root, path)?;
    fs::mkdirat(&directory, &name, Mode::from_raw_mode(0o755))
        .map_err(|e| io_error("create directory", &root.join(path), e.into()))
}

/// Rename or move an entry without replacing a destination or changing the Git index.
pub fn move_entry(host: &Host, root: &Path, from: &Path, to: &Path) -> Result<(), FileError> {
    if !host.is_local() {
        return crate::remote::mutate(
            host,
            root,
            from,
            crate::remote::Mutation::Move { to: to.into() },
            &crate::command::Cancellation::default(),
        );
    }
    let (source, name) = parent(host, root, from)?;
    let (destination, new_name) = parent(host, root, to)?;
    if from == to {
        return Ok(());
    }
    if to.starts_with(from) {
        return Err(unsupported(to, "a directory cannot be moved inside itself"));
    }
    let source_kind = kind(&source, &name, &root.join(from))?;
    if !matches!(source_kind, FileType::RegularFile | FileType::Directory) {
        return Err(unsupported(
            from,
            "only regular files and directories can be moved",
        ));
    }
    if source_kind == FileType::Directory {
        inspect_directory(&source, &name, &root.join(from), 0)?;
    }
    match fs::renameat_with(
        &source,
        &name,
        &destination,
        &new_name,
        RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::XDEV) => Err(unsupported(
            to,
            "cannot move between filesystems; no copy or deletion was performed",
        )),
        Err(rustix::io::Errno::EXIST) if from.parent() == to.parent() => {
            // Case-insensitive volumes resolve both spellings to the same directory entry.
            let names = directory_names(&source, root.join(from).parent().unwrap_or(root))?;
            let old = fs::statat(&source, &name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|e| io_error("inspect source", from, e.into()))?;
            let new = fs::statat(&destination, &new_name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|e| io_error("inspect destination", to, e.into()))?;
            if names.contains(&new_name) || old.st_dev != new.st_dev || old.st_ino != new.st_ino {
                return Err(io_error(
                    "move entry (destination exists)",
                    &root.join(to),
                    std::io::ErrorKind::AlreadyExists.into(),
                ));
            }
            // NOREPLACE on Darwin accepts a case-only rename on current APFS. If another
            // filesystem rejects it, keep both entries intact rather than emulate replacement.
            Err(unsupported(
                to,
                "this filesystem does not support a safe case-only rename; use an intermediate name",
            ))
        }
        Err(error) => Err(io_error(
            "move entry without replacing destination",
            &root.join(to),
            error.into(),
        )),
    }
}

fn directory_names(directory: &OwnedFd, path: &Path) -> Result<Vec<OsString>, FileError> {
    let reader =
        Dir::read_from(directory).map_err(|e| io_error("read directory", path, e.into()))?;
    reader
        .filter_map(|entry| match entry {
            Ok(entry) if matches!(entry.file_name().to_bytes(), b"." | b"..") => None,
            Ok(entry) => Some(Ok(std::ffi::OsStr::from_bytes(
                entry.file_name().to_bytes(),
            )
            .to_owned())),
            Err(error) => Some(Err(io_error("read directory", path, error.into()))),
        })
        .collect()
}

fn inspect_directory(
    parent: &OwnedFd,
    name: &std::ffi::OsStr,
    path: &Path,
    depth: usize,
) -> Result<(), FileError> {
    if depth >= MAX_DEPTH {
        return Err(unsupported(
            path,
            "directory nesting exceeds the safe operation limit",
        ));
    }
    let directory = fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty())
        .map_err(|e| io_error("open directory", path, e.into()))?;
    for child in directory_names(&directory, path)? {
        let child_path = path.join(&child);
        if child.as_bytes().eq_ignore_ascii_case(b".git") {
            return Err(unsupported(
                path,
                "contains a nested Git repository; manage it separately",
            ));
        }
        if kind(&directory, &child, &child_path)? == FileType::Directory {
            inspect_directory(&directory, &child, &child_path, depth + 1)?;
        }
    }
    Ok(())
}

/// Permanently remove a file or directory. Symlinks inside a directory are unlinked, never followed.
/// A recursive failure reports that the removal may have partially completed.
pub fn delete_entry(host: &Host, root: &Path, path: &Path) -> Result<(), FileError> {
    if !host.is_local() {
        return crate::remote::mutate(
            host,
            root,
            path,
            crate::remote::Mutation::Delete,
            &crate::command::Cancellation::default(),
        );
    }
    let (directory, name) = parent(host, root, path)?;
    let absolute = root.join(path);
    let entry_kind = kind(&directory, &name, &absolute)?;
    if !matches!(entry_kind, FileType::RegularFile | FileType::Directory) {
        return Err(unsupported(
            path,
            "only regular files and directories can be deleted",
        ));
    }
    if entry_kind == FileType::Directory {
        inspect_directory(&directory, &name, &absolute, 0)?;
    }
    remove(&directory, &name, &absolute, 0).map_err(|source| {
        if entry_kind == FileType::Directory {
            FileError::PartialDelete {
                path: absolute,
                source: Box::new(source),
            }
        } else {
            source
        }
    })
}

fn remove(
    parent: &OwnedFd,
    name: &std::ffi::OsStr,
    path: &Path,
    depth: usize,
) -> Result<(), FileError> {
    if depth >= MAX_DEPTH {
        return Err(unsupported(
            path,
            "directory nesting exceeds the safe operation limit",
        ));
    }
    if name.as_bytes().eq_ignore_ascii_case(b".git") {
        return Err(unsupported(path, "Git internals cannot be deleted"));
    }
    let directory = kind(parent, name, path)? == FileType::Directory;
    if directory {
        let child = fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty())
            .map_err(|e| io_error("open directory", path, e.into()))?;
        for entry in directory_names(&child, path)? {
            remove(&child, &entry, &path.join(&entry), depth + 1)?;
        }
    }
    fs::unlinkat(
        parent,
        name,
        if directory {
            AtFlags::REMOVEDIR
        } else {
            AtFlags::empty()
        },
    )
    .map_err(|e| io_error("delete entry", path, e.into()))
}

#[cfg(test)]
mod tests;
