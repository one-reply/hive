//! What a pull or merge request will say, shown and editable before anything leaves the
//! machine: its title, description and the branch to merge into.

use crate::ui::{
    dialog::{field, initial_focus},
    text_field::TextField,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Icon, WindowExt as _};
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
    let focus_title = initial_focus(title.focus_handle(cx));
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        focus_title(window, cx);
        let (title, body, base) = (title.clone(), body.clone(), base.clone());
        let submit = submit.clone();
        dialog
            .icon(Icon::new(Lucide::GitPullRequest))
            .title("Create pull request")
            .description(hint.clone())
            .child(field(
                "Title",
                TextField::input(Input::new(&title), &title, cx),
                cx,
            ))
            .child(field(
                "Description",
                TextField::textarea(Textarea::new(&body), &body, cx),
                cx,
            ))
            .child(field(
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

#[cfg(test)]
mod tests {
    use super::open;
    use gpui_kit as gpui;
    use gpui_kit::base::Root;
    use gpui_kit::prelude::*;
    use gpui_kit::{Context, TestAppContext, Window, div};
    use hive_core::git::pull_request::Draft;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Host;
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    #[gpui::test]
    fn the_title_has_the_focus_when_the_dialog_opens(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| Host);
            Root::new(view, window, cx)
        });
        let submitted = Rc::new(RefCell::new(None));
        let sink = submitted.clone();
        let draft = Draft {
            title: "Subject".into(),
            ..Draft::default()
        };
        cx.update(|window, cx| {
            open("hint", &draft, window, cx, move |draft, _, _| {
                *sink.borrow_mut() = Some(draft);
            });
        });
        cx.run_until_parked();
        // The first frame mounts the dialog layer, the second paints the fields in it.
        for _ in 0..2 {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            cx.run_until_parked();
        }

        // Typed without clicking anywhere: it has to land in the title.
        cx.simulate_input("!");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let title = submitted.borrow().as_ref().map(|draft| draft.title.clone());
        assert_eq!(title.as_deref().map(str::len), Some("Subject!".len()));
        assert!(title.is_some_and(|title| title.contains('!')));
    }
}
