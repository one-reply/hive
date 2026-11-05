use super::*;
use crate::state::files::FileOperation;
use gpui_kit::component::menu::PopupMenu;

#[derive(Clone, Copy)]
enum EntryAction {
    Open,
    Changes,
    File,
    Folder,
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
    let actions = [
        ("Open", EntryAction::Open, existing && !folder),
        ("Show Changes", EntryAction::Changes, changes),
        ("New File", EntryAction::File, mutable),
        ("New Folder", EntryAction::Folder, mutable),
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
        if [2, 7].contains(&index) {
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
            EntryAction::File | EntryAction::Folder => {
                let parent = if self
                    .files
                    .read(cx)
                    .entry(&path)
                    .is_some_and(|e| e.directory)
                {
                    path
                } else {
                    path.parent().unwrap_or(Path::new("")).into()
                };
                self.new_entry(
                    matches!(action, EntryAction::Folder),
                    Some(parent),
                    window,
                    cx,
                );
            }
            EntryAction::CopyRelative => cx.write_to_clipboard(ClipboardItem::new_string(
                path.to_string_lossy().into_owned(),
            )),
            EntryAction::CopyAbsolute | EntryAction::Reveal => {
                if let Some(target) = &self.files.read(cx).target {
                    let absolute = target.path.join(path);
                    if matches!(action, EntryAction::Reveal) {
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
        if let Some(path) = self.files.read(cx).selected(cx) {
            self.confirm_delete(path, window, cx);
        }
    }

    fn confirm_delete(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.files.read(cx);
        let Some(target) = state.target.clone() else {
            return;
        };
        let Some(entry) = state.entry(&path) else {
            return;
        };
        if state.busy || entry.unsupported {
            return;
        }
        let directory = entry.directory;
        let documents = AppState::get(cx)
            .documents
            .read(cx)
            .in_path(&target, &path, cx);
        let dirty: Vec<_> = documents
            .iter()
            .filter_map(|d| {
                let d = d.read(cx);
                d.dirty.then(|| d.path.display().to_string())
            })
            .collect();
        let mut detail = format!(
            "{}\n\n{}This cannot be undone. Nothing will be moved to Trash.",
            target.path.join(&path).display(),
            if directory {
                "The folder and all its contents will be permanently deleted. "
            } else {
                "This file will be permanently deleted. "
            }
        );
        if !dirty.is_empty() {
            detail.push_str(&format!(
                "\n\nUnsaved editor changes will also be discarded:\n{}",
                dirty.join("\n")
            ));
        }
        let delete_label = if dirty.is_empty() {
            "Delete Permanently"
        } else {
            "Delete and Discard Unsaved Changes"
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            "Permanently delete this entry?",
            Some(&detail),
            &["Cancel", delete_label],
            cx,
        );
        self._interaction = Some(cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(1) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                if this.files.read(cx).target.as_ref() != Some(&target) {
                    window.push_notification(
                        Notification::error(
                            "Worktree changed. Select the entry again before deleting.",
                        ),
                        cx,
                    );
                    return;
                }
                this.files.update(cx, |files, cx| {
                    files.perform(FileOperation::Delete(path), window, cx)
                });
            });
        }));
    }
}
