//! Everything Hive persists between runs, in one JSON file (`~/.hive/state.json`).
//! Move to SQLite only when
//! we need queries over history.

use crate::host::Host;
use crate::identity::{self, ProjectIdentity};
use crate::settings::Settings;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod lenient;
mod persistence;
pub mod workspace;
pub use persistence::{LoadedState, Recovery, RecoveryCause, SCHEMA_VERSION, StateError};

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct ProjectRef {
    #[serde(with = "crate::path_encoding::persisted")]
    pub path: PathBuf,
    #[serde(default)]
    pub host: Host,
    #[serde(default)]
    pub worktrees: WorktreeFilter,
    /// Display details; an unreadable badge or color must not cost the project itself.
    #[serde(default, deserialize_with = "lenient::or_default")]
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
    /// Also worktrees created by other tools (Claude Code, Codex, plain `git worktree add`).
    All,
    /// The primary worktree and the ones Hive created; also what an unknown value means.
    #[default]
    #[serde(other)]
    Hive,
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
    /// Workspace layout and selection, restored without replaying commands.
    pub workspace: workspace::Workspace,
    /// Installed version whose release highlights were last presented.
    pub last_seen_release: Option<String>,
}

impl State {
    /// Marks release highlights as presented; returns false on repeated launches.
    pub fn mark_release_seen(&mut self, version: &str) -> bool {
        if self.last_seen_release.as_deref() == Some(version) {
            return false;
        }
        self.last_seen_release = Some(version.to_owned());
        true
    }

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

    /// Remove only this host's project and its workspace, leaving other hosts untouched.
    pub fn remove_project(&mut self, host: &Host, path: &Path) -> bool {
        let before = self.projects.len();
        self.projects.retain(|p| p.host != *host || p.path != path);
        if self.projects.len() == before {
            return false;
        }
        self.workspace
            .projects
            .retain(|p| p.host != *host || p.project != path);
        if self
            .workspace
            .selection
            .as_ref()
            .is_some_and(|s| s.host == *host && s.project == path)
        {
            self.workspace.selection = None;
        }
        true
    }

    /// Moves `project` before another project, or to the end when `before` is `None`.
    /// Matches folders by path and host, preserves their colors, and returns whether order changed.
    /// Missing folders and drops that leave the order unchanged are ignored.
    pub fn move_project(&mut self, project: &ProjectRef, before: Option<&ProjectRef>) -> bool {
        let Some(source) = self.projects.iter().position(|p| p.is_same_folder(project)) else {
            return false;
        };
        let target = match before {
            Some(before) => {
                let Some(target) = self.projects.iter().position(|p| p.is_same_folder(before))
                else {
                    return false;
                };
                target
            }
            None => self.projects.len(),
        };
        let destination = if source < target { target - 1 } else { target };
        if source == destination {
            return false;
        }
        // Pin position-based colors before moving the affected folders.
        let start = source.min(destination);
        for (offset, project) in self.projects[start..=source.max(destination)]
            .iter_mut()
            .enumerate()
        {
            let color = project.identity.color_or_default(start + offset);
            project.identity.set_color(color);
        }
        let project = self.projects.remove(source);
        self.projects.insert(destination, project);
        true
    }
}

/// Application data directory. Debug builds can isolate smoke tests from the running app.
pub fn hive_dir() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("HIVE_DEV_DATA_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return path;
    }
    match home_dir() {
        Some(home) => home.join(".hive"),
        None => {
            // Never a relative `.hive` in whatever the current directory happens to be.
            let fallback = std::env::temp_dir().join("hive");
            tracing::error!(path = ?fallback, "no home directory; using a temporary data folder");
            fallback
        }
    }
}

