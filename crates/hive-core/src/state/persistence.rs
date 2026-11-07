use super::{ProjectRef, State, hive_dir};
use crate::{host::Host, settings::Settings};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
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

/// Layout version this build writes into `state.json`. Bump it when the meaning of an
/// existing field changes and add the matching upgrade in [`State::load_from`]; fields
/// added with `#[serde(default)]` need no bump. Files without a version are version 0.
pub const SCHEMA_VERSION: u32 = 1;

/// Loaded state and, when the file could not be used as written, what happened to it.
pub struct LoadedState {
    /// State to show in the application.
    pub state: State,
    /// Set when part or all of the file was not used; the UI must report it.
    pub recovery: Option<Recovery>,
}

/// Why `state.json` could not be used as written. The untouched original is at `backup`.
#[derive(Debug, PartialEq, Eq)]
pub struct Recovery {
    /// Byte-for-byte copy of the file as it was found.
    pub backup: PathBuf,
    /// What was lost.
    pub cause: RecoveryCause,
}

/// What a recovery replaced.
#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryCause {
    /// Not a JSON object: everything is default.
    Corrupt,
    /// These parts failed to parse and were reset or dropped; everything else was kept.
    Partial(Vec<String>),
    /// Written by a newer Hive; data this version does not understand survives only in the backup.
    Newer {
        /// The file's [`SCHEMA_VERSION`].
        version: u32,
    },
}

/// `state.json` as written: the version beside the top-level sections of [`State`].
#[derive(Serialize)]
struct Envelope<'a> {
    version: u32,
    #[serde(flatten)]
    state: &'a State,
}

