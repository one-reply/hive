use super::*;
use crate::state::files::FileOperation;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use hive_core::projects::Target;
use std::{cell::Cell, ops::Range, rc::Rc};

pub(super) enum EditKind {
    File(PathBuf),
    Folder(PathBuf),
    Rename(PathBuf),
}
pub(super) struct Edit {
    pub target: Target,
    kind: EditKind,
    input: Entity<InputState>,
    /// Why the current name cannot be used: checked while typing, then by the file system.
    pub error: Option<String>,
    /// Where the in-row field was laid out last frame, to place its error tooltip.
    field_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    _subscription: Subscription,
}
impl Edit {
    /// The entry being renamed in place; `None` while a new entry is named above the tree.
    pub(super) fn renaming(&self) -> Option<&Path> {
        match &self.kind {
            EditKind::Rename(path) => Some(path),
            EditKind::File(_) | EditKind::Folder(_) => None,
        }
    }
    fn parent(&self) -> &Path {
        match &self.kind {
            EditKind::File(parent) | EditKind::Folder(parent) => parent,
            EditKind::Rename(path) => path.parent().unwrap_or(Path::new("")),
        }
    }
    /// The name field drawn in place of a row's label: the label's font and position, with
    /// only a line under the typed name, red while the name is rejected. A frame would not
    /// fit in the row's height and reads as a form rather than the row itself.
    pub(super) fn field(
        &self,
        busy: bool,
        font_size: f32,
        window: &mut Window,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let font_size = rems(font_size / 16.).to_pixels(window.rem_size());
        let name: SharedString = self.input.read(cx).value().to_string().into();
        let run = TextRun {
            len: name.len(),
            font: font(theme.font_family.clone()),
            color: theme.foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let text_width = window
            .text_system()
            .shape_line(name, font_size, &[run], None)
            .width;
        div()
            .relative()
            .size_full()
            .child(
                Input::new(&self.input)
                    .appearance(false)
                    .disabled(busy)
                    .size_full()
                    .p_0()
                    .text_size(font_size),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .bottom(px(UNDERLINE_OFFSET))
                    .w(text_width.max(px(UNDERLINE_MIN_WIDTH)))
                    .max_w_full()
                    .h(px(1.))
                    .bg(if self.error.is_some() {
                        theme.danger
                    } else {
                        theme.muted_foreground
                    }),
            )
            .child(self.measure_field())
            .when_some(self.error_tooltip(window, cx), |field, tooltip| {
                field.child(tooltip)
            })
            .into_any_element()
    }
    /// Records the field's window bounds, re-rendering once when they move so the tooltip
    /// follows the row (scrolling, resizing).
    fn measure_field(&self) -> impl IntoElement {
        let bounds_cell = self.field_bounds.clone();
        canvas(
            move |bounds, window, _| {
                if bounds_cell.replace(Some(bounds)) != Some(bounds) {
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }
    /// The rejected name's reason, floating under the field, or above it near the window's
    /// bottom edge, so it never covers the row or shifts the tree.
    fn error_tooltip(&self, window: &Window, cx: &App) -> Option<AnyElement> {
        let error = self.error.clone()?;
        let field = self.field_bounds.get()?;
        let theme = cx.theme();
        let (anchor, y) = if tooltip_above(field, window.viewport_size().height) {
            (Anchor::BottomLeft, field.top() - px(TOOLTIP_GAP))
        } else {
            (Anchor::TopLeft, field.bottom() + px(TOOLTIP_GAP))
        };
        let tooltip = div()
            .max_w(field.size.width.max(px(TOOLTIP_MIN_WIDTH)))
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.danger)
            .bg(theme.tokens.popover)
            .text_color(theme.popover_foreground)
            .text_size(rems(META_SIZE / 16.))
            .shadow_md()
            .child(error);
        Some(
            deferred(
                anchored()
                    .position(point(field.left(), y))
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(TOOLTIP_GAP))
                    .child(tooltip),
            )
            .into_any_element(),
        )
    }
    /// The form above the tree for a new entry, which has no row to edit in yet.
    pub(super) fn render_form(
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
            .key_context(EDIT_CONTEXT)
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
            .child(
                crate::ui::text_field::TextField::input(
                    Input::new(&self.input).small().disabled(busy),
                    &self.input,
                    cx,
                )
                .invalid(self.error.is_some()),
            )
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
/// How far the line under a renamed name sits above the row's bottom edge.
const UNDERLINE_OFFSET: f32 = 3.;
/// Space between the field and its error tooltip, and the tooltip and the window edge.
const TOOLTIP_GAP: f32 = 4.;
/// Room kept below the field before the tooltip flips above it: two wrapped lines.
const TOOLTIP_ROOM: f32 = 48.;
/// Keeps a short field's tooltip wide enough to read.
const TOOLTIP_MIN_WIDTH: f32 = 200.;
/// Keeps the line visible while the field is empty.
const UNDERLINE_MIN_WIDTH: f32 = 12.;
/// Key context of the name field, so Escape cancels the edit wherever the field is drawn.
pub(super) const EDIT_CONTEXT: &str = "FileNameEdit";

/// Whether the error tooltip goes above the field: only when below would leave the window.
fn tooltip_above(field: Bounds<Pixels>, viewport_height: Pixels) -> bool {
    field.bottom() + px(TOOLTIP_GAP + TOOLTIP_ROOM) > viewport_height
}

/// The part of `name` preselected for a rename: the stem of a file, so typing keeps the
/// extension, or the whole name for a folder or a dotfile such as `.gitignore`.
fn preselected(name: &str, directory: bool) -> Range<usize> {
    let end = match name.rfind('.') {
        Some(dot) if !directory && dot > 0 => dot,
        _ => name.len(),
    };
    0..end
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
            self.selected(cx)
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
        if let Some(path) = self.selected(cx) {
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
        let (initial, selection) = match &kind {
            EditKind::Rename(path) => {
                let Some(entry) = state.entry(path).filter(|e| !e.unsupported) else {
                    return;
                };
                if state.missing.contains(path) {
                    return;
                }
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let selection = preselected(&name, entry.directory);
                (name, selection)
            }
            EditKind::File(_) | EditKind::Folder(_) => (String::new(), 0..0),
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.submit_edit(window, cx),
                InputEvent::Change => this.check_edit(cx),
                InputEvent::Blur => this.leave_edit(window, cx),
                InputEvent::Focus => {}
            });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.set_selected_range(selection, cx);
        });
        self.edit = Some(Edit {
            target,
            kind,
            input,
            error: None,
            field_bounds: Rc::default(),
            _subscription: subscription,
        });
        cx.notify();
    }

    /// The path the typed name stands for, or why it cannot be used.
    fn validate(&self, edit: &Edit, name: &str, cx: &App) -> Result<PathBuf, String> {
        let parent = edit.parent();
        let path = hive_core::files::entry_path(parent, name).map_err(|e| e.to_string())?;
        let listed = self.files.read(cx).listed(parent);
        if hive_core::files::name_taken(listed, name, edit.renaming()) {
            return Err(format!(
                "A file or folder named {name} already exists in {}",
                display_parent(parent)
            ));
        }
        Ok(path)
    }

    /// The typed name, and whether it is still the renamed entry's own name.
    fn edited_name(edit: &Edit, cx: &App) -> (String, bool) {
        let name = edit.input.read(cx).value().to_string();
        let unchanged = edit
            .renaming()
            .and_then(Path::file_name)
            .is_some_and(|own| own.to_string_lossy() == name);
        (name, unchanged)
    }

    /// Flags a rejected name while the user types; an empty field waits for Enter.
    fn check_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = &self.edit else {
            return;
        };
        let (name, _) = Self::edited_name(edit, cx);
        let error = if name.is_empty() {
            None
        } else {
            self.validate(edit, &name, cx).err()
        };
        if let Some(edit) = &mut self.edit
            && edit.error != error
        {
            edit.error = error;
            cx.notify();
        }
    }

