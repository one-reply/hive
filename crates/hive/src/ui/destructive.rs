//! Confirmations of actions that cannot be taken back: a delete, a discard, a force push.
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{AlertDialog, DialogFooter};
use gpui_kit::component::{Disableable as _, WindowExt as _};
use gpui_kit::*;

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

fn with_buttons(alert: AlertDialog, confirm: Button) -> AlertDialog {
    let cancel = Button::new("destructive-cancel")
        .cursor_pointer()
        .label("Cancel")
        .on_click(|_, window, cx| window.close_dialog(cx));
    alert
        // What Enter triggers: it must neither run the action nor dismiss the question.
        .on_ok(|_, _, _| false)
        .footer(DialogFooter::new().child(cancel).child(confirm))
}

fn confirm_button(ok_text: impl Into<SharedString>) -> Button {
    Button::new("destructive-confirm")
        .cursor_pointer()
        .danger()
        .label(ok_text)
}

#[cfg(test)]
mod tests {
    use super::confirmation;
    use gpui_kit as gpui;
    use gpui_kit::base::Root;
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::dialog::AlertDialog;
    use gpui_kit::prelude::*;
    use gpui_kit::{
        App, Context, KeyDownEvent, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext,
        Window, div,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    struct Host;
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// A window that shows the alert `build` describes, drawn and ready for input.
    fn open(
        cx: &mut TestAppContext,
        build: impl Fn(AlertDialog, &mut Window, &mut App) -> AlertDialog + 'static,
    ) -> &mut VisualTestContext {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Host);
            Root::new(view, window, cx)
        });
        cx.update(|window, cx| window.open_alert_dialog(cx, build));
        cx.run_until_parked();
        // The first frame mounts the dialog layer, the second paints the dialog in it.
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx
    }

    fn is_open(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| window.has_active_dialog(cx))
    }

    /// A key pressed and released, which is what activates a focused button.
    fn press(key: &'static str, cx: &mut VisualTestContext) {
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
}
