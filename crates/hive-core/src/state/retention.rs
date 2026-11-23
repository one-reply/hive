//! What earlier launches left in the data folder beside `state.json`: copies of a file that
//! could not be used as written, and temporary files of saves that never finished. They are
//! the user's files, so little is removed, and only by a Hive that may write the folder.

use super::persistence::{StateError, io_error, sync_directory, unique_file};
use super::{Access, hive_dir};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Copies of each kind that stay in the folder; older ones are removed.
const BACKUPS_KEPT: usize = 5;
/// A save writes its temporary file and renames it within milliseconds, so one this old was
/// left by a process that died. A younger one may belong to a save still running: an older
/// Hive takes no lock on the folder.
const TEMPORARY_STALE_AFTER: Duration = Duration::from_secs(60 * 60);
/// Name of a save's temporary file, before the process id and sequence number.
pub(super) const TEMPORARY_PREFIX: &str = ".state";
/// Extension of a save's temporary file.
pub(super) const TEMPORARY_EXTENSION: &str = "tmp";
const BACKUP_EXTENSION: &str = "json";

/// Why a copy of `state.json` was set aside.
#[derive(Clone, Copy, Debug)]
pub(super) enum Backup {
    /// The file was not a JSON object.
    Corrupt,
    /// Parts of the file were unreadable, or a newer Hive wrote it.
    Original,
}

impl Backup {
    const ALL: [Self; 2] = [Self::Corrupt, Self::Original];

    fn prefix(self) -> &'static str {
        match self {
            Self::Corrupt => "state.corrupt",
            Self::Original => "state.original",
        }
    }
}

/// A file Hive numbered with its process id and a sequence, and when it was last written.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Leftover {
    modified: SystemTime,
    path: PathBuf,
}

/// An exact copy of the file as found, durable before anything replaces the file.
///
/// A file nothing rewrites (a newer Hive's, or one this session may not save) is found the
/// same at every launch; the copy the last launch made is then returned instead of another.
pub(super) fn preserve(dir: &Path, bytes: &[u8], kind: Backup) -> Result<PathBuf, StateError> {
    if let Some(newest) = numbered(dir, kind.prefix(), BACKUP_EXTENSION).pop()
        && fs::read(&newest.path).is_ok_and(|kept| kept == bytes)
    {
        return Ok(newest.path);
    }
    let (backup, mut file) = unique_file(dir, kind.prefix(), BACKUP_EXTENSION)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| io_error("back up state to", &backup, e))?;
    sync_directory(dir)?;
    Ok(backup)
}

/// Blocking: removes from the data folder all but the newest copies of an unusable
/// `state.json`, and the temporary files of saves that died long ago. Call it once the state
/// has been read, so the copy this launch reports is among the newest.
///
/// Does nothing for a Hive that only reads the folder: another one holds it, and its files
/// are not this one's to delete.
pub fn tidy(access: &Access) {
    tidy_as(access, &hive_dir(), SystemTime::now());
}

fn tidy_as(access: &Access, dir: &Path, now: SystemTime) {
    if !access.can_write() {
        return;
    }
    for kind in Backup::ALL {
        let copies = numbered(dir, kind.prefix(), BACKUP_EXTENSION);
        let excess = copies.len().saturating_sub(BACKUPS_KEPT);
        for old in &copies[..excess] {
            remove(&old.path, "old state backup");
        }
    }
    for temporary in numbered(dir, TEMPORARY_PREFIX, TEMPORARY_EXTENSION) {
        // A time in the future says nothing about age; the file stays.
        let stale = now
            .duration_since(temporary.modified)
            .is_ok_and(|age| age > TEMPORARY_STALE_AFTER);
        if stale {
            remove(&temporary.path, "temporary file of an unfinished save");
        }
    }
}

fn remove(path: &Path, what: &str) {
    match fs::remove_file(path) {
        Ok(()) => tracing::info!(?path, "removed {what}"),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(?path, %error, "could not remove {what}"),
    }
}

/// The regular files in `dir` named `<prefix>.<process id>.<sequence>.<extension>`, oldest
/// first. Anything else that merely resembles the name (a note the user kept, a folder, a
/// link) is not Hive's and is left out.
fn numbered(dir: &Path, prefix: &str, extension: &str) -> Vec<Leftover> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(?dir, %error, "could not list the data folder");
            }
            return Vec::new();
        }
    };
    let mut found: Vec<Leftover> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| is_numbered(name, prefix, extension))
        })
        .filter_map(|entry| {
            // The entry's own metadata: a symbolic link is not followed.
            let metadata = entry.metadata().ok().filter(fs::Metadata::is_file)?;
            Some(Leftover {
                modified: metadata.modified().ok()?,
                path: entry.path(),
            })
        })
        .collect();
    found.sort();
    found
}

