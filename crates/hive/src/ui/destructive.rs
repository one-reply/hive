//! Confirmations of actions that cannot be taken back: a delete, a discard, a force push.
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{AlertDialog, DialogFooter};
use gpui_kit::component::{Disableable as _, WindowExt as _};
use gpui_kit::*;
use std::rc::Rc;

/// Gives `alert` Cancel and a red `ok_text` button that runs `on_confirm`, and nothing else
/// that does. `on_confirm` says whether the dialog closes, as `AlertDialog::on_ok` does.
///
/// A dialog binds Enter to its confirming action wherever the focus is, and takes the focus
/// the moment it opens. Some of these open after a read that takes a while (what a force
/// push discards, whether a branch is merged), when the user is typing again: the Enter meant
/// for the commit message would then delete or overwrite. So here Enter does nothing; the
/// button is pressed with the pointer, or reached with Tab and pressed with Space. Escape
/// and Cancel close the dialog as everywhere else.
pub(crate) fn confirmation(
    alert: AlertDialog,
    ok_text: impl Into<SharedString>,
    on_confirm: impl Fn(&mut Window, &mut App) -> bool + 'static,
) -> AlertDialog {
    let confirm = confirm_button(ok_text).on_click(move |_, window, cx| {
        if on_confirm(window, cx) {
            window.close_dialog(cx);
        }
    });
    with_buttons(alert, confirm)
}

/// The same two buttons with the red one disabled, for the time its action cannot run;
/// `reason` is its tooltip.
pub(crate) fn unavailable(
    alert: AlertDialog,
    ok_text: impl Into<SharedString>,
    reason: &'static str,
) -> AlertDialog {
    with_buttons(
        alert,
        confirm_button(ok_text).disabled(true).tooltip(reason),
    )
}

/// A question with a safe answer besides the destructive one, such as Save and Discard for
/// unsaved changes. Enter and the `safe.0` button run `safe.1`; the red `destructive.0`
/// button runs `destructive.1` and, as in every confirmation here, only when it is pressed
/// itself. Both say whether the dialog closes. `on_cancel` runs when the question is
/// dismissed with Cancel or Escape, for a caller that owes someone the answer "no".
pub(crate) fn choice(
    alert: AlertDialog,
    safe: (
        impl Into<SharedString>,
        impl Fn(&mut Window, &mut App) -> bool + 'static,
    ),
    destructive: (
        impl Into<SharedString>,
        impl Fn(&mut Window, &mut App) -> bool + 'static,
    ),
    on_cancel: impl Fn(&mut Window, &mut App) + 'static,
) -> AlertDialog {
    let (safe_text, on_safe) = (safe.0.into(), Rc::new(safe.1));
    let (destructive_text, on_destructive) = destructive;
    let on_cancel = Rc::new(on_cancel);
    let cancelled_by_button = on_cancel.clone();
    let cancel = Button::new("destructive-cancel")
        .cursor_pointer()
        .label("Cancel")
        .on_click(move |_, window, cx| {
            cancelled_by_button(window, cx);
            window.close_dialog(cx);
        });
    let by_button = on_safe.clone();
    let safe_button = Button::new("destructive-safe")
        .cursor_pointer()
        .primary()
        .icon(Lucide::CornerDownLeft)
        .label(safe_text)
        .on_click(move |_, window, cx| {
            if by_button(window, cx) {
                window.close_dialog(cx);
            }
        });
    let destructive_button = confirm_button(destructive_text).on_click(move |_, window, cx| {
        if on_destructive(window, cx) {
            window.close_dialog(cx);
        }
    });
    alert
        .on_ok(move |_, window, cx| on_safe(window, cx))
        // Escape, which the library closes the dialog for once this returns true.
        .on_cancel(move |_, window, cx| {
            on_cancel(window, cx);
            true
        })
        .footer(
            DialogFooter::new()
                .child(cancel)
                .child(destructive_button)
                .child(safe_button),
        )
}

