//! Initial sidebar visibility and persistent manual overrides.
use super::{
    ProjectRef, State,
    workspace::{Layout, Tab, Tabs, Workspace},
};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A user's explicit choice takes precedence over automatic startup visibility.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectExpansion {
    /// Keep the project expanded across launches.
    Expanded,
    /// Keep the project collapsed across launches, even when it has open tabs.
    Collapsed,
    /// Derive the initial visibility from the saved selection and open tabs.
    #[default]
    #[serde(other)]
    Automatic,
}

impl ProjectRef {
    /// Evaluate once at sidebar creation, before lazy tab restoration changes the workspace.
    pub fn expanded_on_start(&self, workspace: &Workspace) -> bool {
        match self.expansion {
            ProjectExpansion::Expanded => true,
            ProjectExpansion::Collapsed => false,
            ProjectExpansion::Automatic => {
                workspace
                    .selection
                    .as_ref()
                    .is_some_and(|s| s.host == self.host && s.project == self.path)
                    || workspace.projects.iter().any(|p| {
                        p.host == self.host
                            && p.project == self.path
                            && p.layout.as_ref().is_some_and(has_tabs)
                    })
            }
        }
    }
}

fn has_tabs(layout: &Layout<Tabs>) -> bool {
    match layout {
        Layout::Leaf { value } => value.items.iter().any(|tab| !matches!(tab, Tab::Unknown)),
        Layout::Split { children, .. } => children.iter().any(has_tabs),
        Layout::Unknown => false,
    }
}

impl State {
    /// Persist an explicit sidebar choice only for the matching registered project.
    pub fn set_project_expanded(&mut self, host: &Host, path: &Path, expanded: bool) -> bool {
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|p| p.host == *host && p.path == path)
        else {
            return false;
        };
        let value = if expanded {
            ProjectExpansion::Expanded
        } else {
            ProjectExpansion::Collapsed
        };
        if project.expansion == value {
            return false;
        }
        project.expansion = value;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::workspace::{ProjectWorkspace, Selection, SplitAxis};

    fn project() -> ProjectRef {
        ProjectRef::new("/repo".into(), Host::Local)
    }
    fn workspace(host: Host, items: Vec<Tab>) -> Workspace {
        Workspace {
            projects: vec![ProjectWorkspace {
                project: "/repo".into(),
                host,
                worktree: None,
                focused: 0,
                layout: Some(Layout::Split {
                    axis: SplitAxis::Horizontal,
                    sizes: vec![],
                    children: vec![Layout::Leaf {
                        value: Tabs { items, active: 0 },
                    }],
                }),
            }],
            ..Default::default()
        }
    }
    fn tab() -> Tab {
        Tab::File {
            worktree: "/repo".into(),
            path: "file.rs".into(),
            cursor: [0; 2],
            scroll: [0.; 2],
        }
    }

    #[test]
    fn automatic_uses_selection_or_saved_tabs_not_merely_a_saved_project() {
        let p = project();
        assert!(!p.expanded_on_start(&Workspace::default()));
        assert!(!p.expanded_on_start(&workspace(Host::Local, vec![])));
        assert!(!p.expanded_on_start(&workspace(Host::Local, vec![Tab::Unknown])));
        assert!(p.expanded_on_start(&workspace(Host::Local, vec![tab()])));
        let selected = Workspace {
            selection: Some(Selection {
                host: Host::Local,
                project: p.path.clone(),
                worktree: None,
            }),
            ..Default::default()
        };
        assert!(p.expanded_on_start(&selected));
        let mut manual = p.clone();
        manual.expansion = ProjectExpansion::Collapsed;
        assert!(!manual.expanded_on_start(&selected));
        assert!(!manual.expanded_on_start(&workspace(Host::Local, vec![tab()])));
        manual.expansion = ProjectExpansion::Expanded;
        assert!(manual.expanded_on_start(&Workspace::default()));
        assert!(p.has_same_scan_settings(&manual));
    }

    #[test]
    fn preferences_and_activity_are_scoped_to_host_and_removed_with_project() {
        let mut remote = project();
        remote.host = Host::Ssh {
            target: "server".into(),
        };
        let mut state = State {
            projects: vec![project(), remote.clone()],
            ..Default::default()
        };
        assert!(!project().expanded_on_start(&workspace(remote.host.clone(), vec![tab()])));
        assert!(state.set_project_expanded(&remote.host, &remote.path, false));
        assert!(!state.set_project_expanded(&remote.host, &remote.path, false));
        assert_eq!(state.projects[0].expansion, ProjectExpansion::Automatic);
        assert_eq!(state.projects[1].expansion, ProjectExpansion::Collapsed);
        state.remove_project(&remote.host, &remote.path);
        state.add_project(remote);
        assert_eq!(state.projects[1].expansion, ProjectExpansion::Automatic);
        assert!(!state.set_project_expanded(&Host::Local, Path::new("/missing"), true));
    }

    #[test]
    fn old_unknown_and_invalid_preferences_load_and_manual_choices_roundtrip() {
        for json in [
            r#"{"path":"/repo"}"#,
            r#"{"path":"/repo","expansion":"future"}"#,
            r#"{"path":"/repo","expansion":42}"#,
        ] {
            let p: ProjectRef = serde_json::from_str(json).unwrap();
            assert_eq!(p.expansion, ProjectExpansion::Automatic);
        }
        let mut state = State {
            projects: vec![project()],
            ..Default::default()
        };
        for expanded in [false, true] {
            state.set_project_expanded(&Host::Local, Path::new("/repo"), expanded);
            let restored: State =
                serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
            assert_eq!(
                restored.projects[0].expanded_on_start(&Workspace::default()),
                expanded
            );
        }
    }
}
