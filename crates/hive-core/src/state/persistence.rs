use super::{ProjectRef, State, hive_dir};
use crate::{host::Host, settings::Settings};
use serde::Deserialize;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

/// A persistence failure. A failed load must not be followed by a default-state write.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// The filesystem could not complete an operation.
    #[error("cannot {operation} {path}: {source}")]
    Io {
        /// Operation attempted.
        operation: &'static str,
        /// File or directory involved.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: io::Error,
    },
    /// The legacy file or state being serialized was invalid.
    #[error("invalid state at {path}: {source}")]
    Json {
        /// State file involved.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
}

/// Loaded state and, when recovery was necessary, the preserved original's location.
pub struct LoadedState {
    /// State to show in the application.
    pub state: State,
    /// Exact backup of malformed `state.json`; the UI must report this recovery.
    pub recovered_from: Option<PathBuf>,
}

fn io_error(operation: &'static str, path: &Path, source: io::Error) -> StateError {
    StateError::Io {
        operation,
        path: path.to_owned(),
        source,
    }
}

impl State {
    /// Blocking: load state, migrating only when it is absent and preserving malformed files.
    pub fn load() -> Result<LoadedState, StateError> {
        let mut loaded = Self::load_from(&hive_dir())?;
        loaded.state.workspace.prepare(&loaded.state.projects);
        Ok(loaded)
    }

    fn load_from(dir: &Path) -> Result<LoadedState, StateError> {
        let path = dir.join("state.json");
        let Some(bytes) = read_optional(&path)? else {
            return Self::migrate(dir);
        };
        match serde_json::from_slice(&bytes) {
            Ok(state) => Ok(LoadedState {
                state,
                recovered_from: None,
            }),
            Err(error) => {
                let (backup, mut file) = unique_file(dir, "state.corrupt", "json")?;
                file.write_all(&bytes)
                    .and_then(|_| file.sync_all())
                    .map_err(|e| io_error("back up corrupt state to", &backup, e))?;
                sync_directory(dir)?;
                tracing::warn!(?path, ?backup, %error, "preserved corrupt state; using defaults");
                Ok(LoadedState {
                    state: Self::default(),
                    recovered_from: Some(backup),
                })
            }
        }
    }

    fn migrate(dir: &Path) -> Result<LoadedState, StateError> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Legacy {
            theme: crate::settings::ThemeChoice,
            projects: Vec<PathBuf>,
        }
        let path = dir.join("settings.json");
        let state = match read_optional(&path)? {
            Some(bytes) => {
                let old: Legacy = serde_json::from_slice(&bytes)
                    .map_err(|source| StateError::Json { path, source })?;
                let mut settings = Settings::default();
                settings.theme = old.theme;
                let state = State {
                    settings,
                    projects: old
                        .projects
                        .into_iter()
                        .map(|p| ProjectRef::new(p, Host::Local))
                        .collect(),
                    ..State::default()
                };
                state.save_to(&dir.join("state.json"))?;
                state
            }
            None => State::default(),
        };
        Ok(LoadedState {
            state,
            recovered_from: None,
        })
    }

    /// Blocking: durably write a replacement before atomically renaming it over the old file.
    pub fn save(&self) -> Result<(), StateError> {
        self.save_to(&hive_dir().join("state.json"))
    }

    fn save_to(&self, path: &Path) -> Result<(), StateError> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|source| StateError::Json {
            path: path.to_owned(),
            source,
        })?;
        let dir = path.parent().expect("state path has a parent");
        fs::create_dir_all(dir).map_err(|e| io_error("create directory", dir, e))?;
        let (temporary, mut file) = unique_file(dir, ".state", "tmp")?;
        let result = (|| {
            match fs::metadata(path) {
                Ok(metadata) => file
                    .set_permissions(metadata.permissions())
                    .map_err(|e| io_error("preserve permissions for", &temporary, e))?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error("read permissions for", path, error)),
            }
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| io_error("write", &temporary, e))?;
            fs::rename(&temporary, path).map_err(|e| io_error("replace", path, e))?;
            sync_directory(dir)
        })();
        if result.is_err()
            && let Err(error) = fs::remove_file(&temporary)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(?temporary, %error, "failed to remove temporary state file");
        }
        result
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, StateError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error("read", path, e)),
    }
}

fn unique_file(dir: &Path, prefix: &str, extension: &str) -> Result<(PathBuf, File), StateError> {
    loop {
        let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "{prefix}.{}.{sequence}.{extension}",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_error("create", &path, e)),
        }
    }
}

fn sync_directory(dir: &Path) -> Result<(), StateError> {
    File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(|e| io_error("sync directory", dir, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "hive-state-{}-{}",
                std::process::id(),
                NEXT_FILE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700)).unwrap();
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn corrupt_state_is_preserved_and_never_imports_legacy() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        fs::write(&path, b"{broken\xff").unwrap();
        fs::write(
            fixture.0.join("settings.json"),
            br#"{"projects":["/legacy"]}"#,
        )
        .unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        let backup = loaded.recovered_from.unwrap();
        assert_eq!(fs::read(&backup).unwrap(), b"{broken\xff");
        assert_eq!(fs::read(&path).unwrap(), b"{broken\xff");
        assert!(loaded.state.projects.is_empty());
        loaded.state.save_to(&path).unwrap();
        assert_eq!(fs::read(backup).unwrap(), b"{broken\xff");
        assert!(
            State::load_from(&fixture.0)
                .unwrap()
                .recovered_from
                .is_none()
        );
    }

    #[test]
    fn missing_state_migrates_but_unreadable_state_is_an_error() {
        let fixture = Fixture::new();
        fs::write(
            fixture.0.join("settings.json"),
            br#"{"projects":["/legacy"],"theme":"dark"}"#,
        )
        .unwrap();
        assert_eq!(
            State::load_from(&fixture.0).unwrap().state.projects.len(),
            1
        );
        let path = fixture.0.join("state.json");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            State::load_from(&fixture.0),
            Err(StateError::Io { .. })
        ));
        assert!(path.is_dir());
    }

    #[test]
    fn failed_replace_keeps_destination_and_cleans_temporary_file() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("original"), b"keep").unwrap();
        assert!(State::default().save_to(&path).is_err());
        assert_eq!(fs::read(path.join("original")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn repeated_atomic_writes_keep_last_snapshot_and_accept_unknown_fields() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        fs::write(&path, br#"{"future":true}"#).unwrap();
        let mut state = State::load_from(&fixture.0).unwrap().state;
        for i in 0..100 {
            state.projects = vec![ProjectRef::new(
                PathBuf::from(format!("/project-{i}")),
                Host::Local,
            )];
            state.save_to(&path).unwrap();
        }
        assert_eq!(
            State::load_from(&fixture.0).unwrap().state.projects[0].path,
            Path::new("/project-99")
        );
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn backup_and_write_failures_preserve_original_bytes() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        fs::write(&path, b"broken original").unwrap();
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o500)).unwrap();
        assert!(State::load_from(&fixture.0).is_err());
        assert!(State::default().save_to(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken original");
    }
}
