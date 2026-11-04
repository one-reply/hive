//! Source control sidebar. Renders snapshots and dispatches worktree-scoped intents.
use super::{git_controls, workspace::ToggleGitPanel};
use crate::{
    app::AppState,
    state::git::{Git, Section, Target},
};
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;
use hive_core::git::{
    changes::Action,
    diff::DiffSource,
    status::{Change, FileChange},
};

mod files;

actions!(source_control, [CommitChanges]);
const HEADER_HEIGHT: Pixels = px(34.);
const ERROR_HEIGHT: Pixels = px(120.);

pub(crate) struct GitPanel {
    git: Entity<Git>,
    filter: Entity<InputState>,
    message: Entity<TextareaState>,
    draft_target: Option<Target>,
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
            _subscriptions: subscriptions,
        }
    }

    fn commit(&self, cx: &mut Context<Self>) {
        let state = self.git.read(cx);
        if state.is_busy()
            || state
                .status
                .as_ref()
                .is_none_or(|s| s.has_conflicts() || !s.files.iter().any(|f| f.staged.is_some()))
        {
            return;
        }
        let message = self.message.read(cx).value().to_string();
        if !message.trim().is_empty() {
            self.git
                .update(cx, |g, cx| g.perform(Action::Commit(message), cx));
        }
    }
}

impl Render for GitPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.git.read(cx);
        let target = state.target.clone();
        let status = state.status.clone();
        let busy = state.is_busy();
        let section = state.section;
        let error = state.error.clone();
        let remotes = state.remotes.clone();
        let remote = state.remote.clone();
        let upstream_remote = state.upstream_remote.clone();
        let branch = status
            .as_ref()
            .and_then(|s| s.branch.clone())
            .unwrap_or_else(|| {
                if status.is_some() {
                    "Detached HEAD"
                } else {
                    "Choose worktree"
                }
                .into()
            });
        let staged = status
            .as_ref()
            .is_some_and(|s| !s.has_conflicts() && s.files.iter().any(|f| f.staged.is_some()));
        let has_upstream = status.as_ref().is_some_and(|s| s.upstream.is_some());
        let can_commit = staged && !busy && !self.message.read(cx).value().trim().is_empty();
        let mut panel = v_flex().size_full().min_w_0().key_context("SourceControl").bg(cx.theme().background)
            .on_action(cx.listener(|this, _: &CommitChanges, _, cx| this.commit(cx)))
            .child(h_flex().h(HEADER_HEIGHT).px_3().justify_between().items_center().border_b_1().border_color(cx.theme().border)
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Source Control"))
                .child(h_flex().child(Button::new("git-refresh").cursor_pointer().ghost().small().label("↻").tooltip("Refresh Git").disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.git.update(cx, |g, cx| { g.clear_error(cx); g.refresh(true, cx); }))))
                    .child(Button::new("git-close").cursor_pointer().ghost().small().label("×").tooltip_with_action("Hide source control", &ToggleGitPanel, Some(super::keymap::CONTEXT))
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleGitPanel), cx)))))
            .child(v_flex().p_3().gap_2()
                .child(div().text_xs().text_color(cx.theme().muted_foreground).truncate().child(target.as_ref().map(|t| t.path.display().to_string()).unwrap_or_else(|| "Select a worktree".into())))
                .child(Button::new("branch-picker").cursor_pointer().ghost().small().label(branch.clone()).disabled(target.is_none() || busy)
                    .on_click(cx.listener(|this, _, window, cx| git_controls::branches(this.git.clone(), false, window, cx))))
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child(status.as_ref().map(|s| format!("{}   ↑{}  ↓{}", s.upstream.as_deref().unwrap_or("No upstream"), s.ahead, s.behind)).unwrap_or_default()))
                .child(h_flex().gap_1()
                    .child(Button::new("remote").cursor_pointer().ghost().small().label(remote.clone().unwrap_or_else(|| "Remote…".into())).dropdown_menu({
                        let git = self.git.clone(); move |menu, _, _| remotes.iter().fold(menu, |menu, remote| {
                            let (git, remote) = (git.clone(), remote.clone());
                            menu.item(PopupMenuItem::new(remote.clone()).on_click(move |_, _, cx| {
                                git.update(cx, |g, cx| { g.remote = Some(remote.clone()); cx.notify(); });
                            }))
                        })
                    }))
                    .child(Button::new("fetch").cursor_pointer().ghost().small().label("Fetch").disabled(busy || remote.is_none())
                        .on_click(cx.listener(|this, _, _, cx| { let remote = this.git.read(cx).remote.clone(); if let Some(remote) = remote { this.git.update(cx, |g, cx| g.perform(Action::Fetch(remote), cx)); } })))
                    .child(Button::new("pull").cursor_pointer().ghost().small().label("Pull").disabled(busy || !has_upstream)
                        .on_click(cx.listener(|this, _, _, cx| this.git.update(cx, |g, cx| g.perform(Action::Pull, cx)))))
                    .child(Button::new("push").cursor_pointer().ghost().small().label(if has_upstream { "Push" } else { "Publish" }).disabled(busy || target.is_none() || (if has_upstream { upstream_remote.is_none() } else { remote.is_none() }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let state = this.git.read(cx);
                            let publish = state.status.as_ref().is_none_or(|s| s.upstream.is_none());
                            let remote = if publish { state.remote.clone() } else { state.upstream_remote.clone() };
                            if let Some(remote) = remote {
                                let action = Action::Push { remote: remote.clone(), publish };
                                if publish { git_controls::confirm(this.git.clone(), action, "Publish branch", format!("Push {branch} to {remote} and set its upstream?"), window, cx); }
                                else { this.git.update(cx, |g, cx| g.perform(action, cx)); }
                            }
                        })))));
        if let Some(error) = error {
            panel = panel.child(
                v_flex()
                    .px_3()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .id("git-error")
                            .max_h(ERROR_HEIGHT)
                            .overflow_y_scroll()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                    .child(
                        Button::new("error-terminal")
                            .cursor_pointer()
                            .ghost()
                            .small()
                            .label("Open terminal")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git.update(cx, |g, cx| g.terminal(cx))
                            })),
                    ),
            );
        }
        panel = panel
            .child(
                h_flex().px_2().gap_1().children(
                    [
                        (Section::Changes, "Changes"),
                        (Section::Compare, "Compare"),
                        (Section::History, "History"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(i, (value, label))| {
                        Button::new(("git-section", i))
                            .cursor_pointer()
                            .small()
                            .ghost()
                            .selected(section == value)
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.git.update(cx, |g, cx| g.set_section(value, cx))
                            }))
                    }),
                ),
            )
            .child(div().p_2().child(Input::new(&self.filter).small()))
            .child(
                v_flex()
                    .id("git-files")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .child(if section == Section::Changes {
                        self.changes(cx)
                    } else {
                        self.review(cx)
                    }),
            );
        if section == Section::Changes {
            panel = panel.child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(Textarea::new(&self.message))
                    .child(
                        Button::new("commit-staged")
                            .cursor_pointer()
                            .primary()
                            .label(if busy { "Working…" } else { "Commit staged" })
                            .tooltip_with_action(
                                "Commit staged changes",
                                &CommitChanges,
                                Some("SourceControl"),
                            )
                            .disabled(!can_commit)
                            .on_click(cx.listener(|this, _, _, cx| this.commit(cx))),
                    ),
            );
        }
        panel
    }
}
