//! The body of a diff or conflict tab that has no widget to show.
use gpui_kit::{
    Div, FocusHandle, InteractiveElement as _, ParentElement as _, SharedString, Styled as _, div,
};

/// A message shown in place of a tab's Diff widget.
pub(crate) fn render(message: impl Into<SharedString>, focus: &FocusHandle) -> Div {
    // The Diff widget is absent here, but its focus must still route the tab's shortcuts.
    div().track_focus(focus).p_3().child(message.into())
}

#[cfg(test)]
mod tests {
    use crate::ui::{editor_panel::CloseFile, keymap::SHORTCUTS};
    use gpui_kit as gpui;
    use gpui_kit::component::diff::{Diff, DiffFile, DiffState};
    use gpui_kit::{
        AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
        KeyBinding, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext,
        Window, div,
    };

    struct Harness {
        display: Entity<DiffState>,
        message: Option<&'static str>,
        closes: usize,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let body = match self.message {
                Some(message) => super::render(message, &self.display.read(cx).focus_handle(cx))
                    .into_any_element(),
                None => Diff::new(&self.display).into_any_element(),
            };
            div()
                .size_full()
                .key_context("DiffPanel")
                .on_action(cx.listener(|this, _: &CloseFile, _, _| this.closes += 1))
                .child(body)
        }
    }

    #[gpui::test]
    fn close_shortcut_survives_empty_error_and_diff_transitions(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let shortcut = SHORTCUTS
                .iter()
                .find(|shortcut| shortcut.label == "Close file")
                .expect("close file shortcut");
            cx.bind_keys([KeyBinding::new(
                shortcut.default,
                CloseFile,
                Some(shortcut.context),
            )]);
        });
        let (view, cx) = cx.add_window_view(|window, cx| {
            let files =
                DiffFile::parse("--- a/file.txt\n+++ b/file.txt\n@@ -1 +1 @@\n-before\n+after\n")
                    .unwrap();
            let display = cx.new(|cx| DiffState::new(files, cx));
            display.read(cx).focus_handle(cx).focus(window, cx);
            Harness {
                display,
                message: None,
                closes: 0,
            }
        });
        for (index, message) in [
            None,
            Some("No changes in this view."),
            Some("Cannot read diff."),
            None,
        ]
        .into_iter()
        .enumerate()
        {
            view.update(cx, |view, cx| {
                view.message = message;
                cx.notify();
            });
            VisualTestContext::update(cx, |window, cx| window.draw(cx).clear(cx));
            cx.simulate_keystrokes("cmd-w");
            cx.run_until_parked();
            assert_eq!(
                view.read_with(cx, |view, _| view.closes),
                index + 1,
                "{message:?}"
            );
        }
    }
}
