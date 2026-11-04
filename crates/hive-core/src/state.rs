//! Everything Hive persists between runs, in one JSON file (`~/.hive/state.json`).
//! Sessions and dock layout will be added as fields here. Move to SQLite only when
//! we need queries over history.

use crate::host::Host;
use crate::identity::{self, ProjectIdentity};
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

impl State {
    /// Adds `project` unless its folder is already listed; returns whether it was added.
    /// A project without a custom color gets one no other project shows yet.
    pub fn add_project(&mut self, mut project: ProjectRef) -> bool {
        if self.projects.iter().any(|p| p.is_same_folder(&project)) {
            return false;
        }
        if project.identity.color().is_none() {
            let used: Vec<u32> = self
                .projects
                .iter()
                .enumerate()
                .map(|(ix, p)| p.identity.color_or_default(ix))
                .collect();
            project.identity.set_color(identity::unused_color(&used));
        }
        self.projects.push(project);
        true
    }
}

pub fn hive_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".hive")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(path: &str) -> ProjectRef {
        ProjectRef::new(PathBuf::from(path), Host::Local)
    }

    #[test]
    fn new_project_gets_a_color_nobody_shows() {
        let mut state = State::default();
        let mut custom = project("/a");
        custom.identity.set_color(0xf97316);
        assert!(state.add_project(custom));
        state.projects.push(project("/b")); // no color: shows the default for position 1
        assert!(state.add_project(project("/c")));
        let color = state.projects[2].identity.color();
        assert_eq!(color, Some(0x22c55e));
        assert_ne!(color, Some(identity::default_color(1)));
    }

    #[test]
    fn same_folder_is_not_added_twice() {
        let mut state = State::default();
        assert!(state.add_project(project("/a")));
        assert!(!state.add_project(project("/a")));
        assert_eq!(state.projects.len(), 1);
    }
}