/// `state.json` as read: each section parsed on its own, so one bad section cannot reset
/// the others. Any JSON object parses into this.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Sections {
    version: u32,
    settings: Option<Value>,
    projects: Option<Value>,
    workspace: Option<Value>,
    last_seen_release: Option<Value>,
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
        let sections: Sections = match serde_json::from_slice(&bytes) {
            Ok(sections) => sections,
            Err(error) => {
                let backup = preserve(dir, &bytes, "state.corrupt")?;
                tracing::warn!(?path, ?backup, %error, "preserved corrupt state; using defaults");
                return Ok(LoadedState {
                    state: Self::default(),
                    recovery: Some(Recovery {
                        backup,
                        cause: RecoveryCause::Corrupt,
                    }),
                });
            }
        };
        let mut lost = Vec::new();
        let state = State {
            settings: section("settings", sections.settings, &mut lost),
            projects: projects(sections.projects, &mut lost),
            workspace: section("workspace", sections.workspace, &mut lost),
            last_seen_release: section("last_seen_release", sections.last_seen_release, &mut lost),
        };
        let cause = if sections.version > SCHEMA_VERSION {
            tracing::warn!(
                ?path,
                version = sections.version,
                ?lost,
                "state written by a newer Hive"
            );
            Some(RecoveryCause::Newer {
                version: sections.version,
            })
        } else if lost.is_empty() {
            None
        } else {
            tracing::warn!(?path, ?lost, "parts of the state were unreadable and reset");
            Some(RecoveryCause::Partial(lost))
        };
        let recovery = match cause {
            Some(cause) => Some(Recovery {
                backup: preserve(dir, &bytes, "state.original")?,
                cause,
            }),
            None => None,
        };
        Ok(LoadedState { state, recovery })
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
            recovery: None,
        })
    }

    /// Blocking: durably write a replacement before atomically renaming it over the old file.
    pub fn save(&self) -> Result<(), StateError> {
        self.save_to(&hive_dir().join("state.json"))
    }

    fn save_to(&self, path: &Path) -> Result<(), StateError> {
        let envelope = Envelope {
            version: SCHEMA_VERSION,
            state: self,
        };
        let bytes = serde_json::to_vec_pretty(&envelope).map_err(|source| StateError::Json {
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

/// A top-level section, or its default when it is absent or unreadable (then listed in `lost`).
fn section<T: Default + DeserializeOwned>(
    name: &str,
    value: Option<Value>,
    lost: &mut Vec<String>,
) -> T {
    let Some(value) = value else {
        return T::default();
    };
    serde_json::from_value(value).unwrap_or_else(|error| {
        tracing::warn!(section = name, %error, "unreadable state section reset to defaults");
        lost.push(name.to_owned());
        T::default()
    })
}

/// Registered projects, dropping only the entries that cannot be read; each is listed in `lost`.
fn projects(value: Option<Value>, lost: &mut Vec<String>) -> Vec<ProjectRef> {
    let Some(Value::Array(entries)) = value else {
        if value.is_some() {
            lost.push("projects".to_owned());
        }
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let label = entry
                .get("path")
                .and_then(Value::as_str)
                .map_or_else(|| "project".to_owned(), |path| format!("project {path}"));
            serde_json::from_value(entry)
                .inspect_err(|error| {
                    tracing::warn!(%label, %error, "unreadable project dropped");
                    lost.push(label.clone());
                })
                .ok()
        })
        .collect()
}

/// Writes an exact copy of the file as found, durably, before anything replaces it.
fn preserve(dir: &Path, bytes: &[u8], prefix: &str) -> Result<PathBuf, StateError> {
    let (backup, mut file) = unique_file(dir, prefix, "json")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| io_error("back up state to", &backup, e))?;
    sync_directory(dir)?;
    Ok(backup)
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
    fn old_state_gains_independent_palettes_and_saves_them_without_a_version_bump() {
        use crate::settings::{DarkTheme, LightTheme, ThemeChoice};
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        fs::write(&path, br#"{"version":1,"settings":{"theme":"system","ui_font_size":18},"projects":[{"path":"/saved"}]}"#).unwrap();
        let mut loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.recovery.is_none());
        assert_eq!(loaded.state.settings.light_theme(), LightTheme::Hive);
        assert_eq!(loaded.state.settings.dark_theme(), DarkTheme::Hive);
        loaded.state.settings.set_light_theme(LightTheme::Solarized);
        loaded.state.settings.set_dark_theme(DarkTheme::Dracula);
        loaded.state.save_to(&path).unwrap();
        let reloaded = State::load_from(&fixture.0).unwrap();
        assert!(reloaded.recovery.is_none());
        assert_eq!(reloaded.state.settings.theme, ThemeChoice::System);
        assert_eq!(reloaded.state.settings.light_theme(), LightTheme::Solarized);
        assert_eq!(reloaded.state.settings.dark_theme(), DarkTheme::Dracula);
        assert_eq!(reloaded.state.settings.ui_font_size(), 18.);
        assert_eq!(reloaded.state.projects.len(), 1);
        let saved: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["version"], 1);
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
        let recovery = loaded.recovery.unwrap();
        assert_eq!(recovery.cause, RecoveryCause::Corrupt);
        assert_eq!(fs::read(&recovery.backup).unwrap(), b"{broken\xff");
        assert_eq!(fs::read(&path).unwrap(), b"{broken\xff");
        assert!(loaded.state.projects.is_empty());
        loaded.state.save_to(&path).unwrap();
        assert_eq!(fs::read(recovery.backup).unwrap(), b"{broken\xff");
        assert!(State::load_from(&fixture.0).unwrap().recovery.is_none());
    }

    #[test]
    fn unknown_enum_values_fall_back_without_losing_the_rest() {
        let fixture = Fixture::new();
        fs::write(
            fixture.0.join("state.json"),
            br#"{"settings":{"theme":"neon","sidebar_width":300},
                 "projects":[{"path":"/a","worktrees":"everything","identity":{"badge":{"kind":"hologram"}}}],
                 "workspace":{"projects":[{"project":"/a","layout":{"kind":"split","axis":"diagonal","children":[]}}]}}"#,
        )
        .unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.recovery.is_none());
        let state = loaded.state;
        assert_eq!(state.settings.theme, crate::settings::ThemeChoice::System);
        assert_eq!(state.settings.sidebar_width(), 300.);
        assert_eq!(state.projects.len(), 1);
        assert_eq!(
            state.projects[0].worktrees,
            super::super::WorktreeFilter::Hive
        );
        assert_eq!(state.projects[0].identity, Default::default());
        assert!(matches!(
            state.workspace.projects[0].layout,
            Some(super::super::workspace::Layout::Split {
                axis: super::super::workspace::SplitAxis::Horizontal,
                ..
            })
        ));
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 1);
    }

    #[test]
    fn one_unreadable_project_or_section_is_reset_reported_and_backed_up() {
        let fixture = Fixture::new();
        let original = br#"{"settings":{"sidebar_width":"wide"},
            "projects":[{"path":"/a"},{"path":"/b","host":{"kind":"teleport"}},{"path":"/c"}],
            "workspace":{"projects":[{"project":"/a"},{"project":7}]}}"#;
        fs::write(fixture.0.join("state.json"), original).unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        let paths: Vec<_> = loaded
            .state
            .projects
            .iter()
            .map(|p| p.path.clone())
            .collect();
        assert_eq!(paths, [Path::new("/a"), Path::new("/c")]);
        assert_eq!(
            loaded.state.settings.sidebar_width(),
            State::default().settings.sidebar_width()
        );
        assert_eq!(loaded.state.workspace.projects.len(), 1);
        let recovery = loaded.recovery.unwrap();
        assert_eq!(
            recovery.cause,
            RecoveryCause::Partial(vec!["settings".into(), "project /b".into()])
        );
        assert_eq!(fs::read(&recovery.backup).unwrap(), original);
    }

    #[test]
    fn state_from_a_newer_hive_is_kept_in_a_backup_and_reported() {
        let fixture = Fixture::new();
        let original = br#"{"version":99,"projects":[{"path":"/a","future":1}]}"#;
        fs::write(fixture.0.join("state.json"), original).unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert_eq!(loaded.state.projects.len(), 1);
        let recovery = loaded.recovery.unwrap();
        assert_eq!(recovery.cause, RecoveryCause::Newer { version: 99 });
        assert_eq!(fs::read(&recovery.backup).unwrap(), original);
    }

    #[test]
    fn saved_state_carries_the_schema_version_and_reloads_cleanly() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        State::default().save_to(&path).unwrap();
        let written: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["version"], Value::from(SCHEMA_VERSION));
        assert!(written.get("settings").is_some());
        assert!(State::load_from(&fixture.0).unwrap().recovery.is_none());
    }

    #[test]
    fn release_marker_loads_old_state_and_survives_restart() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        fs::write(
            &path,
            br#"{"version":1,"projects":[{"path":"/a"}],"future":true}"#,
        )
        .unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.recovery.is_none());
        assert!(loaded.state.last_seen_release.is_none());
        let mut state = loaded.state;
        assert!(state.mark_release_seen("0.1.4"));
        state.save_to(&path).unwrap();
        let mut restored = State::load_from(&fixture.0).unwrap().state;
        assert!(!restored.mark_release_seen("0.1.4"));
        assert_eq!(restored.projects[0].path, Path::new("/a"));
    }

    #[test]
    fn malformed_release_marker_preserves_other_state_and_original_bytes() {
        let fixture = Fixture::new();
        let original = br#"{"last_seen_release":42,"projects":[{"path":"/a"}]}"#;
        fs::write(fixture.0.join("state.json"), original).unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.state.last_seen_release.is_none());
        assert_eq!(loaded.state.projects[0].path, Path::new("/a"));
        let recovery = loaded.recovery.unwrap();
        assert_eq!(
            recovery.cause,
            RecoveryCause::Partial(vec!["last_seen_release".into()])
        );
        assert_eq!(fs::read(recovery.backup).unwrap(), original);
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
