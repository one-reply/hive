//! Everything Hive persists between runs, in one JSON file (`~/.hive/state.json`).
//! Sessions and dock layout will be added as fields here. Move to SQLite only when
//! we need queries over history.

use crate::host::Host;
use crate::identity::ProjectIdentity;
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod persistence;
pub use persistence::{LoadedState, StateError};

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ProjectRef {
    pub path: PathBuf,
    #[serde(default)]
    pub host: Host,
    #[serde(default)]
    pub worktrees: WorktreeFilter,
    #[serde(default)]
    pub identity: ProjectIdentity,
}

impl ProjectRef {
    /// A project with default view settings.
    pub fn new(path: PathBuf, host: Host) -> Self {
        Self {
            path,
            host,
            worktrees: WorktreeFilter::default(),
            identity: ProjectIdentity::default(),
        }
    }

    /// Same folder on the same host, whatever the view settings.
    pub fn is_same_folder(&self, other: &ProjectRef) -> bool {
        self.path == other.path && self.host == other.host
    }
}

/// Which of a project's worktrees the sidebar lists.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorktreeFilter {
    /// The primary worktree and the ones Hive created.
    #[default]
    Hive,
    /// Also worktrees created by other tools (Claude Code, Codex, plain `git worktree add`).
    All,
}

impl WorktreeFilter {
    /// The other filter: what "Show/Hide other worktrees" switches to.
    pub fn toggled(self) -> Self {
        match self {
            WorktreeFilter::Hive => WorktreeFilter::All,
            WorktreeFilter::All => WorktreeFilter::Hive,
        }
    }
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
