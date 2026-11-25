use super::*;
use crate::state::documents::Indentation;
use gpui_kit::base::input::{AddCursorAbove, AddCursorBelow};
use gpui_kit::component::{
    button::ButtonVariants as _,
    input::{Escape, Redo, SelectAll, Undo},
    menu::{DropdownMenu as _, PopupMenuItem},
};
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
    fn apply_line_edit(&self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        self.document
            .update(cx, |d, cx| d.edit_lines(command, window, cx));
    }
}

pub(super) fn status(document: &Entity<Document>, window: &mut Window, cx: &mut App) -> Div {
    let d = document.read(cx);
    let position = d.editor.read(cx).cursor_position();
    let count = d.cursor_count();
    let multiple = d.multiple_cursors_without_count();
    let indentation = d.indentation;
    let position_label = format!("Ln {}, Col {}", position.line + 1, position.character + 1);
    let hover = Hover::new("cursor-location", window, cx);
    let navigation = document.clone();
    let document = document.clone();
    h_flex()
        .justify_end()
        .gap_1()
        .px_2()
        .border_t_1()
        .border_color(cx.theme().border)
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .when(multiple, |bar| bar.child("Multiple cursors"))
        .when_some(count.filter(|count| *count > 1), |bar, count| {
            bar.child(format!("{count} cursors"))
        })
        .child(
            Button::new("cursor-location")
                .small()
                .cursor_pointer()
                .label(position_label)
                .tooltip("Go to line and column")
                .on_click(move |_, window, cx| goto_line(&navigation, window, cx))
                .smooth_hover(ButtonLook::Ghost, &hover, cx),
        )
        .child(
            Button::new("file-indentation")
                .small()
                .ghost()
                .cursor_pointer()
                .label(indentation.label())
                .tooltip("Change indentation for this file")
                .dropdown_menu(move |mut menu, _, _| {
                    for style in Indentation::ALL {
                        let document = document.clone();
                        menu = menu.item(
                            PopupMenuItem::new(style.label())
                                .checked(indentation == style)
                                .on_click(move |_, window, cx| {
                                    document.update(cx, |d, cx| {
                                        d.set_indentation(style, cx);
                                        d.editor.read(cx).focus_handle(cx).focus(window, cx);
                                    });
                                }),
                        );
                    }
                    menu
                }),
        )
}
