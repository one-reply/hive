//! Quitting Hive ends every terminal it runs, and closing the main window ends the ones in
//! it. Both ask first about the agents still working there, then about unsaved documents; a
//! "no" to either question leaves everything as it was.
//!
//! The agents come first: asking about them changes nothing, while the documents' question
//! saves or discards on its way, which a quit cancelled afterwards could not take back.
use super::*;
use crate::terminal::panel::{Ending, all_running_agents, asks_before_stopping, confirm_stopping};
use crate::ui::editor_panel::close;
use gpui_kit::base::Root;

impl Workspace {
    /// The agents running in this window's terminals, one line each.
    fn running_agents(&self, cx: &App) -> Vec<SharedString> {
        self.centers
            .values()
            .flat_map(|center| center.running_agents(cx))
            .collect()
    }
}

/// The agents running in the terminals `window` holds: none for a window other than the main
/// one, such as Settings, whose closing ends no terminal.
fn agents_in(window: &Window, cx: &App) -> Vec<SharedString> {
    window
        .root::<Root>()
        .flatten()
        .and_then(|root| root.read(cx).view().clone().downcast::<Workspace>().ok())
        .map(|workspace| workspace.read(cx).running_agents(cx))
        .unwrap_or_default()
}

/// Whether `window` may close without a question.
pub(crate) fn closes_at_once(window: &Window, cx: &App) -> bool {
    let unsaved = !AppState::get(cx).documents.read(cx).dirty(cx).is_empty();
    nothing_to_ask(unsaved, asks_before_stopping(&agents_in(window, cx), cx))
}

/// A window with unsaved documents or with agents to ask about stays until it is answered.
fn nothing_to_ask(unsaved_documents: bool, asks_about_agents: bool) -> bool {
    !unsaved_documents && !asks_about_agents
}

/// Closes `window` once the questions it owes are answered "yes".
pub(crate) fn close_window(window: &mut Window, cx: &mut App) {
    let agents = agents_in(window, cx);
    confirm_stopping(Ending::Window, agents, window, cx, |window, cx| {
        close::all(window, cx, |window, _| window.remove_window())
    });
}

/// A window for the quit's questions when every window is closed. The terminals of pipeline
/// steps outlive the main window, and their agents are still working; with nowhere to ask
/// about them, the main window comes back. `None` when there is nothing to ask.
pub(crate) fn window_to_ask_in(cx: &mut App) -> Option<AnyWindowHandle> {
    if !asks_before_stopping(&all_running_agents(cx), cx) {
        return None;
    }
    crate::open_main_window(cx);
    cx.windows().first().copied()
}

/// Answers a request to quit, asked in `window`. An update that restarts Hive makes the same
/// request, so it asks the same questions instead of ending working agents silently. A
/// dismissed question drops `request`, which cancels the quit.
pub(crate) fn quit(request: crate::quit::Request, window: &mut Window, cx: &mut App) {
    let agents = all_running_agents(cx);
    confirm_stopping(Ending::App, agents, window, cx, move |window, cx| {
        close::quit(request, window, cx)
    });
}

#[cfg(test)]
mod tests {
    use super::nothing_to_ask;

    #[test]
    fn a_window_closes_at_once_only_with_nothing_unsaved_and_no_agent_to_ask_about() {
        assert!(nothing_to_ask(false, false));
        assert!(!nothing_to_ask(true, false));
        assert!(
            !nothing_to_ask(false, true),
            "a running agent holds the window as an unsaved document does"
        );
        assert!(!nothing_to_ask(true, true));
    }
}
