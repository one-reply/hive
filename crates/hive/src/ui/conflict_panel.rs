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
mod rows;
pub(crate) mod sides;
mod syntax;
mod three_way;
mod toolbar;

use body::{Body, File, Progress};
use rows::{DisplayRow, RowKey, build_rows};
use syntax::MergeHighlights;
use toolbar::SideNames;

struct InlineEditor {
    conflict: usize,
    state: Entity<EditorState>,
}

pub(crate) struct ConflictPanel {
    pub conflict: Entity<GitConflict>,
    synced_generation: Option<u64>,
    resolutions: Vec<Option<ConflictResolution>>,
    expanded_folds: HashSet<usize>,
    collapse_unchanged: bool,
    shown_bases: HashSet<usize>,
    editing: Option<InlineEditor>,
    highlights: MergeHighlights,
    highlights_generation: Option<u64>,
    highlights_theme: Option<Arc<HighlightTheme>>,
    _highlights_task: Option<Task<()>>,
    rows: Vec<DisplayRow>,
    conflict_rows: Vec<usize>,
    resolved_lines: Vec<Vec<String>>,
    list: ListState,
    focus: FocusHandle,
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
        let focus = cx.focus_handle();
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
                this.sync_document(cx);
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
            resolutions: Vec::new(),
            expanded_folds: HashSet::new(),
            collapse_unchanged: true,
            shown_bases: HashSet::new(),
            editing: None,
            highlights: MergeHighlights::default(),
            highlights_generation: None,
            highlights_theme: None,
            _highlights_task: None,
            rows: Vec::new(),
            conflict_rows: Vec::new(),
            resolved_lines: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(0.)),
            focus,
            git,
            area,
            group: None,
            alone: false,
            current: None,
            closing: false,
            _subscriptions: subscriptions,
        };
        this.sync_document(cx);
        this
    }

    fn document<'a>(&self, cx: &'a App) -> Option<&'a MergeDocument> {
        match self.conflict.read(cx).content() {
            Some(Content::Text { document, .. }) => Some(document),
            _ => None,
        }
    }

    /// Reloads resolutions, rows, and background highlighting when the file on disk changes.
    fn sync_document(&mut self, cx: &mut Context<Self>) {
        let conflict = self.conflict.read(cx);
        let generation = conflict.generation();
        if self.synced_generation == Some(generation) {
            return;
        }
        self.synced_generation = Some(generation);
        let count = match conflict.content() {
            Some(Content::Text { document, .. }) => document.conflict_count,
            _ => 0,
        };
        self.resolutions = vec![None; count];
        self.expanded_folds.clear();
        self.shown_bases.clear();
        self.editing = None;
        self.current = None;
        self.rebuild_rows(true, cx);
        self.prepare_highlights(cx);
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
        let Some(doc) = self.document(cx) else {
            self.rows.clear();
            self.conflict_rows.clear();
            self.resolved_lines.clear();
            self.list.reset(0);
            return;
        };
        let old_keys: Vec<RowKey> = self.rows.iter().map(DisplayRow::key).collect();
        let projected = build_rows(
            doc,
            &self.resolutions,
            &self.expanded_folds,
            self.collapse_unchanged,
            self.editing.as_ref().map(|e| e.conflict),
            &self.shown_bases,
        );
        let new_keys: Vec<RowKey> = projected.rows.iter().map(DisplayRow::key).collect();
        if reset {
            self.list.reset(projected.rows.len());
            if let Some(&first_conflict) = projected.conflict_rows.first() {
                self.list.scroll_to(ListOffset {
                    item_ix: first_conflict.saturating_sub(2),
                    offset_in_item: px(0.),
                });
            }
        } else {
            crate::ui::list_scroll::replace_rows(&self.list, &old_keys, &new_keys);
        }
        self.rows = projected.rows;
        self.conflict_rows = projected.conflict_rows;
        self.resolved_lines = projected.resolved_lines;
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
        self.resolutions.iter().map(Option::is_some).collect()
    }

    fn resolve_conflict(
        &mut self,
        ix: usize,
        resolution: Option<ConflictResolution>,
        cx: &mut Context<Self>,
    ) {
        if ix >= self.resolutions.len() {
            return;
        }
        if self.resolutions[ix] == resolution {
            return;
        }
        self.resolutions[ix] = resolution;
        self.current = Some(ix);
        self.rebuild_rows(false, cx);
        cx.notify();
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

    fn start_editing(&mut self, conflict_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(region) = self.document(cx).and_then(|doc| doc.conflict(conflict_ix)) else {
            return;
        };
        let choice = self.resolutions.get(conflict_ix).and_then(Option::as_ref);
        let seed = region.resolution_text(choice.unwrap_or(&ConflictResolution::BothCurrentFirst));
        let language = editor_language(Path::new(&self.conflict.read(cx).request.path));
        let state = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .default_value(seed)
                .soft_wrap(false);
            editor.set_highlighter(language, cx);
            editor
        });
        let editor_focus = state.read(cx).focus_handle(cx);
        self.editing = Some(InlineEditor {
            conflict: conflict_ix,
            state,
        });
        self.current = Some(conflict_ix);
        self.rebuild_rows(false, cx);
        if let Some(&row_ix) = self.conflict_rows.get(conflict_ix) {
            self.list.scroll_to_reveal_item(row_ix);
        }
        editor_focus.focus(window, cx);
        cx.notify();
    }

    fn apply_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editing.take() else {
            return;
        };
        let text = editor.state.read(cx).value().to_string();
        if editor.conflict < self.resolutions.len() {
            self.resolutions[editor.conflict] = Some(ConflictResolution::Custom(text));
        }
        self.rebuild_rows(false, cx);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn cancel_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            self.rebuild_rows(false, cx);
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    fn navigate(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(next) = body::next_conflict(self.current, &self.resolutions(), forward) else {
            return;
        };
        self.current = Some(next);
        if let Some(&row_ix) = self.conflict_rows.get(next) {
            self.list.scroll_to(ListOffset {
                item_ix: row_ix.saturating_sub(1),
                offset_in_item: px(0.),
            });
            cx.notify();
        }
    }

    /// Chooses `resolution` for every conflict still undecided; each stays undoable.
    fn accept_all(&mut self, resolution: ConflictResolution, cx: &mut Context<Self>) {
        let mut changed = false;
        for slot in &mut self.resolutions {
            if slot.is_none() {
                *slot = Some(resolution.clone());
                changed = true;
            }
        }
        if changed {
            self.rebuild_rows(false, cx);
            cx.notify();
        }
    }

    /// Writes the chosen resolutions and stages the file; a file whose markers are already
    /// gone is only staged.
    fn mark_resolved(&mut self, cx: &mut Context<Self>) {
        if self.editing.is_some() || !Progress::of(&self.resolutions()).is_complete() {
            return;
        }
        let text = self
            .document(cx)
            .and_then(|doc| doc.resolved_text(&self.resolutions));
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
        self.editing
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
                .child(self.three_way_body(doc, &names, cx)),
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
                        .child("No conflict markers are left in this file. Mark it resolved to stage this version."),
                )
                .child(self.three_way_body(doc, &names, cx)),
            (Body::Conflicts | Body::Merged, None) => {
                content.child(tab_message::render("Loading conflicts…", &focus))
            }
        }
    }
}
