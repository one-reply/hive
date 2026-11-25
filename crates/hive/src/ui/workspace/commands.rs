//! Search the actions that can run at the focus which opened the palette.
use crate::ui::{key_hint, keymap};
use gpui_kit::{
    component::{
        WindowExt as _,
        command::{Command, CommandItem, CommandState},
        h_flex,
    },
    *,
};
use std::cell::Cell;

const WIDTH: Pixels = px(540.);

fn available(
    origin: &FocusHandle,
    window: &Window,
) -> Vec<(&'static keymap::Shortcut, KeyBinding)> {
    keymap::SHORTCUTS
        .iter()
        .filter_map(|shortcut| {
            let action = shortcut.action();
            if action.name() == super::ShowCommands.name()
                || !window.is_action_available_in(&*action, origin)
            {
                return None;
            }
            let binding = window.highest_precedence_binding_for_action_in(&*action, origin)?;
            Some((shortcut, binding))
        })
        .collect()
}

pub(super) fn open(window: &mut Window, cx: &mut App) {
    let Some(origin) = window.focused(cx) else {
        return;
    };
    let entries = available(&origin, window);
    let state = cx.new(|cx| CommandState::new(window, cx));
    let first = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            let state = state.clone();
            window.defer(cx, move |window, cx| {
                state.update(cx, |command, cx| command.focus(window, cx))
            });
        }
        let items = entries.iter().map(|(shortcut, binding)| {
            let label = shortcut.label;
            let binding = binding.clone();
            let initials: String = label
                .split_whitespace()
                .filter_map(|word| word.chars().next())
                .collect();
            CommandItem::new()
                .label(label)
                .keywords([shortcut.name().to_owned(), initials])
                .child(move |_, cx| {
                    h_flex().w_full().justify_between().child(label).child(
                        h_flex().gap_1().children(
                            binding
                                .keystrokes()
                                .iter()
                                .map(|stroke| key_hint::render(stroke.as_keystroke(), cx)),
                        ),
                    )
                })
        });
        let actions = entries.iter().map(|(s, _)| *s).collect::<Vec<_>>();
        let origin = origin.clone();
        dialog.title("Command palette").w(WIDTH).max_w(WIDTH).child(
            Command::new(&state)
                .searchable(true)
                .bordered(false)
                .placeholder("Find a command…")
                .items(items)
                .on_confirm(move |index, window, cx| {
                    let Some(command) = actions.get(index.row) else {
                        return;
                    };
                    let action = command.action();
                    let origin = origin.clone();
                    window.close_dialog(cx);
                    window.defer(cx, move |window, cx| {
                        origin.focus(window, cx);
                        origin.dispatch_action(&*action, window, cx);
                    });
                }),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::{available, open};
    use crate::ui::{
        editor_panel::ToggleComment,
        keymap::{CONTEXT, FILE_EDITOR_INPUT_CONTEXT},
        workspace::ShowCommands,
    };
    use gpui::{
        AppContext as _, AsKeystroke as _, Context, Entity, FocusHandle, Focusable as _,
        InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, Styled as _,
        TestAppContext, Window, div,
    };
    use gpui_kit as gpui;
    use gpui_kit::component::{
        Root,
        input::{Editor, EditorState},
    };
    use std::{cell::Cell, rc::Rc};

    struct Harness {
        editor: Entity<EditorState>,
        terminal: FocusHandle,
        calls: Rc<Cell<usize>>,
    }
    impl Render for Harness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .key_context(CONTEXT)
                .size_full()
                .on_action(cx.listener(|_, _: &ShowCommands, w, cx| open(w, cx)))
                .child(
                    div()
                        .key_context("EditorPanel")
                        .on_action(cx.listener(|this, _: &ToggleComment, _, _| {
                            this.calls.set(this.calls.get() + 1)
                        }))
                        .child(Editor::new(&self.editor)),
                )
                .child(
                    div()
                        .id("terminal")
                        .key_context("Terminal")
                        .track_focus(&self.terminal),
                )
        }
    }

    #[gpui::test]
    fn palette_uses_origin_context_and_reassigned_key_and_dispatches_to_origin(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-shift-p", ShowCommands, Some(CONTEXT)),
                KeyBinding::new(
                    "ctrl-alt-j ctrl-k",
                    ToggleComment,
                    Some(FILE_EDITOR_INPUT_CONTEXT),
                ),
            ]);
        });
        let mut handles = None;
        let calls = Rc::new(Cell::new(0));
        let (_, cx) = cx.add_window_view(|w, cx| {
            let editor = cx.new(|cx| EditorState::new(w, cx));
            let terminal = cx.focus_handle();
            handles = Some((editor.clone(), terminal.clone()));
            let view = cx.new(|_| Harness {
                editor,
                terminal,
                calls: calls.clone(),
            });
            Root::new(view, w, cx)
        });
        let (editor, terminal) = handles.unwrap();
        cx.update(|w, cx| {
            terminal.focus(w, cx);
            assert!(
                available(&terminal, w)
                    .iter()
                    .all(|(s, _)| s.label != "Toggle comment")
            );
            let input = editor.read(cx).focus_handle(cx);
            input.focus(w, cx);
            let entries = available(&input, w);
            let (_, binding) = entries
                .iter()
                .find(|(s, _)| s.label == "Toggle comment")
                .unwrap();
            let hints: Vec<_> = binding
                .keystrokes()
                .iter()
                .map(|k| crate::ui::key_hint::label(k.as_keystroke()))
                .collect();
            assert_eq!(hints, ["⌃⌥J", "⌃K"]);
        });
        for key in ["cmd-shift-p", "t c", "enter"] {
            cx.simulate_keystrokes(key);
            cx.run_until_parked();
            cx.update(|w, cx| w.draw(cx).clear(cx));
        }
        assert_eq!(calls.get(), 1);
    }
}
