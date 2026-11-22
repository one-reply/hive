//! The branch picker with sync counters, then remote actions with refresh.
use super::*;
use crate::state::git::RemoteOperation;
use std::time::Duration;

/// Fade-in of the confirmation that replaces the upstream name after a remote operation.
const SYNCED_FADE_IN: Duration = Duration::from_millis(200);

impl GitPanel {
    /// The branch picker with sync counters, then remote actions and refresh.
    pub(super) fn header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.git.read(cx);
        let status = state.status.clone();
        let busy = state.is_busy();
        let operation = state.remote_operation();
        // A remote operation spins its own button, so the refresh icon stays still meanwhile.
        let spinning = state.loading || (state.is_running() && operation.is_none());
        let synced: Option<SharedString> = state.synced().map(|s| s.to_string().into());
        let push_entry = self.with_facts(cx, availability::push_entry);
        let [fetch_blocker, pull_blocker, push_blocker] =
            [Entry::Fetch, Entry::Pull, push_entry].map(|entry| self.blocker(entry, cx));
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
        let theme = cx.theme();
        let muted = theme.muted_foreground;
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

        let branch_row = h_flex()
            .gap_2()
            .items_center()
            .min_w_0()
            .child(
                Button::new("branch-picker")
                    .cursor_pointer()
                    .small()
                    .min_w_0()
                    .icon(Lucide::GitBranch)
                    .label(branch.clone())
                    .dropdown_caret(true)
                    .tooltip("Switch or create branch")
                    .disabled(status.is_none() || busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        git_controls::branches(this.git.clone(), false, window, cx)
                    }))
                    // Outlined like a select, so the branch reads as a control, not a label.
                    .smooth_hover(
                        ButtonLook::Outline,
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
            .child(match synced {
                Some(synced) => synced_note(synced, cx).into_any_element(),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .text_size(META_TEXT)
                    .text_color(muted)
                    .truncate()
                    .text_right()
                    .child(upstream.clone().unwrap_or_else(|| "No upstream".into()))
                    .into_any_element(),
            });

        let push_disabled = busy || push_blocker.is_some();
        let fetching = operation == Some(RemoteOperation::Fetch);
        let pulling = operation == Some(RemoteOperation::Pull);
        let pushing = matches!(
            operation,
            Some(RemoteOperation::Push | RemoteOperation::Publish)
        );
        let push_label: SharedString = match (operation, has_upstream, ahead) {
            (Some(RemoteOperation::Push), ..) => "Pushing…".into(),
            (Some(RemoteOperation::Publish), ..) => "Publishing…".into(),
            (_, false, _) => "Publish".into(),
            (_, true, 0) => "Push".into(),
            (_, true, n) => format!("Push {n}").into(),
        };
        let mut actions = h_flex()
            .flex_1()
            .min_w_0()
            .gap_1p5()
            .child(
                sync_button(
                    "fetch",
                    Lucide::CloudDownload,
                    if fetching { "Fetching…" } else { "Fetch" },
                    "Fetch from remote",
                    fetching,
                )
                .disabled((busy && !fetching) || fetch_blocker.is_some())
                .when_some(fetch_blocker, |b, blocker| b.tooltip(blocker.explanation()))
                .on_click(cx.listener(|this, _, window, cx| this.perform(Entry::Fetch, window, cx)))
                .smooth_hover(
                    ButtonLook::Outline,
                    &fetch_hover.enabled(!busy && fetch_blocker.is_none()),
                    cx,
                ),
            )
            .child(
                sync_button(
                    "pull",
                    Lucide::ArrowDown,
                    if pulling { "Pulling…" } else { "Pull" },
                    "Pull fast-forward only",
                    pulling,
                )
                .disabled((busy && !pulling) || pull_blocker.is_some())
                .when_some(pull_blocker, |b, blocker| b.tooltip(blocker.explanation()))
                .on_click(cx.listener(|this, _, window, cx| this.perform(Entry::Pull, window, cx)))
                .smooth_hover(
                    ButtonLook::Outline,
                    &pull_hover.enabled(!busy && pull_blocker.is_none()),
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
                    pushing,
                )
                .disabled(push_disabled && !pushing)
                .when_some(push_blocker.filter(|_| !busy), |b, blocker| {
                    b.tooltip(blocker.explanation())
                })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.perform(push_entry, window, cx)),
                )
                .smooth_hover(
                    ButtonLook::Outline,
                    &push_hover.enabled(!push_disabled),
                    cx,
                ),
            );
        if remotes.len() > 1 {
            let git = self.git.clone();
            actions = actions.child(
                Button::new("remote")
                    .cursor_pointer()
                    .ghost()
                    .small()
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
        let top_row = h_flex()
            .justify_between()
            .items_center()
            .gap_2()
            .child(actions)
            // Same control as "Refresh limits" in the status bar: a loading button spins
            // its own icon and ignores clicks until the operation ends.
            .child(
                Button::new("git-refresh")
                    .cursor_pointer()
                    .small()
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

        // The branch names the context the actions below operate on, so it comes first.
        v_flex()
            .px_3()
            .pt_3()
            .pb_3()
            .gap_2()
            .child(branch_row)
            .child(top_row)
    }
}

/// A remote action; while it runs, its icon spins and the button ignores clicks without
/// greying out like the other, disabled ones.
fn sync_button(
    id: &'static str,
    icon: Lucide,
    label: impl Into<SharedString>,
    tooltip: &'static str,
    running: bool,
) -> Button {
    Button::new(id)
        .cursor_pointer()
        .small()
        .icon(icon)
        .label(label)
        .tooltip(tooltip)
        .loading(running)
}

/// What the last remote operation changed, in place of the upstream name for a few seconds.
fn synced_note(text: SharedString, cx: &App) -> impl IntoElement {
    h_flex()
        .flex_1()
        .min_w_0()
        .justify_end()
        .items_center()
        .gap_1()
        .text_size(META_TEXT)
        .text_color(cx.theme().success)
        .child(Icon::new(Lucide::Check).size_3())
        .child(div().min_w_0().truncate().child(text))
        .with_animation(
            "git-synced-note",
            Animation::new(SYNCED_FADE_IN).with_easing(ease_in_out),
            |note, progress| note.opacity(progress),
        )
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
