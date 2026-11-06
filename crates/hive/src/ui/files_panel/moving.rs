use super::*;
use crate::state::files::FileOperation;
use gpui_kit::component::{Icon, list::ListItem};
use hive_core::projects::Target;

const EXPAND_DELAY: Duration = Duration::from_millis(650);
const EDGE_SIZE: Pixels = px(28.);
const SCROLL_STEP: Pixels = px(18.);
const SCROLL_INTERVAL: Duration = Duration::from_millis(35);

#[derive(Clone)]
pub(crate) struct DraggedFiles {
    pub target: Target,
    pub paths: std::sync::Arc<[PathBuf]>,
}

impl DraggedFiles {
    pub(super) fn can_move_to(&self, target: Option<&Target>, directory: &Path) -> bool {
        matches!(self.paths.as_ref(), [path] if target == Some(&self.target) && !directory.starts_with(path))
    }
}

impl FilesPanel {
    pub(super) fn drop_file(
        &mut self,
        drag: &DraggedFiles,
        directory: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drag_hover = None;
        self._drag_expand = None;
        if !drag.can_move_to(self.files.read(cx).target.as_ref(), directory) {
            return;
        }
        let [path] = drag.paths.as_ref() else { return };
        let Some(name) = path.file_name() else {
            return;
        };
        let operation = FileOperation::Move {
            from: path.clone(),
            to: directory.join(name),
        };
        self.files
            .update(cx, |files, cx| files.perform(operation, window, cx));
    }

    pub(super) fn drag_expand(
        &mut self,
        path: PathBuf,
        drag: &DraggedFiles,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !drag.can_move_to(self.files.read(cx).target.as_ref(), &path) {
            return;
        }
        if self
            .drag_hover
            .as_ref()
            .is_some_and(|(previous, _)| *previous == path)
        {
            return;
        }
        self.drag_hover = Some((path.clone(), Instant::now()));
        let target = drag.target.clone();
        self._drag_expand = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EXPAND_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if cx.has_active_drag()
                    && this.files.read(cx).target.as_ref() == Some(&target)
                    && this
                        .drag_hover
                        .as_ref()
                        .is_some_and(|(hovered, _)| *hovered == path)
                {
                    this.files.update(cx, |files, cx| files.expand(path, cx));
                }
            });
        }));
    }

    pub(super) fn drag_scroll(
        &mut self,
        event: &DragMoveEvent<DraggedFiles>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let y = event.event.position.y;
        let direction = if !event.bounds.contains(&event.event.position) {
            None
        } else if y < event.bounds.top() + EDGE_SIZE {
            Some(true)
        } else if y > event.bounds.bottom() - EDGE_SIZE {
            Some(false)
        } else {
            None
        };
        if self.drag_scroll == direction {
            return;
        }
        self.drag_scroll = direction;
        self._drag_scroll = None;
        let Some(up) = direction else { return };
        let handle = self
            .tree()
            .read(cx)
            .scroll_handle()
            .0
            .borrow()
            .base_handle
            .clone();
        self._drag_scroll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let keep_scrolling = this
                    .update_in(cx, |this, window, cx| {
                        if !cx.has_active_drag() || this.drag_scroll != Some(up) {
                            this.drag_scroll = None;
                            return false;
                        }
                        let mut offset = handle.offset();
                        offset.y += if up { SCROLL_STEP } else { -SCROLL_STEP };
                        handle.set_offset(offset);
                        window.refresh();
                        true
                    })
                    .unwrap_or(false);
                if !keep_scrolling {
                    break;
                }
                cx.background_executor().timer(SCROLL_INTERVAL).await;
            }
        }));
    }

    pub(super) fn move_picker(
        &mut self,
        source: PathBuf,
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
        let projects = AppState::get(cx).projects.clone();
        let destinations = cx.new(|cx| Files::new(projects, Some(source.clone()), cx));
        let picker = cx
            .new(|cx| DestinationPicker::new(destinations, self.files.clone(), target, source, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title("Move to…").w(px(420.)).child(picker.clone())
        });
    }
}

struct DestinationPicker {
    destinations: Entity<Files>,
    /// The picker's own rows and selection, rebuilt from `destinations`.
    tree: Entity<TreeState>,
    synced_generation: Option<u64>,
    files: Entity<Files>,
    target: Target,
    source: PathBuf,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}
