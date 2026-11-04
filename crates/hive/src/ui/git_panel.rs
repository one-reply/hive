//! Git section of the right sidebar. Renders snapshots and dispatches worktree-scoped intents.
use super::git_controls;
use crate::{
    app::AppState,
    state::git::{Git, Section, Target},
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
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
            _subscriptions: subscriptions,
        }
    }

    fn commit(&self, cx: &mut Context<Self>) {
        let state = self.git.read(cx);
        if state.is_busy() || state.status.as_ref().is_none_or(|s| staged_count(s) == 0) {
            return;
        }
        let message = self.message.read(cx).value().to_string();
        if !message.trim().is_empty() {
            self.git
                .update(cx, |g, cx| g.perform(Action::Commit(message), cx));
        }
    }

    fn toggle_group(&mut self, group: &'static str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(group) {
            self.collapsed.insert(group);
        }
        cx.notify();
    }

    fn error(&self, error: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
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
                h_flex().justify_end().child(
                    Button::new("error-terminal")
                        .cursor_pointer()
                        .outline()
                        .xsmall()
                        .icon(Lucide::SquareTerminal)
                        .label("Open terminal")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.git.update(cx, |g, cx| g.terminal(cx))
                        })),
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

    fn footer(
        &self,
        status: Option<&Status>,
        busy: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let staged = status.map(staged_count).unwrap_or(0);
        let conflicts = status.is_some_and(|s| s.has_conflicts());
        let empty_message = self.message.read(cx).value().trim().is_empty();
        let blocker = if conflicts {
            Some("Resolve conflicts first")
        } else if staged == 0 {
            Some("Stage changes first")
        } else if empty_message {
            Some("Write a commit message")
        } else {
            None
        };
        let label: SharedString = match staged {
            0 => "Commit".into(),
            1 => "Commit 1 file".into(),
            n => format!("Commit {n} files").into(),
        };
        let shortcut = Kbd::binding_for_action(&CommitChanges, Some("SourceControl"), window);
        v_flex()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(Textarea::new(&self.message))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("commit-staged")
                            .cursor_pointer()
                            .primary()
                            .small()
                            .flex_1()
                            .icon(Lucide::GitCommitHorizontal)
                            .label(label)
                            .loading(busy)
                            .disabled(busy || blocker.is_some())
                            .when_some(blocker, |b, reason| b.tooltip(reason))
                            .when(blocker.is_none(), |b| {
                                b.tooltip_with_action(
                                    "Commit staged changes",
                                    &CommitChanges,
                                    Some("SourceControl"),
                                )
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.commit(cx))),
                    )
                    .when_some(shortcut, |row, kbd| row.child(kbd.outline())),
            )
    }
}

impl Render for GitPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.git.read(cx);
        let target = state.target.clone();
        let status = state.status.clone();
        let busy = state.is_busy();
        let section = state.section;
        let error: Option<SharedString> = state.error.clone().map(Into::into);
        let has_files = match section {
            Section::Changes => status.as_ref().is_some_and(|s| !s.files.is_empty()),
            Section::Compare => !state.files.is_empty(),
            Section::History => !state.commits.is_empty() || !state.files.is_empty(),
        };
        let changes = status.as_ref().map(|s| s.files.len()).unwrap_or(0);
        let panel = v_flex()
            .size_full()
            .min_w_0()
            .key_context("SourceControl")
            .bg(cx.theme().background)
            .on_action(cx.listener(|this, _: &CommitChanges, _, cx| this.commit(cx)));
        let Some(target) = target else {
            return panel.child(empty_state(
                Lucide::GitBranch,
                "No worktree selected",
                "Pick a worktree in the sidebar to see its changes.",
                cx,
            ));
        };
        panel
            .child(self.header(&target, cx))
            .when_some(error, |p, error| p.child(self.error(error, cx)))
            .child(div().px_2().child(self.tabs(section, changes)))
            .when(has_files, |p| {
                p.child(
                    div().px_3().py_2().child(
                        Input::new(&self.filter)
                            .small()
                            .prefix(Icon::new(Lucide::Search).size_3p5())
                            .cleanable(true),
                    ),
                )
            })
            .child(
                v_flex()
                    .id("git-files")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .child(match section {
                        Section::Changes => self.changes(cx),
                        Section::Compare => self.compare(cx),
                        Section::History => self.history(cx),
                    }),
            )
            .when(section == Section::Changes, |p| {
                p.child(self.footer(status.as_ref(), busy, window, cx))
            })
    }
}

fn staged_count(status: &Status) -> usize {
    if status.has_conflicts() {
        return 0;
    }
    status.files.iter().filter(|f| f.staged.is_some()).count()
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
        .child(Icon::new(Lucide::LoaderCircle).size_3())
        .child(label)
}
