use crate::{
    app::AppState,
    state::files::{Files, FilesEvent},
    ui::hover::{ButtonLook, Hover, SmoothButton as _},
};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    tree::{Tree, TreeEvent, TreeState},
    v_flex,
};
use gpui_kit::component::{Disableable, tooltip::Tooltip};
use gpui_kit::*;
use gpui_kit::{assets::IconName as Lucide, prelude::FluentBuilder};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

mod actions;
mod editing;
mod icons;
mod motion;
mod moving;
mod row;
pub(crate) mod search;
mod tree;
use editing::Edit;

actions!(
    files_panel,
    [
        OpenSelectedFile,
        RenameSelectedFile,
        DeleteSelectedFile,
        CancelFileEdit
    ]
);
const LABEL_SIZE: f32 = 13.;
const META_SIZE: f32 = 11.;
const ICON_SIZE: f32 = 16.;
const INDENT: f32 = 16.;
/// A touch taller than `Button::small` so the search field reads as an input, not a chip.
const SEARCH_HEIGHT: f32 = 28.;

pub(crate) struct FilesPanel {
    pub files: Entity<Files>,
    /// The widget's rows, selection and scroll; rebuilt from `Files::nodes` when they change.
    tree: Entity<TreeState>,
    synced_generation: Option<u64>,
    edit: Option<Edit>,
    manual_refresh: bool,
    operation_error: Option<String>,
    _interaction: Option<Task<()>>,
    drag_hover: Option<(PathBuf, Instant)>,
    drag_scroll: Option<bool>,
    _drag_scroll: Option<Task<()>>,
    _drag_expand: Option<Task<()>>,
    motion: motion::TreeMotion,
    _subscriptions: Vec<Subscription>,
}
impl FilesPanel {
    pub fn new(files: Entity<Files>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let mut motion = motion::TreeMotion::default();
        motion.observe(tree.read(cx), true);
        let subscriptions = vec![
            cx.observe(&tree, |this, tree, cx| {
                this.motion.observe(tree.read(cx), cx.reduce_motion())
            }),
            cx.subscribe(&tree, |this, _, event, cx| {
                let (path, expanded) = match event {
                    TreeEvent::Expanded(path) => (PathBuf::from(path.as_ref()), true),
                    TreeEvent::Collapsed(path) => (PathBuf::from(path.as_ref()), false),
                };
                this.files
                    .update(cx, |files, cx| files.set_expanded(path, expanded, cx));
            }),
            cx.observe(&files, |this, files, cx| {
                this.sync_tree(cx);
                let state = files.read(cx);
                if !state.loading {
                    this.manual_refresh = false;
                }
                if this
                    .edit
                    .as_ref()
                    .is_some_and(|edit| Some(&edit.target) != state.target.as_ref())
                {
                    this.edit = None;
                }
                cx.notify();
            }),
            cx.subscribe_in(&files, window, |this, _, event: &FilesEvent, window, cx| {
                if this
                    .edit
                    .as_ref()
                    .is_some_and(|edit| edit.target == event.target)
                {
                    match &event.result {
                        Ok(()) => {
                            // The rename field held focus; hand it back to its row. A new
                            // file opens in the editor instead, which takes focus itself.
                            if this.edit.take().is_some_and(|e| e.renaming().is_some()) {
                                this.focus_tree(window, cx);
                            }
                        }
                        Err(error) => {
                            if let Some(edit) = &mut this.edit {
                                edit.error = Some(error.clone());
                            }
                        }
                    }
                }
                // A failure the open edit already shows next to its field is not repeated.
                let shown_by_edit = this.edit.as_ref().is_some_and(|edit| edit.error.is_some());
                this.operation_error = event
                    .result
                    .as_ref()
                    .err()
                    .filter(|_| !shown_by_edit)
                    .cloned()
                    .or_else(|| event.warning.clone());
                cx.notify();
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                let active = window.is_window_active();
                this.files
                    .update(cx, |files, cx| files.set_window_active(active, cx));
            }),
        ];
        let mut this = Self {
            files,
            tree,
            synced_generation: None,
            edit: None,
            manual_refresh: false,
            operation_error: None,
            _interaction: None,
            drag_hover: None,
            drag_scroll: None,
            _drag_scroll: None,
            _drag_expand: None,
            motion,
            _subscriptions: subscriptions,
        };
        this.sync_tree(cx);
        this
    }