fn with_buttons(alert: AlertDialog, confirm: Button) -> AlertDialog {
    alert
        // What Enter triggers: it must neither run the action nor dismiss the question.
        .on_ok(|_, _, _| false)
        .footer(DialogFooter::new().child(cancel_button()).child(confirm))
}

fn cancel_button() -> Button {
    Button::new("destructive-cancel")
        .cursor_pointer()
        .label("Cancel")
        .on_click(|_, window, cx| window.close_dialog(cx))
}

fn confirm_button(ok_text: impl Into<SharedString>) -> Button {
    Button::new("destructive-confirm")
        .cursor_pointer()
        .danger()
        .label(ok_text)
}

/// A window with a dialog layer and the keys a person would press, for tests of the
/// confirmations built here.
#[cfg(test)]
pub(crate) mod testing {
    use gpui_kit::base::Root;
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::dialog::AlertDialog;
    use gpui_kit::prelude::*;
    use gpui_kit::{
        App, Context, KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext,
        Window, div,
    };

    struct Host;
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// An empty window that can show dialogs.
    pub(crate) fn window(cx: &mut TestAppContext) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Host);
            Root::new(view, window, cx)
        });
        cx
    }

    /// Lets a dialog opened just now mount and paint, so it takes the keys that follow.
    pub(crate) fn settle(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        // The first frame mounts the dialog layer, the second paints the dialog in it.
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// A window that shows the alert `build` describes, drawn and ready for input.
    pub(crate) fn open(
        cx: &mut TestAppContext,
        build: impl Fn(AlertDialog, &mut Window, &mut App) -> AlertDialog + 'static,
    ) -> &mut VisualTestContext {
        let cx = window(cx);
        show(cx, build);
        cx
    }

    /// Shows the alert `build` describes in a window that is already there.
    pub(crate) fn show(
        cx: &mut VisualTestContext,
        build: impl Fn(AlertDialog, &mut Window, &mut App) -> AlertDialog + 'static,
    ) {
        cx.update(|window, cx| window.open_alert_dialog(cx, build));
        settle(cx);
    }

    pub(crate) fn is_open(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| window.has_active_dialog(cx))
    }

    /// A key pressed and released, which is what activates a focused button.
    pub(crate) fn press(key: &'static str, cx: &mut VisualTestContext) {
        let keystroke = Keystroke::parse(key).expect("a key name");
        cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{is_open, open, press, show};
    use super::{choice, confirmation};
    use gpui_kit as gpui;
    use gpui_kit::component::dialog::AlertDialog;
    use gpui_kit::{App, TestAppContext, Window};
    use std::cell::Cell;
    use std::rc::Rc;

    /// How often Save, Discard and a dismissal were chosen.
    type Answers = [Rc<Cell<u32>>; 3];

    /// "Save changes?", where Save keeps the dialog open and Discard closes it.
    fn unsaved_changes(
        answers: &Answers,
    ) -> impl Fn(AlertDialog, &mut Window, &mut App) -> AlertDialog + 'static {
        let answers = answers.clone();
        move |alert, _, _| {
            let [saved, discarded, cancelled] = answers.clone();
            choice(
                alert.confirm().title("Save changes?"),
                ("Save", move |_: &mut Window, _: &mut App| {
                    saved.set(saved.get() + 1);
                    false
                }),
                ("Discard", move |_: &mut Window, _: &mut App| {
                    discarded.set(discarded.get() + 1);
                    true
                }),
                move |_, _| cancelled.set(cancelled.get() + 1),
            )
        }
    }

    fn counts(answers: &Answers) -> [u32; 3] {
        [answers[0].get(), answers[1].get(), answers[2].get()]
    }

    #[gpui::test]
    fn enter_confirms_an_ordinary_alert_the_moment_it_opens(cx: &mut TestAppContext) {
        // The library behaviour a destructive confirmation must not inherit.
        let confirmed = Rc::new(Cell::new(0));
        let count = confirmed.clone();
        let cx = open(cx, move |alert, _, _| {
            let count = count.clone();
            alert.confirm().title("Rename").on_ok(move |_, _, _| {
                count.set(count.get() + 1);
                true
            })
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(confirmed.get(), 1);
    }

    #[gpui::test]
    fn enter_on_a_focused_standard_cancel_button_cancels(cx: &mut TestAppContext) {
        let cancelled = Rc::new(Cell::new(false));
        let result = cancelled.clone();
        let cx = open(cx, move |alert, _, _| {
            let result = result.clone();
            alert
                .confirm()
                .title("Create branch")
                .on_ok(|_, _, _| panic!("Enter on Cancel ran the default action"))
                .on_cancel(move |_, _, _| {
                    result.set(true);
                    true
                })
        });
        press("tab", cx);
        press("enter", cx);
        assert!(cancelled.get());
        assert!(!is_open(cx));
    }

    #[gpui::test]
    fn enter_does_not_confirm_a_destructive_action_but_its_button_does(cx: &mut TestAppContext) {
        let confirmed = Rc::new(Cell::new(0));
        let count = confirmed.clone();
        let cx = open(cx, move |alert, _, _| {
            let count = count.clone();
            confirmation(
                alert.confirm().title("Force Push"),
                "Force Push",
                move |_, _| {
                    count.set(count.get() + 1);
                    true
                },
            )
        });
        for _ in 0..3 {
            cx.simulate_keystrokes("enter");
            cx.run_until_parked();
        }
        assert_eq!(confirmed.get(), 0);
        assert!(is_open(cx), "Enter must neither confirm nor dismiss");

        // Tab reaches Cancel, then the red button; Space presses what has the focus.
        cx.simulate_keystrokes("tab tab");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        press("enter", cx);
        assert_eq!(confirmed.get(), 0, "Enter stays inert on the button too");
        press("space", cx);
        assert_eq!(confirmed.get(), 1);
        assert!(!is_open(cx));
    }

    #[gpui::test]
    fn escape_cancels_a_destructive_confirmation(cx: &mut TestAppContext) {
        let confirmed = Rc::new(Cell::new(0));
        let count = confirmed.clone();
        let cx = open(cx, move |alert, _, _| {
            let count = count.clone();
            confirmation(alert.confirm().title("Delete"), "Delete", move |_, _| {
                count.set(count.get() + 1);
                true
            })
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(confirmed.get(), 0);
        assert!(!is_open(cx));
    }

    #[gpui::test]
    fn enter_takes_the_safe_answer_and_only_its_own_button_the_destructive_one(
        cx: &mut TestAppContext,
    ) {
        let answers = Answers::default();
        let cx = open(cx, unsaved_changes(&answers));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(counts(&answers), [1, 0, 0]);
        assert!(is_open(cx), "the safe answer said to keep the dialog");

        // Tab reaches Cancel, then the red button; Enter there is still the safe answer.
        cx.simulate_keystrokes("tab tab");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        press("enter", cx);
        assert_eq!(counts(&answers), [2, 0, 0]);
        press("space", cx);
        assert_eq!(counts(&answers), [2, 1, 0], "an answer is not a dismissal");
        assert!(!is_open(cx));
    }

    #[gpui::test]
    fn escape_and_cancel_tell_the_caller_the_question_was_dismissed(cx: &mut TestAppContext) {
        let answers = Answers::default();
        let cx = open(cx, unsaved_changes(&answers));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(counts(&answers), [0, 0, 1]);
        assert!(!is_open(cx));

        show(cx, unsaved_changes(&answers));
        // Tab reaches Cancel first; Space presses it.
        cx.simulate_keystrokes("tab");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        press("space", cx);
        assert_eq!(counts(&answers), [0, 0, 2]);
        assert!(!is_open(cx));
    }
}
