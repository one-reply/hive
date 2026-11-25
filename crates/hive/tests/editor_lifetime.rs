//! A closed editor must release its buffer even if no other text field gets focus.
use gpui_kit as gpui;
use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, Focusable as _, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, TestAppContext, Window,
    base::Root,
    component::{
        WindowExt as _,
        input::{Editor, EditorState},
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
