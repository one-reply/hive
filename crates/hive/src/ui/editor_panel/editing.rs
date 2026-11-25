use super::*;
use crate::state::documents::Indentation;
use gpui_kit::base::input::{AddCursorAbove, AddCursorBelow};
use gpui_kit::component::input::{Escape, Redo, SelectAll, Undo};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use hive_core::line_edit::Command;

pub(super) fn bind(body: Stateful<Div>, cx: &mut Context<EditorPanel>) -> Stateful<Div> {
    body.on_action(cx.listener(|this, _: &SelectNextOccurrence, _, cx| {
        this.document
            .update(cx, |d, cx| d.select_next_occurrence(cx));
    }))
    .on_action(
        cx.listener(|this, _: &ToggleComment, w, cx| this.apply_line_edit(Command::Comment, w, cx)),
    )
    .on_action(
        cx.listener(|this, _: &MoveLinesUp, w, cx| this.apply_line_edit(Command::MoveUp, w, cx)),
    )
    .on_action(
        cx.listener(|this, _: &MoveLinesDown, w, cx| {
            this.apply_line_edit(Command::MoveDown, w, cx)
        }),
    )
    .on_action(cx.listener(|this, _: &DuplicateLinesUp, w, cx| {
        this.apply_line_edit(Command::DuplicateUp, w, cx)
    }))
    .on_action(cx.listener(|this, _: &DuplicateLinesDown, w, cx| {
        this.apply_line_edit(Command::DuplicateDown, w, cx)
    }))
    .capture_action(cx.listener(|this, _: &AddCursorAbove, _, cx| {
        this.document
            .update(cx, |d, cx| d.vertical_cursor(true, cx));
        cx.propagate();
    }))
    .capture_action(cx.listener(|this, _: &AddCursorBelow, _, cx| {
        this.document
            .update(cx, |d, cx| d.vertical_cursor(false, cx));
        cx.propagate();
    }))
    .capture_action(cx.listener(|this, _: &SelectAll, _, cx| {
        this.document
            .update(cx, |d, cx| d.native_selections(false, cx));
        cx.propagate();
    }))
    .capture_action(cx.listener(|this, _: &Undo, _, cx| {
        this.document
            .update(cx, |d, cx| d.native_selection_motion(cx));
        cx.propagate();
    }))
    .capture_action(cx.listener(|this, _: &Redo, _, cx| {
        this.document
            .update(cx, |d, cx| d.native_selection_motion(cx));
        cx.propagate();
    }))
    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
        if matches!(
            event.keystroke.key.as_str(),
            "left" | "right" | "up" | "down" | "home" | "end" | "pageup" | "pagedown"
        ) {
            this.document
                .update(cx, |d, cx| d.native_selection_motion(cx));
        }
    }))
    .capture_action(cx.listener(|this, _: &Escape, _, cx| {
        let editor = this.document.read(cx).editor.read(cx);
        if !editor.completion_menu_state().open && !editor.code_action_menu_state().open {
            this.document
                .update(cx, |d, cx| d.native_selections(false, cx));
        }
        cx.propagate();
    }))
}

impl EditorPanel {
    pub(crate) fn indentation_menu(&self, menu: PopupMenu, cx: &App) -> PopupMenu {
        let current = self.document.read(cx).indentation;
        let mut menu = menu.label("File indentation");
        for style in Indentation::ALL {
            let document = self.document.clone();
            menu = menu.item(
                PopupMenuItem::new(style.label())
                    .checked(style == current)
                    .on_click(move |_, _, cx| {
                        document.update(cx, |d, cx| d.set_indentation(style, cx));
                    }),
            );
        }
        menu
    }

    fn apply_line_edit(&self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        self.document
            .update(cx, |d, cx| d.edit_lines(command, window, cx));
    }
}
