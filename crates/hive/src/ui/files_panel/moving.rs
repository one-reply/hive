use super::*;
use crate::state::files::FileOperation;
use gpui_kit::component::{Icon, list::ListItem};
use hive_core::projects::Target;

const EXPAND_DELAY: Duration = Duration::from_millis(650);
const EDGE_SIZE: Pixels = px(28.);
const SCROLL_STEP: Pixels = px(18.);
const SCROLL_INTERVAL: Duration = Duration::from_millis(35);

#[derive(Clone)]
pub(super) struct DraggedFile {
    pub target: Target,
    pub path: PathBuf,
}
impl Render for DraggedFile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .rounded_md()
            .shadow_md()
            .bg(cx.theme().sidebar)
            .text_sm()
            .child(Icon::new(Lucide::File).size_4())
            .child(
                self.path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
    }
}

impl FilesPanel {
    pub(super) fn drop_file(
        &mut self,
        drag: &DraggedFile,
        directory: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drag_hover = None;
        self._drag_expand = None;
        if self.files.read(cx).target.as_ref() != Some(&drag.target)
            || directory.starts_with(&drag.path)
        {
            return;
        }
        let Some(name) = drag.path.file_name() else {
            return;
        };
        let operation = FileOperation::Move {
            from: drag.path.clone(),
            to: directory.join(name),
        };
        self.files
            .update(cx, |files, cx| files.perform(operation, window, cx));
    }

    pub(super) fn drag_expand(
        &mut self,
        path: PathBuf,
        drag: &DraggedFile,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.files.read(cx).target.as_ref() != Some(&drag.target) || path.starts_with(&drag.path)
        {
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
        event: &DragMoveEvent<DraggedFile>,
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
            .files
            .read(cx)
            .tree
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
        let destinations = cx.new(|cx| {
            let mut files = Files::new(projects, window, cx);
            files.destination_for = Some(source.clone());
            files
        });
        let picker = cx
            .new(|cx| DestinationPicker::new(destinations, self.files.clone(), target, source, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title("Move to…").w(px(420.)).child(picker.clone())
        });
    }
}

struct DestinationPicker {
    destinations: Entity<Files>,
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
        let subscriptions = vec![
            cx.observe(&destinations, |_, _, cx| cx.notify()),
            cx.subscribe(&files, |this, _, event: &FilesEvent, cx| {
                if event.target == this.target && event.operation.path() == this.source {
                    this.error = event.result.as_ref().err().cloned();
                    cx.notify();
                }
            }),
        ];
        Self {
            destinations,
            files,
            target,
            source,
            error: None,
            _subscriptions: subscriptions,
        }
    }
}
impl Render for DestinationPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tree = self.destinations.read(cx).tree.clone();
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
                                let directory =
                                    this.destinations.read(cx).selected(cx).unwrap_or_default();
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
