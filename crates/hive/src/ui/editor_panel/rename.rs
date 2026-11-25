use super::*;
use crate::{state::documents::RenamePreview, ui::text_field::TextField};
use gpui_kit::component::{
    WindowExt as _,
    dialog::DialogButtonProps,
    input::{Input, InputState},
    notification::Notification,
};
use std::sync::Arc;

pub(super) fn open(document: &Entity<Document>, window: &mut Window, cx: &mut App) {
    let editor = document.read(cx).editor.read(cx);
    let current = editor
        .text()
        .word_range(editor.cursor())
        .map(|r| editor.text().slice(r).to_string())
        .unwrap_or_default();
    let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
    input.update(cx, |input, cx| {
        input.set_selected_range(0..input.value().len(), cx)
    });
    let document = document.clone();
    let focus = input.clone();
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let (input, document) = (input.clone(), document.clone());
        dialog
            .title("Rename symbol")
            .child(TextField::input(Input::new(&input), &input, cx))
            .button_props(
                DialogButtonProps::default()
                    .ok_text("Preview rename")
                    .show_cancel(true)
                    .on_ok(move |_, window, cx| {
                        let name = input.read(cx).value().to_string();
                        if name.trim().is_empty() {
                            return false;
                        }
                        prepare(document.clone(), name, window, cx);
                        true
                    }),
            )
    });
    window.defer(cx, move |window, cx| {
        focus.update(cx, |input, cx| input.focus(window, cx))
    });
}

fn prepare(document: Entity<Document>, name: String, window: &mut Window, cx: &mut App) {
    let task = Document::rename(&document, name.clone(), cx);
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(preview) => review(document, name, preview, window, cx),
                Err(error) => window.push_notification(Notification::error(error), cx),
            });
        })
        .detach();
}

fn review(
    document: Entity<Document>,
    name: String,
    preview: Arc<RenamePreview>,
    window: &mut Window,
    cx: &mut App,
) {
    let rows = Arc::new(preview.rows());
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let rows = rows.clone();
        let (document, name, preview) = (document.clone(), name.clone(), preview.clone());
        dialog
            .title(format!("Rename to {name} · {} files", preview.file_count()))
            .w(px(740.))
            .max_w(px(740.))
            .child(
                uniform_list("rename-preview", rows.len(), move |range, _, _| {
                    range
                        .map(|i| {
                            div()
                                .px_2()
                                .h(px(25.))
                                .overflow_hidden()
                                .child(rows[i].clone())
                        })
                        .collect::<Vec<_>>()
                })
                .h(px(340.)),
            )
            .button_props(
                DialogButtonProps::default()
                    .ok_text("Apply to buffers")
                    .show_cancel(true)
                    .on_ok(move |_, window, cx| {
                        let task = preview.apply(window, cx);
                        let (document, name) = (document.clone(), name.clone());
                        window
                            .spawn(cx, async move |cx| {
                                if let Err(error) = task.await {
                                    let _ = cx.update(|window, cx| {
                                        window.open_alert_dialog(cx, move |dialog, _, _| {
                                            let (document, name) = (document.clone(), name.clone());
                                            dialog
                                                .title("Rename could not be applied")
                                                .child(error.clone())
                                                .button_props(
                                                    DialogButtonProps::default()
                                                        .ok_text("Recalculate")
                                                        .show_cancel(true)
                                                        .on_ok(move |_, window, cx| {
                                                            prepare(
                                                                document.clone(),
                                                                name.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                            true
                                                        }),
                                                )
                                        })
                                    });
                                }
                            })
                            .detach();
                        true
                    }),
            )
    });
}
