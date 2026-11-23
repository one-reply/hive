//! Paste: copy entries from anywhere on this Mac into a folder of a local worktree, as Finder
//! does. Nothing at the destination is ever replaced: a taken name gets Finder's " copy" suffix.
//! Sources are read through directory handles without following symlinks below the pasted entry,
//! so a link inside a copied folder is recreated as a link, never followed out of it.
use super::{DIRECTORY_FLAGS, MAX_DEPTH, directory_names, entry_path, parent, remove};
use crate::files::{FileError, io_error, unsupported};
use crate::{command::Cancellation, host::Host};
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, FileType, Mode, OFlags},
    io::Errno,
};
use std::ffi::{CString, OsStr};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Bytes moved per read, between cancellation checks.
const COPY_BUFFER: usize = 256 * 1024;
/// Names tried for one entry before giving up rather than scanning a folder of copies forever.
const MAX_ATTEMPTS: usize = 1000;

/// The name of the `attempt`-th try at pasting `name`: the name itself first, then Finder's
/// `report copy.pdf`, `report copy 2.pdf`, … A file keeps its last extension at the end; a
/// folder, or a name whose only dot leads (`.env`) or trails, gets the suffix at the end.
pub fn paste_name(name: &str, directory: bool, attempt: usize) -> String {
    let suffix = match attempt {
        0 => return name.to_owned(),
        1 => " copy".to_owned(),
        attempt => format!(" copy {attempt}"),
    };
    let extension = name
        .rfind('.')
        .filter(|&dot| !directory && dot > 0 && dot + 1 < name.len());
    match extension {
        Some(dot) => format!("{}{suffix}{}", &name[..dot], &name[dot..]),
        None => format!("{name}{suffix}"),
    }
}

/// Copy `sources`, absolute paths on this Mac, into `directory` of a local worktree.
///
/// Returns the created entries relative to `root`, in the order of `sources`. Every entry is
/// created exclusively; one whose name the volume reports as taken (on APFS that ignores case
/// and Unicode form) is pasted as the next free [`paste_name`]. An entry that fails is removed
/// again, while entries pasted before it stay and the error says how many there are. Folders
/// containing a Git repository, a folder pasted inside itself, and special files (sockets,
/// devices, FIFOs) are refused.
pub fn paste_entries(
    host: &Host,
    root: &Path,
    directory: &Path,
    sources: &[PathBuf],
    cancellation: &Cancellation,
) -> Result<Vec<PathBuf>, FileError> {
    if !host.is_local() {
        return Err(unsupported(
            root,
            "pasting files from this Mac works only in local worktrees",
        ));
    }
    let destination = open_destination(host, root, directory)?;
    let destination_path = root.join(directory);
    let canonical_destination = std::fs::canonicalize(&destination_path)
        .map_err(|e| io_error("resolve", &destination_path, e))?;
    let folder = Folder {
        handle: &destination,
        path: &destination_path,
        canonical: &canonical_destination,
    };
    let mut pasted = Vec::with_capacity(sources.len());
    for source in sources {
        match paste_one(source, &folder, cancellation) {
            Ok(name) => pasted.push(directory.join(name)),
            Err(error) if pasted.is_empty() => return Err(error),
            Err(error) => {
                return Err(FileError::PartialPaste {
                    path: destination_path,
                    pasted: pasted.len(),
                    total: sources.len(),
                    source: Box::new(error),
                });
            }
        }
    }
    Ok(pasted)
}

/// The open destination folder of one paste.
struct Folder<'a> {
    handle: &'a OwnedFd,
    path: &'a Path,
    canonical: &'a Path,
}

fn open_destination(host: &Host, root: &Path, directory: &Path) -> Result<OwnedFd, FileError> {
    if directory.as_os_str().is_empty() {
        if !root.is_absolute() {
            return Err(unsupported(
                root,
                "file operations require an absolute local worktree",
            ));
        }
        return fs::open(root, DIRECTORY_FLAGS, Mode::empty())
            .map_err(|e| io_error("open worktree", root, e.into()));
    }
    let (parent, name) = parent(host, root, directory)?;
    fs::openat(&parent, &name, DIRECTORY_FLAGS, Mode::empty())
        .map_err(|e| io_error("open directory", &root.join(directory), e.into()))
}

