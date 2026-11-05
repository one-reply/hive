use super::*;
use crate::state::{files::FileOperation, git::Target};
use gpui_kit::component::input::{Input, InputEvent, InputState};

pub(super) enum EditKind {
    File(PathBuf),
    Folder(PathBuf),
    Rename(PathBuf),
}
pub(super) struct Edit {
    pub target: Target,
    kind: EditKind,
    input: Entity<InputState>,
    pub error: Option<String>,
    _subscription: Subscription,
}
impl Edit {
    pub(super) fn render(
        &self,
        busy: bool,
        _: &mut Window,
        cx: &mut Context<FilesPanel>,
    ) -> impl IntoElement {
        let label = match &self.kind {
            EditKind::File(parent) => format!("New file in {}", display_parent(parent)),
            EditKind::Folder(parent) => format!("New folder in {}", display_parent(parent)),
            EditKind::Rename(path) => format!("Rename {}", path.display()),
        };
        v_flex()
            .key_context("FileNameEdit")
            .px_2()
            .pb_2()
            .gap_1()
            .child(
                div()
                    .text_size(rems(META_SIZE / 16.))
                    .truncate()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(crate::ui::text_field::TextField::input(
                Input::new(&self.input).small().disabled(busy),
                &self.input,
                cx,
            ))
            .child(
                div()
                    .text_size(rems(META_SIZE / 16.))
                    .text_color(if self.error.is_some() {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(
                        self.error
                            .clone()
                            .unwrap_or_else(|| "Enter to confirm · Esc to cancel".into()),
                    ),
            )
    }
}
fn display_parent(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        "worktree root".into()
    } else {
        path.display().to_string()
    }
}

impl FilesPanel {
    pub(super) fn new_entry(
        &mut self,
        folder: bool,
        parent: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.files.read(cx);
        let parent = parent.unwrap_or_else(|| {
            state
                .selected(cx)
                .map(|path| {
                    if state.entry(&path).is_some_and(|e| e.directory) {
                        path
                    } else {
                        path.parent().unwrap_or(Path::new("")).into()
                    }
                })
                .unwrap_or_default()
        });
        self.start_edit(
            if folder {
                EditKind::Folder(parent)
            } else {
                EditKind::File(parent)
            },
            window,
            cx,
        );
    }
    pub(super) fn rename_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.files.read(cx).selected(cx) {
            self.start_edit(EditKind::Rename(path), window, cx);
        }
    }
    pub(super) fn start_edit(
        &mut self,
        kind: EditKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.files.read(cx);
        if state.busy {
            return;
        }
        let Some(target) = state.target.clone() else {
            return;
        };
        if let EditKind::Rename(path) = &kind
            && (state.missing.contains(path) || state.entry(path).is_none_or(|e| e.unsupported))
        {
            return;
        }
        let initial = if let EditKind::Rename(path) = &kind {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            String::new()
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let subscription = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.submit_edit(window, cx);
            }
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.edit = Some(Edit {
            target,
            kind,
            input,
            error: None,
            _subscription: subscription,
        });
        cx.notify();
    }
    fn submit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = &mut self.edit else {
            return;
        };
        if self.files.read(cx).busy || self.files.read(cx).target.as_ref() != Some(&edit.target) {
            return;
        }
        let name = edit.input.read(cx).value().to_string();
        let parent = match &edit.kind {
            EditKind::File(parent) | EditKind::Folder(parent) => parent.as_path(),
            EditKind::Rename(path) => path.parent().unwrap_or(Path::new("")),
        };
        let path = match hive_core::files::entry_path(parent, &name) {
            Ok(path) => path,
            Err(error) => {
                edit.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let operation = match &edit.kind {
            EditKind::File(_) => FileOperation::CreateFile(path),
            EditKind::Folder(_) => FileOperation::CreateDirectory(path),
            EditKind::Rename(from) => FileOperation::Move {
                from: from.clone(),
                to: path,
            },
        };
        edit.error = None;
        self.files
            .update(cx, |files, cx| files.perform(operation, window, cx));
    }
}
