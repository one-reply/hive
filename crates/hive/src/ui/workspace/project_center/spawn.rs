//! `hive agent start`: a named agent opens beside the pane that asked for it, without taking
//! the user's keyboard focus or switching what the user is looking at.
use super::*;
use crate::state::agents::SpawnRequest;
use crate::state::terminal_session::TerminalSession;
use hive_core::agent::launch::Launch;

impl ProjectCenter {
    pub(in crate::ui::workspace) fn spawn_agent(
        &mut self,
        request: &SpawnRequest,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<TerminalSession> {
        self.prune(cx);
        let launch = Launch::New(request.kind);
        let focused = window.focused(cx);
        let caller = (!request.tab)
            .then(|| {
                self.terminals.iter().find_map(|panel| {
                    let pane = panel.read(cx).pane_of(request.caller, cx)?;
                    Some((panel.clone(), pane))
                })
            })
            .flatten();
        let session = match caller {
            Some((panel, pane)) => {
                let session = panel.update(cx, |panel, cx| {
                    let session = panel.split_agent(
                        pane.clone(),
                        &request.worktree,
                        launch,
                        Axis::Horizontal,
                        window,
                        cx,
                    );
                    // The caller stays the tab's active pane, as if nothing moved under it.
                    panel.focus_pane(pane, window, cx);
                    session
                });
                restore_focus(focused, window, cx);
                session
            }
            None => {
                let panel = TerminalPanel::open_with_launch(
                    &self.host,
                    Some(&request.worktree),
                    Some(launch),
                    // Nobody at the keyboard asked for this tab. It takes the keyboard only
                    // from the tab it covers, and never from a hidden project or from a field
                    // the user is typing in elsewhere.
                    Reveal::Unprompted,
                    self.dock.downgrade(),
                    window,
                    cx,
                );
                let session = panel.read(cx).active_session(cx);
                self.add(panel_handle(panel.clone()), window, cx);
                self.terminals.push(panel);
                self.sync_alone(cx);
                session
            }
        };
        session.update(cx, |session, cx| {
            session.set_name(Some(request.name.clone()), cx)
        });
        session
    }
}

fn restore_focus(focused: Option<FocusHandle>, window: &mut Window, cx: &mut App) {
    match focused {
        Some(handle) => handle.focus(window, cx),
        None => window.blur(cx),
    }
}
