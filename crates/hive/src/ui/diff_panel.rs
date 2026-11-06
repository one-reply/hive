//! Dockable readonly diff with explicit index actions for complete text hunks.
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use crate::{
    app::AppState,
    state::{
        git::{DiffRequest, Git},
        git_diff::GitDiff,
        store::Store,
    },
    ui::workspace::right_sidebar_toggle,
};
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::diff::{
    Diff, DiffAnnotation, DiffLinePosition, DiffMode, DiffSide, DiffState,
};
use gpui_kit::component::dock::{BasePanel, DockArea, DockEvent, Panel, PanelEvent, TabGroup};
use gpui_kit::component::input::Position;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use hive_core::git::diff::DiffSource;

mod comments;

pub(crate) struct DiffPanel {
    pub review: Entity<GitDiff>,
    /// The widget's rows, selection and scroll; reloaded from `GitDiff::files` when they change.
    display: Entity<DiffState>,
    synced_generation: Option<u64>,
    git: Entity<Git>,
    store: Entity<Store>,
    area: WeakEntity<DockArea>,
    group: Option<WeakEntity<TabGroup>>,
    alone: bool,
    current_hunk: usize,
    _subscriptions: Vec<Subscription>,
}

impl DiffPanel {
    pub fn new(
        request: DiffRequest,
        area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let app = AppState::get(cx);
        let (git, store) = (app.git.clone(), app.store.clone());
        let review = cx.new(|cx| GitDiff::new(request, git.clone(), cx));
        let display = cx.new(|cx| DiffState::new([], cx).with_mode(diff_mode(&store, cx)));
        let focus = display.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.on_focus(&focus, window, |this, _, cx| {
                if let Some(area) = this.area.upgrade() {
                    cx.defer(move |cx| area.update(cx, |_, cx| cx.emit(DockEvent::LayoutChanged)));
                }
            }),
            cx.observe(&review, |this, _, cx| {
                this.sync_display(cx);
                cx.notify()
            }),
            cx.observe(&git, |_, _, cx| cx.notify()),
            cx.observe(&store, |this, store, cx| {
                let mode = diff_mode(&store, cx);
                this.display.update(cx, |d, cx| d.set_mode(mode, cx));
                cx.notify()
            }),
        ];
        let mut this = Self {
            review,
            display,
            synced_generation: None,
            git,
            store,
            area,
            group: None,
            alone: false,
            current_hunk: 0,
            _subscriptions: subscriptions,
        };
        this.sync_display(cx);
        this
    }

    /// Reloads the widget when the parsed patch changed.
    fn sync_display(&mut self, cx: &mut Context<Self>) {
        let review = self.review.read(cx);
        if self.synced_generation == Some(review.files_generation()) {
            return;
        }
        self.synced_generation = Some(review.files_generation());
        let files = review.files().to_vec();
        self.display.update(cx, |d, cx| d.set_files(files, cx));
    }

    pub fn set_alone(&mut self, alone: bool, cx: &mut Context<Self>) {
        if self.alone != alone {
            self.alone = alone;
            if let Some(group) = self.group.as_ref().and_then(|g| g.upgrade()) {
                group.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        }
    }

    pub(crate) fn label(&self, cx: &App) -> SharedString {
        let request = &self.review.read(cx).request;
        let source = match &request.source {
            DiffSource::WorkingTree => "Unstaged".into(),
            DiffSource::Staged => "Staged".into(),
            DiffSource::Untracked => "Untracked".into(),
            DiffSource::Commit(oid) => format!("Commit {}", &oid[..oid.len().min(7)]),
            DiffSource::Compare(base) => format!("Compare {base}"),
        };
        format!("{} · {source}", request.path).into()
    }

    fn close(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(area) = self.area.upgrade() else {
            return;
        };
        let panel = cx.entity();
        window.defer(cx, move |window, cx| {
            area.update(cx, |area, cx| area.remove_panel(panel, window, cx))
        });
    }

    fn navigate(&mut self, next: bool, cx: &mut Context<Self>) {
        let review = self.review.read(cx);
        let Some(patch) = &review.patch else {
            return;
        };
        let count = patch.hunks().len();
        if count == 0 {
            return;
        }
        self.current_hunk = if next {
            (self.current_hunk + 1) % count
        } else {
            (self.current_hunk + count - 1) % count
        };
        let hunk = &patch.hunks()[self.current_hunk];
        let (side, line) = if hunk.new_line == 0 {
            (DiffSide::Original, hunk.old_line)
        } else {
            (DiffSide::Modified, hunk.new_line)
        };
        let position = DiffLinePosition::new(review.request.path.clone(), side, line.max(1));
        self.display
            .update(cx, |d, cx| d.scroll_to_line(position, cx));
    }
}

