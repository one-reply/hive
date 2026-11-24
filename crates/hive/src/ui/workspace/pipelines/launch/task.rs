//! The Task field: a Markdown editor with a formatting toolbar above it. Agents read
//! Markdown as written, so the field edits the source and highlights it rather than
//! rendering it.
use super::format::{self, Format};
use super::*;
use gpui_kit::component::attachment::{Attachment, AttachmentGroup, AttachmentMedia};
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::{WindowExt as _, notification::Notification};
use hive_core::pipeline::images;

/// Tall enough for a few sentences of intent and acceptance criteria without scrolling.
const TASK_HEIGHT: Pixels = px(184.);
/// Key context of the field, where ⌘B and ⌘I format the selection.
const KEY_CONTEXT: &str = "TaskEditor";

actions!(pipeline_task, [ToggleBold, ToggleItalic]);

/// The toolbar's formats that have a key of their own.
pub(crate) fn key_bindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new("cmd-b", ToggleBold, Some(KEY_CONTEXT)),
        KeyBinding::new("cmd-i", ToggleItalic, Some(KEY_CONTEXT)),
    ]
}

/// Buttons of the toolbar, in the order they are drawn.
const FORMATS: [(&str, &str, Lucide, Format); 6] = [
    ("task-bold", "Bold", Lucide::Bold, Format::Bold),
    ("task-italic", "Italic", Lucide::Italic, Format::Italic),
    ("task-heading", "Heading", Lucide::Heading, Format::Heading),
    (
        "task-bullets",
        "Bulleted list",
        Lucide::List,
        Format::Bullets,
    ),
    (
        "task-numbers",
        "Numbered list",
        Lucide::ListOrdered,
        Format::Numbers,
    ),
    ("task-code", "Code", Lucide::Code, Format::Code),
];

/// A code editor stripped of what makes it read as one: the task is prose with markup.
pub(super) fn editor(window: &mut Window, cx: &mut App) -> Entity<EditorState> {
    cx.new(|cx| {
        EditorState::new(window, cx)
            .language("markdown")
            .line_number(false)
            .folding(false)
            .searchable(false)
            // Pairing quotes and brackets gets in the way of prose.
            .auto_close(false)
            .placeholder("What should the agents build or fix? Include what “done” looks like.")
    })
}

/// The image a paste carries. macOS reports a clipboard that also holds text as text only.
fn pasted_image(clipboard: &ClipboardItem) -> Option<Vec<u8>> {
    clipboard.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(image.bytes.clone()),
        _ => None,
    })
}

impl LaunchForm {
    /// Applies a toolbar format to the selection as one undo step, then returns the focus
    /// the button took.
    fn format(&mut self, format: Format, window: &mut Window, cx: &mut Context<Self>) {
        self.task.update(cx, |task, cx| {
            let edit = format::apply(&task.value(), task.selected_range(), format);
            task.set_selected_range(edit.range, cx);
            task.replace(edit.text, window, cx);
            task.set_selected_range(edit.selection, cx);
            task.focus(window, cx);
        });
    }

    /// Puts `text` where the caret is, replacing a selection.
    pub(super) fn insert_into_task(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_owned();
        self.task
            .update(cx, |task, cx| task.replace(text, window, cx));
    }

    /// Saves a pasted image and puts its reference where the caret is by then. A form closed
    /// in the meantime leaves the file to the next start's cleanup.
    fn attach_image(&mut self, bytes: Vec<u8>, window: &mut Window, cx: &mut Context<Self>) {
        let saving = crate::state::pipelines::save_task_image(bytes, cx);
        cx.spawn_in(window, async move |form, cx| {
            let saved = saving.await;
            let _ = form.update_in(cx, |form, window, cx| match saved {
                Ok(reference) => form.insert_into_task(&reference, window, cx),
                Err(message) => window.push_notification(
                    Notification::error(message).title("Could not paste the image"),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Takes image `index` out of the task: the text holds the only record of it.
    fn remove_image(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.task.update(cx, |task, cx| {
            let value = task.value();
            let Some((range, _)) = images::references(&value).into_iter().nth(index) else {
                return;
            };
            task.set_selected_range(range, cx);
            task.replace("", window, cx);
        });
    }

    /// Thumbnails of the images the task refers to, in the order they appear in it.
    fn task_images(&self, cx: &mut Context<Self>) -> Option<AttachmentGroup> {
        let value = self.task.read(cx).value();
        let references = images::references(&value);
        if references.is_empty() {
            return None;
        }
        let directory = images::directory();
        let thumbnails =
            references
                .into_iter()
                .enumerate()
                .map(|(index, (_, name))| {
                    Attachment::new()
                        .id(("task-image", index))
                        .media(AttachmentMedia::new().src(directory.join(name)))
                        .tooltip("Remove to take the image out of the task")
                        .on_remove(cx.listener(move |form, _, window, cx| {
                            form.remove_image(index, window, cx)
                        }))
                });
        Some(AttachmentGroup::new("task-images").children(thumbnails))
    }

    fn task_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut toolbar = h_flex()
            .id("task-toolbar")
            .role(Role::Toolbar)
            .aria_label("Task formatting")
            .gap_0p5();
        for (id, label, icon, format) in FORMATS {
            toolbar = toolbar.child(icon_action(
                id,
                label,
                icon,
                window,
                cx,
                move |form: &mut Self, w, cx| form.format(format, w, cx),
            ));
        }
        toolbar
            .child(div().flex_1())
            .child(self.dictation_button(window, cx))
    }

    pub(super) fn task_field(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let hint = if self.is_direct() {
            "Describe the change for the developer."
        } else {
            "Plain language is fine. The planner asks if something is unclear."
        };
        let form = cx.weak_entity();
        let theme = cx.theme();
        let editor = Editor::new(&self.task)
            .h(TASK_HEIGHT)
            .font_family(theme.font_family.clone())
            .text_size(theme.font_size)
            // Text pastes as usual; an image is saved and referenced instead.
            .on_paste(move |clipboard, window, cx| {
                let Some(bytes) = pasted_image(clipboard) else {
                    return false;
                };
                let _ = form.update(cx, |form, cx| form.attach_image(bytes, window, cx));
                true
            });
        v_flex()
            .gap_2()
            .key_context(KEY_CONTEXT)
            // Before the editor, so a held Space records instead of typing.
            .capture_key_down(cx.listener(Self::space_down))
            .capture_key_up(cx.listener(|form, event, _, cx| form.space_up(event, cx)))
            .on_action(cx.listener(|form, _: &ToggleBold, w, cx| form.format(Format::Bold, w, cx)))
            .on_action(
                cx.listener(|form, _: &ToggleItalic, w, cx| form.format(Format::Italic, w, cx)),
            )
            .child(section(required_heading("Task", cx), hint, cx))
            .child(
                v_flex()
                    .gap_1()
                    // Above the frame: its scrollbar spans the frame's whole height.
                    .child(self.task_toolbar(window, cx))
                    .child(self.with_dictation(
                        TextField::editor(editor, &self.task, cx),
                        window,
                        cx,
                    )),
            )
            .children(self.task_images(cx))
    }
}
