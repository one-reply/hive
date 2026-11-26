//! The branch picker with its upstream and refresh, then the remote actions as one
//! segmented control whose buttons count the commits they would move.
use super::*;
use crate::state::git::RemoteOperation;
use std::time::Duration;

/// Fade-in of the confirmation that replaces the upstream name after a remote operation.
const SYNCED_FADE_IN: Duration = Duration::from_millis(200);
/// The border around the segmented control (`border_1`) and the hairline between its buttons.
const SEGMENT_LINE: Pixels = px(1.);

impl GitPanel {
    /// The branch the actions below operate on, then the actions themselves.
    pub(super) fn header(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .px_3()
            .pt_3()
            .pb_3()
            .gap_2()
            .child(self.branch_row(window, cx))
            .child(self.sync_row(window, cx))
    }

    /// The branch picker, what the branch tracks (or what the last remote operation changed)
    /// and the refresh button.
    fn branch_row(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let [refresh_hover, branch_hover] =
            ["git-refresh", "branch-picker"].map(|id| Hover::new(id, window, cx));
        let state = self.git.read(cx);
        let status = state.status.as_ref();
        let busy = state.is_busy();
        let loaded = status.is_some();
        // A remote operation spins its own button, so the refresh icon stays still meanwhile.
        // Reading what a confirmation will show spins it like the user's own refresh.
        let spinning = read_spins(state.loading, self.manual_refresh, loaded)
            || state.is_preparing()
            || (state.is_running() && state.remote_operation().is_none());
        let synced: Option<SharedString> = state.synced().map(|s| s.to_string().into());
        let detached = status.is_some_and(|s| s.branch.is_none());
        let branch: SharedString = status
            .and_then(|s| s.branch.clone())
            .map(Into::into)
            .unwrap_or_else(|| if detached { "Detached HEAD" } else { "…" }.into());
        let upstream = upstream_note(status);
        let muted = cx.theme().muted_foreground;

        h_flex()
            .gap_2()
            .items_center()
            .min_w_0()
            .child(
                Button::new("branch-picker")
                    .cursor_pointer()
                    .small()
                    .min_w_0()
                    .icon(Lucide::GitBranch)
                    .label(branch)
                    .dropdown_caret(true)
                    .tooltip("Switch or create branch")
                    .disabled(!loaded || busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        git_controls::branches(this.git.clone(), false, window, cx)
                    }))
                    // Outlined like a select, so the branch reads as a control, not a label.
                    .smooth_hover(
                        ButtonLook::Outline,
                        &branch_hover.enabled(loaded && !busy),
                        cx,
                    )
                    .min_w_0(),
            )
            .when(detached, |s| {
                s.child(Tag::warning().xsmall().child("detached"))
            })
            .child(match synced {
                Some(synced) => synced_note(synced, cx).into_any_element(),
                None => div()
                    .flex_1()
                    .min_w_0()
                    .text_size(META_TEXT)
                    .text_color(muted)
                    .truncate()
                    .text_right()
                    .child(upstream)
                    .into_any_element(),
            })
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
                        this.manual_refresh = true;
                        this.git.update(cx, |g, cx| {
                            g.clear_error(cx);
                            g.refresh(true, cx);
                        });
                        // Starting a read does not notify, and the button spins from now.
                        cx.notify();
                    }))
                    .smooth_hover(ButtonLook::Ghost, &refresh_hover.enabled(!spinning), cx),
            )
    }

    /// Fetch, Pull and Push (or Publish) joined into one control, then the remote picker
    /// when there is more than one remote to pick.
    fn sync_row(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let push_entry = self.with_facts(cx, availability::push_entry);
        let entries = [Entry::Fetch, Entry::Pull, push_entry];
        let blockers = entries.map(|entry| self.blocker(entry, cx));
        let ids = ["fetch", "pull", "push"];
        let hovers = ids.map(|id| Hover::new(id, window, cx));
        let state = self.git.read(cx);
        let busy = state.is_busy();
        let operation = state.remote_operation();
        let status = state.status.as_ref();
        let remote = state.remote.clone();
        let remotes = state.remotes.clone();

        let buttons = entries
            .into_iter()
            .zip(ids)
            .zip(blockers)
            .zip(hovers)
            .zip([Segment::First, Segment::Middle, Segment::Last])
            .map(|((((entry, id), blocker), hover), position)| {
                let running = operation.filter(|&operation| runs(entry, operation));
                // Another action running greys the button out; its own spins instead.
                let enabled = !busy && blocker.is_none();
                let tooltip: SharedString = match blocker {
                    Some(blocker) if !busy => blocker.explanation().into(),
                    _ => describe(entry, status, remote.as_deref()),
                };
                let button = Button::new(id)
                    .cursor_pointer()
                    .small()
                    .icon(icon(entry))
                    .label(label(entry, running))
                    .tooltip(tooltip)
                    .loading(running.is_some())
                    .disabled(!enabled && running.is_none())
                    .when_some(pending(entry, status), |button, count| {
                        button.child(pending_badge(count, enabled || running.is_some()))
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.perform(entry, window, cx)),
                    );
                segment(button, position, &hover.enabled(enabled), cx)
            });
        let line = cx.theme().input;
        let mut control = h_flex()
            .flex_1()
            .min_w_0()
            .h_6()
            .border_1()
            .border_color(line)
            .rounded(cx.theme().radius)
            .bg(cx.theme().input_background());
        for (ix, button) in buttons.enumerate() {
            if ix > 0 {
                control = control.child(div().w(SEGMENT_LINE).h_full().bg(line));
            }
            control = control.child(button);
        }

        h_flex()
            .gap_1p5()
            .items_center()
            .child(control)
            .when(remotes.len() > 1, |row| {
                row.child(self.remote_picker(remotes, remote))
            })
    }

    /// Picks the remote that Fetch and Publish talk to.
    fn remote_picker(&self, remotes: Vec<String>, remote: Option<String>) -> impl IntoElement {
        let git = self.git.clone();
        Button::new("remote")
            .cursor_pointer()
            .ghost()
            .small()
            .icon(Lucide::Ellipsis)
            .tooltip(format!("Remote: {}", remote.as_deref().unwrap_or("none")))
            .dropdown_menu(move |menu, _, _| {
                remotes.iter().fold(menu, |menu, remote| {
                    let (git, remote) = (git.clone(), remote.clone());
                    menu.item(
                        PopupMenuItem::new(remote.clone()).on_click(move |_, _, cx| {
                            git.update(cx, |g, cx| {
                                g.remote = Some(remote.clone());
                                cx.notify();
                            });
                        }),
                    )
                })
            })
    }
}

