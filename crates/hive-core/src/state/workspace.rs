//! Workspace snapshots contain paths and layout only, never running processes or terminal output.

use crate::{git::diff::DiffSource, host::Host};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Project and worktree selected in the sidebar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// Project's registered path.
    pub project: PathBuf,
    /// Selected worktree; absent for a plain folder.
    #[serde(default)]
    pub worktree: Option<PathBuf>,
}

/// Work retained across application launches.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    /// Last sidebar selection.
    pub selection: Option<Selection>,
    /// Layouts of projects that have been opened, including deliberately empty ones.
    /// One unreadable layout is dropped rather than costing every project its tabs.
    #[serde(deserialize_with = "super::lenient::elements")]
    pub projects: Vec<ProjectWorkspace>,
}

impl Workspace {
    /// Blocking: discard stale local directories and layouts for removed projects before opening windows.
    pub fn prepare(&mut self, projects: &[super::ProjectRef]) {
        self.prepare_with(projects, &|path| path.is_dir());
    }

    fn prepare_with(&mut self, projects: &[super::ProjectRef], is_dir: &impl Fn(&Path) -> bool) {
        self.projects.retain(|saved| {
            projects
                .iter()
                .any(|p| p.path == saved.project && p.host == saved.host)
        });
        for saved in &mut self.projects {
            if saved.host != Host::Local {
                continue;
            }
            let mut old_index = 0;
            let mut new_index = 0;
            let mut focused = 0;
            saved.layout = saved.layout.as_ref().and_then(|layout| {
                layout.filter_map(&mut |tabs| {
                    let mut active = 0;
                    let mut items = Vec::new();
                    for (index, tab) in tabs.items.iter().enumerate() {
                        if let Some(tab) = tab.prepare(is_dir) {
                            if index <= tabs.active {
                                active = items.len();
                            }
                            if old_index <= saved.focused {
                                focused = new_index;
                            }
                            items.push(tab);
                            new_index += 1;
                        }
                        old_index += 1;
                    }
                    (!items.is_empty()).then_some(Tabs { items, active })
                })
            });
            saved.focused = focused;
        }
    }
}

/// One project's tabs and the selection that last accompanied them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectWorkspace {
    /// Registered project path.
    pub project: PathBuf,
    /// Host identity prevents restoring a local layout for a different host.
    #[serde(default)]
    pub host: Host,
    /// Worktree shown when this layout was saved.
    #[serde(default)]
    pub worktree: Option<PathBuf>,
    /// Focused tab's index in depth-first order across tab groups.
    #[serde(default)]
    pub focused: usize,
    /// Absent when the user closed every tab.
    #[serde(default)]
    pub layout: Option<Layout<Tabs>>,
}

/// Direction in which split children are arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    /// Children arranged top to bottom.
    Vertical,
    /// Children arranged left to right; also what an unknown axis means.
    #[serde(other)]
    Horizontal,
}

/// Serializable layout shared by dock groups and terminal panes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Layout<T> {
    /// A tab group or terminal working directory.
    Leaf {
        /// Leaf-specific state.
        value: T,
    },
    /// Resizable row or column.
    Split {
        /// Direction of the split.
        axis: SplitAxis,
        /// Children in display order.
        children: Vec<Self>,
        /// Last measured sizes; missing or invalid values use equal shares.
        #[serde(default)]
        sizes: Vec<f32>,
    },
    /// An unsupported future node, skipped when restoring.
    #[serde(other)]
    Unknown,
}

impl<T> Layout<T> {
    /// Maps surviving leaves and collapses empty or single-child splits.
    pub fn filter_map<U>(&self, map: &mut impl FnMut(&T) -> Option<U>) -> Option<Layout<U>> {
        match self {
            Self::Leaf { value } => map(value).map(|value| Layout::Leaf { value }),
            Self::Split {
                axis,
                children,
                sizes,
            } => {
                let mut kept = Vec::new();
                let mut kept_sizes = Vec::new();
                for (index, child) in children.iter().enumerate() {
                    if let Some(child) = child.filter_map(map) {
                        kept.push(child);
                        kept_sizes.push(
                            sizes
                                .get(index)
                                .copied()
                                .filter(|s| s.is_finite() && *s > 0.)
                                .unwrap_or(0.),
                        );
                    }
                }
                match kept.len() {
                    0 => None,
                    1 => kept.pop(),
                    _ => Some(Layout::Split {
                        axis: *axis,
                        children: kept,
                        sizes: kept_sizes,
                    }),
                }
            }
            Self::Unknown => None,
        }
    }
}

/// Ordered tabs in one dock group.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tabs {
    /// Tabs in display order.
    pub items: Vec<Tab>,
    /// Displayed tab, clamped during restoration.
    pub active: usize,
}

