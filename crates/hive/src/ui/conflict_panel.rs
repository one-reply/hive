//! Dockable resolver for one unmerged file: a side per conflict, then write and stage.
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use crate::{
    app::AppState,
    state::{
        documents::OpenFile,
        git::{ConflictRequest, ConflictState, Git},
        git_conflict::{Content, GitConflict},
    },
    ui::{editor_panel::CloseFile, tab_message, workspace::right_sidebar_toggle},
};
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::diff::{
    Diff, DiffConflictResolution, DiffEvent, DiffLinePosition, DiffSide, DiffState,
};
use gpui_kit::component::dock::{BasePanel, DockArea, DockEvent, Panel, PanelEvent, TabGroup};
use gpui_kit::component::input::Position;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::git::conflicts::{Conflict, Side};

mod body;
pub(crate) mod sides;
mod toolbar;

use body::{Body, File, Progress};

pub(crate) struct ConflictPanel {
    pub conflict: Entity<GitConflict>,
    /// The widget's rows, scroll and chosen resolutions; reloaded when the file changes.
    display: Entity<DiffState>,
    synced_generation: Option<u64>,
    git: Entity<Git>,
    area: WeakEntity<DockArea>,
    group: Option<WeakEntity<TabGroup>>,
    alone: bool,
    /// The conflict the arrows last moved to.
    current: Option<usize>,
    /// Set once the user resolves the file from this tab, which then closes as soon as Git
    /// confirms the file is no longer unmerged.
    closing: bool,
    _subscriptions: Vec<Subscription>,
}

impl ConflictPanel {
    pub fn new(
        request: ConflictRequest,
        area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let git = AppState::get(cx).git.clone();
        let conflict = cx.new(|cx| GitConflict::new(request, git.clone(), cx));
        let display = cx.new(|cx| DiffState::new([], cx));
        let focus = display.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.on_focus(&focus, window, |this, _, cx| {
                if let Some(area) = this.area.upgrade() {
                    cx.defer(move |cx| area.update(cx, |_, cx| cx.emit(DockEvent::LayoutChanged)));
                }
            }),
            cx.observe(&conflict, |this, conflict, cx| {
                // A resolution that was not written leaves the file in conflict and the tab open.
                if conflict.read(cx).error.is_some() {
                    this.closing = false;
                }
                this.sync_display(cx);
                cx.notify()
            }),
            cx.observe_in(&git, window, |this, _, window, cx| {
                if this.closing && this.state(cx) == ConflictState::Resolved {
                    this.close(window, cx);
                }
                cx.notify()
            }),
            cx.subscribe(&display, |_, _, event, cx| {
                if matches!(event, DiffEvent::ConflictResolved(..)) {
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            conflict,
            display,
            synced_generation: None,
            git,
            area,
            group: None,
            alone: false,
            current: None,
            closing: false,
            _subscriptions: subscriptions,
        };
        this.sync_display(cx);
        this
    }

    /// Reloads the widget when the working file changed; its resolutions start over.
    fn sync_display(&mut self, cx: &mut Context<Self>) {
        let conflict = self.conflict.read(cx);
        if self.synced_generation == Some(conflict.generation()) {
            return;
        }
        self.synced_generation = Some(conflict.generation());
        let files = match conflict.content() {
            Some(Content::Text { file, .. }) => vec![file.clone()],
            _ => Vec::new(),
        };
        self.current = None;
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
        format!("{} · Conflicts", self.conflict.read(cx).request.path).into()
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

    fn state(&self, cx: &App) -> ConflictState {
        self.git
            .read(cx)
            .conflict_state(&self.conflict.read(cx).request)
    }

    /// Whether each conflict of the file has a chosen resolution, in file order.
    fn resolutions(&self, cx: &App) -> Vec<bool> {
        let display = self.display.read(cx);
        let path = &self.conflict.read(cx).request.path;
        let count = display
            .files()
            .first()
            .map_or(0, |file| file.conflicts().len());
        (0..count)
            .map(|ix| display.conflict_resolution(path, ix).is_some())
            .collect()
    }

    fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(next) = body::next_conflict(self.current, &self.resolutions(cx), forward) else {
            return;
        };
        self.current = Some(next);
        let path = self.conflict.read(cx).request.path.clone();
        let line = self
            .display
            .read(cx)
            .files()
            .first()
            .and_then(|file| body::anchor_line(file.conflicts(), next));
        if let Some(line) = line {
            let position = DiffLinePosition::new(path, DiffSide::Modified, line);
            self.display
                .update(cx, |d, cx| d.scroll_to_line(position, cx));
        }
    }

    /// Chooses `resolution` for every conflict still undecided; each stays undoable.
    fn accept_all(&mut self, resolution: DiffConflictResolution, cx: &mut Context<Self>) {
        let path = self.conflict.read(cx).request.path.clone();
        let resolved = self.resolutions(cx);
        self.display.update(cx, |display, cx| {
            for (ix, _) in resolved.iter().enumerate().filter(|(_, done)| !**done) {
                display.resolve_conflict(&path, ix, Some(resolution), cx);
            }
        });
        cx.notify();
    }

    /// Writes the chosen resolutions and stages the file; a file whose markers are already
    /// gone is only staged.
    fn mark_resolved(&mut self, cx: &mut Context<Self>) {
        if !Progress::of(&self.resolutions(cx)).is_complete() {
            return;
        }
        let path = self.conflict.read(cx).request.path.clone();
        let text = self.display.read(cx).resolved_text(&path);
        self.closing = true;
        self.conflict.update(cx, |conflict, cx| match text {
            Some(text) => conflict.apply(text, cx),
            None => conflict.mark_resolved(cx),
        });
    }

    fn take_side(
        &mut self,
        kind: Option<Conflict>,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let request = self.conflict.read(cx).request.clone();
        self.closing = true;
        sides::take(
            self.git.clone(),
            request.target,
            &request.path,
            kind,
            side,
            window,
            cx,
        );
    }

    fn resolve_with_agent(&self, cx: &mut Context<Self>) {
        let request = self.conflict.read(cx).request.clone();
        self.git.update(cx, |git, cx| {
            git.prepare_conflict_draft(request.target, &[request.path], cx)
        });
    }

    fn open_file(&self, line: Option<usize>, cx: &mut App) {
        let request = self.conflict.read(cx).request.clone();
        open_file(&request, line, cx);
    }

    /// The explanation of a file that cannot be resolved in place, above its whole-file choices.
    fn sides(
        &self,
        explanation: String,
        kind: Option<Conflict>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy = self
            .git
            .read(cx)
            .is_busy_at(&self.conflict.read(cx).request.target);
        let focus = self.focus_handle(cx);
        let buttons = sides::choices(kind).iter().map(|&side| {
            let id = match side {
                Side::Ours => "conflict-use-current",
                Side::Theirs => "conflict-use-incoming",
            };
            let hover = Hover::new(id, window, cx).enabled(!busy);
            Button::new(id)
                .cursor_pointer()
                .small()
                .label(sides::label(kind, side))
                .tooltip(sides::tooltip(kind, side))
                .disabled(busy)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.take_side(kind, side, window, cx)),
                )
                .smooth_hover(ButtonLook::Outline, &hover, cx)
        });
        let buttons: Vec<_> = buttons.collect();
        tab_message::render(explanation, &focus)
            .flex()
            .flex_col()
            .gap_3()
            .child(h_flex().gap_2().children(buttons))
    }
}

