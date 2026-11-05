use crate::{
    state::files::Files,
    ui::hover::{ButtonLook, Hover, SmoothButton as _},
};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, button::Button, h_flex, list::ListItem, tree::Tree, v_flex,
};
use gpui_kit::*;
use std::path::PathBuf;
pub(crate) mod search;
actions!(files_panel, [OpenSelectedFile]);

pub(crate) struct FilesPanel {
    pub files: Entity<Files>,
    _observer: Subscription,
}
impl FilesPanel {
    pub fn new(files: Entity<Files>, cx: &mut Context<Self>) -> Self {
        cx.bind_keys([KeyBinding::new(
            "enter",
            OpenSelectedFile,
            Some("FilesPanel > Tree"),
        )]);
        let observer = cx.observe(&files, |_, _, cx| cx.notify());
        Self {
            files,
            _observer: observer,
        }
    }
}
impl Render for FilesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = Hover::new("files-refresh", window, cx);
        let files = self.files.clone();
        let state = files.read(cx);
        let tree = state.tree.clone();
        let title = state
            .target
            .as_ref()
            .map(|t| t.path.display().to_string())
            .unwrap_or_else(|| "Select a worktree".into());
        let error = state.error.clone();
        let mut body = v_flex()
            .key_context("FilesPanel")
            .on_action(cx.listener(|this, _: &OpenSelectedFile, _, cx| {
                let entry = this
                    .files
                    .read(cx)
                    .tree
                    .read(cx)
                    .selected_entry()
                    .map(|entry| {
                        (
                            entry.item().id.clone(),
                            entry.is_folder(),
                            entry.is_disabled(),
                        )
                    });
                if let Some((path, false, false)) = entry {
                    this.files.update(cx, |files, cx| {
                        files.open(PathBuf::from(path.as_ref()), false, cx)
                    });
                }
            }))
            .size_full()
            .min_h_0()
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .child(div().truncate().flex_1().text_sm().child(title))
                    .child(
                        Button::new("files-refresh")
                            .small()
                            .cursor_pointer()
                            .label("↻")
                            .tooltip("Refresh files")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.files.update(cx, |f, cx| f.refresh(cx))
                            }))
                            .smooth_hover(ButtonLook::Ghost, &hover, cx),
                    ),
            );
        if let Some(error) = error {
            body = body.child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(error),
            );
        }
        body.child(
            Tree::new(&tree, move |ix, entry, _, _, _| {
                let path = PathBuf::from(entry.item().id.as_ref());
                let folder = entry.is_folder();
                let files = files.clone();
                ListItem::new(ix)
                    .pl(px(entry.depth() as f32 * 16.))
                    .child(format!(
                        "{} {}",
                        if folder {
                            if entry.is_expanded() { "▾" } else { "▸" }
                        } else {
                            " "
                        },
                        entry.item().label
                    ))
                    .on_click(move |event, _, cx| {
                        if !folder {
                            let preview = event.click_count() < 2;
                            files.update(cx, |files, cx| files.open(path.clone(), preview, cx));
                        }
                    })
            })
            .flex_1()
            .min_h_0(),
        )
    }
}
