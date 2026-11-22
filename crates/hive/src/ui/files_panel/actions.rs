use super::*;
use crate::state::files::FileOperation;
use gpui_kit::component::menu::PopupMenu;

#[derive(Clone, Copy)]
enum EntryAction {
    Open,
    Changes,
    File,
    Folder,
    Paste,
    Rename,
    Move,
    Delete,
    CopyRelative,
    CopyAbsolute,
    Reveal,
}

pub(super) fn menu(
    mut menu: PopupMenu,
    panel: Entity<FilesPanel>,
    path: PathBuf,
    placeholder: bool,
    cx: &App,
) -> PopupMenu {
    let files = panel.read(cx).files.read(cx);
    let entry = files.entry(&path);
    let folder = entry.is_some_and(|e| e.directory);
    let existing = entry.is_some_and(|e| !e.unsupported);
    let mutable = existing && !files.busy && !placeholder;
    let changes = files
        .decorations
        .get(&path)
        .is_some_and(|d| d.staged.is_some() || d.unstaged.is_some());
    let local = files.target().is_some_and(|target| target.host.is_local());
    let pastable = mutable && local && !pasting::copied_paths(cx).is_empty();
    let actions = [
        ("Open", EntryAction::Open, existing && !folder),
        ("Show Changes", EntryAction::Changes, changes),
        ("New File", EntryAction::File, mutable),
        ("New Folder", EntryAction::Folder, mutable),
        (pasting::menu_label(local), EntryAction::Paste, pastable),
        ("Rename…", EntryAction::Rename, mutable),
        ("Move to…", EntryAction::Move, mutable),
        ("Delete Permanently…", EntryAction::Delete, mutable),
        (
            "Copy Relative Path",
            EntryAction::CopyRelative,
            !placeholder,
        ),
        (
            "Copy Absolute Path",
            EntryAction::CopyAbsolute,
            !placeholder,
        ),
        ("Reveal in Finder", EntryAction::Reveal, existing),
    ];
    for (index, (label, action, enabled)) in actions.into_iter().enumerate() {
        if matches!(action, EntryAction::Reveal) && !local {
            continue;
        }
        if [2, 8].contains(&index) {
            menu = menu.separator();
        }
        let (panel, path) = (panel.clone(), path.clone());
        menu = menu.item(PopupMenuItem::new(label).disabled(!enabled).on_click(
            move |_, window, cx| {
                panel.update(cx, |panel, cx| {
                    panel.entry_action(action, path.clone(), window, cx)
                });
            },
        ));
    }
    menu
}

impl FilesPanel {
    fn entry_action(
        &mut self,
        action: EntryAction,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            EntryAction::Open => self
                .files
                .update(cx, |files, cx| files.open(path, false, cx)),
            EntryAction::Changes => self
                .files
                .update(cx, |files, cx| files.show_changes(&path, cx)),
            EntryAction::Rename => self.start_edit(editing::EditKind::Rename(path), window, cx),
            EntryAction::Move => self.move_picker(path, window, cx),
            EntryAction::Delete => self.confirm_delete(path, window, cx),
            EntryAction::File | EntryAction::Folder | EntryAction::Paste => {
                let is_folder = self
                    .files
                    .read(cx)
                    .entry(&path)
                    .is_some_and(|e| e.directory);
                let folder = pasting::folder_for(&path, is_folder);
                match action {
                    EntryAction::Paste => self.paste_clipboard(folder, window, cx),
                    action => self.new_entry(
                        matches!(action, EntryAction::Folder),
                        Some(folder),
                        window,
                        cx,
                    ),
                }
            }
            EntryAction::CopyRelative => cx.write_to_clipboard(ClipboardItem::new_string(
                path.to_string_lossy().into_owned(),
            )),
            EntryAction::CopyAbsolute | EntryAction::Reveal => {
                if let Some(target) = self.files.read(cx).target() {
                    let absolute = target.path.join(path);
                    if matches!(action, EntryAction::Reveal) && target.host.is_local() {
                        cx.reveal_path(&absolute);
                    } else {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            absolute.to_string_lossy().into_owned(),
                        ));
                    }
                }
            }
        }
    }

    pub(super) fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected(cx) {
            self.confirm_delete(path, window, cx);
        }
    }

    fn confirm_delete(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.files.read(cx);
        let Some(target) = state.target().cloned() else {
            return;
        };
        let Some(entry) = state.entry(&path) else {
            return;
        };
        if state.busy || entry.unsupported {
            return;
        }
        let documents = AppState::get(cx)
            .documents
            .read(cx)
            .in_path(&target, &path, cx);
        let dirty = documents
            .iter()
            .filter_map(|d| {
                let d = d.read(cx);
                d.dirty.then(|| d.path.display().to_string())
            })
            .collect();
        let deletion = Deletion::new(&target.path.join(&path), entry.directory, dirty);
        let panel = cx.weak_entity();
        ask_to_delete(
            deletion,
            move |window, cx| {
                let Some(panel) = panel.upgrade() else {
                    return;
                };
                let files = panel.read(cx).files.clone();
                let operation = FileOperation::Delete(path.clone());
                files.update(cx, |files, cx| files.perform(operation, window, cx));
            },
            window,
            cx,
        );
    }
}