/// Where a button sits in the segmented control, which decides the corners it rounds.
#[derive(Clone, Copy)]
enum Segment {
    First,
    Middle,
    Last,
}

/// One button of the segmented control: borderless, filling its share of the row.
fn segment(button: Button, position: Segment, hover: &Hover, cx: &App) -> Stateful<Div> {
    // The control draws one border around all of its buttons, so a button rounds only the
    // outer corners it shares with that border, inset by its width.
    let outer = (cx.theme().radius - SEGMENT_LINE).max(px(0.));
    let (left, right) = match position {
        Segment::First => (outer, px(0.)),
        Segment::Middle => (px(0.), px(0.)),
        Segment::Last => (px(0.), outer),
    };
    button
        .w_full()
        .h_full()
        .rounded_tl(left)
        .rounded_bl(left)
        .rounded_tr(right)
        .rounded_br(right)
        .smooth_hover(ButtonLook::Ghost, hover, cx)
        .flex_1()
        .h_full()
}

/// Whether a status read spins the refresh button: the user's own Refresh, or the first read
/// while there is nothing to show yet. The poll reads every two seconds while the panel is
/// open, and `Spinner` redraws the whole window on every frame it turns, so polls read quietly.
fn read_spins(loading: bool, requested: bool, loaded: bool) -> bool {
    loading && (requested || !loaded)
}

