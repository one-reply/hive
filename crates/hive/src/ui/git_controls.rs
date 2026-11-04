//! Branch selection and confirmations with their original worktree captured at opening.
use crate::state::git::{Git, Target};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use hive_core::git::{branches::Branch, changes::Action};

const DIALOG_WIDTH: Pixels = px(460.);
const BRANCH_LIST_HEIGHT: Pixels = px(320.);

pub(super) fn branches(git: Entity<Git>, compare: bool, window: &mut Window, cx: &mut App) {
    git.update(cx, |g, cx| g.refresh(true, cx));
    let Some(target) = git.read(cx).target.clone() else {
        return;
    };
    let picker = cx.new(|cx| BranchPicker::new(git.clone(), target, compare, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(if compare {
                "Compare against…"
            } else {
                "Switch branch"
            })
            .w(DIALOG_WIDTH)
            .child(picker.clone())
    });
}

struct BranchPicker {
    git: Entity<Git>,
    target: Target,
    compare: bool,
    filter: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}
impl BranchPicker {
    fn new(
        git: Entity<Git>,
        target: Target,
        compare: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx
            .new(|cx| InputState::new(window, cx).placeholder("Search local and remote branches"));
        let subscriptions = vec![
            cx.observe(&filter, |_, _, cx| cx.notify()),
            cx.observe(&git, |_, _, cx| cx.notify()),
        ];
        Self {
            git,
            target,
            compare,
            filter,
            _subscriptions: subscriptions,
        }
    }

    fn choose(&self, branch: &Branch, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        if self.compare {
            if self.git.read(cx).target.as_ref() == Some(&self.target) {
                self.git
                    .update(cx, |g, cx| g.set_base(branch.reference.clone(), cx));
            }
        } else if let Some(path) = &branch.worktree {
            let target = Target {
                path: path.clone(),
                ..self.target.clone()
            };
            self.git.update(cx, |g, cx| g.terminal_at(target, cx));
        } else if branch.remote {
            create_branch(
                self.git.clone(),
                self.target.clone(),
                Some(branch.clone()),
                window,
                cx,
            );
        } else {
            self.git.update(cx, |g, cx| {
                g.perform_at(self.target.clone(), Action::Switch(branch.name.clone()), cx)
            });
        }
    }
}
impl Render for BranchPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter.read(cx).value().to_lowercase();
        let branches = if self.git.read(cx).target.as_ref() == Some(&self.target) {
            self.git.read(cx).branches.clone()
        } else {
            vec![]
        };
        let rows = branches
            .into_iter()
            .filter(|b| b.name.to_lowercase().contains(&filter))
            .enumerate()
            .map(|(i, b)| {
                let detail = if b.worktree.is_some() {
                    " · open worktree"
                } else if b.remote {
                    " · remote"
                } else {
                    ""
                };
                Button::new(("branch", i))
                    .cursor_pointer()
                    .ghost()
                    .small()
                    .w_full()
                    .label(format!("{}{detail}", b.name))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose(&b, window, cx)))
            })
            .collect::<Vec<_>>();
        v_flex()
            .gap_2()
            .child(Input::new(&self.filter))
            .child(
                v_flex()
                    .id("branches")
                    .max_h(BRANCH_LIST_HEIGHT)
                    .overflow_y_scroll()
                    .children(rows),
            )
            .child(
                Button::new("new-branch")
                    .cursor_pointer()
                    .small()
                    .label("Create branch…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.close_dialog(cx);
                        create_branch(this.git.clone(), this.target.clone(), None, window, cx);
                    })),
            )
    }
}

fn create_branch(
    git: Entity<Git>,
    target: Target,
    remote: Option<Branch>,
    window: &mut Window,
    cx: &mut App,
) {
    let name = cx.new(|cx| InputState::new(window, cx).placeholder("feature/name"));
    let base = cx.new(|cx| InputState::new(window, cx).placeholder("HEAD or branch name"));
    base.update(cx, |b, cx| {
        b.set_value(
            remote
                .as_ref()
                .map(|r| r.reference.as_str())
                .unwrap_or("HEAD"),
            window,
            cx,
        )
    });
    if let Some(remote) = &remote {
        let suggested = remote
            .name
            .split_once('/')
            .map(|(_, name)| name)
            .unwrap_or(&remote.name);
        name.update(cx, |n, cx| n.set_value(suggested.to_string(), window, cx));
    }
    window.open_dialog(cx, move |dialog, _, _| {
        let (git, target, name, base) = (git.clone(), target.clone(), name.clone(), base.clone());
        let track = remote.is_some();
        dialog
            .title(if track {
                "Track remote branch"
            } else {
                "Create and switch branch"
            })
            .w(DIALOG_WIDTH)
            .child(
                v_flex()
                    .gap_2()
                    .child("Branch name")
                    .child(Input::new(&name))
                    .child("Start from")
                    .child(Input::new(&base)),
            )
            .button_props(
                DialogButtonProps::default()
                    .ok_text("Create branch")
                    .show_cancel(true)
                    .on_ok(move |_, _, cx| {
                        let action = Action::CreateBranch {
                            name: name.read(cx).value().trim().into(),
                            base: base.read(cx).value().trim().into(),
                            track,
                        };
                        git.update(cx, |g, cx| g.perform_at(target.clone(), action, cx));
                        true
                    }),
            )
    });
}

pub(super) fn confirm(
    git: Entity<Git>,
    action: Action,
    title: &'static str,
    description: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(target) = git.read(cx).target.clone() else {
        return;
    };
    window.open_dialog(cx, move |dialog, _, cx| {
        let (git, target, action) = (git.clone(), target.clone(), action.clone());
        dialog
            .title(title)
            .w(DIALOG_WIDTH)
            .child(
                v_flex().gap_2().child(description.clone()).child(
                    h_flex()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(target.path.display().to_string()),
                ),
            )
            .button_props(
                DialogButtonProps::default()
                    .ok_text(title)
                    .show_cancel(true)
                    .on_ok(move |_, _, cx| {
                        git.update(cx, |g, cx| g.perform_at(target.clone(), action.clone(), cx));
                        true
                    }),
            )
    });
}
