//! Git section of the right sidebar. Renders snapshots and dispatches worktree-scoped intents.
use super::git_controls;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _, SmoothHoverBg as _};
use crate::ui::text_field::TextField;
use crate::{
    app::AppState,
    state::git::{Git, Section, Target},
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::git::{
    changes::Action,
    diff::DiffSource,
    status::{Change, FileChange, Status},
};
use std::collections::HashSet;

mod commit;
mod file_row;
mod files;
mod header;
mod review;

actions!(source_control, [CommitChanges]);

const TITLE_TEXT: Pixels = px(13.);
const META_TEXT: Pixels = px(11.);
const ROW_HEIGHT: Pixels = px(26.);
const GROUP_HEIGHT: Pixels = px(24.);
const BADGE_SIZE: Pixels = px(16.);
const EMPTY_ICON: Pixels = px(28.);
const ERROR_HEIGHT: Pixels = px(120.);

pub(crate) struct GitPanel {
    git: Entity<Git>,
    filter: Entity<InputState>,
    message: Entity<TextareaState>,
    draft_target: Option<Target>,
    /// File groups the user folded; everything else is expanded.
    collapsed: HashSet<&'static str>,
    changes_list: ListState,
    change_rows: Vec<(usize, Option<usize>)>,
    /// Session-only height of the commit message box.
    message_size: commit::MessageSize,
    _subscriptions: Vec<Subscription>,
}

impl GitPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let git = AppState::get(cx).git.clone();
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files…"));
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Commit message")
        });
        let subscriptions = vec![
            cx.observe_in(&git, window, |this, _, window, cx| {
                let state = this.git.read(cx);
                let target = state.target.clone();
                let draft = target
                    .as_ref()
                    .map(|t| state.draft(t).to_string())
                    .unwrap_or_default();
                this.draft_target = target;
                if this.message.read(cx).value().as_ref() != draft {
                    this.message
                        .update(cx, |m, cx| m.set_value(draft, window, cx));
                }
                cx.notify();
            }),
            cx.observe(&filter, |_, _, cx| cx.notify()),
            cx.subscribe(&message, |this, input, event, cx| {
                if matches!(event, InputEvent::Change)
                    && let Some(target) = this.draft_target.clone()
                {
                    let message = input.read(cx).value().to_string();
                    this.git
                        .update(cx, |g, cx| g.set_draft(target, message, cx));
                }
            }),
        ];
        Self {
            git,
            filter,
            message,
            draft_target: None,
            collapsed: HashSet::new(),
            changes_list: ListState::new(0, ListAlignment::Top, ROW_HEIGHT),
            change_rows: Vec::new(),
            message_size: commit::MessageSize::default(),
            _subscriptions: subscriptions,
        }
    }

    fn toggle_group(&mut self, group: &'static str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(group) {
            self.collapsed.insert(group);
        }
        cx.notify();
    }

    fn error(
        &self,
        error: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let terminal_hover = Hover::new("error-terminal", window, cx);
        let verify_hover = Hover::new("error-verified", window, cx);
        let needs_verification = self.git.read(cx).needs_verification();
        let inspected = self.git.read(cx).result_inspected();
        v_flex()
            .px_3()
            .pb_2()
            .gap_1()
            .child(
                div()
                    .id("git-error")
                    .max_h(ERROR_HEIGHT)
                    .overflow_y_scroll()
                    .child(
                        Alert::error("git-error-alert", error)
                            .small()
                            .on_close(cx.listener(|this, _, _, cx| {
                                this.git.update(cx, |g, cx| g.clear_error(cx))
                            })),
                    ),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .when(needs_verification, |row| {
                        row.child(
                            Button::new("error-verified")
                                .cursor_pointer()
                                .xsmall()
                                .label(if inspected {
                                    "I reviewed this result"
                                } else {
                                    "Verify actual result"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.git.update(cx, |g, cx| {
                                        if inspected {
                                            g.acknowledge_outcome(cx)
                                        } else {
                                            g.verify_result(cx)
                                        }
                                    })
                                }))
                                .smooth_hover(ButtonLook::Outline, &verify_hover, cx),
                        )
                    })
                    .child(
                        Button::new("error-terminal")
                            .cursor_pointer()
                            .xsmall()
                            .icon(Lucide::SquareTerminal)
                            .label("Open terminal")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git.update(cx, |g, cx| g.terminal(cx))
                            }))
                            .smooth_hover(ButtonLook::Outline, &terminal_hover, cx),
                    ),
            )
    }

    fn tabs(&self, section: Section, changes: usize) -> impl IntoElement {
        let git = self.git.clone();
        TabBar::new("git-sections")
            .underline()
            .small()
            .selected_index(match section {
                Section::Changes => 0,
                Section::Compare => 1,
                Section::History => 2,
            })
            .child(
                Tab::new()
                    .label("Changes")
                    .when(changes > 0, |t| t.suffix(count_tag(changes))),
            )
            .child(Tab::new().label("Compare"))
            .child(Tab::new().label("History"))
            .on_click(move |ix, _, cx| {
                let section = match ix {
                    0 => Section::Changes,
                    1 => Section::Compare,
                    _ => Section::History,
                };
                git.update(cx, |g, cx| g.set_section(section, cx));
            })
    }
}

