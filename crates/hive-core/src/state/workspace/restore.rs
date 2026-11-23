//! Restoring saved tabs at launch: tabs of deleted folders go, tabs of folders that cannot be
//! reached now (an unmounted volume) wait in `ProjectWorkspace::unavailable` for a later launch.

use super::{Layout, ProjectWorkspace, SplitAxis, Tab, Tabs, TerminalPane, Workspace};
use crate::host::Host;
use crate::state::ProjectRef;
use std::path::Path;

impl Workspace {
    /// Blocking: before opening windows, drop layouts of removed projects and tabs whose
    /// folders were deleted, and set aside tabs whose folders cannot be reached right now.
    pub fn prepare(&mut self, projects: &[ProjectRef]) {
        self.prepare_with(projects, &|path| path.is_dir());
    }

    pub(super) fn prepare_with(
        &mut self,
        projects: &[ProjectRef],
        is_dir: &impl Fn(&Path) -> bool,
    ) {
        self.projects.retain(|saved| {
            projects
                .iter()
                .any(|p| p.path == saved.project && p.host == saved.host)
        });
        for saved in &mut self.projects {
            if saved.host == Host::Local {
                saved.prepare(is_dir);
            }
        }
    }
}

impl ProjectWorkspace {
    fn prepare(&mut self, is_dir: &impl Fn(&Path) -> bool) {
        let mut unavailable = Vec::new();
        let mut returning = Vec::new();
        for tab in std::mem::take(&mut self.unavailable) {
            match tab.restore(is_dir) {
                Restored::Ready(tab) => returning.push(tab),
                Restored::Unreachable => unavailable.push(tab),
                Restored::Gone => {}
            }
        }
        let mut old_index = 0;
        let mut new_index = 0;
        let mut focused = 0;
        let layout = self.layout.as_ref().and_then(|layout| {
            layout.filter_map(&mut |tabs| {
                let mut active = 0;
                let mut items = Vec::new();
                for (index, tab) in tabs.items.iter().enumerate() {
                    match tab.restore(is_dir) {
                        Restored::Ready(tab) => {
                            if index <= tabs.active {
                                active = items.len();
                            }
                            if old_index <= self.focused {
                                focused = new_index;
                            }
                            items.push(tab);
                            new_index += 1;
                        }
                        Restored::Unreachable => unavailable.push(tab.clone()),
                        Restored::Gone => {}
                    }
                    old_index += 1;
                }
                (!items.is_empty()).then_some(Tabs { items, active })
            })
        });
        self.layout = append_tabs(layout, returning);
        self.focused = focused;
        self.unavailable = unavailable;
    }
}

/// Tabs coming back join the last group in depth-first order, so the saved focus index of
/// every other tab stays valid.
fn append_tabs(layout: Option<Layout<Tabs>>, tabs: Vec<Tab>) -> Option<Layout<Tabs>> {
    if tabs.is_empty() {
        return layout;
    }
    let group = |items| Layout::Leaf {
        value: Tabs { items, active: 0 },
    };
    let Some(mut layout) = layout else {
        return Some(group(tabs));
    };
    match last_group(&mut layout) {
        Some(last) => last.items.extend(tabs),
        // Prepared layouts have no empty splits or unknown nodes; this keeps the tabs anyway.
        None => {
            return Some(Layout::Split {
                axis: SplitAxis::Horizontal,
                children: vec![layout, group(tabs)],
                sizes: Vec::new(),
            });
        }
    }
    Some(layout)
}

fn last_group(layout: &mut Layout<Tabs>) -> Option<&mut Tabs> {
    match layout {
        Layout::Leaf { value } => Some(value),
        Layout::Split { children, .. } => children.last_mut().and_then(last_group),
        Layout::Unknown => None,
    }
}

/// What restoring a saved tab finds on disk.
#[derive(Debug, PartialEq)]
pub(super) enum Restored {
    /// The tab can open, possibly with a terminal moved to its worktree.
    Ready(Tab),
    /// Its folder was deleted (or the tab is invalid): drop it.
    Gone,
    /// Its folder cannot be reached right now: keep it for a later launch.
    Unreachable,
}

/// Whether a saved folder can be used. A missing folder whose parent exists was deleted; one
/// whose parent is missing too sits on something that is not there now, like an unmounted
/// volume, and may come back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Folder {
    Present,
    Gone,
    Unreachable,
}

impl Folder {
    fn of(path: &Path, is_dir: &impl Fn(&Path) -> bool) -> Self {
        if !path.is_absolute() {
            return Self::Gone;
        }
        if is_dir(path) {
            return Self::Present;
        }
        let parent_exists = path.parent().is_some_and(is_dir);
        tracing::warn!(?path, parent_exists, "workspace folder is unavailable");
        if parent_exists {
            Self::Gone
        } else {
            Self::Unreachable
        }
    }

    fn restore(self, tab: &Tab) -> Restored {
        match self {
            Self::Present => Restored::Ready(tab.clone()),
            Self::Gone => Restored::Gone,
            Self::Unreachable => Restored::Unreachable,
        }
    }
}

impl TerminalPane {
    /// Where the restored shell starts: its last directory, else its worktree. The worktree
    /// decides whether a pane whose directory is missing was deleted or is out of reach.
    fn restore(&self, is_dir: &impl Fn(&Path) -> bool) -> Result<Self, Folder> {
        let directory = Folder::of(&self.directory, is_dir);
        if directory == Folder::Present {
            return Ok(self.clone());
        }
        let Some(worktree) = self.worktree.as_deref() else {
            return Err(directory);
        };
        match Folder::of(worktree, is_dir) {
            Folder::Present => Ok(Self {
                directory: worktree.to_owned(),
                ..self.clone()
            }),
            missing => Err(missing),
        }
    }
}

impl Tab {
    pub(super) fn restore(&self, is_dir: &impl Fn(&Path) -> bool) -> Restored {
        match self {
            Self::Terminal { panes, active } => Self::restore_terminal(panes, *active, is_dir),
            Self::Diff { worktree, .. } => Folder::of(worktree, is_dir).restore(self),
            Self::File { worktree, path, .. } => {
                let inside = !path.as_os_str().is_empty()
                    && path
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(name) if name != ".git"));
                if !inside {
                    return Restored::Gone;
                }
                Folder::of(worktree, is_dir).restore(self)
            }
            Self::Unknown => Restored::Gone,
        }
    }

    /// A tab with a pane that cannot be reached waits whole, so none of its panes is lost;
    /// otherwise panes whose folders were deleted are dropped.
    fn restore_terminal(
        panes: &Layout<TerminalPane>,
        active: usize,
        is_dir: &impl Fn(&Path) -> bool,
    ) -> Restored {
        let mut unreachable = false;
        let mut old_index = 0;
        let mut new_index = 0;
        let mut selected = 0;
        let restored = panes.filter_map(&mut |pane| {
            let restored = pane.restore(is_dir);
            unreachable |= restored == Err(Folder::Unreachable);
            if restored.is_ok() {
                if old_index <= active {
                    selected = new_index;
                }
                new_index += 1;
            }
            old_index += 1;
            restored.ok()
        });
        match restored {
            _ if unreachable => Restored::Unreachable,
            Some(panes) => Restored::Ready(Self::Terminal {
                panes,
                active: selected,
            }),
            None => Restored::Gone,
        }
    }
}
