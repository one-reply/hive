use super::*;
use crate::state::terminal_session::TerminalSession;
use hive_core::agent::launch::Launch;

impl TerminalPanel {
    pub(crate) fn active_session(&self, cx: &App) -> Entity<TerminalSession> {
        self.active.read(cx).session().clone()
    }
    pub(crate) fn worktree_panes(&self, worktree: &Path, cx: &App) -> Vec<Entity<TerminalView>> {
        let mut panes = Vec::new();
        self.panes.for_each(&mut |view| {
            if view.read(cx).cwd() == Some(worktree) && !view.read(cx).exited(cx) {
                panes.push(view.clone());
            }
        });
        panes
    }

    pub(crate) fn split_agent(
        &mut self,
        pane: Entity<TerminalView>,
        worktree: &Path,
        launch: Launch,
        axis: Axis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalSession> {
        self.active = pane;
        let view = cx.new(|cx| {
            TerminalView::on_host(
                &self.host,
                (Some(worktree), Some(worktree)),
                Some(launch),
                true,
                window,
                cx,
            )
        });
        let session = view.read(cx).session().clone();
        self.insert_pane(view, axis, window, cx);
        session
    }
}