impl Render for GitPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.git.read(cx);
        let has_target = state.target.is_some();
        let status = state.status.clone();
        let busy = state.is_busy();
        let section = state.section;
        let error: Option<SharedString> = state.error.clone().map(Into::into);
        let has_files = match section {
            Section::Changes => status.as_ref().is_some_and(|s| !s.files.is_empty()),
            Section::Compare => !state.files.is_empty(),
            Section::History => state.selected_commit.is_some() && !state.files.is_empty(),
        };
        let changes = status.as_ref().map(|s| s.files.len()).unwrap_or(0);
        let panel = v_flex()
            .size_full()
            .min_w_0()
            .key_context("SourceControl")
            .bg(cx.theme().background)
            .on_action(cx.listener(|this, _: &CommitChanges, _, cx| this.commit(cx)))
            // The panel's bounds cap the message box at half its height.
            .on_drag_move(cx.listener(
                |this, event: &DragMoveEvent<commit::MessageResize>, _, cx| {
                    if this
                        .message_size
                        .drag(event.event.position.y, event.bounds.size.height)
                    {
                        cx.notify();
                    }
                },
            ));
        if !has_target {
            return panel.child(empty_state(
                Lucide::GitBranch,
                "No worktree selected",
                "Pick a worktree in the sidebar to see its changes.",
                cx,
            ));
        }
        panel
            .child(self.header(window, cx))
            .when_some(error, |p, error| p.child(self.error(error, window, cx)))
            .child(div().px_2().child(self.tabs(section, changes)))
            .when(has_files, |p| {
                p.child(
                    div().px_3().py_2().child(TextField::input(
                        Input::new(&self.filter)
                            .prefix(Icon::new(Lucide::Search).size_3p5())
                            .cleanable(true),
                        &self.filter,
                        cx,
                    )),
                )
            })
            .child(
                v_flex()
                    .id("git-files")
                    .flex_1()
                    .min_h_0()
                    .when(section != Section::Changes, |el| el.overflow_y_scroll())
                    .px_2()
                    .pb_2()
                    .child(match section {
                        Section::Changes => self.changes(window, cx),
                        Section::Compare => self.compare(window, cx),
                        Section::History => self.history(window, cx),
                    }),
            )
            .when(section == Section::Changes, |p| {
                p.child(self.footer(status.as_ref(), busy, window, cx))
            })
    }
}

fn count_tag(count: usize) -> impl IntoElement {
    Tag::secondary()
        .xsmall()
        .rounded_full()
        .child(count.to_string())
}

/// Centered placeholder for a panel or list without content.
fn empty_state(
    icon: Lucide,
    title: &'static str,
    description: &'static str,
    cx: &App,
) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_1p5()
        .p_6()
        .text_center()
        .child(
            Icon::new(icon)
                .size(EMPTY_ICON)
                .text_color(muted.opacity(0.6)),
        )
        .child(
            div()
                .text_size(TITLE_TEXT)
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            div()
                .text_size(META_TEXT)
                .text_color(muted)
                .child(description),
        )
}

fn loading_row(label: &'static str, cx: &App) -> impl IntoElement {
    h_flex()
        .p_3()
        .gap_2()
        .items_center()
        .text_size(META_TEXT)
        .text_color(cx.theme().muted_foreground)
        .child(Spinner::new().icon(Lucide::LoaderCircle).xsmall())
        .child(label)
}
