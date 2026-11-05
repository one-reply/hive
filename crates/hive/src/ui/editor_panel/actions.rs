use super::*;
use crate::state::git::GitEvent;
use crate::ui::text_field::TextField;
use gpui_kit::component::input::{EditorState, Textarea, TextareaState};
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

    pub(super) fn ask_agent(&self, window: &mut Window, cx: &mut App) {
        use gpui_kit::component::{
            IndexPath,
            searchable_list::SearchableVec,
            select::{Select, SelectState},
        };
        let document = self.document.read(cx);
        let editor = document.editor.read(cx);
        let range = editor.selected_range();
        let text = document.text(cx);
        let Some(prompt) = hive_core::files::selection_prompt(
            &document.target.path,
            &document.path,
            &text,
            range,
            document.dirty,
        ) else {
            return;
        };
        let target = document.target.clone();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(14)
                .default_value(prompt)
        });
        let sessions = AppState::get(cx)
            .agents
            .read(cx)
            .sessions(&target.path)
            .to_vec();
        let labels: Vec<_> = sessions
            .iter()
            .map(|s| format!("{} · {}", s.kind.name(), s.session))
            .collect();
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(labels.clone()),
                (!labels.is_empty()).then_some(IndexPath::default()),
                window,
                cx,
            )
        });
        window.open_alert_dialog(cx, move |dialog, window, cx| {
            let (input, select, sessions, labels, target) = (
                input.clone(),
                select.clone(),
                sessions.clone(),
                labels.clone(),
                target.clone(),
            );
            let copy = input.clone();
            let hover = Hover::new("selection-copy", window, cx);
            dialog
                .title("Ask agent about selection")
                .w(DIALOG_WIDTH)
                .child(TextField::textarea(Textarea::new(&input), &input, cx))
                .child(
                    Select::new(&select)
                        .placeholder("No running agent · use Copy")
                        .w_full(),
                )
                .child(
                    Button::new("selection-copy")
                        .cursor_pointer()
                        .label("Copy")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                copy.read(cx).value().to_string(),
                            ))
                        })
                        .smooth_hover(ButtonLook::Ghost, &hover, cx),
                )
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Prepare in agent")
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let selected = select.read(cx).selected_value();
                            let Some(session) = selected
                                .and_then(|value| labels.iter().position(|label| label == value))
                                .and_then(|ix| sessions.get(ix))
                            else {
                                return false;
                            };
                            let prompt = input.read(cx).value().to_string();
                            let agents = AppState::get(cx).agents.clone();
                            let Some(terminal) =
                                agents.read(cx).session_at(&target.path, session.session)
                            else {
                                cx.write_to_clipboard(ClipboardItem::new_string(prompt));
                                return false;
                            };
                            terminal.update(cx, |terminal, cx| {
                                terminal.prepare_agent_draft(session.kind, prompt, false, cx)
                            });
                            let id = session.session;
                            cx.defer(move |cx| {
                                agents.update(cx, |agents, cx| agents.reveal(id, cx))
                            });
                            true
                        }),
                )
        });
    }
}