/// Opens the working file in the editor, at a one-based line when one is given.
fn open_file(request: &ConflictRequest, line: Option<usize>, cx: &mut App) {
    let open = OpenFile {
        target: request.target.clone(),
        path: request.path.clone().into(),
        position: line.map(|line| {
            Position::new(u32::try_from(line.saturating_sub(1)).unwrap_or(u32::MAX), 0)
        }),
        preview: false,
        placement: Default::default(),
    };
    AppState::get(cx)
        .documents
        .clone()
        .update(cx, |documents, cx| documents.request(open, cx));
}

impl EventEmitter<PanelEvent> for ConflictPanel {}
impl Focusable for ConflictPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.display.read(cx).focus_handle(cx)
    }
}
impl BasePanel for ConflictPanel {
    fn panel_name(&self) -> &'static str {
        "GitConflictPanel"
    }
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.conflict.read(cx).request.target.clone();
        self.conflict.update(cx, |conflict, cx| {
            conflict.set_active(active);
            if active {
                conflict.refresh(cx);
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
        self.conflict.update(cx, |c, _| c.set_active(false));
    }
}
impl Panel for ConflictPanel {
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        (!self.alone).then(|| self.label(cx))
    }
    fn title(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let close_hover = Hover::new("conflict-close", window, cx);
        h_flex().gap_2().child(self.label(cx)).child(
            Button::new("conflict-close")
                .cursor_pointer()
                .small()
                .label("×")
                .tooltip("Close conflicts")
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
impl Render for ConflictPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state(cx);
        let kind = match state {
            ConflictState::Unmerged(kind) => Some(kind),
            ConflictState::Unknown | ConflictState::Resolved => None,
        };
        let conflict = self.conflict.read(cx);
        let error = conflict.error.clone();
        let body = body::body(state, conflict.content().map(File::from));
        let progress = matches!(body, Body::Conflicts | Body::Merged)
            .then(|| Progress::of(&self.resolutions(cx)));
        let panel = cx.entity().downgrade();
        let content = v_flex()
            .key_context("ConflictPanel")
            .size_full()
            .min_w_0()
            .min_h_0()
            .on_action(cx.listener(|this, _: &CloseFile, window, cx| this.close(window, cx)))
            .child(self.toolbar(progress, state != ConflictState::Resolved, window, cx))
            .when_some(error, |content, error| {
                content.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            });
        let focus = self.focus_handle(cx);
        match body {
            Body::Loading => content.child(tab_message::render("Loading conflicts…", &focus)),
            Body::Resolved => content.child(tab_message::render(
                "This file is no longer in conflict.",
                &focus,
            )),
            Body::Sides(explanation) => content.child(self.sides(explanation, kind, window, cx)),
            Body::Conflicts | Body::Merged => content
                .when(body == Body::Merged, |content| {
                    content.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("No conflict markers are left in this file. Mark it resolved to stage this version."),
                    )
                })
                .child(
                    Diff::new(&self.display)
                        .on_line_click(move |position, event, _, cx| {
                            if event.click_count() < 2 {
                                return;
                            }
                            let line = position.line();
                            let _ = panel.update(cx, |panel, cx| panel.open_file(Some(line), cx));
                        })
                        .flex_1()
                        .min_h_0(),
                ),
        }
    }
}