fn is_numbered(name: &str, prefix: &str, extension: &str) -> bool {
    let Some(numbers) = name
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_suffix(extension))
        .and_then(|rest| rest.strip_suffix('.'))
    else {
        return false;
    };
    let is_number = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    numbers
        .split_once('.')
        .is_some_and(|(process, sequence)| is_number(process) && is_number(sequence))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::UNIX_EPOCH;

    const HOUR: u64 = 60 * 60;

    /// A fixed "now": nothing here reads the clock.
    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "hive-retention-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        /// A file last written `age` seconds before [`now`]; a negative age is in the future.
        fn file(&self, name: &str, age: i64) {
            let path = self.0.join(name);
            fs::write(&path, name).unwrap();
            let modified = if age >= 0 {
                now() - Duration::from_secs(age.unsigned_abs())
            } else {
                now() + Duration::from_secs(age.unsigned_abs())
            };
            fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<_> = fs::read_dir(&self.0)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_the_newest_copies_of_each_kind_are_kept() {
        let folder = Folder::new();
        // Ages do not follow the names: a process id says nothing about when it ran.
        for (name, hours) in [
            ("state.original.900.0.json", 8),
            ("state.original.100.0.json", 1),
            ("state.original.100.1.json", 7),
            ("state.original.500.0.json", 2),
            ("state.original.300.0.json", 6),
            ("state.original.700.0.json", 3),
            ("state.original.200.0.json", 5),
            ("state.original.800.0.json", 4),
            ("state.corrupt.900.0.json", 9),
            ("state.corrupt.100.0.json", 1),
        ] {
            folder.file(name, hours * HOUR as i64);
        }
        let others = [
            "editor-recovery.json",
            "settings.json",
            "state.json",
            "state.lock",
            // Named almost like a copy, and older than every copy above.
            "state.original.1.2.json.bak",
            "state.original.notes.json",
            "state.original.x.1.json",
            "state.originals.1.2.json",
        ];
        for name in others {
            folder.file(name, 100 * HOUR as i64);
        }
        fs::create_dir(folder.0.join("state.original.1.1.json")).unwrap();
        std::os::unix::fs::symlink(
            folder.0.join("state.json"),
            folder.0.join("state.original.2.2.json"),
        )
        .unwrap();

        tidy_as(&Access::Unguarded, &folder.0, now());

        let mut expected: Vec<String> = [
            "state.corrupt.100.0.json",
            "state.corrupt.900.0.json",
            "state.original.1.1.json",
            "state.original.2.2.json",
            "state.original.100.0.json",
            "state.original.200.0.json",
            "state.original.500.0.json",
            "state.original.700.0.json",
            "state.original.800.0.json",
        ]
        .into_iter()
        .chain(others)
        .map(String::from)
        .collect();
        expected.sort();
        assert_eq!(folder.names(), expected);
    }

    #[test]
    fn a_temporary_file_is_removed_only_once_no_save_can_still_be_writing_it() {
        let folder = Folder::new();
        let stale = TEMPORARY_STALE_AFTER.as_secs() as i64;
        folder.file(".state.11.0.tmp", 2 * stale);
        folder.file(".state.11.1.tmp", stale + 1);
        // A save that may still be running, in this Hive or an older one without the lock.
        folder.file(".state.12.0.tmp", 0);
        folder.file(".state.12.1.tmp", stale);
        folder.file(".state.13.0.tmp", -stale);
        // Not a save's file, however old.
        folder.file(".state.notes.tmp", 100 * stale);
        folder.file(".statement.1.2.tmp", 100 * stale);
        folder.file("state.json", 100 * stale);

        tidy_as(&Access::Unguarded, &folder.0, now());

        assert_eq!(
            folder.names(),
            [
                ".state.12.0.tmp",
                ".state.12.1.tmp",
                ".state.13.0.tmp",
                ".state.notes.tmp",
                ".statement.1.2.tmp",
                "state.json",
            ]
        );
    }

    #[test]
    fn a_hive_that_only_reads_the_folder_removes_nothing() {
        let folder = Folder::new();
        for sequence in 0..8 {
            folder.file(
                &format!("state.original.1.{sequence}.json"),
                sequence * HOUR as i64,
            );
        }
        folder.file(".state.1.0.tmp", 100 * HOUR as i64);
        let before = folder.names();

        tidy_as(&Access::Held, &folder.0, now());
        assert_eq!(folder.names(), before);

        tidy_as(&Access::Unguarded, &folder.0, now());
        assert_eq!(folder.names().len(), BACKUPS_KEPT);
    }

    #[test]
    fn only_the_newest_copy_stands_in_for_a_new_one() {
        let folder = Folder::new();
        folder.file("state.original.1.0.json", 2 * HOUR as i64);
        folder.file("state.original.1.1.json", HOUR as i64);
        // The newest copy holds these bytes: no third file.
        let reused = preserve(&folder.0, b"state.original.1.1.json", Backup::Original).unwrap();
        assert_eq!(reused, folder.0.join("state.original.1.1.json"));
        assert_eq!(folder.names().len(), 2);
        // An older copy with the same bytes could be pruned at this launch, so it is not
        // what the notice points to: a new copy is written.
        let written = preserve(&folder.0, b"state.original.1.0.json", Backup::Original).unwrap();
        assert_eq!(fs::read(&written).unwrap(), b"state.original.1.0.json");
        assert_eq!(folder.names().len(), 3);
        // A copy of the other kind never stands in.
        let corrupt = preserve(&folder.0, b"state.original.1.1.json", Backup::Corrupt).unwrap();
        assert!(
            corrupt
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("state.corrupt."),
            "{corrupt:?}"
        );
    }

    #[test]
    fn a_folder_that_is_missing_has_nothing_to_remove() {
        let folder = Folder::new();
        tidy_as(&Access::Unguarded, &folder.0.join("absent"), now());
        assert!(folder.names().is_empty());
    }
}