/// Paste one source under the first free name and return that name.
fn paste_one(
    source: &Path,
    folder: &Folder,
    cancellation: &Cancellation,
) -> Result<String, FileError> {
    check_cancelled(source, cancellation)?;
    if !source.is_absolute() {
        return Err(unsupported(source, "a pasted path must be absolute"));
    }
    let name = source
        .file_name()
        .ok_or_else(|| unsupported(source, "a volume or root folder cannot be pasted"))?;
    let name = name
        .to_str()
        .ok_or_else(|| unsupported(source, "the name is not valid UTF-8"))?;
    entry_path(Path::new(""), name)?;
    let source_parent = source.parent().unwrap_or(Path::new("/"));
    // Finder paths may pass through links such as /tmp; only the pasted entry is not followed.
    let source_parent = fs::open(
        source_parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| io_error("open folder", source_parent, e.into()))?;
    let entry = Source::open(&source_parent, OsStr::new(name), source)?;
    let directory = matches!(entry, Source::Directory { .. });
    if directory {
        let canonical =
            std::fs::canonicalize(source).map_err(|e| io_error("resolve", source, e))?;
        if folder.canonical.starts_with(&canonical) {
            return Err(unsupported(
                source,
                "a folder cannot be pasted inside itself",
            ));
        }
    }
    for attempt in 0..MAX_ATTEMPTS {
        let candidate = paste_name(name, directory, attempt);
        let target = folder.path.join(&candidate);
        // The volume decides what is taken: a listing would miss an entry created a moment
        // ago and the volume's own rules for case and Unicode form.
        let created = match entry.create(folder.handle, OsStr::new(&candidate)) {
            Ok(created) => created,
            Err(Errno::EXIST) => continue,
            Err(error) => return Err(io_error("create", &target, error.into())),
        };
        if let Err(error) = entry.fill(created, source, &target, 0, cancellation) {
            discard(folder.handle, &candidate, &target);
            return Err(error);
        }
        return Ok(candidate);
    }
    Err(unsupported(
        source,
        "too many copies with this name already exist in the folder",
    ))
}

/// Remove an entry this paste created and could not finish. Its data came from elsewhere, so a
/// leftover is only clutter: log it rather than hide the copy error behind it.
fn discard(folder: &OwnedFd, name: &str, path: &Path) {
    if let Err(error) = remove(folder, OsStr::new(name), path, 0) {
        tracing::warn!(%error, ?path, "could not remove an unfinished pasted entry");
    }
}

fn check_cancelled(path: &Path, cancellation: &Cancellation) -> Result<(), FileError> {
    if cancellation.is_cancelled() {
        return Err(io_error(
            "copy",
            path,
            std::io::ErrorKind::Interrupted.into(),
        ));
    }
    Ok(())
}

/// One entry to copy, opened before anything is created at the destination.
enum Source {
    File { handle: OwnedFd, mode: Mode },
    Directory { handle: OwnedFd, mode: Mode },
    Link(CString),
}

