//! Tab accepts the open suggestion, as in VS Code. gpui-component binds Tab only to
//! indentation (`IndentInline`) and lets the suggestion menu answer Enter alone.
use gpui_kit::component::input::{EditorState, Enter, IndentInline};
use gpui_kit::{App, Entity, Window};

/// Capture-phase listener for an element around `editor`: while the suggestion menu is open,
/// Tab inserts the selected item the way Enter does; otherwise the editor indents as before.
pub(super) fn accept_on_tab(
    editor: Entity<EditorState>,
) -> impl Fn(&IndentInline, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        let accept = Box::new(Enter {
            secondary: false,
            shift: false,
        });
        if editor.update(cx, |editor, cx| {
            editor.route_overlay_action(accept, window, cx)
        }) {
            cx.stop_propagation();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::accept_on_tab;
    // `#[gpui::test]` expands to `gpui::` paths.
    use gpui_kit as gpui;
    use gpui_kit::component::input::{CompletionProvider, Editor, EditorState, Rope};
    use gpui_kit::{
        App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
        ParentElement as _, Render, Styled as _, Task, TestAppContext, VisualTestContext, Window,
        div,
    };
    use lsp_types::{CompletionContext, CompletionItem, CompletionResponse};
    use std::rc::Rc;

    /// Offers `total` after any typed character.
    struct Total;

    impl CompletionProvider for Total {
        fn completions(
            &self,
            _: &Rope,
            _: usize,
            _: CompletionContext,
            _: &mut Window,
            _: &mut App,
        ) -> Task<gpui_kit::Result<CompletionResponse>> {
            let item = CompletionItem {
                label: "total".into(),
                ..Default::default()
            };
            Task::ready(Ok(CompletionResponse::Array(vec![item])))
        }

        fn is_completion_trigger(&self, _: usize, _: &str, _: &mut App) -> bool {
            true
        }
    }

    struct Harness {
        editor: Entity<EditorState>,
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .capture_action(accept_on_tab(self.editor.clone()))
                .child(Editor::new(&self.editor))
        }
    }

    /// The editor text after pressing `keys` in an empty editor that suggests `total`.
    fn text_after(keys: &str, cx: &mut TestAppContext) -> String {
        cx.update(gpui_kit::init);
        let mut editor = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|cx| EditorState::new(window, cx));
            state.update(cx, |state, cx| {
                state.lsp_mut().completion_provider = Some(Rc::new(Total));
                state.focus(window, cx);
            });
            editor = Some(state.clone());
            Harness { editor: state }
        });
        let editor = editor.expect("the window builds the harness");
        for key in keys.split_whitespace() {
            VisualTestContext::update(cx, |window, cx| window.draw(cx).clear(cx));
            cx.simulate_keystrokes(key);
            cx.run_until_parked();
        }
        cx.read(|cx| editor.read(cx).text().to_string())
    }

    #[gpui::test]
    fn tab_inserts_the_open_suggestion(cx: &mut TestAppContext) {
        assert_eq!(text_after("t tab", cx), "total");
    }

    #[gpui::test]
    fn tab_indents_once_the_menu_is_dismissed(cx: &mut TestAppContext) {
        let text = text_after("t escape tab", cx);
        assert!(text.starts_with('t') && text.len() > 1, "{text:?}");
        assert_ne!(text, "total");
    }
}
