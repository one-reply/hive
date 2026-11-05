//! Every cancellable close route enters here before mutating layout or project state.
use super::*;

#[derive(Clone)]
pub(crate) enum Scope {
    Document(Entity<Document>),
    Project(std::path::PathBuf),
    Worktree(std::path::PathBuf),
    All,
}

impl Scope {
    fn dirty(&self, cx: &App) -> Option<Entity<Document>> {
        if let Self::Document(document) = self {
            return document.read(cx).dirty.then(|| document.clone());
        }
        AppState::get(cx)
            .documents
            .read(cx)
            .dirty(cx)
            .into_iter()
            .find(|document| {
                let target = &document.read(cx).target;
                match self {
                    Self::Project(path) => &target.project == path,
                    Self::Worktree(path) => &target.path == path,
                    Self::All => true,
                    Self::Document(_) => false,
                }
            })
    }
}

pub(crate) fn guard(
    scope: Scope,
    window: &mut Window,
    cx: &mut App,
    complete: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    guard_result(scope, window, cx, move |approved, window, cx| {
        if approved {
            complete(window, cx);
        }
    });
}

fn guard_result(
    scope: Scope,
    window: &mut Window,
    cx: &mut App,
    complete: impl FnOnce(bool, &mut Window, &mut App) + 'static,
) {
    if !AppState::get(cx)
        .documents
        .read(cx)
        .mutating_paths
        .is_empty()
    {
        use gpui_kit::component::{WindowExt as _, notification::Notification};
        window.push_notification(
            Notification::info("Wait for the file operation to finish before closing."),
            cx,
        );
        complete(false, window, cx);
        return;
    }
    // Re-read the whole scope after each async save: another buffer may have changed meanwhile.
    let Some(document) = scope.dirty(cx) else {
        window.defer(cx, move |window, cx| complete(true, window, cx));
        return;
    };
    let message = format!("Save changes to {}?", document.read(cx).path.display());
    let answer = window.prompt(
        PromptLevel::Warning,
        &message,
        Some("Your changes have not been saved to the project file."),
        &["Save", "Discard", "Cancel"],
        cx,
    );
    let handle = window.window_handle();
    cx.spawn(async move |cx| match answer.await {
        Ok(0) => {
            let save = handle.update(cx, |_, window, cx| {
                document.update(cx, |d, cx| d.save(None, window, cx))
            });
            let approved = match save {
                Ok(save) => save.recv().await == Ok(true),
                Err(_) => false,
            };
            let _ = handle.update(cx, |_, window, cx| {
                if approved {
                    guard_result(scope, window, cx, complete);
                } else {
                    complete(false, window, cx);
                }
            });
        }
        Ok(1) => {
            let _ = handle.update(cx, |_, window, cx| {
                document.update(cx, |d, cx| d.discard(window, cx));
                guard_result(scope, window, cx, complete);
            });
        }
        _ => {
            let _ = handle.update(cx, |_, window, cx| complete(false, window, cx));
        }
    })
    .detach();
}

pub(crate) fn all(
    window: &mut Window,
    cx: &mut App,
    complete: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    guard(Scope::All, window, cx, complete);
}

pub(crate) fn quit(window: &mut Window, cx: &mut App) {
    guard_result(Scope::All, window, cx, |approved, _, cx| {
        crate::quit::reply(approved, cx)
    });
}

pub(crate) fn recovered(
    record: hive_core::files::recovery::Record,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui_kit::component::{WindowExt as _, input::EditorState};
    let editor = cx.new(|cx| EditorState::new(window, cx).default_value(record.draft.text.clone()));
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        let (copy, key) = (record.draft.text.clone(), record.key.clone());
        let [copy_hover, delete_hover] =
            ["recovery-copy", "recovery-delete"].map(|id| Hover::new(id, window, cx));
        dialog
            .title(format!("Recovered draft · {}", record.draft.path.display()))
            .child(format!(
                "Project unavailable: {}",
                record.draft.project.display()
            ))
            .child(Editor::new(&editor).readonly(true).h(px(350.)))
            .child(
                Button::new("recovery-copy")
                    .cursor_pointer()
                    .label("Copy")
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                    })
                    .smooth_hover(ButtonLook::Ghost, &copy_hover, cx),
            )
            .child(
                Button::new("recovery-delete")
                    .cursor_pointer()
                    .label("Delete draft")
                    .on_click(move |_, window, cx| {
                        AppState::get(cx)
                            .store
                            .read(cx)
                            .save_recovery(key.clone(), None);
                        AppState::get(cx)
                            .documents
                            .clone()
                            .update(cx, |documents, _| {
                                documents.recovered.retain(|r| r.key != key)
                            });
                        window.close_dialog(cx);
                    })
                    .smooth_hover(ButtonLook::Outline, &delete_hover, cx),
            )
    });
}