impl Source {
    fn open(parent: &OwnedFd, name: &OsStr, path: &Path) -> Result<Self, FileError> {
        let stat = fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|e| io_error("inspect", path, e.into()))?;
        let mode = Mode::from_raw_mode(stat.st_mode & 0o777);
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::RegularFile => Self::open_file(parent, name, path, mode),
            FileType::Directory => fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty())
                .map(|handle| Self::Directory { handle, mode })
                .map_err(|e| io_error("open directory", path, e.into())),
            FileType::Symlink => fs::readlinkat(parent, name, Vec::new())
                .map(Self::Link)
                .map_err(|e| io_error("read link", path, e.into())),
            _ => Err(special_file(path)),
        }
    }

    fn open_file(
        parent: &OwnedFd,
        name: &OsStr,
        path: &Path,
        mode: Mode,
    ) -> Result<Self, FileError> {
        // Non-blocking: a FIFO put in the file's place since it was inspected would otherwise
        // hold this open until someone writes to it. It changes nothing for a regular file.
        let handle = fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| io_error("open", path, e.into()))?;
        let opened = fs::fstat(&handle).map_err(|e| io_error("inspect", path, e.into()))?;
        if FileType::from_raw_mode(opened.st_mode) != FileType::RegularFile {
            return Err(special_file(path));
        }
        Ok(Self::File { handle, mode })
    }

    /// Create the entry exclusively; `EEXIST` means the name is taken.
    fn create(&self, folder: &OwnedFd, name: &OsStr) -> Result<Option<OwnedFd>, Errno> {
        match self {
            Self::File { mode, .. } => fs::openat(
                folder,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                *mode,
            )
            .map(Some),
            Self::Directory { .. } => {
                // Owner-only until filled, so a read-only source folder can still be filled.
                fs::mkdirat(folder, name, Mode::RWXU)?;
                fs::openat(folder, name, DIRECTORY_FLAGS, Mode::empty())
                    .map(Some)
                    .inspect_err(|_| {
                        // Still empty and ours; the open error is what the caller reports.
                        let _ = fs::unlinkat(folder, name, AtFlags::REMOVEDIR);
                    })
            }
            Self::Link(target) => fs::symlinkat(target.as_c_str(), folder, name).map(|()| None),
        }
    }

    /// Copy content into what `create` returned.
    fn fill(
        self,
        created: Option<OwnedFd>,
        path: &Path,
        target: &Path,
        depth: usize,
        cancellation: &Cancellation,
    ) -> Result<(), FileError> {
        const CREATED: &str = "files and folders are created with a handle";
        match self {
            Self::File { handle, .. } => {
                copy_bytes(handle, created.expect(CREATED), path, target, cancellation)
            }
            Self::Directory { handle, mode } => {
                let created = created.expect(CREATED);
                copy_children(&handle, path, &created, target, depth + 1, cancellation)?;
                fs::fchmod(&created, mode)
                    .map_err(|e| io_error("set permissions", target, e.into()))
            }
            Self::Link(_) => Ok(()),
        }
    }
}

fn special_file(path: &Path) -> FileError {
    unsupported(path, "only files, folders and symbolic links can be pasted")
}

fn copy_children(
    source: &OwnedFd,
    path: &Path,
    folder: &OwnedFd,
    target: &Path,
    depth: usize,
    cancellation: &Cancellation,
) -> Result<(), FileError> {
    if depth >= MAX_DEPTH {
        return Err(unsupported(
            path,
            "directory nesting exceeds the safe operation limit",
        ));
    }
    for name in directory_names(source, path)? {
        let (child_path, child_target) = (path.join(&name), target.join(&name));
        check_cancelled(&child_path, cancellation)?;
        if name.as_bytes().eq_ignore_ascii_case(b".git") {
            return Err(unsupported(
                path,
                "contains a nested Git repository; copy it separately",
            ));
        }
        let entry = Source::open(source, &name, &child_path)?;
        let created = entry
            .create(folder, &name)
            .map_err(|e| io_error("create", &child_target, e.into()))?;
        entry.fill(created, &child_path, &child_target, depth, cancellation)?;
    }
    Ok(())
}

fn copy_bytes(
    source: OwnedFd,
    target: OwnedFd,
    path: &Path,
    target_path: &Path,
    cancellation: &Cancellation,
) -> Result<(), FileError> {
    let mut reader = std::fs::File::from(source);
    let mut writer = std::fs::File::from(target);
    let mut buffer = vec![0; COPY_BUFFER];
    loop {
        check_cancelled(path, cancellation)?;
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error("read", path, error)),
        };
        writer
            .write_all(&buffer[..read])
            .map_err(|e| io_error("write", target_path, e))?;
    }
}

#[cfg(test)]
mod tests;