/// Whether `operation` is the one the button for `entry` started.
fn runs(entry: Entry, operation: RemoteOperation) -> bool {
    matches!(
        (entry, operation),
        (Entry::Fetch, RemoteOperation::Fetch)
            | (Entry::Pull, RemoteOperation::Pull)
            | (Entry::Push | Entry::Publish, RemoteOperation::Push)
            | (Entry::Push | Entry::Publish, RemoteOperation::Publish)
    )
}

fn icon(entry: Entry) -> Lucide {
    match entry {
        Entry::Fetch => Lucide::CloudDownload,
        Entry::Pull => Lucide::ArrowDown,
        Entry::Publish => Lucide::CloudUpload,
        _ => Lucide::ArrowUp,
    }
}

/// The button's label, which names the operation it `running` while that is under way.
fn label(entry: Entry, running: Option<RemoteOperation>) -> &'static str {
    match running {
        Some(RemoteOperation::Fetch) => "Fetching…",
        Some(RemoteOperation::Pull) => "Pulling…",
        Some(RemoteOperation::Push) => "Pushing…",
        Some(RemoteOperation::Publish) => "Publishing…",
        // The menu's "Publish Branch" is too long for a third of the row.
        None if entry == Entry::Publish => "Publish",
        None => entry.label(),
    }
}

/// The commits `entry` would move between the branch and its upstream, when there are any.
/// Publish has no upstream to count against, and Fetch only learns the count.
fn pending(entry: Entry, status: Option<&Status>) -> Option<usize> {
    let status = status.filter(|status| status.tracked_upstream().is_some())?;
    let count = match entry {
        Entry::Pull => status.behind,
        Entry::Push => status.ahead,
        _ => return None,
    };
    (count > 0).then_some(count)
}

/// The tooltip of an available remote action: what it moves and where.
fn describe(entry: Entry, status: Option<&Status>, remote: Option<&str>) -> SharedString {
    let upstream = status.and_then(Status::tracked_upstream);
    let commits = match pending(entry, status) {
        Some(1) => " 1 commit".to_owned(),
        Some(count) => format!(" {count} commits"),
        None => String::new(),
    };
    match (entry, upstream) {
        (Entry::Fetch, _) => format!("Fetch from {}", remote.unwrap_or("remote")).into(),
        (Entry::Pull, Some(upstream)) => {
            format!("Pull{commits} from {upstream}, fast-forward only").into()
        }
        (Entry::Pull, None) => "Pull fast-forward only".into(),
        (Entry::Push, Some(upstream)) => format!("Push{commits} to {upstream}").into(),
        _ => "Push and set upstream".into(),
    }
}

/// The count on a remote action; it stands out while the action can run.
fn pending_badge(count: usize, available: bool) -> Tag {
    if available {
        Tag::info()
    } else {
        Tag::secondary()
    }
    .xsmall()
    .rounded_full()
    .child(count.to_string())
}