    /// Rebuilds the rows from the model when it changed, keeping the selection and serving
    /// a pending reveal once its row exists.
    fn sync_tree(&mut self, cx: &mut Context<Self>) {
        let files = self.files.read(cx);
        if self.synced_generation == Some(files.tree_generation()) {
            return;
        }
        self.synced_generation = Some(files.tree_generation());
        let items = if files.destination_for().is_some() {
            tree::destination_items(files.nodes())
        } else {
            tree::items(files.nodes())
        };
        let reveal = files.reveal_request().map(Path::to_path_buf);
        let selected = self
            .tree
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone());
        let revealed = self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            if let Some(id) = selected {
                tree.set_selected_index(tree.index_of(&id), cx);
            }
            let Some(path) = reveal else {
                return false;
            };
            let id: SharedString = path.to_string_lossy().into_owned().into();
            match tree.index_of(&id) {
                Some(ix) => {
                    tree.set_selected_index(Some(ix), cx);
                    tree.scroll_to_item(ix, ScrollStrategy::Nearest);
                    true
                }
                None => false,
            }
        });
        if revealed {
            self.files.update(cx, |files, _| files.revealed());
        }
    }

    /// The selected row's path, unless it is a placeholder.
    pub(crate) fn selected(&self, cx: &App) -> Option<PathBuf> {
        self.tree
            .read(cx)
            .selected_entry()
            .filter(|e| !e.is_disabled())
            .map(|e| PathBuf::from(e.item().id.as_ref()))
    }

    fn selected_is_folder(&self, cx: &App) -> bool {
        self.tree
            .read(cx)
            .selected_entry()
            .is_some_and(|e| e.is_folder())
    }

    /// Moves keyboard focus to the tree.
    fn focus_tree(&self, window: &mut Window, cx: &mut App) {
        self.tree.update(cx, |tree, cx| tree.focus(window, cx));
    }

    /// The widget state, for callers that focus or scroll it outside a panel lease.
    pub(crate) fn tree(&self) -> &Entity<TreeState> {
        &self.tree
    }
}
impl Render for FilesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.files.clone();
        let state = files.read(cx);
        let tree = self.tree.clone();
        let target = state.target.clone();
        let title = target
            .as_ref()
            .and_then(|t| t.path.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Files".into());
        let full_path = target
            .as_ref()
            .map(|t| t.path.display().to_string())
            .unwrap_or_default();
        let settings = state.settings.clone();
        let error = state.error.clone();
        let available = target
            .as_ref()
            .is_some_and(|t| t.host == hive_core::host::Host::Local);
        let loading = state.loading && (self.manual_refresh || tree.read(cx).entry(0).is_none());
        let busy = state.busy;
        let empty = !state.loading && tree.read(cx).entry(0).is_none();
        let active = state.active.clone();
        let panel = cx.entity();
        let menu_panel = panel.clone();
        let root_drop = panel.clone();
        let [refresh_hover, search_hover] =
            ["files-refresh", "files-search"].map(|id| Hover::new(id, window, cx));
        let mut body = v_flex()
            .key_context("FilesPanel")
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(cx.theme().sidebar)
            .on_action(cx.listener(|this, _: &OpenSelectedFile, _, cx| {
                if let Some(path) = this.selected(cx) {
                    let folder = this.selected_is_folder(cx);
                    this.files.update(cx, |files, cx| {
                        if folder {
                            files.toggle_directory(path, cx);
                        } else {
                            files.open(path, false, cx);
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &RenameSelectedFile, window, cx| {
                this.rename_selected(window, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteSelectedFile, window, cx| {
                this.delete_selected(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CancelFileEdit, window, cx| {
                if !this.files.read(cx).busy {
                    this.edit = None;
                    this.focus_tree(window, cx);
                    cx.notify();
                }
            }))
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .id("files-root-label")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(rems(LABEL_SIZE / 16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .tooltip(move |window, cx| {
                                Tooltip::new(full_path.clone()).build(window, cx)
                            })
                            .drag_over::<moving::DraggedFile>(|style, _, _, cx| {
                                style.bg(cx.theme().drop_target)
                            })
                            .on_drop(move |drag: &moving::DraggedFile, window, cx| {
                                root_drop.update(cx, |p, cx| {
                                    p.drop_file(drag, Path::new(""), window, cx)
                                })
                            })
                            .child(title),
                    )
                    .child(
                        Button::new("files-refresh")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::RefreshCw)
                            .loading(loading)
                            .disabled(!available || busy)
                            .tooltip("Refresh files and Git status")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.manual_refresh = true;
                                this.files.update(cx, |files, cx| files.refresh_all(cx));
                            }))
                            .smooth_hover(ButtonLook::Ghost, &refresh_hover, cx),
                    )
                    .child(
                        Button::new("files-menu")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::Ellipsis)
                            .tooltip("Files actions")
                            .dropdown_menu(move |menu, _, cx| {
                                let create = menu_panel.clone();
                                let folder = menu_panel.clone();
                                let collapse = menu_panel.clone();
                                let reveal = menu_panel.clone();
                                let active = active.clone();
                                let root = menu_panel
                                    .read(cx)
                                    .files
                                    .read(cx)
                                    .target
                                    .as_ref()
                                    .map(|t| t.path.clone());
                                let copy_root = root.clone();
                                menu.item(
                                    PopupMenuItem::new("New File")
                                        .disabled(!available || busy)
                                        .on_click(move |_, window, cx| {
                                            create.update(cx, |p, cx| {
                                                p.new_entry(false, None, window, cx)
                                            })
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("New Folder")
                                        .disabled(!available || busy)
                                        .on_click(move |_, window, cx| {
                                            folder.update(cx, |p, cx| {
                                                p.new_entry(true, None, window, cx)
                                            })
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("Collapse All")
                                        .disabled(!available)
                                        .on_click(move |_, _, cx| {
                                            collapse
                                                .read(cx)
                                                .files
                                                .clone()
                                                .update(cx, |f, cx| f.collapse_all(cx))
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Reveal Active File")
                                        .disabled(active.is_none())
                                        .on_click(move |_, _, cx| {
                                            if let Some(path) = active.clone() {
                                                reveal
                                                    .read(cx)
                                                    .files
                                                    .clone()
                                                    .update(cx, |f, cx| f.reveal(path, cx));
                                            }
                                        }),
                                )
                                .separator()
                                .item(
                                    PopupMenuItem::new("Copy Worktree Path")
                                        .disabled(root.is_none())
                                        .on_click(move |_, _, cx| {
                                            if let Some(root) = &copy_root {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    root.display().to_string(),
                                                ));
                                            }
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Reveal Worktree in Finder")
                                        .disabled(root.is_none())
                                        .on_click(move |_, _, cx| {
                                            if let Some(root) = &root {
                                                cx.reveal_path(root);
                                            }
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new("Files Settings…")
                                        .on_click(|_, _, cx| crate::ui::settings::open_files(cx)),
                                )
                            }),
                    ),
            )
            .child(
                div().px_2().pb_2().child(
                    Button::new("files-search")
                        .small()
                        .w_full()
                        .h(px(SEARCH_HEIGHT))
                        .cursor_pointer()
                        .icon(Lucide::Search)
                        .label("Find file…")
                        .tooltip("Open file · ⌘P")
                        .disabled(!available)
                        .on_click(cx.listener(|this, _, window, cx| {
                            search::open(this.files.clone(), window, cx)
                        }))
                        .smooth_hover(ButtonLook::Outline, &search_hover, cx),
                ),
            );
        if let Some(edit) = self.edit.as_ref().filter(|edit| edit.renaming().is_none()) {
            body = body.child(edit.render_form(busy, window, cx));
        }
        if let Some(error) = self.operation_error.clone().or(error) {
            body = body.child(
                div()
                    .px_2()
                    .pb_2()
                    .text_size(rems(META_SIZE / 16.))
                    .text_color(cx.theme().danger)
                    .child(error),
            );
        }
        if target.is_none() {
            body = body.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Select a project or worktree to browse its files."),
            );
        } else if empty && available {
            body = body.child(div().p_3().text_sm().text_color(cx.theme().muted_foreground).child("No files to show. Create a file from the menu, or check visibility in Files Settings."));
        }
        let context_panel = panel.clone();
        let drag_panel = panel.clone();
        body.child(
            div()
                .id("files-tree-area")
                .flex_1()
                .min_h_0()
                .min_w_0()
                .on_drag_move(
                    move |event: &DragMoveEvent<moving::DraggedFile>, window, cx| {
                        drag_panel.update(cx, |p, cx| p.drag_scroll(event, window, cx))
                    },
                )
                .child(
                    Tree::new(&tree, move |ix, entry, selected, window, cx| {
                        row::render(ix, entry, selected, &panel, &settings, window, cx)
                    })
                    .context_menu(move |_, entry, menu, _, cx| {
                        actions::menu(
                            menu,
                            context_panel.clone(),
                            PathBuf::from(entry.item().id.as_ref()),
                            entry.is_disabled(),
                            cx,
                        )
                    })
                    .flex_1()
                    .min_h_0(),
                ),
        )
    }
}