/// The user's side-by-side preference as the widget's mode.
fn diff_mode(store: &Entity<Store>, cx: &App) -> DiffMode {
    if store.read(cx).state.settings.diff_split {
        DiffMode::Split
    } else {
        DiffMode::Unified
    }
}

impl EventEmitter<PanelEvent> for DiffPanel {}
impl Focusable for DiffPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.display.read(cx).focus_handle(cx)
    }
}
impl BasePanel for DiffPanel {
    fn panel_name(&self) -> &'static str {
        "GitDiffPanel"
    }
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.review.read(cx).request.target.clone();
        self.review.update(cx, |r, cx| {
            r.set_active(active);
            if active {
                r.refresh(cx);
            }
        });
        let git = self.git.clone();
        let id = cx.entity_id();
        cx.defer(move |cx| {
            git.update(cx, |g, cx| {
                g.set_diff_visible(id, active.then(|| target.clone()));
                if active {
                    g.activate(target, cx);
                }
            })
        });
        if active {
            self.focus_handle(cx).focus(window, cx);
        }
    }
    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, _: &mut Window, _: &mut Context<Self>) {
        self.group = Some(group);
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let id = cx.entity_id();
        self.git.update(cx, |g, _| g.set_diff_visible(id, None));
        self.review.update(cx, |r, _| r.set_active(false));
    }
}
impl Panel for DiffPanel {
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        (!self.alone).then(|| self.label(cx))
    }
    fn title(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close_hover = Hover::new("diff-close", window, cx);
        h_flex().gap_2().child(self.label(cx)).child(
            Button::new("diff-close")
                .cursor_pointer()
                .small()
                .label("×")
                .tooltip("Close diff")
                .on_click(cx.listener(|this, _, window, cx| this.close(window, cx)))
                .smooth_hover(ButtonLook::Ghost, &close_hover, cx),
        )
    }
    /// The right-sidebar toggle keeps the window's top-right corner while that sidebar is closed.
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        (!AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .git_panel_open)
            .then(|| vec![right_sidebar_toggle()])
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}
impl Render for DiffPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let [prev_hover, next_hover, mode_hover, refresh_hover] =
            ["diff-prev", "diff-next", "diff-mode", "diff-refresh"]
                .map(|id| Hover::new(id, window, cx));
        let notes_hover = Hover::new("review-notes", window, cx);
        let review = self.review.read(cx);
        let request = review.request.clone();
        let note_count = self.git.read(cx).notes(&request.target).len();
        let patch = review.patch.clone();
        let error = review.error.clone();
        let display = self.display.clone();
        let source = match &request.source {
            DiffSource::WorkingTree => "Unstaged".into(),
            DiffSource::Staged => "Staged".into(),
            DiffSource::Untracked => "Untracked".into(),
            DiffSource::Commit(oid) => {
                format!("Commit {} · first parent", &oid[..oid.len().min(7)])
            }
            DiffSource::Compare(base) => format!("{base} … HEAD · committed"),
        };
        let split = self.store.read(cx).state.settings.diff_split;
        let mutable = matches!(request.source, DiffSource::WorkingTree | DiffSource::Staged);
        let partial = mutable
            && !request.whole_file_only
            && patch.as_ref().is_some_and(|p| p.can_stage_hunks());
        let busy = self.git.read(cx).is_busy_at(&request.target);
        let mut annotations = Vec::new();
        if partial && let Some(patch) = &patch {
            for (index, hunk) in patch.hunks().iter().enumerate() {
                let (side, line) = if hunk.new_line == 0 {
                    (DiffSide::Original, hunk.old_line)
                } else {
                    (DiffSide::Modified, hunk.new_line)
                };
                annotations.push(DiffAnnotation::line(
                    ("hunk", index),
                    DiffLinePosition::new(request.path.clone(), side, line.max(1)),
                ));
            }
        }
        let ids = annotations
            .iter()
            .map(|a| a.id().clone())
            .collect::<Vec<_>>();
        let git = self.git.clone();
        let request_action = request.clone();
        let patch_action = patch.clone();
        let panel = cx.entity().downgrade();
        let file_request = request.clone();
        let diff = Diff::new(&display)
            .on_line_click(move |position, event, _, cx| {
                if event.click_count() >= 2
                    && position.side() == DiffSide::Modified
                    && matches!(
                        file_request.source,
                        DiffSource::WorkingTree | DiffSource::Untracked
                    )
                {
                    let documents = AppState::get(cx).documents.clone();
                    documents.update(cx, |documents, cx| {
                        documents.request(
                            crate::state::documents::OpenFile {
                                target: file_request.target.clone(),
                                path: file_request.path.clone().into(),
                                position: Some(Position::new(
                                    u32::try_from(position.line().saturating_sub(1))
                                        .unwrap_or(u32::MAX),
                                    0,
                                )),
                                preview: false,
                                placement: Default::default(),
                            },
                            cx,
                        )
                    });
                }
            })
            .on_add_annotation(move |range, window, cx| {
                let _ = panel.update(cx, |panel, cx| panel.add_note(range.clone(), window, cx));
            })
            .flex_1()
            .min_h_0()
            .annotations(annotations)
            .render_annotation(move |annotation, window, cx| {
                let hover = Hover::new(annotation.id().clone(), window, cx).enabled(!busy);
                let index = ids.iter().position(|id| id == annotation.id());
                let (git, request, patch) =
                    (git.clone(), request_action.clone(), patch_action.clone());
                h_flex().justify_end().child(
                    Button::new(annotation.id().clone())
                        .cursor_pointer()
                        .small()
                        .label(if request.source == DiffSource::Staged {
                            "Unstage hunk"
                        } else {
                            "Stage hunk"
                        })
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            if let (Some(index), Some(patch)) = (index, patch.as_ref()) {
                                git.update(cx, |g, cx| {
                                    g.apply_hunk(request.clone(), patch.clone(), index, cx)
                                });
                            }
                        })
                        .smooth_hover(ButtonLook::Outline, &hover, cx),
                )
            });
        let mut content = v_flex().size_full().min_w_0().min_h_0().child(
            h_flex()
                .p_2()
                .gap_2()
                .items_center()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(div().flex_1().min_w_0().truncate().text_sm().child(source))
                .child({
                    let hover = Hover::new("diff-open-file", window, cx);
                    let request = request.clone();
                    Button::new("diff-open-file")
                        .small()
                        .cursor_pointer()
                        .label("Open file")
                        .tooltip("Open the current file on disk")
                        .on_click(move |_, _, cx| {
                            AppState::get(cx)
                                .documents
                                .clone()
                                .update(cx, |documents, cx| {
                                    documents.request(
                                        crate::state::documents::OpenFile {
                                            target: request.target.clone(),
                                            path: request.path.clone().into(),
                                            position: None,
                                            preview: false,
                                            placement: Default::default(),
                                        },
                                        cx,
                                    )
                                })
                        })
                        .smooth_hover(ButtonLook::Ghost, &hover, cx)
                })
                .child(
                    Button::new("review-notes")
                        .cursor_pointer()
                        .small()
                        .label(format!("Review notes · {note_count}"))
                        .disabled(note_count == 0)
                        .on_click(cx.listener(|this, _, window, cx| this.review_prompt(window, cx)))
                        .smooth_hover(ButtonLook::Outline, &notes_hover, cx),
                )
                .child(
                    Button::new("diff-prev")
                        .cursor_pointer()
                        .small()
                        .label("↑")
                        .tooltip("Previous hunk")
                        .on_click(cx.listener(|this, _, _, cx| this.navigate(false, cx)))
                        .smooth_hover(ButtonLook::Ghost, &prev_hover, cx),
                )
                .child(
                    Button::new("diff-next")
                        .cursor_pointer()
                        .small()
                        .label("↓")
                        .tooltip("Next hunk")
                        .on_click(cx.listener(|this, _, _, cx| this.navigate(true, cx)))
                        .smooth_hover(ButtonLook::Ghost, &next_hover, cx),
                )
                .child(
                    Button::new("diff-mode")
                        .cursor_pointer()
                        .small()
                        .label(if split { "Split" } else { "Unified" })
                        .on_click(cx.listener(|this, _, _, cx| {
                            let split = this.store.read(cx).state.settings.diff_split;
                            this.store.update(cx, |s, cx| s.set_diff_split(!split, cx));
                        }))
                        .smooth_hover(ButtonLook::Ghost, &mode_hover, cx),
                )
                .child(
                    Button::new("diff-refresh")
                        .cursor_pointer()
                        .small()
                        .label("↻")
                        .tooltip("Refresh diff")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.review.update(cx, |r, cx| r.refresh(cx))
                        }))
                        .smooth_hover(ButtonLook::Ghost, &refresh_hover, cx),
                ),
        );
        if let Some(error) = error {
            content = content.child(div().p_3().text_color(cx.theme().danger).child(error));
        } else if patch.as_ref().is_some_and(|p| p.patch().is_empty()) {
            content = content.child(div().p_3().child("No changes in this view."));
        } else {
            if mutable && !partial {
                content = content.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Use the sidebar to stage this change as a whole file."),
                );
            }
            content = content.child(diff);
        }
        content
    }
}
