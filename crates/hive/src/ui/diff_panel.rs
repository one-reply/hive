//! Dockable readonly diff with explicit index actions for complete text hunks.
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use crate::{
    app::AppState,
    state::{
        git::{DiffRequest, Git},
        git_diff::GitDiff,
        store::Store,
    },
    ui::{
        code_reference::{self, CopyCodeReference},
        editor_panel::CloseFile,
        tab_message,
        workspace::right_sidebar_toggle,
    },
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
use hive_core::agent::reference::{self, CodeReference};
use hive_core::git::diff::DiffSource;
use hive_core::git::review_notes::Line;

mod blocks;
mod comments;
mod viewport;
use viewport::Reveal;

pub(crate) struct DiffPanel {
    pub review: Entity<GitDiff>,
    /// The shared widget owns row layout, code selection, and scrolling.
    display: Entity<DiffState>,
    synced_generation: Option<u64>,
    git: Entity<Git>,
    store: Entity<Store>,
    /// `(diff_split, git_panel_open)` as last drawn.
    drawn_settings: (bool, bool),
    area: WeakEntity<DockArea>,
    group: Option<WeakEntity<TabGroup>>,
    alone: bool,
    reveal: Option<Reveal>,
    visible: Vec<SharedString>,
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
        Self::over(
            request,
            area,
            app.git.clone(),
            app.store.clone(),
            window,
            cx,
        )
    }

    fn over(
        request: DiffRequest,
        area: WeakEntity<DockArea>,
        git: Entity<Git>,
        store: Entity<Store>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let reveal = Some(Reveal::new(request.block_key()));
        let review = cx.new(|cx| GitDiff::new(request, git.clone(), cx));
        let display = cx.new(|cx| DiffState::new([], cx).with_mode(diff_mode(&store, cx)));
        let focus = display.read(cx).focus_handle(cx);
        let mut reviewed = store.read(cx).state.reviewed.clone();
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
            // The store saves on every layout and cursor change; the panel draws two settings.
            cx.observe(&store, move |this, store, cx| {
                let marks = &store.read(cx).state.reviewed;
                if *marks != reviewed {
                    reviewed.clone_from(marks);
                    cx.notify();
                }
                let drawn = drawn_settings(&store, cx);
                if this.drawn_settings == drawn {
                    return;
                }
                this.drawn_settings = drawn;
                let mode = diff_mode(&store, cx);
                this.display.update(cx, |d, cx| d.set_mode(mode, cx));
                cx.notify()
            }),
        ];
        let mut this = Self {
            drawn_settings: drawn_settings(&store, cx),
            review,
            display,
            synced_generation: None,
            git,
            store,
            area,
            group: None,
            alone: false,
            reveal,
            visible: vec![],
            _subscriptions: subscriptions,
        };
        this.sync_display(cx);
        this
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

    pub(crate) fn label(&self, _: &App) -> SharedString {
        "Files changed".into()
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

    /// The selected lines as a reference: selected line numbers, or selected text that
    /// appears once in the diff.
    fn code_reference(&self, cx: &App) -> Option<CodeReference> {
        let review = self.review.read(cx);
        let display = self.display.read(cx);
        let selected = display.selected_lines();
        let key = selected
            .as_ref()
            .map(|range| range.path())
            .or_else(|| display.selected_text_file(cx).map(|file| file.path()))?;
        let block = review.blocks.get(key)?;
        let (request, patch) = (&block.entry.request, block.patch.as_ref()?);
        let (version, lines) = match selected {
            Some(range) => reference::diff_lines(
                patch,
                &request.source,
                line(range.side(), range.start()),
                line(range.end_side(), range.end()),
            )?,
            None => reference::find_in_diff(patch, &request.source, &display.selected_text(cx))?,
        };
        CodeReference::new(
            request.target.host.clone(),
            &request.target.path,
            std::path::Path::new(&request.path),
            lines,
            version,
        )
        .inspect_err(|error| tracing::warn!(%error, "no code reference for this diff"))
        .ok()
    }

    fn navigate(&mut self, next: bool, cx: &mut Context<Self>) {
        self.reveal = None;
        self.display.update(cx, |display, cx| {
            if next {
                display.next_change(cx)
            } else {
                display.previous_change(cx)
            }
        });
    }
}

/// A selection endpoint of the widget as `hive-core` names it.
fn line(side: DiffSide, number: usize) -> Line {
    match side {
        DiffSide::Original => Line::Original(number),
        DiffSide::Modified => Line::Modified(number),
    }
}

/// The user's side-by-side preference as the widget's mode.
fn drawn_settings(store: &Entity<Store>, cx: &App) -> (bool, bool) {
    let settings = &store.read(cx).state.settings;
    (settings.diff_split, settings.git_panel_open)
}

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
        self.review.update(cx, |r, cx| r.set_active(active, cx));
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
        self.review.update(cx, |r, cx| r.set_active(false, cx));
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
        let listed = review.listed;
        let empty = review.order.is_empty();
        let error = review.error.clone();
        let display = self.display.clone();
        let source = match &request.source {
            DiffSource::WorkingTree | DiffSource::Staged | DiffSource::Untracked => {
                "Local changes".into()
            }
            DiffSource::Commit(oid) => {
                format!(
                    "Commit {} · first parent",
                    oid.chars().take(7).collect::<String>()
                )
            }
            DiffSource::Compare(base) => format!("{base} … HEAD · committed"),
        };
        let split = self.store.read(cx).state.settings.diff_split;
        let pan = super::diff_view::pan_handle(&display, window, cx);
        let diff = self.blocks(pan, cx);
        let mut content = v_flex()
            .key_context("DiffPanel")
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, _| {
                if matches!(
                    event.keystroke.key.as_str(),
                    "up" | "down" | "pageup" | "pagedown" | "home" | "end"
                ) {
                    this.reveal = None;
                }
            }))
            .size_full()
            .min_w_0()
            .min_h_0()
            .on_action(cx.listener(|this, _: &CloseFile, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|this, _: &CopyCodeReference, window, cx| {
                code_reference::copy(
                    this.code_reference(cx),
                    "Select line numbers, or text that appears once in this diff.",
                    window,
                    cx,
                )
            }))
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(source))
                    .child(
                        Button::new("review-notes")
                            .cursor_pointer()
                            .small()
                            .label(format!("Review notes · {note_count}"))
                            .disabled(note_count == 0)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.review_prompt(window, cx)),
                            )
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
                                this.review.update(cx, |r, cx| r.reload(cx));
                            }))
                            .smooth_hover(ButtonLook::Ghost, &refresh_hover, cx),
                    ),
            );
        if let Some(error) = error {
            content = content.child(
                tab_message::render(error, &self.focus_handle(cx)).text_color(cx.theme().danger),
            );
        } else if empty {
            content = content.child(tab_message::render(
                if listed {
                    "No changes in this view."
                } else {
                    "Loading changes…"
                },
                &self.focus_handle(cx),
            ));
        } else {
            content = content.child(
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.track_viewport(cx))
                    .child(super::diff_view::render(&display, diff, window, cx)),
            );
        }
        content
    }
}

#[cfg(test)]
mod tests;