/// The user's home directory: `$HOME`, else the account database. Independent of
/// `HIVE_DEV_DATA_DIR`, so CLI credentials under `~/.claude` and `~/.codex` are found
/// even when Hive's own data is redirected.
pub fn home_dir() -> Option<PathBuf> {
    std::env::home_dir().filter(|home| home.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_paths_on_three_hosts_keep_independent_state_and_layouts() {
        use super::workspace::{ProjectWorkspace, Selection};
        let hosts = [
            Host::Local,
            Host::Ssh {
                target: "home".into(),
            },
            Host::Ssh {
                target: "other".into(),
            },
        ];
        let mut state = State::default();
        for host in &hosts {
            assert!(state.add_project(ProjectRef::new("/project".into(), host.clone())));
            assert!(!state.add_project(ProjectRef::new("/project".into(), host.clone())));
            state.workspace.projects.push(ProjectWorkspace {
                host: host.clone(),
                project: "/project".into(),
                worktree: None,
                focused: 0,
                layout: None,
            });
        }
        state.workspace.selection = Some(Selection {
            host: hosts[2].clone(),
            project: "/project".into(),
            worktree: None,
        });
        assert!(state.remove_project(&hosts[1], Path::new("/project")));
        assert_eq!(state.projects.len(), 2);
        assert_eq!(state.workspace.projects.len(), 2);
        assert_eq!(state.workspace.selection.as_ref().unwrap().host, hosts[2]);
        assert!(state.remove_project(&hosts[2], Path::new("/project")));
        assert!(state.workspace.selection.is_none());
        assert_eq!(state.projects[0].host, Host::Local);
    }

    #[test]
    fn old_selection_defaults_to_local_and_byte_paths_roundtrip() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let old: workspace::Selection =
            serde_json::from_str(r#"{"project":"/project","future":true}"#).unwrap();
        assert_eq!(old.host, Host::Local);
        let mut state = State::default();
        let path = PathBuf::from(OsString::from_vec(b"/project-\xff".to_vec()));
        let host = Host::Ssh {
            target: "home".into(),
        };
        state.add_project(ProjectRef::new(path.clone(), host.clone()));
        state.workspace.selection = Some(workspace::Selection {
            host,
            project: path.clone(),
            worktree: Some(path),
        });
        let loaded: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        assert_eq!(loaded.projects, state.projects);
        assert_eq!(loaded.workspace.selection, state.workspace.selection);
    }

    #[test]
    fn release_highlights_are_presented_once_per_installed_version() {
        let mut state = State::default();
        assert!(state.mark_release_seen("0.1.4"));
        assert!(!state.mark_release_seen("0.1.4"));
        assert!(state.mark_release_seen("0.1.5"));
        assert!(!state.mark_release_seen("0.1.5"));
        assert_eq!(state.last_seen_release.as_deref(), Some("0.1.5"));
    }

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

    #[test]
    fn moves_projects_in_both_directions_and_to_the_end() {
        let mut state = State {
            projects: vec![project("/a"), project("/b"), project("/c"), project("/d")],
            ..State::default()
        };
        for (source, before, expected) in [
            ("/d", Some("/a"), ["/d", "/a", "/b", "/c"]),
            ("/a", Some("/c"), ["/d", "/b", "/a", "/c"]),
            ("/d", None, ["/b", "/a", "/c", "/d"]),
        ] {
            assert!(state.move_project(&project(source), before.map(project).as_ref()));
            assert_eq!(
                state
                    .projects
                    .iter()
                    .map(|p| p.path.as_path())
                    .collect::<Vec<_>>(),
                expected.map(std::path::Path::new)
            );
        }
    }

    #[test]
    fn moving_projects_preserves_identity_filter_host_and_serialized_order() {
        let local = project("/a");
        let mut remote = ProjectRef::new(
            "/a".into(),
            Host::Ssh {
                target: "server".into(),
            },
        );
        remote.identity.set_name("Remote");
        remote.identity.set_color(0x123456);
        remote.identity.set_emoji("🐝");
        remote.worktrees = WorktreeFilter::All;
        let mut state = State {
            projects: vec![local.clone(), remote.clone(), project("/c")],
            ..State::default()
        };
        let colors: Vec<_> = state
            .projects
            .iter()
            .enumerate()
            .map(|(ix, p)| p.identity.color_or_default(ix))
            .collect();
        assert!(state.move_project(&remote, Some(&local)));
        assert_eq!(state.projects[0], remote);
        assert_eq!(state.projects[1].host, Host::Local);
        assert_eq!(state.projects[0].identity.color_or_default(0), colors[1]);
        assert_eq!(state.projects[1].identity.color_or_default(1), colors[0]);
        assert_eq!(state.projects[2].identity.color_or_default(2), colors[2]);
        let restored: State =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(restored.projects, state.projects);
    }

    #[test]
    fn missing_projects_and_unchanged_drops_do_not_mutate_state() {
        let mut state = State {
            projects: vec![project("/a"), project("/b")],
            ..State::default()
        };
        let original = state.projects.clone();
        for (source, before) in [
            ("/a", Some("/a")),
            ("/a", Some("/b")),
            ("/b", None),
            ("/missing", Some("/a")),
            ("/a", Some("/missing")),
        ] {
            assert!(!state.move_project(&project(source), before.map(project).as_ref()));
            assert_eq!(state.projects, original);
        }
        assert!(!State::default().move_project(&project("/a"), None));
    }
}
