use super::*;
use crate::state::git::GitEvent;
use gpui_kit::component::input::EditorState;
use gpui_kit::component::{WindowExt as _, dialog::DialogButtonProps, notification::Notification};

const DIALOG_WIDTH: Pixels = px(900.);
const CONFLICT_HEIGHT: Pixels = px(400.);

impl EditorPanel {
    pub(super) fn resolve_conflict(&self, window: &mut Window, cx: &mut App) {
        let document = self.document.read(cx);
        let Some(disk) = document.conflict.clone() else {
            return;
        };
        let text = document.text(cx);
        let mine = cx.new(|cx| EditorState::new(window, cx).default_value(text));
        let theirs = cx.new(|cx| EditorState::new(window, cx).default_value(disk.text()));
        let document = self.document.clone();
        window.open_alert_dialog(cx, move |dialog, window, cx| {
            let (reload, overwrite) = (document.clone(), document.clone());
            let expected = disk.clone();
            let [reload_hover, overwrite_hover] =
                ["conflict-reload", "conflict-overwrite"].map(|id| Hover::new(id, window, cx));
            dialog
                .title("Resolve external changes")
                .w(DIALOG_WIDTH)
                .child(
                    h_flex()
                        .gap_2()
                        .h(CONFLICT_HEIGHT)
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child("Your buffer")
                                .child(Editor::new(&mine).readonly(true).size_full()),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child("Disk version")
                                .child(Editor::new(&theirs).readonly(true).size_full()),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("conflict-reload")
                                .cursor_pointer()
                                .label("Reload from disk")
                                .on_click(move |_, window, cx| {
                                    reload.update(cx, |d, cx| d.reload(window, cx));
                                    window.close_dialog(cx);
                                })
                                .smooth_hover(ButtonLook::Outline, &reload_hover, cx),
                        )
                        .child(
                            Button::new("conflict-overwrite")
                                .cursor_pointer()
                                .label("Overwrite")
                                .on_click(move |_, window, cx| {
                                    overwrite.update(cx, |d, cx| {
                                        d.save(Some(expected.clone()), window, cx);
                                    });
                                    window.close_dialog(cx);
                                })
                                .smooth_hover(ButtonLook::Outline, &overwrite_hover, cx),
                        ),
                )
                .button_props(DialogButtonProps::default().ok_text("Keep editing"))
        });
    }

    pub(super) fn show_changes(&self, choose: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.read(cx).dirty {
            window.push_notification(
                Notification::info(
                    "Unsaved edits remain in the editor. Git diff shows the disk/index version.",
                ),
                cx,
            );
        }
        let task = self.document.read(cx).changes(cx);
        cx.spawn_in(window, async move |_, cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(requests) if requests.is_empty() => window.push_notification(
                    Notification::info(
                        "No changes on disk. Save the buffer to include your edits.",
                    ),
                    cx,
                ),
                Ok(requests) => {
                    let git = AppState::get(cx).git.clone();
                    if requests.len() == 1 || !choose {
                        git.update(cx, |_, cx| cx.emit(GitEvent::OpenDiff(requests[0].clone())));
                    } else {
                        window.open_alert_dialog(cx, move |dialog, window, cx| {
                            let mut dialog = dialog.title("Show changes");
                            for (index, request) in requests.iter().enumerate() {
                                let (git, request) = (git.clone(), request.clone());
                                let hover = Hover::new(("changes-source", index), window, cx);
                                dialog = dialog.child(
                                    Button::new(("changes-source", index))
                                        .cursor_pointer()
                                        .label(if index == 0 { "Unstaged" } else { "Staged" })
                                        .on_click(move |_, window, cx| {
                                            git.update(cx, |_, cx| {
                                                cx.emit(GitEvent::OpenDiff(request.clone()))
                                            });
                                            window.close_dialog(cx);
                                        })
                                        .smooth_hover(ButtonLook::Outline, &hover, cx),
                                );
                            }
                            dialog
                        });
                    }
                }
                Err(error) => window.push_notification(Notification::error(error), cx),
            });
        })
        .detach();
    }
}
