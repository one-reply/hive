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
    /// The launch form with the project's draft.
    Draft,
}

/// What of the screen in view decides whether a destination is already shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shown {
    Run(u64),
    Launch,
    Other,
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
    /// The run or the launch form is already on screen: its view stays, with everything
    /// typed into it.
    Stay,
    Go,
    Ask(Unsaved),
}

fn navigation(shown: Shown, unsaved: Option<Unsaved>, destination: Destination) -> Navigation {
    match (shown, destination) {
        (Shown::Run(shown), Destination::Run { id, .. }) if shown == id => Navigation::Stay,
        (Shown::Launch, Destination::Draft) => Navigation::Stay,
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
        let shown = match &self.screen {
            Screen::Run(view) => Shown::Run(view.read(cx).id()),
            Screen::Launch(_) => Shown::Launch,
            Screen::Library | Screen::Editor(_) => Shown::Other,
        };
        match navigation(shown, self.unsaved(cx), destination) {
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
        // A form left on screen keeps living as the project's draft, so its microphone
        // would stay open. Deferred: the form itself may be what navigates away.
        if let Screen::Launch(form) = &self.screen {
            let form = form.downgrade();
            cx.defer(move |cx| {
                let _ = form.update(cx, |form, cx| form.stop_dictation(cx));
            });
        }
        match destination {
            Destination::Run { id, .. } => {
                self.screen = Screen::Run(cx.new(|cx| run::RunView::new(id, window, cx)));
                self.show_attempt(destination, window, cx);
            }
            Destination::Draft | Destination::Library => {
                // A draft started or discarded since it was asked for leaves the library.
                let draft = (destination == Destination::Draft)
                    .then(|| self.draft_template(cx))
                    .flatten();
                match draft {
                    Some(template) => self.launch(template, window, cx),
                    None => {
                        self.screen = Screen::Library;
                        self.focus.focus(window, cx);
                    }
                }
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
    use super::{Destination, Navigation, Shown, Unsaved, navigation};

    fn run(id: u64) -> Destination {
        Destination::Run { id, attempt: None }
    }

    #[test]
    fn a_run_already_on_screen_keeps_its_view_and_what_was_typed_into_it() {
        assert_eq!(
            navigation(Shown::Run(7), Some(Unsaved::Plan), run(7)),
            Navigation::Stay
        );
        assert_eq!(navigation(Shown::Run(7), None, run(7)), Navigation::Stay);
    }

    #[test]
    fn a_draft_whose_form_is_already_on_screen_keeps_that_form() {
        assert_eq!(
            navigation(Shown::Launch, None, Destination::Draft),
            Navigation::Stay
        );
    }

    #[test]
    fn a_draft_replaces_another_screen_like_any_other_destination() {
        assert_eq!(
            navigation(Shown::Other, None, Destination::Draft),
            Navigation::Go
        );
        assert_eq!(
            navigation(Shown::Run(7), None, Destination::Draft),
            Navigation::Go
        );
        assert_eq!(
            navigation(Shown::Other, Some(Unsaved::Template), Destination::Draft),
            Navigation::Ask(Unsaved::Template)
        );
        // The form saves itself as the draft, so leaving it for a run asks nothing.
        assert_eq!(navigation(Shown::Launch, None, run(7)), Navigation::Go);
    }

    #[test]
    fn leaving_unsaved_text_asks_first_and_a_clean_screen_just_goes() {
        assert_eq!(
            navigation(Shown::Run(7), Some(Unsaved::Feedback), run(8)),
            Navigation::Ask(Unsaved::Feedback)
        );
        assert_eq!(
            navigation(Shown::Run(7), Some(Unsaved::Plan), Destination::Library),
            Navigation::Ask(Unsaved::Plan)
        );
        assert_eq!(
            navigation(Shown::Other, Some(Unsaved::Template), run(7)),
            Navigation::Ask(Unsaved::Template)
        );
        assert_eq!(navigation(Shown::Run(7), None, run(8)), Navigation::Go);
        assert_eq!(
            navigation(Shown::Other, None, Destination::Library),
            Navigation::Go
        );
    }
}
