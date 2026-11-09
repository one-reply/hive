//! Session subscriptions and in-place agent/shell handoff.
use super::*;

impl TerminalView {
    pub(super) fn subscribe_session(
        session: &Entity<TerminalSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        vec![
            cx.observe(session, |_, _, cx| cx.notify()),
            cx.subscribe_in(session, window, |this, _, event, window, cx| match event {
                SessionEvent::TitleChanged => cx.emit(TerminalViewEvent::TitleChanged),
                SessionEvent::DirectoryChanged => cx.emit(TerminalViewEvent::DirectoryChanged),
                SessionEvent::Exited if this.shell_after_exit => this.continue_in_shell(window, cx),
                SessionEvent::Exited => cx.emit(TerminalViewEvent::Exited),
                SessionEvent::PasteFailed(error) => window.push_notification(
                    Notification::error(error.clone()).title("Could not paste image"),
                    cx,
                ),
                SessionEvent::StartAgent(agent, placement) => {
                    cx.emit(TerminalViewEvent::StartAgent(*agent, *placement));
                }
                SessionEvent::LaunchFailed(error) => window.push_notification(
                    Notification::error(error.clone()).title("Could not start agent"),
                    cx,
                ),
            }),
        ]
    }

    /// Replaces the ended agent with a shell in its last directory, keeping the tile in place.
    pub(super) fn continue_in_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shell_after_exit = false;
        let directory = self.restore_directory(cx);
        let worktree = self.cwd.clone();
        let focused = self.focus_handle.is_focused(window);
        let host = self.session.read(cx).host.clone();
        let session = self
            .shell
            .take()
            .filter(|shell| {
                let session = shell.read(cx);
                !session.exited && !matches!(session.status, SessionStatus::Failed(_))
            })
            .unwrap_or_else(|| {
                cx.new(|cx| {
                    let mut session =
                        TerminalSession::new(host, Some(directory), worktree, None, true, cx);
                    session.set_focused(focused);
                    session
                })
            });
        self.replace_session(session, window, cx);
    }

    pub(crate) fn start_agent(
        &mut self,
        launch: Launch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let directory = self.restore_directory(cx);
        let worktree = self.cwd.clone();
        let host = self.session.read(cx).host.clone();
        let session = cx.new(|cx| {
            TerminalSession::new(host, Some(directory), worktree, Some(launch), true, cx)
        });
        if self.shell.is_none() {
            self.shell = Some(self.session.clone());
        }
        self.shell_after_exit = true;
        self.replace_session(session, window, cx);
    }

    fn replace_session(
        &mut self,
        session: Entity<TerminalSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session
            .update(cx, |session, _| session.set_focused(false));
        session.update(cx, |session, _| {
            session.set_focused(self.focus_handle.is_focused(window));
            if let Some(grid) = self.grid {
                session.resize(grid.cols, grid.rows);
            }
        });
        self._session_subscriptions = Self::subscribe_session(&session, window, cx);
        self.session = session;
        self.stop_selection();
        self.scroll_remainder = 0.;
        cx.emit(TerminalViewEvent::TitleChanged);
        cx.emit(TerminalViewEvent::DirectoryChanged);
        cx.notify();
    }
}
