//! Branch selection and confirmations with their original worktree captured at opening.
use crate::state::git::{Git, Target};
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _, SmoothHoverBg as _};
use crate::ui::text_field::TextField;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::git::{branches::Branch, changes::Action};

const DIALOG_WIDTH: Pixels = px(460.);
const BRANCH_LIST_HEIGHT: Pixels = px(320.);
const BRANCH_ROW_HEIGHT: Pixels = px(28.);

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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter.read(cx).value().to_lowercase();
        let branches = if self.git.read(cx).target.as_ref() == Some(&self.target) {
            self.git.read(cx).branches.clone()
        } else {
            vec![]
        };
        let matching: Vec<Branch> = branches
            .into_iter()
            .filter(|b| b.name.to_lowercase().contains(&filter))
            .collect();
        let (remote, local): (Vec<_>, Vec<_>) = matching.into_iter().partition(|b| b.remote);
        let current = self
            .git
            .read(cx)
            .status
            .as_ref()
            .and_then(|s| s.branch.clone());
        let mut list = v_flex()
            .id("branches")
            .max_h(BRANCH_LIST_HEIGHT)
            .overflow_y_scroll()
            .gap_0p5();
        for (title, group) in [("Local", local), ("Remote", remote)] {
            if group.is_empty() {
                continue;
            }
            let rows: Vec<AnyElement> = group
                .into_iter()
                .enumerate()
                .map(|(i, b)| {
                    let current = !b.remote && current.as_deref() == Some(b.name.as_str());
                    self.branch_row(title, i, b, current, window, cx)
                })
                .collect();
            list = list.child(section_title(title, cx)).children(rows);
        }
        let new_branch_hover = Hover::new("new-branch", window, cx);
        let error: Option<SharedString> = self.git.read(cx).error.clone().map(Into::into);
        v_flex()
            .gap_2()
            .when_some(error, |col, error| {
                let git = self.git.clone();
                col.child(
                    Alert::error("branch-error", error)
                        .small()
                        .on_close(move |_, _, cx| git.update(cx, |g, cx| g.clear_error(cx))),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(TextField::input(
                        Input::new(&self.filter),
                        &self.filter,
                        cx,
                    )))
                    .child(
                        Button::new("new-branch")
                            .cursor_pointer()
                            .icon(Lucide::Plus)
                            .label("Create branch…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                window.close_dialog(cx);
                                create_branch(
                                    this.git.clone(),
                                    this.target.clone(),
                                    None,
                                    window,
                                    cx,
                                );
                            }))
                            .smooth_hover(ButtonLook::Outline, &new_branch_hover, cx),
                    ),
            )
            .child(list)
    }
}

