use super::retention::{Backup, TEMPORARY_EXTENSION, TEMPORARY_PREFIX, preserve};
use super::{ProjectRef, State, hive_dir, lenient};
use crate::{host::Host, settings::Settings};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};
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

/// Version 2 protects pipeline history on downgrade: v1 preserves newer files before rewriting them.
/// Version 3 does the same for the pipeline prompt library and the stage links into it.
/// Version 4 marks a change of meaning: a run's required checks now precede its first review,
/// so a build that ran them after it would take a saved `commands` phase for a finished
/// review and skip that review.
/// Version 5 adds immutable review trees, targeted feedback and direct-to-development runs.
/// Older and unversioned files load with defaults for any missing sections.
/// Version 6 records criterion decisions, shared contracts and regression checkpoints.
pub const SCHEMA_VERSION: u32 = 6;

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
    /// Written by a newer Hive. Saving would rewrite it in this version's schema and lose what
    /// this version does not understand, so the application must not save it.
    Newer {
        /// The file's [`SCHEMA_VERSION`].
        version: u32,
    },
}

/// `state.json` as written: the version beside the top-level sections of [`State`], then the
/// sections this version does not know, as they were read.
#[derive(Serialize)]
struct Envelope<'a> {
    version: u32,
    #[serde(flatten)]
    state: &'a State,
    #[serde(flatten)]
    unknown: &'a Map<String, Value>,
}