/// What the branch row says the branch tracks. An upstream that is gone from the remote is
/// named with the reason Push, Pull and Sync give way to Publish.
fn upstream_note(status: Option<&Status>) -> SharedString {
    match status.and_then(|status| status.upstream.as_deref().map(|name| (status, name))) {
        None => "No upstream".into(),
        Some((status, name)) if status.upstream_gone => format!("{name} · not on remote").into(),
        Some((_, name)) => name.to_owned().into(),
    }
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

#[cfg(test)]
mod tests {
    use super::{Entry, RemoteOperation, describe, label, pending, read_spins, upstream_note};
    use hive_core::git::status::Status;

    #[test]
    fn a_background_poll_reads_without_spinning_the_refresh_button() {
        let (loading, requested, loaded) = (true, false, true);
        assert!(!read_spins(loading, requested, loaded));
    }

    #[test]
    fn the_users_refresh_and_the_first_read_spin_until_they_end() {
        assert!(read_spins(true, true, true), "the user's own refresh");
        assert!(read_spins(true, false, false), "nothing to show yet");
        assert!(read_spins(true, true, false));
        for (requested, loaded) in [(false, false), (true, true), (false, true)] {
            assert!(!read_spins(false, requested, loaded), "no read under way");
        }
    }

    fn tracking(ahead: usize, behind: usize) -> Status {
        Status {
            branch: Some("feature".into()),
            upstream: Some("origin/feature".into()),
            ahead,
            behind,
            ..Status::default()
        }
    }

    #[test]
    fn the_branch_row_says_when_the_upstream_is_gone_from_the_remote() {
        let mut status = Status::default();
        assert_eq!(upstream_note(None), "No upstream");
        assert_eq!(upstream_note(Some(&status)), "No upstream");
        status.upstream = Some("origin/feature".into());
        assert_eq!(upstream_note(Some(&status)), "origin/feature");
        status.upstream_gone = true;
        assert_eq!(
            upstream_note(Some(&status)),
            "origin/feature · not on remote"
        );
    }

    #[test]
    fn pull_counts_incoming_and_push_counts_outgoing_commits() {
        let status = tracking(2, 3);
        assert_eq!(pending(Entry::Pull, Some(&status)), Some(3));
        assert_eq!(pending(Entry::Push, Some(&status)), Some(2));
        assert_eq!(pending(Entry::Fetch, Some(&status)), None);
    }

    #[test]
    fn no_count_without_commits_to_move_or_an_upstream_to_count_against() {
        assert_eq!(pending(Entry::Pull, None), None);
        assert_eq!(pending(Entry::Pull, Some(&tracking(0, 0))), None);
        let mut gone = tracking(2, 3);
        gone.upstream_gone = true;
        assert_eq!(pending(Entry::Push, Some(&gone)), None);
        assert_eq!(pending(Entry::Pull, Some(&gone)), None);
        // Publishing a branch that tracks another one sends it under its own name, so the
        // commits counted against that other branch are not what it moves.
        assert_eq!(pending(Entry::Publish, Some(&tracking(2, 0))), None);
    }

    #[test]
    fn tooltips_name_the_commits_and_where_they_go() {
        assert_eq!(
            describe(Entry::Pull, Some(&tracking(0, 1)), Some("origin")),
            "Pull 1 commit from origin/feature, fast-forward only"
        );
        assert_eq!(
            describe(Entry::Pull, Some(&tracking(0, 0)), Some("origin")),
            "Pull from origin/feature, fast-forward only"
        );
        assert_eq!(
            describe(Entry::Push, Some(&tracking(4, 0)), Some("origin")),
            "Push 4 commits to origin/feature"
        );
        assert_eq!(
            describe(Entry::Fetch, None, Some("upstream")),
            "Fetch from upstream"
        );
        assert_eq!(describe(Entry::Fetch, None, None), "Fetch from remote");
        assert_eq!(
            describe(Entry::Publish, None, Some("origin")),
            "Push and set upstream"
        );
        assert_eq!(describe(Entry::Pull, None, None), "Pull fast-forward only");
    }

    #[test]
    fn a_running_action_says_so_and_publish_is_named_for_what_it_does() {
        assert_eq!(label(Entry::Fetch, None), "Fetch");
        assert_eq!(
            label(Entry::Fetch, Some(RemoteOperation::Fetch)),
            "Fetching…"
        );
        assert_eq!(label(Entry::Pull, Some(RemoteOperation::Pull)), "Pulling…");
        assert_eq!(label(Entry::Push, None), "Push");
        assert_eq!(label(Entry::Push, Some(RemoteOperation::Push)), "Pushing…");
        assert_eq!(label(Entry::Publish, None), "Publish");
        // The button names the operation that runs, even when the branch changed under it.
        assert_eq!(
            label(Entry::Push, Some(RemoteOperation::Publish)),
            "Publishing…"
        );
    }
}
