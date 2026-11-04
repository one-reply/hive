//! Worktree name, branch picker with sync counters, and the remote actions.
use super::*;

impl GitPanel {
    /// Worktree name, branch picker with sync counters, then the remote actions.
    pub(super) fn header(
        &self,
        target: &Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.git.read(cx);
        let status = state.status.clone();
        let busy = state.is_busy();
        let spinning = busy || state.loading;
        let has_remote = state.remote.is_some();
        let [
            refresh_hover,
            branch_hover,
            fetch_hover,
            pull_hover,
            push_hover,
        ] = ["git-refresh", "branch-picker", "fetch", "pull", "push"]
            .map(|id| Hover::new(id, window, cx));
        let state = self.git.read(cx);
        let remotes = state.remotes.clone();
        let remote = state.remote.clone();
        let upstream_remote = state.upstream_remote.clone();
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let name = target
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.path.display().to_string());
        let detached = status.as_ref().is_some_and(|s| s.branch.is_none());
        let branch: SharedString = status
            .as_ref()
            .and_then(|s| s.branch.clone())
            .map(Into::into)
            .unwrap_or_else(|| if detached { "Detached HEAD" } else { "…" }.into());
        let (ahead, behind) = status
            .as_ref()
            .map(|s| (s.ahead, s.behind))
            .unwrap_or_default();
        let upstream = status.as_ref().and_then(|s| s.upstream.clone());
        let has_upstream = upstream.is_some();

