//! File access with explicit cancellation owned by the caller's background task.
use super::{Entry, FileError, FileIndex, Snapshot};
use crate::{command::Cancellation, host::Host};
use std::path::Path;

/// A borrowed host and cancellation token for one sequence of filesystem operations.
pub struct Access<'a> {
    host: &'a Host,
    cancellation: &'a Cancellation,
}
impl<'a> Access<'a> {
    /// Bind operations to a host and the lifetime of their owning task.
    pub fn new(host: &'a Host, cancellation: &'a Cancellation) -> Self {
        Self { host, cancellation }
    }
    /// Read one text snapshot.
    pub fn read(&self, root: &Path, path: &Path) -> Result<Snapshot, FileError> {
        if self.host.is_local() {
            super::read(self.host, root, path)
        } else {
            crate::remote::read(self.host, root, path, self.cancellation)
        }
    }
    /// Read encoded image bytes for local decoding.
    pub fn image(&self, root: &Path, path: &Path) -> Result<Vec<u8>, FileError> {
        if self.host.is_local() {
            super::images::read(self.host, root, path)
        } else {
            crate::remote::image(self.host, root, path, self.cancellation)
        }
    }
    /// List a directory without following symlinks.
    pub fn list_directory(&self, root: &Path, path: &Path) -> Result<Vec<Entry>, FileError> {
        if self.host.is_local() {
            super::list_directory(self.host, root, path)
        } else {
            crate::remote::list_directory(self.host, root, path, self.cancellation)
        }
    }
    /// Index files for search.
    pub fn list_files(&self, root: &Path) -> Result<FileIndex, FileError> {
        if self.host.is_local() {
            super::list_files(self.host, root)
        } else {
            crate::remote::list_files(self.host, root, self.cancellation)
        }
    }
    /// Save conditionally against the last accepted snapshot.
    pub fn save(
        &self,
        root: &Path,
        path: &Path,
        expected: &Snapshot,
        text: &str,
    ) -> Result<Snapshot, FileError> {
        if self.host.is_local() {
            super::save(self.host, root, path, expected, text)
        } else {
            crate::remote::save(self.host, root, path, expected, text, self.cancellation)
        }
    }
    /// Create a file without overwriting any entry.
    pub fn create_file(&self, root: &Path, path: &Path) -> Result<(), FileError> {
        if self.host.is_local() {
            super::create_file(self.host, root, path)
        } else {
            crate::remote::mutate(
                self.host,
                root,
                path,
                crate::remote::Mutation::CreateFile,
                self.cancellation,
            )
        }
    }
    /// Create a new directory without overwriting any entry.
    pub fn create_directory(&self, root: &Path, path: &Path) -> Result<(), FileError> {
        if self.host.is_local() {
            super::create_directory(self.host, root, path)
        } else {
            crate::remote::mutate(
                self.host,
                root,
                path,
                crate::remote::Mutation::CreateDirectory,
                self.cancellation,
            )
        }
    }
    /// Delete an entry with the shared symlink and partial-delete rules.
    pub fn delete_entry(&self, root: &Path, path: &Path) -> Result<(), FileError> {
        if self.host.is_local() {
            super::delete_entry(self.host, root, path)
        } else {
            crate::remote::mutate(
                self.host,
                root,
                path,
                crate::remote::Mutation::Delete,
                self.cancellation,
            )
        }
    }
    /// Move without overwriting the destination.
    pub fn move_entry(&self, root: &Path, path: &Path, to: &Path) -> Result<(), FileError> {
        if self.host.is_local() {
            super::move_entry(self.host, root, path, to)
        } else {
            crate::remote::mutate(
                self.host,
                root,
                path,
                crate::remote::Mutation::Move { to: to.into() },
                self.cancellation,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_reaches_every_remote_file_operation_before_spawning() {
        let host = Host::Ssh {
            target: "cancelled-test-host".into(),
        };
        let cancellation = Cancellation::default();
        cancellation.cancel();
        let access = Access::new(&host, &cancellation);
        let root = Path::new("/unused");
        let path = Path::new("a");
        let base = Snapshot::parse(path, b"base").unwrap();
        let results = [
            access.read(root, path).map(|_| ()),
            access.image(root, path).map(|_| ()),
            access.list_directory(root, path).map(|_| ()),
            access.list_files(root).map(|_| ()),
            access.save(root, path, &base, "new").map(|_| ()),
            access.create_file(root, path),
            access.create_directory(root, path),
            access.move_entry(root, path, Path::new("b")),
            access.delete_entry(root, path),
        ];
        for result in results {
            assert!(result.unwrap_err().to_string().contains("cancelled"));
        }
    }
}
