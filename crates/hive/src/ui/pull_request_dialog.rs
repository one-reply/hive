//! What a pull or merge request will say, shown and editable before anything leaves the
//! machine: its title, description and the branch to merge into.

use crate::ui::text_field::TextField;
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::*;
use hive_core::git::pull_request::Draft;
use std::rc::Rc;

/// Lines the description shows before it scrolls.
const DESCRIPTION_ROWS: usize = 3;

/// Opens the dialog on `draft`. `hint` says what has to be true of the branch, or what
/// happens first. `submit` gets the edited draft once it has a title; an empty title keeps
/// the dialog open.
pub(crate) fn open(
    hint: impl Into<SharedString>,
    draft: &Draft,
    window: &mut Window,
    cx: &mut App,
    submit: impl Fn(Draft, &mut Window, &mut App) + 'static,
) {
    let hint: SharedString = hint.into();
    let title = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(draft.title.clone())
            .placeholder("Title")
    });
    let body = cx.new(|cx| {
        TextareaState::new(window, cx)
            .rows(DESCRIPTION_ROWS)
            .default_value(draft.body.clone())
            .placeholder("Description")
    });
    let base = cx.new(|cx| {
        InputState::new(window, cx)
            .default_value(draft.base.clone())
            .placeholder("The repository's default branch")
    });
    let submit = Rc::new(submit);
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let (title, body, base) = (title.clone(), body.clone(), base.clone());
        let submit = submit.clone();
        dialog
            .title("Create pull request")
            .child(caption(hint.clone(), cx))
            .child(labeled(
                "Title",
                TextField::input(Input::new(&title), &title, cx),
                cx,
            ))
            .child(labeled(
                "Description",
                TextField::textarea(Textarea::new(&body), &body, cx),
                cx,
            ))
            .child(labeled(
                "Merge into",
                TextField::input(Input::new(&base), &base, cx),
                cx,
            ))
            .button_props(
                DialogButtonProps::default()
                    .ok_text("Create pull request")
                    .show_cancel(true)
                    .on_ok(move |_, window, cx| {
                        let draft = Draft {
                            title: title.read(cx).value().to_string(),
                            body: body.read(cx).value().to_string(),
                            base: base.read(cx).value().trim().to_owned(),
                        };
                        if draft.title.trim().is_empty() {
                            return false;
                        }
                        submit(draft, window, cx);
                        true
                    }),
            )
    });
}

fn caption(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn labeled(label: &'static str, field: impl IntoElement, cx: &App) -> Div {
    v_flex().gap_1().child(caption(label, cx)).child(field)
}