pub(super) fn io_error(operation: &'static str, path: &Path, source: io::Error) -> StateError {
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
        // Each section is parsed on its own, so one bad section cannot reset the others. Only a
        // JSON object is a state file: a struct would also read from an array, by position.
        let Ok(Value::Object(mut file)) = serde_json::from_slice(&bytes) else {
            let backup = preserve(dir, &bytes, Backup::Corrupt)?;
            tracing::warn!(?path, ?backup, "preserved corrupt state; using defaults");
            return Ok(LoadedState {
                state: Self::default(),
                recovery: Some(Recovery {
                    backup,
                    cause: RecoveryCause::Corrupt,
                }),
            });
        };
        let mut lost = Vec::new();
        // `null` is how some tools write "nothing here"; it is an absent section, not a bad one.
        let mut take = |name: &str| file.remove(name).filter(|value| !value.is_null());
        let version = version(take("version"), &mut lost);
        let state = State {
            settings: lenient::fields("settings", take("settings"), &mut lost),
            projects: projects(take("projects"), &mut lost),
            workspace: lenient::fields("workspace", take("workspace"), &mut lost),
            pipelines: pipelines(take("pipelines"), &mut lost),
            last_seen_release: section("last_seen_release", take("last_seen_release"), &mut lost),
            reviewed: reviewed(take("reviewed"), &mut lost),
            // What is left is what a newer Hive added without a version bump.
            unknown_sections: file,
        };
        let cause = if version > SCHEMA_VERSION {
            tracing::warn!(?path, version, ?lost, "state written by a newer Hive");
            Some(RecoveryCause::Newer { version })
        } else if lost.is_empty() {
            None
        } else {
            tracing::warn!(?path, ?lost, "parts of the state were unreadable and reset");
            Some(RecoveryCause::Partial(lost))
        };
        let recovery = match cause {
            Some(cause) => Some(Recovery {
                backup: preserve(dir, &bytes, Backup::Original)?,
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
            unknown: &self.unknown_sections,
        };
        let bytes = serde_json::to_vec_pretty(&envelope).map_err(|source| StateError::Json {
            path: path.to_owned(),
            source,
        })?;
        let dir = path.parent().expect("state path has a parent");
        fs::create_dir_all(dir).map_err(|e| io_error("create directory", dir, e))?;
        let (temporary, mut file) = unique_file(dir, TEMPORARY_PREFIX, TEMPORARY_EXTENSION)?;
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

/// Retain independent runs and templates when a sibling checkpoint cannot be recovered.
fn pipelines(value: Option<Value>, lost: &mut Vec<String>) -> crate::pipeline::Library {
    use crate::pipeline::{
        CATALOG_VERSION, Library, Prompt, Run, Template, link_stock_prompts,
        refresh_stock_instructions, stock_prompts,
    };
    use std::collections::HashSet;
    let Some(value) = value else {
        return Library::default();
    };
    let Some(object) = value.as_object() else {
        lost.push("pipelines".into());
        return Library::default();
    };
    fn items<T: serde::de::DeserializeOwned>(
        value: Option<&Value>,
        name: &str,
        lost: &mut Vec<String>,
        mut valid: impl FnMut(&T) -> bool,
    ) -> Vec<T> {
        let Some(value) = value else {
            return vec![];
        };
        let Some(array) = value.as_array() else {
            lost.push(name.into());
            return vec![];
        };
        array
            .iter()
            .enumerate()
            .filter_map(|(index, v)| {
                let parsed = serde_json::from_value::<T>(v.clone())
                    .ok()
                    .filter(&mut valid);
                if parsed.is_none() {
                    lost.push(format!("{name}[{index}]"));
                }
                parsed
            })
            .collect()
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    let mut templates = if object.contains_key("templates") {
        items(
            object.get("templates"),
            "pipelines.templates",
            lost,
            |t: &Template| {
                t.validate_structure().is_ok() && names.insert(t.name.to_ascii_lowercase())
            },
        )
    } else {
        Library::default().templates
    };
    let runs = items(object.get("runs"), "pipelines.runs", lost, |r: &Run| {
        r.valid_snapshot() && ids.insert(r.id())
    });
    let catalog_version = object
        .get("catalog_version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if catalog_version == 0 {
        for starter in Template::presets().into_iter().skip(1) {
            if !templates
                .iter()
                .any(|t| t.name.eq_ignore_ascii_case(&starter.name))
            {
                templates.push(starter);
            }
        }
    }
    if catalog_version < 2 {
        refresh_stock_instructions(&mut templates);
    }
    let mut prompt_ids = HashSet::new();
    let mut prompt_names = HashSet::new();
    // A library saved before prompts existed, or whose list is unreadable, starts from the
    // built-in prompts. A stored list is the user's and stays as it is, even when empty.
    let prompts = match object.get("prompts") {
        Some(stored) if stored.is_array() => {
            items(Some(stored), "pipelines.prompts", lost, |p: &Prompt| {
                p.is_complete()
                    && prompt_ids.insert(p.id)
                    && prompt_names.insert(p.name.to_ascii_lowercase())
            })
        }
        stored => {
            if stored.is_some() {
                lost.push("pipelines.prompts".into());
            }
            link_stock_prompts(&mut templates);
            stock_prompts()
        }
    };
    let mut draft_owners = HashSet::new();
    let drafts = items(
        object.get("drafts"),
        "pipelines.drafts",
        lost,
        |d: &crate::pipeline::draft::LaunchDraft| {
            d.owner.is_absolute()
                && d.template.validate_structure().is_ok()
                && draft_owners.insert(d.owner.clone())
        },
    );
    let mut library = Library {
        drafts,
        templates,
        prompts,
        runs,
        catalog_version: catalog_version.max(CATALOG_VERSION),
    };
    // A stage whose prompt was dropped above keeps its mirrored text as its own instructions.
    if catalog_version < 3 {
        library.upgrade_shared_contract();
    }
    library.sync_prompts();
    library
}

/// The file's schema version; absent means it predates versions. A number written as text is
/// read as the number: a hand edit must not cost the whole file. Anything else is reported and
/// the file is read as this version's own, which is what an unversioned file gets.
fn version(value: Option<Value>, lost: &mut Vec<String>) -> u32 {
    let version = match &value {
        None => Some(0),
        Some(Value::Number(number)) => number.as_u64(),
        Some(Value::String(text)) => text.trim().parse().ok(),
        Some(_) => None,
    };
    match version {
        // A version past `u32` is still a newer one.
        Some(version) => u32::try_from(version).unwrap_or(u32::MAX),
        None => {
            tracing::warn!(?value, "unreadable state version");
            lost.push("version".to_owned());
            0
        }
    }
}

/// A top-level value that is not a group of fields, or its default when absent or unreadable.
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

/// Review marks, dropping only the ones that cannot be read. A mark is one click to restore;
/// losing the whole list, with a recovery notice, for one of them is not proportionate.
fn reviewed(value: Option<Value>, lost: &mut Vec<String>) -> Vec<crate::git::reviewed::Revision> {
    let Some(value) = value else {
        return Vec::new();
    };
    lenient::elements(value).unwrap_or_else(|error: serde_json::Error| {
        tracing::warn!(section = "reviewed", %error, "unreadable state section reset to defaults");
        lost.push("reviewed".to_owned());
        Vec::new()
    })
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, StateError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error("read", path, e)),
    }
}

pub(super) fn unique_file(
    dir: &Path,
    prefix: &str,
    extension: &str,
) -> Result<(PathBuf, File), StateError> {
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

pub(super) fn sync_directory(dir: &Path) -> Result<(), StateError> {
    File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(|e| io_error("sync directory", dir, e))
}

#[cfg(test)]
mod compatibility_tests;

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
    fn old_state_gains_independent_palettes_and_saves_them_in_the_current_schema() {
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
        assert_eq!(saved["version"], SCHEMA_VERSION);
    }

    #[test]
    fn pipeline_library_migrates_old_state_and_preserves_damaged_run_records() {
        use crate::pipeline::{Lane, Library, Run, Template};
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        fs::write(&path, br#"{"version":1,"projects":[{"path":"/api"}]}"#).unwrap();
        let mut state = State::load_from(&fixture.0).unwrap().state;
        assert_eq!(state.pipelines, Library::default());
        let run = Run::new(
            17,
            "/api".into(),
            "Test".into(),
            Template::default(),
            vec![Lane {
                name: "Development".into(),
                project: "/api".into(),
                base: "HEAD".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        state.pipelines.runs.push(run.clone());
        state.save_to(&path).unwrap();
        let mut saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], SCHEMA_VERSION);
        saved["version"] = 1.into();
        fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let previous = State::load_from(&fixture.0).unwrap();
        assert!(previous.recovery.is_none());
        assert_eq!(previous.state.pipelines, state.pipelines);
        previous.state.save_to(&path).unwrap();
        saved = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], SCHEMA_VERSION);
        saved["pipelines"]["unknown_future_field"] = 42.into();
        let mut malformed = saved["pipelines"]["runs"][0].clone();
        malformed["id"] = 18.into();
        malformed["phase"] = serde_json::json!({"review":9000});
        saved["pipelines"]["runs"]
            .as_array_mut()
            .unwrap()
            .push(malformed);
        let original = serde_json::to_vec(&saved).unwrap();
        fs::write(&path, &original).unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert_eq!(loaded.state.pipelines.runs, [run]);
        assert_eq!(loaded.state.projects.len(), 1);
        let recovery = loaded.recovery.unwrap();
        assert_eq!(
            recovery.cause,
            RecoveryCause::Partial(vec!["pipelines.runs[1]".into()])
        );
        assert_eq!(fs::read(recovery.backup).unwrap(), original);
    }

    #[test]
    fn catalog_version_one_recipes_receive_current_stock_instructions_once() {
        use crate::pipeline::Template;
        let step = |name: &str, instruction: &str| serde_json::json!({"name": name, "instruction": instruction, "profile": {"agent": "Codex"}});
        let saved = serde_json::json!({
            "catalog_version": 1,
            "templates": [{
                "name": "Quick fix",
                "planner": step("Plan", "Identify the root cause, the smallest complete fix and a regression check."),
                "development": step("Development", "Implement the approved assignment in your worktree. Report what changed and what you verified."),
                "reviews": [step("QA", "Custom QA")],
                "lanes": [{"name": "Development"}]
            }]
        });
        let mut lost = vec![];
        let library = pipelines(Some(saved), &mut lost);
        assert!(lost.is_empty(), "{lost:?}");
        let fix = &Template::presets()[1];
        assert_eq!(library.templates[0].planner, fix.planner);
        assert_eq!(library.templates[0].development, fix.development);
        assert_eq!(library.templates[0].reviews[0].instruction, "Custom QA");

        let mut restored = library.clone();
        restored.templates[0].planner.prompt = None;
        restored.templates[0].planner.instruction =
            "Identify the root cause, the smallest complete fix and a regression check.".into();
        let reloaded = pipelines(Some(serde_json::to_value(&restored).unwrap()), &mut lost);
        assert_eq!(reloaded, restored);
    }

    #[test]
    fn upgrading_recipe_catalog_preserves_edits_and_does_not_restore_renamed_recipes() {
        use crate::pipeline::Template;
        let mut original = Template::default();
        original.planner.prompt = None;
        original.planner.instruction = "Custom planning instructions".into();
        let mut lost = vec![];
        let mut library = pipelines(
            Some(serde_json::json!({"templates": [original]})),
            &mut lost,
        );
        assert!(lost.is_empty());
        assert_eq!(library.templates[0], original);
        assert_eq!(library.templates.len(), 3);
        library.templates[1].name = "Hotfix".into();
        let reloaded = pipelines(Some(serde_json::to_value(&library).unwrap()), &mut lost);
        assert_eq!(reloaded, library);
    }

    #[test]
    fn recipes_with_a_retired_effort_or_non_ascii_names_survive_reload() {
        use crate::pipeline::Template;
        let mut retired = Template {
            name: "Ärger".into(),
            ..Default::default()
        };
        retired.planner.profile.effort = "retired-level".into();
        assert!(retired.validate().is_err());
        let lowercase = Template {
            name: "ärger".into(),
            ..Default::default()
        };
        let mut lost = vec![];
        let library = pipelines(
            Some(serde_json::json!({"templates": [retired, lowercase], "catalog_version": 1})),
            &mut lost,
        );
        assert!(lost.is_empty(), "{lost:?}");
        assert_eq!(library.templates, [retired, lowercase]);
    }

    /// A template the way a file saved before prompt links holds it: no `prompt` fields.
    fn without_links(mut template: crate::pipeline::Template) -> crate::pipeline::Template {
        template.planner.prompt = None;
        template.development.prompt = None;
        for review in &mut template.reviews {
            review.prompt = None;
        }
        template
    }

    #[test]
    fn version_two_library_gains_stock_prompts_and_links_only_unedited_stages() {
        use crate::pipeline::{Lane, Library, ReviewKind, Run, Template};
        let stock = Library::default();
        let mut saved = without_links(Template::default());
        saved.development.instruction = "House development rules".into();
        saved.reviews.push(stock.review_step(ReviewKind::Code));
        let saved = without_links(saved);
        let run = Run::new(
            7,
            "/api".into(),
            "Task".into(),
            saved.clone(),
            vec![Lane {
                name: "Development".into(),
                project: "/api".into(),
                base: "HEAD".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        let file = serde_json::json!({"catalog_version": 2, "templates": [saved], "runs": [run]});
        assert!(!file.to_string().contains("\"prompt\""));

        let mut lost = vec![];
        let library = pipelines(Some(file), &mut lost);

        assert!(lost.is_empty(), "{lost:?}");
        assert_eq!(library.prompts(), stock.prompts());
        let loaded = &library.templates[0];
        assert_eq!(loaded.planner, Template::default().planner);
        assert_eq!(loaded.development, saved.development);
        assert_eq!(loaded.reviews[0], Template::default().reviews[0]);
        assert_eq!(loaded.reviews[2], stock.review_step(ReviewKind::Code));
        assert_eq!(library.runs, [run]);
        let reloaded = pipelines(Some(serde_json::to_value(&library).unwrap()), &mut lost);
        assert!(lost.is_empty(), "{lost:?}");
        assert_eq!(reloaded, library);
    }

    #[test]
    fn damaged_or_duplicate_prompts_are_dropped_and_their_stages_keep_the_text() {
        use crate::pipeline::Template;
        let mut template = without_links(Template::default());
        template.planner.prompt = Some(1001);
        template.planner.instruction = "Kept planning text".into();
        template.development.prompt = Some(1000);
        template.development.instruction = "Stale copy".into();
        let file = serde_json::json!({
            "catalog_version": 2,
            "templates": [template],
            "prompts": [
                {"id": 1000, "name": "Mine", "text": "Current text", "future_field": true},
                {"id": 1000, "name": "Same id", "text": "B"},
                {"id": 1001, "name": "mine", "text": "C"},
                {"id": 1002, "name": " ", "text": "D"},
                {"id": "broken"},
            ],
        });
        let mut lost = vec![];
        let library = pipelines(Some(file), &mut lost);

        assert_eq!(
            lost,
            [1, 2, 3, 4].map(|index| format!("pipelines.prompts[{index}]"))
        );
        assert_eq!(library.prompts().len(), 1);
        assert_eq!(library.prompt(1000).unwrap().name, "Mine");
        let loaded = &library.templates[0];
        assert_eq!(loaded.planner.prompt, None);
        assert_eq!(loaded.planner.instruction, "Kept planning text");
        assert_eq!(loaded.development.prompt, Some(1000));
        assert_eq!(loaded.development.instruction, "Current text");
    }

    #[test]
    fn emptied_prompt_library_stays_empty_after_reload() {
        use crate::pipeline::Library;
        let mut library = Library::default();
        let unlinked: Vec<_> = library
            .templates
            .iter()
            .cloned()
            .map(without_links)
            .collect();
        for id in library.prompts().iter().map(|p| p.id).collect::<Vec<_>>() {
            library.delete_prompt(id).unwrap();
        }
        let mut lost = vec![];
        let reloaded = pipelines(Some(serde_json::to_value(&library).unwrap()), &mut lost);
        assert!(lost.is_empty(), "{lost:?}");
        assert!(reloaded.prompts().is_empty());
        assert_eq!(reloaded.templates, unlinked);
    }

    #[test]
    fn launch_drafts_survive_reload_and_a_malformed_sibling_is_isolated() {
        use crate::pipeline::{Library, Template, draft::LaunchDraft};
        let draft = LaunchDraft {
            owner: "/web".into(),
            template: Template::default(),
            original: None,
            frozen: false,
            task: "Keep my task".into(),
            regression: false,
            direct: true,
            criteria: "return 401".into(),
            lanes: vec![],
        };
        let mut file = serde_json::to_value(Library {
            drafts: vec![draft.clone()],
            ..Library::default()
        })
        .unwrap();
        file["drafts"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"owner":9}));
        let mut lost = vec![];
        assert_eq!(pipelines(Some(file), &mut lost).drafts, [draft]);
        assert_eq!(lost, ["pipelines.drafts[1]"]);
    }

    #[test]
    fn library_stripped_by_an_older_build_is_reseeded_and_relinked() {
        use crate::pipeline::Library;
        // An older build drops the fields it does not know and writes the rest back.
        let stripped = Library {
            templates: Library::default()
                .templates
                .into_iter()
                .map(without_links)
                .collect(),
            ..Library::default()
        };
        let mut file = serde_json::to_value(&stripped).unwrap();
        file.as_object_mut().unwrap().remove("prompts");
        let mut lost = vec![];
        assert_eq!(pipelines(Some(file.clone()), &mut lost), Library::default());
        assert!(lost.is_empty(), "{lost:?}");

        file["prompts"] = 5.into();
        assert_eq!(pipelines(Some(file), &mut lost), Library::default());
        assert_eq!(lost, ["pipelines.prompts"]);
    }

    #[test]
    fn empty_template_library_stays_empty_after_reload() {
        let mut state = State::default();
        state.pipelines.templates.clear();
        let fixture = Fixture::new();
        state.save_to(&fixture.0.join("state.json")).unwrap();
        assert!(
            State::load_from(&fixture.0)
                .unwrap()
                .state
                .pipelines
                .templates
                .is_empty()
        );
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
    fn one_unreadable_project_or_field_is_reset_reported_and_backed_up() {
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
            RecoveryCause::Partial(vec!["settings.sidebar_width".into(), "project /b".into()])
        );
        assert_eq!(fs::read(&recovery.backup).unwrap(), original);
    }

    #[test]
    fn one_unreadable_setting_resets_only_itself() {
        use crate::settings::ThemeChoice;
        let fixture = Fixture::new();
        let original = br#"{"version":6,"settings":{"sidebar_width":"wide","theme":"dark",
            "git_panel_open":false,"terminal_font_size":15.0,
            "files":{"language_tools":false,"show_hidden":false,"font_size":"big"},
            "commit_message":{"prompt":7},
            "keybindings":{"workspace::NewTerminal":"cmd-shift-t","workspace::Broken":7}},
            "workspace":{"selection":"gone","projects":[{"project":"/a"}]},
            "projects":[{"path":"/a"}]}"#;
        fs::write(fixture.0.join("state.json"), original).unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();

        let settings = &loaded.state.settings;
        assert_eq!(settings.theme, ThemeChoice::Dark);
        assert!(!settings.git_panel_open);
        assert_eq!(settings.terminal_font_size(), 15.);
        assert!(!settings.files.language_tools, "an opt-out is not undone");
        assert!(!settings.files.show_hidden);
        assert_eq!(
            settings.keybinding("workspace::NewTerminal"),
            Some("cmd-shift-t")
        );
        let defaults = Settings::default();
        assert_eq!(settings.sidebar_width(), defaults.sidebar_width());
        assert_eq!(settings.files.font_size(), defaults.files.font_size());
        assert_eq!(settings.commit_message, defaults.commit_message);
        assert_eq!(settings.keybinding("workspace::Broken"), None);
        assert_eq!(loaded.state.workspace.projects.len(), 1);
        assert!(loaded.state.workspace.selection.is_none());

        let recovery = loaded.recovery.unwrap();
        assert_eq!(
            recovery.cause,
            RecoveryCause::Partial(
                [
                    "settings.commit_message.prompt",
                    "settings.files.font_size",
                    "settings.keybindings.workspace::Broken",
                    "settings.sidebar_width",
                    "workspace.selection",
                ]
                .map(String::from)
                .to_vec()
            )
        );
        assert_eq!(fs::read(&recovery.backup).unwrap(), original);
    }

    #[test]
    fn a_section_that_is_not_an_object_is_reset_whole() {
        let fixture = Fixture::new();
        fs::write(
            fixture.0.join("state.json"),
            br#"{"settings":7,"workspace":[],"projects":[{"path":"/a"}]}"#,
        )
        .unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert_eq!(loaded.state.projects.len(), 1);
        assert_eq!(
            loaded.recovery.unwrap().cause,
            RecoveryCause::Partial(vec!["settings".into(), "workspace".into()])
        );
    }

    #[test]
    fn a_version_written_as_text_still_loads_every_section() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        fs::write(
            &path,
            br#"{"version":"6","settings":{"sidebar_width":300},"projects":[{"path":"/a"},{"path":"/b"}]}"#,
        )
        .unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.recovery.is_none());
        assert_eq!(loaded.state.projects.len(), 2);
        assert_eq!(loaded.state.settings.sidebar_width(), 300.);

        // Text does not hide a newer format from the guard against saving over it.
        fs::write(&path, br#"{"version":" 99 ","projects":[{"path":"/a"}]}"#).unwrap();
        let newer = State::load_from(&fixture.0).unwrap();
        assert_eq!(
            newer.recovery.unwrap().cause,
            RecoveryCause::Newer { version: 99 }
        );
        fs::write(&path, br#"{"version":99999999999}"#).unwrap();
        assert!(matches!(
            State::load_from(&fixture.0)
                .unwrap()
                .recovery
                .unwrap()
                .cause,
            RecoveryCause::Newer { .. }
        ));
    }

    #[test]
    fn an_unreadable_version_is_reported_and_costs_nothing_else() {
        for version in [r#""six""#, "true", "-1", "6.5", "[6]", "{}"] {
            let fixture = Fixture::new();
            let original = format!(r#"{{"version":{version},"projects":[{{"path":"/a"}}]}}"#);
            fs::write(fixture.0.join("state.json"), &original).unwrap();
            let loaded = State::load_from(&fixture.0).unwrap();
            assert_eq!(loaded.state.projects.len(), 1, "{version}");
            let recovery = loaded.recovery.unwrap();
            assert_eq!(
                recovery.cause,
                RecoveryCause::Partial(vec!["version".into()]),
                "{version}"
            );
            assert_eq!(fs::read(&recovery.backup).unwrap(), original.as_bytes());
        }
    }

    #[test]
    fn a_file_that_is_not_a_json_object_is_corrupt_and_preserved() {
        for original in [
            "[]",
            r#"[6,{"sidebar_width":300},[{"path":"/a"}]]"#,
            r#""state""#,
            "42",
            "true",
            "null",
        ] {
            let fixture = Fixture::new();
            fs::write(fixture.0.join("state.json"), original).unwrap();
            let loaded = State::load_from(&fixture.0).unwrap();
            assert!(loaded.state.projects.is_empty(), "{original}");
            let recovery = loaded
                .recovery
                .unwrap_or_else(|| panic!("{original} loaded as a state file"));
            assert_eq!(recovery.cause, RecoveryCause::Corrupt, "{original}");
            assert_eq!(fs::read(&recovery.backup).unwrap(), original.as_bytes());
        }
    }

    #[test]
    fn a_section_written_as_null_is_absent_not_damaged() {
        let fixture = Fixture::new();
        fs::write(
            fixture.0.join("state.json"),
            br#"{"version":null,"settings":null,"projects":null,"workspace":null,"pipelines":null,"last_seen_release":null,"reviewed":null}"#,
        )
        .unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.recovery.is_none());
        assert_eq!(loaded.state.pipelines, crate::pipeline::Library::default());
    }

    #[test]
    fn a_repeated_launch_reuses_the_backup_it_already_made() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        // A newer file is never rewritten, so every launch of this build finds it as it was.
        fs::write(&path, br#"{"version":99,"projects":[{"path":"/a"}]}"#).unwrap();
        let first = State::load_from(&fixture.0).unwrap().recovery.unwrap();
        let second = State::load_from(&fixture.0).unwrap().recovery.unwrap();
        assert_eq!(second.backup, first.backup);
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), 2);

        // Different bytes are a different original and get their own copy.
        fs::write(&path, br#"{"version":99,"projects":[{"path":"/b"}]}"#).unwrap();
        let third = State::load_from(&fixture.0).unwrap().recovery.unwrap();
        assert_ne!(third.backup, first.backup);
        assert_eq!(
            fs::read(&first.backup).unwrap(),
            br#"{"version":99,"projects":[{"path":"/a"}]}"#
        );
        assert_eq!(fs::read(&third.backup).unwrap(), fs::read(&path).unwrap());
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
    fn sections_a_newer_hive_added_without_a_version_bump_survive_a_save() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        let future = serde_json::json!({"boards": [{"id": 1, "cards": ["a", "b"]}], "flag": true});
        let file = serde_json::json!({
            "version": SCHEMA_VERSION,
            "projects": [{"path": "/a"}],
            "future_section": future,
            "future_marker": "kept",
        });
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        let mut loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.recovery.is_none());
        loaded
            .state
            .projects
            .push(ProjectRef::new("/b".into(), Host::Local));
        loaded.state.save_to(&path).unwrap();

        let saved: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["future_section"], future);
        assert_eq!(saved["future_marker"], "kept");
        assert_eq!(saved["version"], SCHEMA_VERSION);
        assert_eq!(saved["projects"].as_array().unwrap().len(), 2);
        let reloaded = State::load_from(&fixture.0).unwrap();
        assert!(reloaded.recovery.is_none());
        assert_eq!(reloaded.state.projects.len(), 2);
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
        assert!(loaded.state.reviewed.is_empty());
        let mut state = loaded.state;
        assert!(state.mark_release_seen("0.1.4"));
        state.save_to(&path).unwrap();
        let mut restored = State::load_from(&fixture.0).unwrap().state;
        assert!(!restored.mark_release_seen("0.1.4"));
        assert_eq!(restored.projects[0].path, Path::new("/a"));
    }

    #[test]
    fn state_without_the_language_tools_switch_keeps_them_on_and_an_opt_out_survives_restart() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        // Files settings as 1.0.22 wrote them, before the switch existed.
        fs::write(&path, br#"{"version":1,"settings":{"files":{"icons":"colored","density":"compact","git_decorations":true,"auto_reveal":true,"show_hidden":true,"show_ignored":true,"save_on_focus_change":true,"code_completion":false,"inline_blame":true,"sticky_scroll":true,"font_size":14.0}},"projects":[{"path":"/a"}]}"#).unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.recovery.is_none());
        let mut state = loaded.state;
        assert!(state.settings.files.language_tools);
        assert!(!state.settings.files.code_completion);
        state.settings.files.language_tools = false;
        state.save_to(&path).unwrap();
        let restored = State::load_from(&fixture.0).unwrap();
        assert!(restored.recovery.is_none());
        assert!(!restored.state.settings.files.language_tools);
        assert!(!restored.state.settings.files.code_completion);
        assert_eq!(restored.state.projects[0].path, Path::new("/a"));
    }

    #[test]
    fn reviewed_revisions_survive_restart_and_ignore_future_fields() {
        let fixture = Fixture::new();
        let path = fixture.0.join("state.json");
        fs::write(&path, br#"{"version":1,"projects":[{"path":"/a"}],"reviewed":[{"host":{"kind":"local"},"worktree":"/a","path":"src/a.rs","source":"WorkingTree","fingerprint":"revision","future":true}]}"#).unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.recovery.is_none());
        assert_eq!(loaded.state.reviewed.len(), 1);
        loaded.state.save_to(&path).unwrap();
        assert_eq!(
            State::load_from(&fixture.0).unwrap().state.reviewed,
            loaded.state.reviewed
        );
    }

    #[test]
    fn one_unreadable_review_mark_is_dropped_without_losing_the_others() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("state.json"), br#"{"version":1,"reviewed":[{"host":{"kind":"local"},"worktree":"/a","path":"src/a.rs","source":{"FromTheFuture":1},"fingerprint":"x"},{"host":{"kind":"local"},"worktree":"/a","path":"src/b.rs","source":"WorkingTree","fingerprint":"y"},7]}"#).unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.recovery.is_none());
        assert_eq!(loaded.state.reviewed.len(), 1);

        fs::write(
            fixture.0.join("state.json"),
            br#"{"version":1,"reviewed":"all"}"#,
        )
        .unwrap();
        let loaded = State::load_from(&fixture.0).unwrap();
        assert!(loaded.state.reviewed.is_empty());
        assert_eq!(
            loaded.recovery.unwrap().cause,
            RecoveryCause::Partial(vec!["reviewed".into()])
        );
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