impl DestinationPicker {
    fn new(
        destinations: Entity<Files>,
        files: Entity<Files>,
        target: Target,
        source: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let subscriptions = vec![
            cx.observe(&destinations, |this, _, cx| {
                this.sync_tree(cx);
                cx.notify()
            }),
            cx.subscribe(&tree, |this, _, event, cx| {
                let (path, expanded) = match event {
                    TreeEvent::Expanded(path) => (PathBuf::from(path.as_ref()), true),
                    TreeEvent::Collapsed(path) => (PathBuf::from(path.as_ref()), false),
                };
                this.destinations
                    .update(cx, |files, cx| files.set_expanded(path, expanded, cx));
            }),
            cx.subscribe(&files, |this, _, event: &FilesEvent, cx| {
                if event.target == this.target && event.operation.path() == this.source {
                    this.error = event.result.as_ref().err().cloned();
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            destinations,
            tree,
            synced_generation: None,
            files,
            target,
            source,
            error: None,
            _subscriptions: subscriptions,
        };
        this.sync_tree(cx);
        this
    }

    fn sync_tree(&mut self, cx: &mut Context<Self>) {
        let destinations = self.destinations.read(cx);
        if self.synced_generation == Some(destinations.tree_generation()) {
            return;
        }
        self.synced_generation = Some(destinations.tree_generation());
        let items = tree::destination_items(destinations.nodes());
        let selected = self
            .tree
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone());
        self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            if let Some(id) = selected {
                tree.set_selected_index(tree.index_of(&id), cx);
            }
        });
    }

    /// The chosen folder; the root row and placeholders count as the worktree root.
    fn selected_directory(&self, cx: &App) -> PathBuf {
        self.tree
            .read(cx)
            .selected_entry()
            .filter(|e| !e.is_disabled())
            .map(|e| PathBuf::from(e.item().id.as_ref()))
            .unwrap_or_default()
    }
}
impl Render for DestinationPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = self.tree.clone();
        let stale = self.files.read(cx).target.as_ref() != Some(&self.target);
        let [cancel_hover, move_hover] =
            ["cancel-move", "confirm-move"].map(|id| Hover::new(id, window, cx));
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .truncate()
                    .child(format!("Move {} into:", self.source.display())),
            )
            .child(
                Tree::new(&tree, |ix, entry, _, _, _| {
                    ListItem::new(ix)
                        .py_1()
                        .pl(rems(entry.depth() as f32))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Icon::new(if entry.is_expanded() {
                                        Lucide::FolderOpen
                                    } else {
                                        Lucide::Folder
                                    })
                                    .size_4(),
                                )
                                .child(entry.item().label.clone()),
                        )
                })
                .h(px(280.)),
            )
            .when_some(
                self.error
                    .clone()
                    .or_else(|| self.destinations.read(cx).error.clone()),
                |view, error| {
                    view.child(div().text_sm().text_color(cx.theme().danger).child(error))
                },
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel-move")
                            .small()
                            .cursor_pointer()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx))
                            .smooth_hover(ButtonLook::Ghost, &cancel_hover, cx),
                    )
                    .child(
                        Button::new("confirm-move")
                            .small()
                            .cursor_pointer()
                            .label("Move Here")
                            .disabled(stale || self.files.read(cx).busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let directory = this.selected_directory(cx);
                                let Some(name) = this.source.file_name() else {
                                    return;
                                };
                                let operation = FileOperation::Move {
                                    from: this.source.clone(),
                                    to: directory.join(name),
                                };
                                this.files
                                    .update(cx, |files, cx| files.perform(operation, window, cx));
                                window.close_dialog(cx);
                            }))
                            .smooth_hover(ButtonLook::Outline, &move_hover, cx),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::DraggedFiles;
    use hive_core::projects::Target;
    use std::path::Path;

    #[test]
    fn folder_drops_never_move_only_one_file_from_a_group() {
        let target = Target {
            project: "/repo".into(),
            path: "/repo".into(),
            host: hive_core::host::Host::Local,
        };
        let mut drag = DraggedFiles {
            target: target.clone(),
            paths: std::sync::Arc::from(["a.rs".into(), "b.rs".into()]),
        };
        assert!(!drag.can_move_to(Some(&target), Path::new("destination")));
        assert!(!drag.can_move_to(Some(&target), Path::new("")));
        drag.paths = std::sync::Arc::from(["folder".into()]);
        assert!(drag.can_move_to(Some(&target), Path::new("destination")));
        assert!(!drag.can_move_to(Some(&target), Path::new("folder/child")));
        assert!(!drag.can_move_to(None, Path::new("destination")));
    }
}