/// What the confirmation of a permanent delete says about one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Deletion {
    path: SharedString,
    consequence: &'static str,
    /// Open documents under the entry whose unsaved edits go with it.
    unsaved: Vec<SharedString>,
    confirm: &'static str,
}

impl Deletion {
    fn new(absolute: &Path, directory: bool, unsaved: Vec<String>) -> Self {
        Self {
            path: absolute.display().to_string().into(),
            consequence: if directory {
                "The folder and all its contents will be permanently deleted. \
                 This cannot be undone. Nothing will be moved to Trash."
            } else {
                "This file will be permanently deleted. \
                 This cannot be undone. Nothing will be moved to Trash."
            },
            confirm: if unsaved.is_empty() {
                "Delete Permanently"
            } else {
                "Delete and Discard Unsaved Changes"
            },
            unsaved: unsaved.into_iter().map(SharedString::from).collect(),
        }
    }
}

/// Asks before a permanent delete. There is no Trash to recover from, so the question is a
/// destructive confirmation: only its red button runs `delete`, never Enter or Space on a
/// dialog that has just opened.
fn ask_to_delete(
    deletion: Deletion,
    delete: impl Fn(&mut Window, &mut App) + Clone + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let delete = delete.clone();
        let mut description = v_flex()
            .gap_2()
            .child(div().child(deletion.path.clone()))
            .child(deletion.consequence);
        if !deletion.unsaved.is_empty() {
            description = description.child(
                v_flex()
                    .text_color(cx.theme().danger)
                    .child("Unsaved editor changes will also be discarded:")
                    .children(deletion.unsaved.iter().cloned()),
            );
        }
        crate::ui::destructive::confirmation(
            alert
                .confirm()
                .title("Permanently delete this entry?")
                .description(description),
            deletion.confirm,
            move |window, cx| {
                delete(window, cx);
                true
            },
        )
    });
}

#[cfg(test)]
mod tests {
    use super::{Deletion, ask_to_delete};
    use crate::ui::destructive::testing::{is_open, press, settle, window};
    use gpui_kit as gpui;
    use gpui_kit::TestAppContext;
    use std::cell::Cell;
    use std::path::Path;
    use std::rc::Rc;

    #[test]
    fn the_confirmation_names_the_entry_what_is_lost_and_unsaved_documents() {
        let file = Deletion::new(Path::new("/repo/src/main.rs"), false, Vec::new());
        assert_eq!(file.path, "/repo/src/main.rs");
        assert!(
            file.consequence
                .starts_with("This file will be permanently deleted.")
        );
        assert!(file.consequence.contains("Nothing will be moved to Trash."));
        assert_eq!(file.confirm, "Delete Permanently");
        assert!(file.unsaved.is_empty());

        let folder = Deletion::new(
            Path::new("/repo/src"),
            true,
            vec!["src/main.rs".into(), "src/lib.rs".into()],
        );
        assert!(
            folder
                .consequence
                .starts_with("The folder and all its contents")
        );
        assert_eq!(folder.confirm, "Delete and Discard Unsaved Changes");
        assert_eq!(folder.unsaved, ["src/main.rs", "src/lib.rs"]);
    }

    #[gpui::test]
    fn neither_space_nor_enter_deletes_when_the_question_opens(cx: &mut TestAppContext) {
        let deleted = Rc::new(Cell::new(0));
        let count = deleted.clone();
        let cx = window(cx);
        cx.update(|window, cx| {
            ask_to_delete(
                Deletion::new(Path::new("/repo/build"), true, Vec::new()),
                move |_, _| count.set(count.get() + 1),
                window,
                cx,
            )
        });
        settle(cx);
        // The native alert this replaces opened with the keyboard focus on its delete button.
        press("space", cx);
        press("enter", cx);
        assert_eq!(deleted.get(), 0);
        assert!(is_open(cx), "the question stays until it is answered");

        cx.simulate_keystrokes("tab tab");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        press("space", cx);
        assert_eq!(deleted.get(), 1);
        assert!(!is_open(cx));
    }
}
