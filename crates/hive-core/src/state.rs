//! Everything Hive persists between runs, in one JSON file (`~/.hive/state.json`).
//! Sessions and dock layout will be added as fields here. Move to SQLite only when
//! we need queries over history.

use crate::host::Host;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ProjectRef {
    pub path: PathBuf,
    #[serde(default)]
    pub host: Host,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(default)]
pub struct State {
    pub settings: Settings,
    pub projects: Vec<ProjectRef>,
}

pub fn hive_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".hive")
}

fn state_path() -> PathBuf {
    hive_dir().join("state.json")
}

impl State {
    pub fn load() -> Self {
        if let Some(state) = read_json::<State>(&state_path()) {
            return state;
        }
        // Pre-workspace prototype wrote a flat `settings.json`; import it once.
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Legacy {
            theme: crate::settings::ThemeChoice,
            projects: Vec<PathBuf>,
        }
        let state = read_json::<Legacy>(&hive_dir().join("settings.json"))
            .map(|old| {
                let mut settings = Settings::default();
                settings.theme = old.theme;
                State {
                    settings,
                    projects: old.projects.into_iter().map(|path| ProjectRef { path, host: Host::Local }).collect(),
                }
            })
            .unwrap_or_default();
        state.save();
        state
    }

    pub fn save(&self) {
        let path = state_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_vec_pretty(self) {
            Ok(json) => {
                if let Err(err) = std::fs::write(&path, json) {
                    tracing::error!(?path, %err, "failed to save state");
                }
            }
            Err(err) => tracing::error!(%err, "failed to serialize state"),
        }
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes)
        .inspect_err(|err| tracing::warn!(?path, %err, "ignoring unreadable state file"))
        .ok()
}
