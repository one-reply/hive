//! Git section of the right sidebar. Renders snapshots and dispatches worktree-scoped intents.
use super::git_controls;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _, SmoothHoverBg as _};
use crate::ui::text_field::TextField;
use crate::{
    app::AppState,
    state::git::{Git, Section},
    state::pull_requests::{PullRequestEvent, PullRequests},
    state::store::Store,
    state::worktree_memory::WorktreeMemory,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::{Disableable as _, Selectable as _};
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
    branches,
    changes::{Action, Discard, ForcePushPreview},
    diff::DiffSource,
    status::{Change, FileChange, Status},
};
use hive_core::projects::Target;
use std::collections::HashSet;

mod availability;
mod commit;
mod dispatch;
mod error;
mod file_row;
mod files;
mod header;
mod operation;
mod pull_request;
mod review;
#[cfg(test)]
mod tests;

use availability::{Blocker, Entry, Facts, PushTarget, Request, staged_count};

actions!(source_control, [CommitChanges]);

const TITLE_TEXT: Pixels = px(13.);
const META_TEXT: Pixels = px(11.);
const ROW_HEIGHT: Pixels = px(26.);
const GROUP_HEIGHT: Pixels = px(24.);
const BADGE_SIZE: Pixels = px(16.);
const EMPTY_ICON: Pixels = px(28.);
const TAB_HOVER_OPACITY: f32 = 0.05;

/// How one worktree's file lists are narrowed: the filter text and the folded groups.
#[derive(Default, PartialEq)]
struct ListView {
    filter: String,
    collapsed: HashSet<&'static str>,
}

pub(crate) struct GitPanel {
    git: Entity<Git>,
    /// What the hosting service says about each worktree's request, and where one is opened.
    pull_requests: Entity<PullRequests>,
    /// The agent the error card offers; the only setting this panel draws.
    default_agent: hive_core::agent::AgentKind,
    filter: Entity<InputState>,
    message: Entity<TextareaState>,
    draft_target: Option<Target>,
    /// File groups the user folded; everything else is expanded.
    collapsed: HashSet<&'static str>,
    /// Filter and folded groups of the worktrees that are not shown.
    list_views: WorktreeMemory<ListView>,
    changes_list: ListState,
    change_rows: Vec<(usize, Option<usize>)>,
    /// Session-only height of the commit message box.
    message_size: commit::MessageSize,
    /// The footer's menu of Git actions.
    actions_menu: commit::ActionsMenu,
    _subscriptions: Vec<Subscription>,
}

impl GitPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::get(cx);
        Self::over(
            app.git.clone(),
            app.store.clone(),
            app.pull_requests.clone(),
            window,
            cx,
        )
    }

    /// The panel of `git`, taking the one setting it draws from `store` and opening requests
    /// through `pull_requests`.
    fn over(
        git: Entity<Git>,
        store: Entity<Store>,
        pull_requests: Entity<PullRequests>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files…"));
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Commit message")
        });
        let subscriptions = vec![
            // The store saves on every layout and cursor change; the panel draws one setting.
            cx.observe(&store, |this, store, cx| {
                let agent = store.read(cx).state.settings.default_cli_agent();
                if this.default_agent != agent {
                    this.default_agent = agent;
                    cx.notify();
                }
            }),
            cx.observe_in(&git, window, |this, _, window, cx| {
                let state = this.git.read(cx);
                let target = state.target.clone();
                let draft = target
                    .as_ref()
                    .map(|t| state.draft(t).to_string())
                    .unwrap_or_default();
                // The menu lists what this worktree allowed while it was idle.
                if this.draft_target != target || state.is_busy() {
                    this.close_actions_menu(window, cx);
                }
                if this.draft_target != target {
                    this.follow_list_view(target.as_ref(), window, cx);
                }
                this.draft_target = target;
                if this.message.read(cx).value().as_ref() != draft {
                    this.message
                        .update(cx, |m, cx| m.set_value(draft, window, cx));
                }
                cx.notify();
            }),
            cx.observe(&filter, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &pull_requests,
                window,
                |_, _, event: &PullRequestEvent, window, cx| {
                    pull_request::announce(event, window, cx)
                },
            ),
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
        let draft_target = git.read(cx).target.clone();
        Self {
            git,
            pull_requests,
            default_agent: store.read(cx).state.settings.default_cli_agent(),
            filter,
            message,
            draft_target,
            collapsed: HashSet::new(),
            list_views: WorktreeMemory::default(),
            changes_list: ListState::new(0, ListAlignment::Top, ROW_HEIGHT),
            change_rows: Vec::new(),
            message_size: commit::MessageSize::default(),
            actions_menu: commit::ActionsMenu::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Parks the filter and folded groups of the worktree shown so far (`draft_target`) and
    /// shows the ones `target` had.
    fn follow_list_view(
        &mut self,
        target: Option<&Target>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shown = ListView {
            filter: self.filter.read(cx).value().to_string(),
            collapsed: std::mem::take(&mut self.collapsed),
        };
        let view = self
            .list_views
            .switch(self.draft_target.as_ref(), target, shown);
        self.collapsed = view.collapsed;
        if self.filter.read(cx).value().as_ref() != view.filter {
            self.filter
                .update(cx, |filter, cx| filter.set_value(view.filter, window, cx));
        }
    }

    fn toggle_group(&mut self, group: &'static str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(group) {
            self.collapsed.insert(group);
        }
        cx.notify();
    }

    fn tabs(
        &self,
        section: Section,
        changes: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                section_tab("Changes", window, cx)
                    .when(changes > 0, |t| t.suffix(count_tag(changes))),
            )
            .child(section_tab("Compare", window, cx))
            .child(section_tab("History", window, cx))
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

fn section_tab(label: &'static str, window: &mut Window, cx: &mut App) -> Tab {
    let hover = Hover::new(SharedString::from(format!("git-tab-{label}")), window, cx);
    let foreground = cx.theme().foreground;
    Tab::new()
        .cursor_pointer()
        .aria_label(label)
        .on_hover(hover.listener())
        // Tab replaces its own background when selected or hovered.
        .child(
            h_flex()
                .h_full()
                .px_1p5()
                .rounded_sm()
                .bg(hover.mix(
                    foreground.opacity(0.),
                    foreground.opacity(TAB_HOVER_OPACITY),
                ))
                .child(label),
        )
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
        let banner = operation::Banner::new(state.operation(), state.conflicted_paths().len());
        let panel = v_flex()
            .size_full()
            .min_w_0()
            .key_context("SourceControl")
            .bg(cx.theme().background)
            .on_action(cx.listener(|this, _: &CommitChanges, window, cx| {
                this.perform(Entry::Commit, window, cx)
            }))
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
        if state.has_no_repository() {
            return panel.child(empty_state(
                Lucide::GitBranch,
                "Not a Git repository",
                "Select another project and return after initializing Git.",
                cx,
            ));
        }
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
            .when_some(banner, |p, banner| {
                p.child(self.operation(banner, window, cx))
            })
            .child(div().px_2().child(self.tabs(section, changes, window, cx)))
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
