use super::*;
use crate::ui::text_field::TextField;
use gpui_kit::component::{
    WindowExt as _,
    dialog::DialogButtonProps,
    input::{Input, InputState, Position},
};
use hive_core::files::location::Location;
use std::cell::Cell;

pub(crate) fn goto_line(document: &Entity<Document>, window: &mut Window, cx: &mut App) {
    let d = document.read(cx);
    if d.base.is_none() || d.content.shows_image() {
        return;
    }
    let current = d.editor.read(cx).cursor_position();
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("Line[:column]")
            .default_value(format!("{}:{}", current.line + 1, current.character + 1))
    });
    let document = document.clone();
    let focus = Cell::new(true);
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            let input = input.clone();
            window.defer(cx, move |window, cx| {
                input.update(cx, |i, cx| {
                    i.focus(window, cx);
                    i.set_selected_range(0..i.value().len(), cx);
                })
            });
        }
        let (input, document) = (input.clone(), document.clone());
        dialog
            .title("Go to Line / Column")
            .child(TextField::input(Input::new(&input), &input, cx))
            .button_props(
                DialogButtonProps::default()
                    .ok_text("Go")
                    .show_cancel(true)
                    .on_ok(move |_, window, cx| {
                        let Some(location) = Location::parse(&input.read(cx).value()) else {
                            return false;
                        };
                        let d = document.read(cx);
                        let request = OpenFile {
                            target: d.target.clone(),
                            path: d.path.clone(),
                            position: Some(Position::new(location.line, location.column)),
                            preview: false,
                            placement: Default::default(),
                        };
                        window.defer(cx, move |_, cx| {
                            AppState::get(cx)
                                .documents
                                .clone()
                                .update(cx, |docs, cx| docs.request(request, cx))
                        });
                        true
                    }),
            )
    });
}
