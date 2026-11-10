//! Stable footer slot: stage, commit, or push, with animated transitions between them.
use super::*;
use gpui_kit::base::motion::{Presence, PresencePhase, Transition};
use std::time::Duration;

const TRANSITION_DURATION: Duration = Duration::from_millis(180);
const TRANSITION_OFFSET: Pixels = px(4.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Stage,
    Commit,
    Push,
}

impl Mode {
    fn for_status(status: Option<&Status>) -> Self {
        let Some(status) = status else {
            return Self::Stage;
        };
        if staged_count(status) > 0 {
            Self::Commit
        } else if !status.has_conflicts()
            && status.oid.is_some()
            && status.branch.is_some()
            && (status.ahead > 0 || status.upstream.is_none())
        {
            Self::Push
        } else {
            Self::Stage
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Stage => "stage-all",
            Self::Commit => "commit-staged",
            Self::Push => "push-commits",
        }
    }
}

impl GitPanel {
    pub(super) fn commit_actions(
        &self,
        status: Option<&Status>,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mode = Mode::for_status(status);
        let key: SharedString = format!("commit-actions:{:?}", self.git.read(cx).target).into();
        let mut slot = div().relative().w_full().h_6().flex_none();
        for shown in [Mode::Stage, Mode::Commit, Mode::Push] {
            let presence = Presence::new((key.clone(), shown.id()), mode == shown)
                .transition(Transition::new(TRANSITION_DURATION).ease(ease_in_out))
                .sample(window, cx);
            if !presence.should_render() || (presence.progress == 0. && mode != shown) {
                continue;
            }
            let interactive = mode == shown && presence.phase == PresencePhase::Present && !busy;
            slot = slot.child(
                self.action_row(shown, status, interactive, busy, window, cx)
                    .absolute()
                    .w_full()
                    .left_0()
                    .right_0()
                    .top(TRANSITION_OFFSET * (1. - presence.progress))
                    .opacity(presence.progress),
            );
        }
        slot
    }

    fn action_row(
        &self,
        mode: Mode,
        status: Option<&Status>,
        interactive: bool,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let push_blocker = if status.is_some_and(|status| status.branch.is_none()) {
            Some("Create a branch before pushing detached HEAD")
        } else if self.git.read(cx).push_action().is_none() {
            Some("Select a remote or configure this branch's upstream")
        } else {
            None
        };
        let blocker = match mode {
            Mode::Commit => commit_blocker(status, &self.message.read(cx).value()),
            Mode::Push => push_blocker,
            Mode::Stage => match status {
                None => Some("Waiting for changes"),
                Some(status) if status.has_conflicts() => Some("Resolve conflicts first"),
                Some(status) if !status.files.iter().any(|file| file.unstaged.is_some()) => {
                    Some("No changes to stage")
                }
                Some(_) => None,
            },
        };
        let loading = self.git.read(cx).is_running();
        let active = mode == Mode::for_status(status) && !busy;
        let hover = Hover::new(mode.id(), window, cx).enabled(active && blocker.is_none());
        let (icon, label, tooltip, look) = match mode {
            Mode::Stage => (
                Lucide::Plus,
                "Stage All",
                "Stage all changes and untracked files",
                ButtonLook::Outline,
            ),
            Mode::Commit => (
                Lucide::GitCommitHorizontal,
                "Commit",
                "Commit staged changes",
                ButtonLook::Primary,
            ),
            Mode::Push => (
                Lucide::ArrowUp,
                "Push",
                "Push committed changes",
                ButtonLook::Primary,
            ),
        };
        let mut row = h_flex().w_full().gap_2().items_center().child(
            Button::new(mode.id())
                .cursor_pointer()
                .small()
                .flex_1()
                .icon(icon)
                .label(label)
                .loading(loading)
                .disabled(!active || blocker.is_some())
                .tab_stop(interactive)
                .tooltip(blocker.unwrap_or(tooltip))
                .when(mode == Mode::Commit && blocker.is_none(), |button| {
                    button.tooltip_with_action(
                        "Commit staged changes",
                        &CommitChanges,
                        Some("SourceControl"),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if !interactive || Mode::for_status(this.git.read(cx).status.as_ref()) != mode {
                        return;
                    }
                    match mode {
                        Mode::Stage => this.stage_all(cx),
                        Mode::Commit => this.commit(cx),
                        Mode::Push => this.push(window, cx),
                    }
                }))
                .smooth_hover(look, &hover, cx)
                .flex_1(),
        );
        if mode == Mode::Commit {
            let blocker = blocker.or(push_blocker);
            let hover =
                Hover::new("commit-and-push", window, cx).enabled(active && blocker.is_none());
            row = row.child(
                Button::new("commit-and-push")
                    .cursor_pointer()
                    .small()
                    .flex_1()
                    .icon(Lucide::ArrowUp)
                    .label("Commit & Push")
                    .loading(loading)
                    .disabled(!active || blocker.is_some())
                    .tab_stop(interactive)
                    .tooltip(blocker.unwrap_or("Commit staged changes, then push"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if interactive {
                            this.commit_and_push(window, cx);
                        }
                    }))
                    .smooth_hover(ButtonLook::Outline, &hover, cx)
                    .flex_1(),
            );
        }
        row
    }

    fn stage_all(&self, cx: &mut Context<Self>) {
        let state = self.git.read(cx);
        if state.is_busy() || Mode::for_status(state.status.as_ref()) != Mode::Stage {
            return;
        }
        let Some(status) = &state.status else { return };
        if status.has_conflicts() {
            return;
        }
        let paths = status
            .files
            .iter()
            .filter(|file| file.unstaged.is_some())
            .map(|file| file.path.clone())
            .collect();
        self.git
            .update(cx, |git, cx| git.perform(Action::Stage(paths), cx));
    }
}

#[cfg(test)]
mod tests {
    use super::Mode;
    use hive_core::git::status::{Change, FileChange, Status};

    #[test]
    fn footer_follows_index_and_pending_commits() {
        let mut status = Status::default();
        assert_eq!(Mode::for_status(None), Mode::Stage);
        assert_eq!(Mode::for_status(Some(&status)), Mode::Stage);
        status.branch = Some("main".into());
        status.files.push(FileChange {
            path: "file".into(),
            old_path: None,
            staged: Some(Change::Modified),
            unstaged: Some(Change::Modified),
            submodule: false,
            staged_stats: None,
            unstaged_stats: None,
            conflict: None,
        });
        assert_eq!(Mode::for_status(Some(&status)), Mode::Commit);
        status.oid = Some("committed".into());
        status.files[0].staged = None;
        assert_eq!(Mode::for_status(Some(&status)), Mode::Push);
        status.upstream = Some("origin/main".into());
        status.ahead = 1;
        assert_eq!(Mode::for_status(Some(&status)), Mode::Push);
        status.files[0].staged = Some(Change::Modified);
        assert_eq!(Mode::for_status(Some(&status)), Mode::Commit);
        status.files[0].staged = None;
        status.ahead = 0;
        assert_eq!(Mode::for_status(Some(&status)), Mode::Stage);
        status.ahead = 1;
        status.branch = None;
        assert_eq!(Mode::for_status(Some(&status)), Mode::Stage);
        status.branch = Some("main".into());
        status.files[0].staged = Some(Change::Conflicted);
        assert_eq!(Mode::for_status(Some(&status)), Mode::Stage);
    }
}
