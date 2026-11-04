//! One project's center: its own dock of terminal and diff tabs. A center stays alive while
//! another project is shown, so switching projects in the sidebar switches whole tab sets.
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::dock::{BasePanelView, DockEvent, Panel};

use super::center_dock::CenterDock;
use super::*;
use crate::state::git::DiffRequest;

mod persistence;

pub(super) struct ProjectCenter {
    dock: Entity<DockArea>,
    /// Tabs we opened; pruned when the dock no longer holds them.
    terminals: Vec<Entity<TerminalPanel>>,
    diffs: Vec<Entity<DiffPanel>>,
    /// Space the tab bar leaves free at its left end, shared with the dock's renderer.
    tab_inset: Rc<Cell<Pixels>>,
    _observer: Subscription,
    _layout: Subscription,
    focused: Option<PanelId>,
}

impl ProjectCenter {
    pub(super) fn new(project: &Path, window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let id = format!("center:{}", project.display());
        let tab_inset = Rc::new(Cell::new(px(0.)));
        let dock = CenterDock::dock_area(id, tab_inset.clone(), window, cx);
        let project = project.to_path_buf();
        // Tabs closed from the dock (× button, exited shell) must drop out of our lists so
        // the welcome screen comes back and the last tab draws its own ×.
        let observed_project = project.clone();
        let observer = cx.observe(&dock, move |workspace, _, cx| {
            if let Some(center) = workspace.centers.get_mut(&observed_project) {
                center.prune(cx);
                center.sync_alone(cx);
            }
            workspace.sync_active_worktrees(cx);
            cx.notify();
        });
        let layout = cx.subscribe_in(&dock, window, move |workspace, _, event, window, cx| {
            if matches!(event, DockEvent::LayoutChanged) {
                workspace.save_center(&project, window, cx);
            }
        });
        Self {
            dock,
            terminals: Vec::new(),
            diffs: Vec::new(),
            tab_inset,
            _observer: observer,
            _layout: layout,
            focused: None,
        }
    }

    pub(super) fn dock(&self) -> &Entity<DockArea> {
        &self.dock
    }

    /// The cell the dock's tab bar reads its left inset from. The dock is its own view, so
    /// the workspace hands the value over through this cell rather than through render args.
    pub(super) fn tab_inset(&self) -> Rc<Cell<Pixels>> {
        self.tab_inset.clone()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.terminals.is_empty() && self.diffs.is_empty()
    }

    /// Forgets panels the dock has already removed.
    fn prune(&mut self, cx: &App) {
        let dock = self.dock.read(cx);
        self.terminals
            .retain(|panel| dock.panel(PanelId::from(panel.entity_id())).is_some());
        self.diffs
            .retain(|panel| dock.panel(PanelId::from(panel.entity_id())).is_some());
    }

    /// Worktree paths with an open terminal here.
    pub(super) fn terminal_paths<'a>(&'a self, cx: &'a App) -> impl Iterator<Item = PathBuf> + 'a {
        self.terminals
            .iter()
            .filter_map(|panel| panel.read(cx).cwd(cx))
    }

    /// Activates and focuses the terminal tab whose shell runs in `cwd`, or opens one. Tells
    /// whether a tab is now showing: `false` when no tab existed and the PTY could not be spawned.
    pub(super) fn show_terminal(&mut self, cwd: &Path, window: &mut Window, cx: &mut App) -> bool {
        self.prune(cx);
        let existing = self
            .terminals
            .iter()
            .find(|panel| panel.read(cx).cwd(cx).as_deref() == Some(cwd))
            .cloned();
        let Some(panel) = existing else {
            return self.open_terminal(Some(cwd), window, cx);
        };
        self.select(&panel, window, cx);
        // Selecting a tab the dock already displays is a no-op there, including its focus:
        // the sidebar shows this tab on selection, so by the time the Git panel asks for it
        // the shell is usually visible already and only the focus is missing.
        panel.update(cx, |panel, cx| panel.focus_active(window, cx));
        true
    }

    /// Shows the tab and pane running terminal `session`. `false` when this center has none.
    pub(super) fn reveal_session(
        &mut self,
        session: EntityId,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        self.prune(cx);
        let found = self.terminals.iter().find_map(|panel| {
            let pane = panel.read(cx).pane_of(session, cx)?;
            Some((panel.clone(), pane))
        });
        let Some((panel, pane)) = found else {
            return false;
        };
        self.select(&panel, window, cx);
        panel.update(cx, |panel, cx| panel.focus_pane(pane, window, cx));
        true
    }

    /// Opens a shell in `cwd` (`None` = the user's home directory) as a new tab.
    pub(super) fn open_terminal(
        &mut self,
        cwd: Option<&Path>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let panel = TerminalPanel::open(cwd, self.dock.downgrade(), window, cx);
        self.add(panel_handle(panel.clone()), window, cx);
        self.terminals.push(panel);
        self.sync_alone(cx);
        true
    }

    /// Activates the diff tab for `request`, or opens one.
    pub(super) fn show_diff(&mut self, request: DiffRequest, window: &mut Window, cx: &mut App) {
        self.prune(cx);
        let existing = self
            .diffs
            .iter()
            .find(|panel| panel.read(cx).review.read(cx).request == request)
            .cloned();
        if let Some(panel) = existing {
            self.select(&panel, window, cx);
            return;
        }
        let panel = cx.new(|cx| DiffPanel::new(request, self.dock.downgrade(), window, cx));
        self.add(panel_handle(panel.clone()), window, cx);
        self.diffs.push(panel);
        self.sync_alone(cx);
    }

    fn select<P: Panel>(&self, panel: &Entity<P>, window: &mut Window, cx: &mut App) {
        let id = PanelId::from(panel.entity_id());
        self.dock
            .update(cx, |dock, cx| dock.select_panel(id, window, cx));
    }

    fn add(&self, panel: Arc<dyn BasePanelView>, window: &mut Window, cx: &mut App) {
        self.dock.update(cx, |dock, cx| {
            dock.add_panel_view(panel, DockPlacement::Center, None, window, cx)
        });
    }

    /// Tells every tab whether it is the only one, so the last tab can draw its own close
    /// button (the tab bar hides it then).
    fn sync_alone(&self, cx: &mut App) {
        let alone = self.terminals.len() + self.diffs.len() == 1;
        for panel in &self.terminals {
            panel.update(cx, |panel, cx| panel.set_alone(alone, cx));
        }
        for panel in &self.diffs {
            panel.update(cx, |panel, cx| panel.set_alone(alone, cx));
        }
    }
}