/// A shell's latest directory and the worktree to which its pane belongs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPane {
    /// Directory in which the replacement shell starts.
    pub directory: PathBuf,
    /// Original worktree association, retained even after a shell changes directory.
    #[serde(default)]
    pub worktree: Option<PathBuf>,
    /// The conversation the pane's agent ran at quit; restoration resumes it in this pane.
    #[serde(default, deserialize_with = "crate::agent::launch::deserialize_saved")]
    pub resume: Option<crate::agent::launch::ResumeSession>,
}

/// The two kinds of workspace tab Hive can restore.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tab {
    /// A pinned file buffer; unsaved contents are stored separately in editor recovery.
    File {
        /// Worktree containing this file.
        worktree: PathBuf,
        /// Worktree-relative path.
        path: PathBuf,
        /// Zero-based UTF-16 cursor position.
        #[serde(default)]
        cursor: [u32; 2],
        /// Editor scroll offset in logical pixels.
        #[serde(default)]
        scroll: [f32; 2],
    },
    /// Fresh shells in the saved pane layout.
    Terminal {
        /// Pane working directories and split sizes.
        panes: Layout<TerminalPane>,
        /// Focused pane in depth-first order.
        #[serde(default)]
        active: usize,
    },
    /// A diff reloaded from Git rather than saved patch contents.
    Diff {
        /// Worktree containing the reviewed file.
        worktree: PathBuf,
        /// Repository-relative file path.
        path: String,
        /// Original path of a renamed file.
        #[serde(default)]
        old_path: Option<String>,
        /// Versions to compare.
        source: DiffSource,
        /// Whether partial staging was unavailable for this change.
        #[serde(default)]
        whole_file_only: bool,
    },
    /// Unsupported future tab, skipped on restoration.
    #[serde(other)]
    Unknown,
}