        let title = h_flex()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("worktree-name")
                    .flex_1()
                    .min_w_0()
                    .text_size(TITLE_TEXT)
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .tooltip({
                        let path: SharedString = target.path.display().to_string().into();
                        move |window, cx| Tooltip::new(path.clone()).build(window, cx)
                    })
                    .child(name),
            )
            // Same control as "Refresh limits" in the status bar: a loading button spins
            // its own icon and ignores clicks until the operation ends.
            .child(
                Button::new("git-refresh")
                    .cursor_pointer()
                    .xsmall()
                    .icon(Lucide::RefreshCw)
                    .loading(spinning)
                    .tooltip("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git.update(cx, |g, cx| {
                            g.clear_error(cx);
                            g.refresh(true, cx);
                        })
                    }))
                    .smooth_hover(ButtonLook::Ghost, &refresh_hover.enabled(!spinning), cx),
            );

        let branch_row = h_flex()
            .gap_1p5()
            .items_center()
            .min_w_0()
            .child(
                Button::new("branch-picker")
                    .cursor_pointer()
                    .xsmall()
                    .min_w_0()
                    .icon(Lucide::GitBranch)
                    .label(branch.clone())
                    .dropdown_caret(true)
                    .tooltip("Switch branch")
                    .disabled(status.is_none() || busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        git_controls::branches(this.git.clone(), false, window, cx)
                    }))
                    .smooth_hover(
                        ButtonLook::Ghost,
                        &branch_hover.enabled(status.is_some() && !busy),
                        cx,
                    )
                    .min_w_0(),
            )
            .when(detached, |s| {
                s.child(Tag::warning().xsmall().child("detached"))
            })
            .when(ahead > 0, |s| s.child(counter(Lucide::ArrowUp, ahead)))
            .when(behind > 0, |s| s.child(counter(Lucide::ArrowDown, behind)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(META_TEXT)
                    .text_color(muted)
                    .truncate()
                    .text_right()
                    .child(upstream.clone().unwrap_or_else(|| "No upstream".into())),
            );

        let push_disabled = busy
            || status.is_none()
            || if has_upstream {
                upstream_remote.is_none()
            } else {
                remote.is_none()
            };
        let push_label: SharedString = match (has_upstream, ahead) {
            (false, _) => "Publish".into(),
            (true, 0) => "Push".into(),
            (true, n) => format!("Push {n}").into(),
        };
        let mut actions = h_flex()
            .gap_1()
            .child(
                sync_button("fetch", Lucide::CloudDownload, "Fetch", "Fetch from remote")
                    .disabled(busy || remote.is_none())
                    .when(remote.is_none(), |b| b.tooltip("No remote configured"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let remote = this.git.read(cx).remote.clone();
                        if let Some(remote) = remote {
                            this.git
                                .update(cx, |g, cx| g.perform(Action::Fetch(remote), cx));
                        }
                    }))
                    .smooth_hover(
                        ButtonLook::Outline,
                        &fetch_hover.enabled(!busy && has_remote),
                        cx,
                    ),
            )
            .child(
                sync_button("pull", Lucide::ArrowDown, "Pull", "Pull fast-forward only")
                    .disabled(busy || !has_upstream)
                    .when(!has_upstream, |b| b.tooltip("No upstream to pull from"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git.update(cx, |g, cx| g.perform(Action::Pull, cx))
                    }))
                    .smooth_hover(
                        ButtonLook::Outline,
                        &pull_hover.enabled(!busy && has_upstream),
                        cx,
                    ),
            )
            .child(
                sync_button(
                    "push",
                    Lucide::ArrowUp,
                    push_label,
                    if has_upstream {
                        "Push to upstream"
                    } else {
                        "Push and set upstream"
                    },
                )
                .disabled(push_disabled)
                .when(!busy && push_disabled && status.is_some(), |b| {
                    b.tooltip("No remote configured")
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    let state = this.git.read(cx);
                    let publish = state.status.as_ref().is_none_or(|s| s.upstream.is_none());
                    let remote = if publish {
                        state.remote.clone()
                    } else {
                        state.upstream_remote.clone()
                    };
                    let Some(remote) = remote else { return };
                    let action = Action::Push {
                        remote: remote.clone(),
                        publish,
                    };
                    if publish {
                        git_controls::confirm(
                            this.git.clone(),
                            action,
                            git_controls::Confirmation {
                                title: "Publish Branch",
                                description: format!(
                                    "{branch} will be pushed to {remote} and start tracking {remote}/{branch}."
                                ),
                                ok_text: "Publish",
                                danger: false,
                            },
                            window,
                            cx,
                        );
                    } else {
                        this.git.update(cx, |g, cx| g.perform(action, cx));
                    }
                }))
                .smooth_hover(ButtonLook::Outline, &push_hover.enabled(!push_disabled), cx),
            );
        if remotes.len() > 1 {
            let git = self.git.clone();
            actions = actions.child(
                Button::new("remote")
                    .cursor_pointer()
                    .ghost()
                    .xsmall()
                    .icon(Lucide::Ellipsis)
                    .tooltip(format!("Remote: {}", remote.as_deref().unwrap_or("none")))
                    .dropdown_menu(move |menu, _, _| {
                        remotes.iter().fold(menu, |menu, remote| {
                            let (git, remote) = (git.clone(), remote.clone());
                            menu.item(PopupMenuItem::new(remote.clone()).on_click(
                                move |_, _, cx| {
                                    git.update(cx, |g, cx| {
                                        g.remote = Some(remote.clone());
                                        cx.notify();
                                    });
                                },
                            ))
                        })
                    }),
            );
        }
        v_flex()
            .px_3()
            .pt_3()
            .pb_2()
            .gap_1p5()
            .child(title)
            .child(branch_row)
            .child(actions)
    }
}

fn sync_button(
    id: &'static str,
    icon: Lucide,
    label: impl Into<SharedString>,
    tooltip: &'static str,
) -> Button {
    Button::new(id)
        .cursor_pointer()
        .xsmall()
        .icon(icon)
        .label(label)
        .tooltip(tooltip)
}

/// Ahead/behind counter next to the branch name.
fn counter(icon: Lucide, count: usize) -> impl IntoElement {
    Tag::secondary().xsmall().child(
        h_flex()
            .gap_0p5()
            .items_center()
            .child(Icon::new(icon).size_3())
            .child(count.to_string()),
    )
}