impl BranchPicker {
    fn branch_row(
        &self,
        group: &'static str,
        ix: usize,
        branch: Branch,
        current: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (fg, muted) = (cx.theme().foreground, cx.theme().muted_foreground);
        let deletable = !branch.remote && !current && branch.worktree.is_none();
        let row_id: SharedString = format!("branch-{group}-{ix}").into();
        let delete_id: SharedString = format!("delete-{group}-{ix}").into();
        let row_hover = Hover::new(row_id.clone(), window, cx);
        let delete_hover = Hover::new(delete_id.clone(), window, cx);
        let detail: Option<&'static str> = if current {
            Some("current")
        } else if branch.worktree.is_some() {
            Some("open worktree")
        } else {
            None
        };
        let idle_bg = if current {
            fg.opacity(0.08)
        } else {
            fg.opacity(0.)
        };
        h_flex()
            .id(row_id)
            .h(BRANCH_ROW_HEIGHT)
            .flex_none()
            .px_2()
            .gap_2()
            .items_center()
            .rounded_md()
            .cursor_pointer()
            .hover_bg(&row_hover, idle_bg, fg.opacity(0.05))
            .child(
                Icon::new(if branch.remote {
                    Lucide::Cloud
                } else {
                    Lucide::GitBranch
                })
                .size_3p5()
                .text_color(muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(branch.name.clone()),
            )
            .when_some(detail, |row, detail| {
                row.child(div().text_xs().text_color(muted).child(detail))
            })
            .when(deletable, |row| {
                let (git, name) = (self.git.clone(), branch.name.clone());
                // Hidden until the row is hovered; the button stops the row's own click.
                row.child(
                    div().opacity(row_hover.progress()).child(
                        Button::new(delete_id)
                            .cursor_pointer()
                            .xsmall()
                            .icon(Lucide::Trash)
                            .tooltip("Delete branch")
                            .on_click(move |_, window, cx| {
                                // The row behind would otherwise switch to this branch.
                                cx.stop_propagation();
                                confirm_delete(git.clone(), name.clone(), window, cx);
                            })
                            .smooth_hover(ButtonLook::Ghost, &delete_hover, cx),
                    ),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| this.choose(&branch, window, cx)))
            .into_any_element()
    }
}

fn section_title(title: &'static str, cx: &App) -> impl IntoElement {
    div()
        .px_2()
        .pt_2()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(title)
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
    window.open_dialog(cx, move |dialog, _, cx| {
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
                    .child(TextField::input(Input::new(&name), &name, cx))
                    .child("Start from")
                    .child(TextField::input(Input::new(&base), &base, cx)),
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

/// Checks off the main thread whether `name` is merged into HEAD, then asks accordingly:
/// a plain delete, or a force delete that spells out what is discarded. A failed check
/// falls back to the safe delete, which Git explains itself.
fn confirm_delete(git: Entity<Git>, name: String, window: &mut Window, cx: &mut App) {
    let Some(target) = git.read(cx).target.clone() else {
        return;
    };
    let check = {
        let (host, path, name) = (target.host.clone(), target.path.clone(), name.clone());
        cx.background_executor()
            .spawn(async move { hive_core::git::branches::is_merged(&host, &path, &name) })
    };
    window
        .spawn(cx, async move |cx| {
            let merged = check.await.unwrap_or_else(|error| {
                tracing::warn!(%error, branch = %name, "merge check failed");
                true
            });
            let confirmation = if merged {
                Confirmation {
                    title: "Delete Branch",
                    description: format!("The local branch {name} will be deleted."),
                    ok_text: "Delete",
                    danger: true,
                }
            } else {
                Confirmation {
                    title: "Branch Has Unmerged Commits",
                    description: format!(
                        "{name} has commits that are not merged into the current branch. Force delete discards them; Git keeps them in the reflog for a while."
                    ),
                    ok_text: "Force Delete",
                    danger: true,
                }
            };
            let action = Action::DeleteBranch {
                name,
                force: !merged,
            };
            cx.update(|window, cx| confirm(git, action, confirmation, window, cx))
                .ok();
        })
        .detach();
}

/// What a confirmation modal says: the title, what will happen, and the button that does it.
pub(super) struct Confirmation {
    pub title: &'static str,
    pub description: String,
    pub ok_text: &'static str,
    /// Destructive actions get a red button.
    pub danger: bool,
}

/// Modal confirmation of a worktree mutation: what will happen, where, then Cancel or act.
pub(super) fn confirm(
    git: Entity<Git>,
    action: Action,
    confirmation: Confirmation,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(target) = git.read(cx).target.clone() else {
        tracing::warn!(?action, "confirmation skipped: no active worktree");
        return;
    };
    let Confirmation {
        title,
        description,
        ok_text,
        danger,
    } = confirmation;
    let description: SharedString =
        format!("{description}\n\nWorktree: {}", target.path.display()).into();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (git, target, action) = (git.clone(), target.clone(), action.clone());
        alert
            .confirm()
            .title(title)
            .description(description.clone())
            .ok_text(ok_text)
            .ok_variant(if danger {
                ButtonVariant::Danger
            } else {
                ButtonVariant::Primary
            })
            .on_ok(move |_, _, cx| {
                git.update(cx, |g, cx| g.perform_at(target.clone(), action.clone(), cx));
                true
            })
    });
}