impl Tab {
    fn prepare(&self, is_dir: &impl Fn(&Path) -> bool) -> Option<Self> {
        let available = |path: &Path| {
            let exists = path.is_absolute() && is_dir(path);
            if !exists {
                tracing::warn!(?path, "skipping unavailable workspace directory");
            }
            exists
        };
        match self {
            Self::Terminal { panes, active } => {
                let mut old_index = 0;
                let mut new_index = 0;
                let mut selected = 0;
                let panes = panes.filter_map(&mut |pane| {
                    let keep = available(&pane.directory);
                    if keep {
                        if old_index <= *active {
                            selected = new_index;
                        }
                        new_index += 1;
                    }
                    old_index += 1;
                    keep.then_some(pane.clone())
                })?;
                Some(Self::Terminal {
                    panes,
                    active: selected,
                })
            }
            Self::Diff { worktree, .. } => available(worktree).then(|| self.clone()),
            Self::File { worktree, path, .. } => (available(worktree)
                && !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(name) if name != ".git")))
            .then(|| self.clone()),
            Self::Unknown => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_tabs_default_position_and_roundtrip() {
        let file: Tab =
            serde_json::from_str(r#"{"kind":"file","worktree":"/project","path":"src/a.rs"}"#)
                .unwrap();
        assert!(matches!(
            &file,
            Tab::File {
                cursor: [0, 0],
                scroll: [0.0, 0.0],
                ..
            }
        ));
        assert_eq!(
            serde_json::from_str::<Tab>(&serde_json::to_string(&file).unwrap()).unwrap(),
            file
        );
        assert!(file.prepare(&|_| true).is_some());
        let outside: Tab =
            serde_json::from_str(r#"{"kind":"file","worktree":"/project","path":"../secret"}"#)
                .unwrap();
        assert!(outside.prepare(&|_| true).is_none());
    }

    #[test]
    fn image_tabs_use_the_existing_file_snapshot_with_unknown_fields() {
        for path in ["images/photo.PNG", "icons/logo.svg"] {
            let tab: Tab = serde_json::from_value(serde_json::json!({
                "kind": "file", "worktree": "/project", "path": path, "future": true
            }))
            .unwrap();
            assert!(matches!(
                &tab,
                Tab::File {
                    cursor: [0, 0],
                    scroll: [0., 0.],
                    ..
                }
            ));
            assert_eq!(
                serde_json::from_str::<Tab>(&serde_json::to_string(&tab).unwrap()).unwrap(),
                tab
            );
        }
    }

    #[test]
    fn saved_sessions_are_optional_validated_and_round_trip() {
        let legacy: TerminalPane = serde_json::from_str(r#"{"directory":"/project"}"#).unwrap();
        assert!(legacy.resume.is_none());
        let valid: TerminalPane = serde_json::from_str(
            r#"{"directory":"/project","resume":{"agent":"ClaudeCode","id":"abc-123"}}"#,
        )
        .unwrap();
        assert_eq!(valid.resume.as_ref().unwrap().id(), "abc-123");
        assert_eq!(
            serde_json::from_str::<TerminalPane>(&serde_json::to_string(&valid).unwrap()).unwrap(),
            valid
        );
        let opencode: TerminalPane = serde_json::from_str(
            r#"{"directory":"/project","resume":{"agent":"OpenCode","id":"ses_hive1a2b"}}"#,
        )
        .unwrap();
        assert_eq!(opencode.resume.as_ref().unwrap().id(), "ses_hive1a2b");
        for value in [
            r#"{"agent":"Future","id":"abc"}"#,
            r#"{"agent":"ClaudeCode","id":"--flags"}"#,
            r#"{"agent":"OpenCode","id":"abc-123"}"#,
            r#"{}"#,
        ] {
            let pane: TerminalPane =
                serde_json::from_str(&format!(r#"{{"directory":"/project","resume":{value}}}"#))
                    .unwrap();
            assert!(pane.resume.is_none());
        }
    }

    #[test]
    fn layout_prunes_unknown_nodes_and_keeps_matching_sizes() {
        let layout = Layout::Split {
            axis: SplitAxis::Horizontal,
            children: vec![
                Layout::Leaf { value: 1 },
                Layout::Unknown,
                Layout::Leaf { value: 3 },
            ],
            sizes: vec![200., 300., f32::NAN],
        };
        assert_eq!(
            layout.filter_map(&mut |v| Some(*v)),
            Some(Layout::Split {
                axis: SplitAxis::Horizontal,
                children: vec![Layout::Leaf { value: 1 }, Layout::Leaf { value: 3 }],
                sizes: vec![200., 0.],
            })
        );
        assert_eq!(
            layout.filter_map(&mut |v| (*v == 3).then_some(*v)),
            Some(Layout::Leaf { value: 3 })
        );
        assert_eq!(layout.filter_map(&mut |_| None::<u32>), None);
    }

    #[test]
    fn old_state_and_future_tabs_are_compatible() {
        let state: super::super::State = serde_json::from_str(r#"{"projects":[]}"#).unwrap();
        assert_eq!(state.workspace, Workspace::default());
        let tabs: Tabs = serde_json::from_str(r#"{"items":[{"kind":"future"},{"kind":"terminal","panes":{"kind":"leaf","value":{"directory":"/tmp"}}}],"extra":true}"#).unwrap();
        assert!(matches!(tabs.items[0], Tab::Unknown));
        assert_eq!(
            serde_json::from_str::<Tabs>(&serde_json::to_string(&tabs).unwrap()).unwrap(),
            tabs
        );
    }

    #[test]
    fn snapshot_round_trip_retains_empty_projects_and_diff_context() {
        let json = r#"{
            "selection":{"project":"/project","worktree":"/branch"},
            "projects":[
                {"project":"/empty","layout":null},
                {"project":"/project","worktree":"/branch","focused":1,"layout":{
                    "kind":"leaf","value":{"active":1,"items":[
                        {"kind":"terminal","active":0,"panes":{"kind":"leaf","value":{
                            "directory":"/branch/src","worktree":"/branch"}}},
                        {"kind":"diff","worktree":"/branch","path":"src/new.rs",
                            "old_path":"src/old.rs","source":"WorkingTree","whole_file_only":true}
                    ]}
                }}
            ]
        }"#;
        let mut workspace: Workspace = serde_json::from_str(json).unwrap();
        let projects: Vec<_> = ["/empty", "/project"]
            .into_iter()
            .map(|p| super::super::ProjectRef::new(p.into(), Host::Local))
            .collect();
        workspace.prepare_with(&projects, &|_| true);
        assert_eq!(workspace.projects.len(), 2);
        assert!(workspace.projects[0].layout.is_none());
        assert_eq!(workspace.projects[1].focused, 1);
        assert_eq!(
            serde_json::from_str::<Workspace>(&serde_json::to_string(&workspace).unwrap()).unwrap(),
            workspace
        );
    }

    #[test]
    fn removed_directories_are_pruned_without_losing_active_survivors() {
        let pane = |path: &str| Layout::Leaf {
            value: TerminalPane {
                directory: PathBuf::from(path),
                worktree: Some("/project".into()),
                resume: None,
            },
        };
        let tab = Tab::Terminal {
            panes: Layout::Split {
                axis: SplitAxis::Vertical,
                children: vec![pane("/gone"), pane("/kept")],
                sizes: vec![120., 240.],
            },
            active: 1,
        };
        let saved = ProjectWorkspace {
            project: "/project".into(),
            host: Host::Local,
            worktree: Some("/project".into()),
            focused: 1,
            layout: Some(Layout::Leaf {
                value: Tabs {
                    items: vec![Tab::Unknown, tab],
                    active: 1,
                },
            }),
        };
        let mut workspace = Workspace {
            selection: None,
            projects: vec![saved],
        };
        workspace.prepare_with(
            &[super::super::ProjectRef::new(
                "/project".into(),
                Host::Local,
            )],
            &|p| p == Path::new("/kept"),
        );
        assert_eq!(workspace.projects[0].focused, 0);
        assert_eq!(
            workspace.projects[0].layout,
            Some(Layout::Leaf {
                value: Tabs {
                    items: vec![Tab::Terminal {
                        panes: pane("/kept"),
                        active: 0
                    }],
                    active: 0,
                }
            })
        );
        workspace.prepare_with(&[], &|_| true);
        assert!(workspace.projects.is_empty());
    }
}
