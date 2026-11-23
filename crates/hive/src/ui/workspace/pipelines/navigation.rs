//! Moving between the panel's screens without losing what the user typed on the one in view.
use super::*;
use hive_core::pipeline::Ticket;

/// Where a request to leave the current screen leads. A breadcrumb click, a notification and
/// an agent reveal all arrive here, whatever screen is open at that moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Destination {
    Library,
    /// A run, opened on the live session of `attempt` when one is named.
    Run {
        id: u64,
        attempt: Option<Ticket>,
    },
}

/// Text on the screen in view that exists nowhere else yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unsaved {
    Template,
    Plan,
    Feedback,
}

impl Unsaved {
    fn description(self) -> &'static str {
        match self {
            Self::Template => "The changes to this template have not been saved.",
            Self::Plan => "The edits to the plan have not been approved.",
            Self::Feedback => "The feedback you typed has not been sent.",
        }
    }
}

/// What a request to show `destination` does to the screen in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Navigation {
    /// The run is already on screen: its view stays, with everything typed into it.
    Stay,
    Go,
    Ask(Unsaved),
}

fn navigation(
    shown_run: Option<u64>,
    unsaved: Option<Unsaved>,
    destination: Destination,
) -> Navigation {
    match destination {
        Destination::Run { id, .. } if shown_run == Some(id) => Navigation::Stay,
        _ => unsaved.map_or(Navigation::Go, Navigation::Ask),
    }
}

impl PipelinePanel {
    /// Goes to `destination`. A screen holding unsaved text is replaced only after the user
    /// agrees to drop it.
    pub(super) fn navigate(
        &mut self,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown_run = match &self.screen {
            Screen::Run(view) => Some(view.read(cx).id()),
            _ => None,
        };
        match navigation(shown_run, self.unsaved(cx), destination) {
            Navigation::Stay => self.show_attempt(destination, window, cx),
            Navigation::Go => self.go(destination, window, cx),
            Navigation::Ask(unsaved) => self.ask_to_leave(unsaved, destination, window, cx),
        }
    }

    fn unsaved(&self, cx: &App) -> Option<Unsaved> {
        match &self.screen {
            Screen::Editor(editor) => editor.read(cx).is_modified(cx).then_some(Unsaved::Template),
            Screen::Run(view) => view.read(cx).unsaved(cx),
            // The launch form saves itself as the project's draft.
            Screen::Launch(_) | Screen::Library => None,
        }
    }

    /// Replaces the screen without asking: the caller has nothing left to lose.
    pub(super) fn go(
        &mut self,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match destination {
            Destination::Library => {
                self.screen = Screen::Library;
                self.focus.focus(window, cx);
            }
            Destination::Run { id, .. } => {
                self.screen = Screen::Run(cx.new(|cx| run::RunView::new(id, window, cx)));
                self.show_attempt(destination, window, cx);
            }
        }
        cx.notify();
    }

    /// Opens the live session the destination names on the run in view.
    fn show_attempt(&self, destination: Destination, window: &mut Window, cx: &mut Context<Self>) {
        let Destination::Run {
            attempt: Some(ticket),
            ..
        } = destination
        else {
            return;
        };
        if let Screen::Run(view) = &self.screen {
            view.update(cx, |view, cx| view.terminal(ticket, window, cx));
        }
    }

    fn ask_to_leave(
        &self,
        unsaved: Unsaved,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let panel = panel.clone();
            let alert = alert
                .confirm()
                .title("Discard unsaved changes?")
                .description(unsaved.description());
            crate::ui::destructive::confirmation(alert, "Discard", move |window, cx| {
                let _ = panel.update(cx, |panel, cx| panel.go(destination, window, cx));
                true
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{Destination, Navigation, Unsaved, navigation};

    fn run(id: u64) -> Destination {
        Destination::Run { id, attempt: None }
    }

    #[test]
    fn a_run_already_on_screen_keeps_its_view_and_what_was_typed_into_it() {
        assert_eq!(
            navigation(Some(7), Some(Unsaved::Plan), run(7)),
            Navigation::Stay
        );
        assert_eq!(navigation(Some(7), None, run(7)), Navigation::Stay);
    }

    #[test]
    fn leaving_unsaved_text_asks_first_and_a_clean_screen_just_goes() {
        assert_eq!(
            navigation(Some(7), Some(Unsaved::Feedback), run(8)),
            Navigation::Ask(Unsaved::Feedback)
        );
        assert_eq!(
            navigation(Some(7), Some(Unsaved::Plan), Destination::Library),
            Navigation::Ask(Unsaved::Plan)
        );
        assert_eq!(
            navigation(None, Some(Unsaved::Template), run(7)),
            Navigation::Ask(Unsaved::Template)
        );
        assert_eq!(navigation(Some(7), None, run(8)), Navigation::Go);
        assert_eq!(navigation(None, None, Destination::Library), Navigation::Go);
    }
}
