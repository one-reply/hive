//! Integration checks for the vendored editor in a rendered window.
use gpui_kit as gpui;
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, TestAppContext, Window,
    base::Root,
    component::{
        WindowExt as _,
        input::{Editor, EditorState, Position},
    },
    div,
};

struct Pane {
    editor: Option<Entity<EditorState>>,
    focus: FocusHandle,
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .children(self.editor.as_ref().map(Editor::new))
    }
}

#[gpui::test]
fn closing_the_last_focused_editor_releases_its_buffer(cx: &mut TestAppContext) {
    cx.update(gpui::init);
    let mut handles = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let editor = cx.new(|cx| EditorState::new(window, cx));
        let weak = editor.downgrade();
        let pane = cx.new(|cx| Pane {
            editor: Some(editor),
            focus: cx.focus_handle(),
        });
        handles = Some((pane.clone(), weak));
        Root::new(pane, window, cx)
    });
    let (pane, weak) = handles.unwrap();
    cx.update(|window, cx| {
        weak.upgrade().unwrap().focus_handle(cx).focus(window, cx);
        window.draw(cx).clear(cx);
        assert!(window.focused_input(cx).unwrap().as_editor().is_some());
        pane.update(cx, |pane, cx| {
            pane.focus.focus(window, cx);
            pane.editor = None;
            cx.notify();
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
}

#[gpui::test]
fn editing_a_nested_block_preserves_adjacent_and_enclosing_folds(cx: &mut TestAppContext) {
    cx.update(gpui::init);
    const SOURCE: &str = "function first() {
  a();
}
function outer() {
  if (ok) {
    b();
  }
}
function last() {
  c();
}
";
    let mut handle = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("typescript")
                .folding(true)
        });
        handle = Some(editor.clone());
        let pane = cx.new(|cx| Pane {
            editor: Some(editor),
            focus: cx.focus_handle(),
        });
        Root::new(pane, window, cx)
    });
    let editor = handle.unwrap();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        editor.update(cx, |state, cx| {
            state.set_value(SOURCE, window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|window, cx| {
        editor.update(cx, |state, cx| {
            assert!(state.display_map().is_fold_candidate(3));
            state.set_cursor_position(Position::new(5, 4), window, cx);
            state.insert("if (ok) {\n      nested();\n    }\n    ", window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|_, cx| {
        let starts: Vec<_> = (0..15)
            .filter(|line| editor.read(cx).display_map().is_fold_candidate(*line))
            .collect();
        assert_eq!(starts, [0, 3, 4, 5, 11]);
    });
}
