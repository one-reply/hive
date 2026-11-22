//! One writer per data folder. Two Hive processes on the same `~/.hive` would each save their
//! own snapshot over the other's, so the first one locks the folder and a second one only reads.

use super::hive_dir;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Lock file beside `state.json`. A rename replaces `state.json`, so it cannot carry the lock.
const LOCK_FILE: &str = "state.lock";

/// Whether this process may write the data folder.
#[derive(Debug)]
pub enum Access {
    /// This process holds the folder's lock until the value is dropped.
    Exclusive(Lock),
    /// The filesystem cannot lock files (some network volumes); writes go ahead unguarded,
    /// as they did before locking existed.
    Unguarded,
    /// Another Hive holds the lock: writing would overwrite its changes.
    Held,
}

/// An advisory lock on the data folder, released when the process exits or this is dropped.
#[derive(Debug)]
pub struct Lock {
    _file: File,
}

impl Access {
    /// Blocking: lock the application data folder for this process, without waiting.
    pub fn acquire() -> Self {
        Self::acquire_in(&hive_dir())
    }

    fn acquire_in(dir: &Path) -> Self {
        let path = dir.join(LOCK_FILE);
        let file = fs::create_dir_all(dir).and_then(|()| {
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .mode(0o600)
                .open(&path)
        });
        let file = match file {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!(?path, %error, "cannot open the data folder lock; saving unguarded");
                return Self::Unguarded;
            }
        };
        match file.try_lock() {
            Ok(()) => Self::Exclusive(Lock { _file: file }),
            Err(TryLockError::WouldBlock) => {
                tracing::warn!(
                    ?path,
                    "another Hive holds the data folder; saving is disabled"
                );
                Self::Held
            }
            Err(TryLockError::Error(error)) => {
                tracing::warn!(?path, %error, "cannot lock the data folder; saving unguarded");
                Self::Unguarded
            }
        }
    }

    /// Whether this process may save `state.json`.
    pub fn can_write(&self) -> bool {
        !matches!(self, Self::Held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "hive-access-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn a_second_hive_on_the_same_folder_may_only_read() {
        let dir = scratch();
        let first = Access::acquire_in(&dir);
        assert!(matches!(first, Access::Exclusive(_)));
        assert!(first.can_write());
        // flock belongs to the open file description, so a second open conflicts even within
        // one process, exactly as a second Hive process would. Release on drop is not checked:
        // a test elsewhere in this process may fork in between, and the child shares the
        // descriptor until it execs.
        let second = Access::acquire_in(&dir);
        assert!(matches!(second, Access::Held));
        assert!(!second.can_write());
        drop(first);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_folder_that_cannot_hold_a_lock_file_is_saved_unguarded() {
        let dir = scratch();
        fs::create_dir_all(&dir).unwrap();
        fs::create_dir(dir.join(LOCK_FILE)).unwrap();
        let access = Access::acquire_in(&dir);
        assert!(matches!(access, Access::Unguarded));
        assert!(access.can_write());
        fs::remove_dir_all(&dir).unwrap();
    }
}
