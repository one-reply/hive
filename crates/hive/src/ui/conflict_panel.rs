//! Dockable JetBrains-style 3-pane conflict resolver: Current | Result | Incoming.
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
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{BasePanel, DockArea, DockEvent, Panel, PanelEvent, TabGroup};
use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::input::{EditorState, Position};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::files::editor_language;
use hive_core::git::conflict_merge::{ConflictResolution, MergeDocument};
use hive_core::git::conflicts::{Conflict, Side};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

mod body;
mod draft;
mod result;
#[cfg(test)]
mod result_tests;
mod rows;
#[cfg(test)]
mod scroll_tests;
pub(crate) mod sides;
mod syntax;
mod three_way;
mod toolbar;

use body::{Body, File, Progress};
use draft::Status;
use result::ResultEditor;
use rows::{DisplayRow, build_rows};
use syntax::MergeHighlights;
use toolbar::SideNames;

// Measuring past the viewport keeps the next wrapped row reachable after a list reset.
const ROW_OVERDRAW: Pixels = px(128.);

pub(crate) struct ConflictPanel {
    pub conflict: Entity<GitConflict>,
    synced_generation: Option<u64>,
    result: Option<ResultEditor>,
    expanded_folds: HashSet<usize>,
    collapse_unchanged: bool,
    shown_bases: HashSet<usize>,
    highlights: MergeHighlights,
    highlights_generation: Option<u64>,
    highlights_theme: Option<Arc<HighlightTheme>>,
    _highlights_task: Option<Task<()>>,
    rows: Vec<DisplayRow>,
    lists: [ListState; 2],
    focus: FocusHandle,
    git: Entity<Git>,
    area: WeakEntity<DockArea>,
    group: Option<WeakEntity<TabGroup>>,
    alone: bool,
    /// The conflict targeted by the visible choices and navigation buttons.
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
        Self::over(request, area, git, window, cx)
    }

    fn over(
        request: ConflictRequest,
        area: WeakEntity<DockArea>,
        git: Entity<Git>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let conflict = cx.new(|cx| GitConflict::new(request, git.clone(), cx));
        let focus = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus, window, |this, _, cx| {
                if let Some(area) = this.area.upgrade() {
                    cx.defer(move |cx| area.update(cx, |_, cx| cx.emit(DockEvent::LayoutChanged)));
                }
            }),
            cx.observe_in(&conflict, window, |this, conflict, window, cx| {
                // A resolution that was not written leaves the file in conflict and the tab open.
                if conflict.read(cx).error.is_some() {
                    this.closing = false;
                }
                this.sync_document(window, cx);
                cx.notify()
            }),
            cx.observe_in(&git, window, |this, _, window, cx| {
                if this.closing && this.state(cx) == ConflictState::Resolved {
                    this.close(window, cx);
                }
                cx.notify()
            }),
        ];
        let mut this = Self {
            conflict,
            synced_generation: None,
            result: None,
            expanded_folds: HashSet::new(),
            collapse_unchanged: true,
            shown_bases: HashSet::new(),
            highlights: MergeHighlights::default(),
            highlights_generation: None,
            highlights_theme: None,
            _highlights_task: None,
            rows: Vec::new(),
            lists: std::array::from_fn(|_| ListState::new(0, ListAlignment::Top, ROW_OVERDRAW)),
            focus,
            git,
            area,
            group: None,
            alone: false,
            current: None,
            closing: false,
            _subscriptions: subscriptions,
        };
        this.install_source_scroll(cx);
        this.sync_document(window, cx);
        this
    }

    fn document<'a>(&self, cx: &'a App) -> Option<&'a MergeDocument> {
        match self.conflict.read(cx).content() {
            Some(Content::Text { document, .. }) => Some(document),
            _ => None,
        }
    }

    /// Reloads resolutions, rows, and background highlighting when the file on disk changes.
    fn sync_document(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let conflict = self.conflict.read(cx);
        let generation = conflict.generation();
        if self.synced_generation == Some(generation) {
            return;
        }
        self.synced_generation = Some(generation);
        self.result = match conflict.content().cloned() {
            Some(Content::Text { document, .. }) => {
                let language = editor_language(Path::new(&conflict.request.path));
                Some(ResultEditor::new(&document, language, window, cx))
            }
            _ => None,
        };
        self.expanded_folds.clear();
        self.shown_bases.clear();
        self.current = self
            .document(cx)
            .and_then(|doc| (doc.conflict_count > 0).then_some(0));
        self.rebuild_rows(true, cx);
        self.prepare_highlights(cx);
        self.reveal_current(false, window, cx);
    }

    fn ensure_theme_highlights(&mut self, cx: &mut Context<Self>) {
        let current_theme = cx.theme().highlight_theme.clone();
        let generation = self.conflict.read(cx).generation();
        let same_theme = self
            .highlights_theme
            .as_ref()
            .is_some_and(|prev| Arc::ptr_eq(prev, &current_theme));
        if self.highlights_generation == Some(generation) && same_theme {
            return;
        }
        if let Some(result) = &self.result {
            result.refresh_marks(cx);
        }
        self.prepare_highlights(cx);
    }

    fn prepare_highlights(&mut self, cx: &mut Context<Self>) {
        self.highlights = MergeHighlights::default();
        let conflict = self.conflict.read(cx);
        let generation = conflict.generation();
        let path = conflict.request.path.clone();
        let Some(Content::Text { document, .. }) = conflict.content().cloned() else {
            self.highlights_theme = Some(cx.theme().highlight_theme.clone());
            self.highlights_generation = Some(generation);
            self._highlights_task = None;
            return;
        };
        let theme = cx.theme().highlight_theme.clone();
        self.highlights_generation = Some(generation);
        self.highlights_theme = Some(theme.clone());
        self._highlights_task = Some(cx.spawn(async move |this, cx| {
            let prepared = cx
                .background_executor()
                .spawn(async move { MergeHighlights::prepare(&path, &document, &theme) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.conflict.read(cx).generation() == generation {
                    this.highlights = prepared;
                    cx.notify();
                }
            });
        }));
    }

    fn rebuild_rows(&mut self, reset: bool, cx: &App) {
        let rows = self
            .document(cx)
            .map(|doc| {
                build_rows(
                    doc,
                    &self.expanded_folds,
                    self.collapse_unchanged,
                    &self.shown_bases,
                )
            })
            .unwrap_or_default();
        for list in &self.lists {
            if reset {
                list.reset(rows.len());
            } else {
                crate::ui::list_scroll::replace_rows(list, &self.rows, &rows);
            }
        }
        self.rows = rows;
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
    fn resolutions(&self) -> Vec<bool> {
        self.result
            .iter()
            .flat_map(|r| &r.blocks)
            .map(|b| b.status != Status::Pending)
            .collect()
    }

    fn expand_fold(&mut self, segment: usize, cx: &mut Context<Self>) {
        if self.expanded_folds.insert(segment) {
            self.rebuild_rows(false, cx);
            cx.notify();
        }
    }

    fn toggle_collapse_unchanged(&mut self, cx: &mut Context<Self>) {
        self.collapse_unchanged = !self.collapse_unchanged;
        self.expanded_folds.clear();
        self.rebuild_rows(false, cx);
        cx.notify();
    }

    fn toggle_base(&mut self, conflict: usize, cx: &mut Context<Self>) {
        if !self.shown_bases.remove(&conflict) {
            self.shown_bases.insert(conflict);
        }
        self.rebuild_rows(false, cx);
        cx.notify();
    }

    fn navigate(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.current = body::next_conflict(self.current, &self.resolutions(), forward);
        self.reveal_current(true, window, cx);
        cx.notify();
    }

    fn mark_resolved(&mut self, cx: &mut Context<Self>) {
        let Some(result) = &self.result else { return };
        if !Progress::of(&self.resolutions()).is_complete() || self.conflict.read(cx).is_saving() {
            return;
        }
        let text = result.state.read(cx).text();
        let unchanged = result.blocks.is_empty() && result.original == *text;
        let draft = (!unchanged).then(|| text.to_string());
        self.closing = true;
        self.conflict.update(cx, |conflict, cx| match draft {
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

    fn side_names(&self, cx: &App) -> SideNames {
        let git = self.git.read(cx);
        let request = &self.conflict.read(cx).request;
        let same_target = git.target.as_ref() == Some(&request.target);
        let op_ctx = same_target.then(|| git.operation_context()).flatten();
        let current_branch = same_target
            .then(|| git.status.as_ref().and_then(|s| s.branch.as_deref()))
            .flatten();
        SideNames::resolve(op_ctx, current_branch, self.document(cx))
    }

    /// The explanation of a file that cannot be resolved in place, above its whole-file choices.
    fn sides(
        &self,
        explanation: String,
        kind: Option<Conflict>,
        names: &SideNames,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy = self
            .git
            .read(cx)
            .is_busy_at(&self.conflict.read(cx).request.target);
        let focus = self.focus_handle(cx);
        let buttons = sides::choices(kind).iter().map(|&side| {
            let (id, branch) = match side {
                Side::Ours => ("conflict-use-current", Some(names.current_label.as_str())),
                Side::Theirs => ("conflict-use-incoming", Some(names.incoming_label.as_str())),
            };
            let hover = Hover::new(id, window, cx).enabled(!busy);
            Button::new(id)
                .cursor_pointer()
                .small()
                .label(sides::label_with_branch(kind, side, branch))
                .tooltip(sides::tooltip_with_branch(kind, side, branch))
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
        self.result
            .as_ref()
            .map(|e| e.state.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.focus.clone())
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
        self.sync_document(window, cx);
        self.ensure_theme_highlights(cx);
        let state = self.state(cx);
        let kind = match state {
            ConflictState::Unmerged(kind) => Some(kind),
            ConflictState::Unknown | ConflictState::Resolved => None,
        };
        let names = self.side_names(cx);
        let (error, body_state, document) = {
            let conflict = self.conflict.read(cx);
            let error = conflict.error.clone();
            let body_state = body::body(state, conflict.content().map(File::from));
            let document = match conflict.content() {
                Some(Content::Text { document, .. }) => Some(document.clone()),
                _ => None,
            };
            (error, body_state, document)
        };
        let progress = matches!(body_state, Body::Conflicts | Body::Merged)
            .then(|| Progress::of(&self.resolutions()));

        let content = v_flex()
            .key_context("ConflictPanel")
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .min_h_0()
            .on_action(cx.listener(|this, _: &CloseFile, window, cx| this.close(window, cx)))
            .child(self.toolbar(
                progress,
                state != ConflictState::Resolved,
                &names,
                window,
                cx,
            ))
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
        match (body_state, document) {
            (Body::Loading, _) => content.child(tab_message::render("Loading conflicts…", &focus)),
            (Body::Resolved, _) => content.child(tab_message::render(
                "This file is no longer in conflict.",
                &focus,
            )),
            (Body::Sides(explanation), _) => content
                .child(self.merge_header(&names, None, window, cx))
                .child(self.sides(explanation, kind, &names, window, cx)),
            (Body::Conflicts, Some(doc)) => content
                .child(self.merge_header(&names, progress, window, cx))
                .child(self.three_way_body(doc, &names, window, cx)),
            (Body::Merged, Some(doc)) => content
                .child(self.merge_header(&names, progress, window, cx))
                .child(
                    div()
                        .px_3()
                        .py_1()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("No conflict markers are left in this file. Edit the result or mark it resolved to stage this version."),
                )
                .child(self.three_way_body(doc, &names, window, cx)),
            (Body::Conflicts | Body::Merged, None) => {
                content.child(tab_message::render("Loading conflicts…", &focus))
            }
        }
    }
}