    /// Like the editor's own rename: leaving the field keeps a valid new name and drops the
    /// rest. Only a rename ends this way; a new entry's form stays until Enter or Escape.
    fn leave_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = &self.edit else {
            return;
        };
        if edit.renaming().is_none() || self.files.read(cx).busy {
            return;
        }
        let (name, unchanged) = Self::edited_name(edit, cx);
        if unchanged || self.validate(edit, &name, cx).is_err() {
            self.edit = None;
            cx.notify();
        } else {
            self.submit_edit(window, cx);
        }
    }

    fn submit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = &self.edit else {
            return;
        };
        if self.files.read(cx).busy || self.files.read(cx).target.as_ref() != Some(&edit.target) {
            return;
        }
        let (name, unchanged) = Self::edited_name(edit, cx);
        if unchanged {
            self.edit = None;
            self.focus_tree(window, cx);
            cx.notify();
            return;
        }
        let validated = self.validate(edit, &name, cx);
        let Some(edit) = &mut self.edit else {
            return;
        };
        let path = match validated {
            Ok(path) => path,
            Err(error) => {
                edit.error = Some(error);
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

#[cfg(test)]
mod tests {
    use super::{preselected, tooltip_above};
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn error_tooltip_goes_below_unless_the_window_ends_there() {
        let row = |top: f32| Bounds::new(point(px(10.), px(top)), size(px(200.), px(22.)));
        assert!(!tooltip_above(row(100.), px(800.)));
        assert!(tooltip_above(row(760.), px(800.)));
    }

    #[test]
    fn rename_preselects_the_file_stem() {
        assert_eq!(preselected("main.rs", false), 0..4);
        assert_eq!(preselected("archive.tar.gz", false), 0..11);
        assert_eq!(preselected("тест.rs", false), 0.."тест".len());
    }

    #[test]
    fn rename_preselects_whole_folder_dotfile_and_bare_names() {
        assert_eq!(preselected("src.d", true), 0..5);
        assert_eq!(preselected(".gitignore", false), 0..10);
        assert_eq!(preselected("Makefile", false), 0..8);
        assert_eq!(preselected("", false), 0..0);
    }
}
