//! Stable footer slot: one split button that stages, commits or pushes, with animated
//! transitions between the three and the menu of the other Git actions on its right half.
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

    fn menu_id(self) -> &'static str {
        match self {
            Self::Stage => "stage-all-menu",
            Self::Commit => "commit-staged-menu",
            Self::Push => "push-commits-menu",
        }
    }

    /// The action of the main half of the split button.
    fn entry(self, facts: &Facts) -> Entry {
        match self {
            Self::Stage => Entry::StageAll,
            Self::Commit => availability::commit_entry(facts),
            Self::Push => availability::push_entry(facts),
        }
    }

    /// How the main half presents `entry`, the action this mode chose.
    fn face(self, entry: Entry) -> Face {
        let (icon, label, tooltip, look) = match self {
            Self::Stage => (
                Lucide::Plus,
                "Stage All",
                "Stage all changes and untracked files",
                ButtonLook::Outline,
            ),
            Self::Commit if entry == Entry::CommitAndPush => (
                Lucide::ArrowUp,
                "Commit & Push",
                "Commit staged changes, then push",
                ButtonLook::Primary,
            ),
            Self::Commit => (
                Lucide::GitCommitHorizontal,
                "Commit",
                "Commit staged changes",
                ButtonLook::Primary,
            ),
            Self::Push => (
                Lucide::ArrowUp,
                "Push",
                "Push committed changes",
                ButtonLook::Primary,
            ),
        };
        Face {
            icon,
            label,
            tooltip,
            look,
        }
    }
}

/// The icon, words and look of the main half of the split button.
struct Face {
    icon: Lucide,
    label: &'static str,
    tooltip: &'static str,
    look: ButtonLook,
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
        self.actions_menu_host(slot, window, cx)
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
        let entry = self.with_facts(cx, |facts| mode.entry(facts));
        let blocker = self.blocker(entry, cx);
        let face = mode.face(entry);
        let loading = self.git.read(cx).is_running();
        let active = mode == Mode::for_status(status) && !busy;
        let hover = Hover::new(mode.id(), window, cx).enabled(active && blocker.is_none());
        let main = Button::new(mode.id())
            .cursor_pointer()
            .small()
            .flex_1()
            .rounded_r(px(0.))
            .icon(face.icon)
            .label(face.label)
            .loading(loading)
            .disabled(!active || blocker.is_some())
            .tab_stop(interactive)
            .tooltip(blocker.map_or(face.tooltip, Blocker::explanation))
            .when(entry == Entry::Commit && blocker.is_none(), |button| {
                button.tooltip_with_action(face.tooltip, &CommitChanges, Some("SourceControl"))
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                if !interactive || Mode::for_status(this.git.read(cx).status.as_ref()) != mode {
                    return;
                }
                this.perform(entry, window, cx);
            }))
            .smooth_hover(face.look, &hover, cx)
            .flex_1();
        // The solid look has no border to share, so a hairline of the panel separates the halves.
        let split = h_flex()
            .flex_1()
            .min_w_0()
            .when(matches!(face.look, ButtonLook::Primary), |split| {
                split.gap_px()
            })
            .child(main)
            .child(self.menu_toggle(mode, face.look, interactive && status.is_some(), cx));
        let mut row = h_flex().w_full().gap_2().items_center().child(split);
        if self.git.read(cx).can_stop() {
            let hover = Hover::new("stop-git-operation", window, cx);
            row = row.child(
                Button::new("stop-git-operation")
                    .cursor_pointer()
                    .small()
                    .icon(Lucide::Square)
                    .accessibility_label("Stop the running Git operation")
                    .tooltip("Stop · Git is ended mid-command; verify the result afterwards")
                    .on_click(cx.listener(|this, _, _, cx| this.git.read(cx).stop_mutation()))
                    .smooth_hover(ButtonLook::Outline, &hover, cx),
            );
        }
        row
    }

    /// The right half of the split button, which opens the menu of the other actions.
    ///
    /// It keeps the library's hover and "open" surface, as every button with a menu does.
    fn menu_toggle(
        &self,
        mode: Mode,
        look: ButtonLook,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let open = self.actions_menu.is_open();
        let toggle = Button::new(mode.menu_id())
            .cursor_pointer()
            .small()
            .rounded_l(px(0.))
            .icon(Lucide::ChevronDown)
            .accessibility_label("More Git actions")
            .selected(open)
            .disabled(!interactive)
            .tab_stop(interactive)
            .when(!open, |toggle| toggle.tooltip("More Git actions"))
            // The pointer toggles on press, comparing with the state this frame drew: the
            // press that closes the menu from outside must not open it again.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    window.prevent_default();
                    if interactive && this.actions_menu.is_open() == open {
                        this.toggle_actions_menu(window, cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if interactive && event.is_keyboard() {
                    this.toggle_actions_menu(window, cx);
                }
            }));
        match look {
            ButtonLook::Primary => toggle.primary(),
            // The main half already draws the edge the two share.
            _ => toggle.outline().border_l_0(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Mode;
    use crate::ui::git_panel::availability::{Entry, Facts, PushTarget, Request};
    use hive_core::git::status::{Change, FileChange, Status};

    fn staged_file() -> FileChange {
        FileChange {
            path: "file".into(),
            old_path: None,
            staged: Some(Change::Modified),
            unstaged: Some(Change::Modified),
            submodule: false,
            staged_stats: None,
            unstaged_stats: None,
            conflict: None,
        }
    }

    #[test]
    fn footer_follows_index_and_pending_commits() {
        let mut status = Status::default();
        assert_eq!(Mode::for_status(None), Mode::Stage);
        assert_eq!(Mode::for_status(Some(&status)), Mode::Stage);
        status.branch = Some("main".into());
        status.files.push(staged_file());
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

    #[test]
    fn the_main_button_commits_and_pushes_wherever_a_push_can_go() {
        let mut status = Status {
            oid: Some("head".into()),
            branch: Some("main".into()),
            upstream: Some("origin/main".into()),
            files: vec![staged_file()],
            ..Status::default()
        };
        let entry = |mode: Mode, status: &Status, push: Option<PushTarget>| {
            mode.entry(&Facts {
                status: Some(status),
                message: "message",
                has_remote: push.is_some(),
                push,
                operation_unfinished: false,
                base: None,
                request: Request::Unknown,
            })
        };
        assert_eq!(entry(Mode::Stage, &status, None), Entry::StageAll);
        assert_eq!(
            entry(Mode::Commit, &status, Some(PushTarget::Upstream)),
            Entry::CommitAndPush
        );
        assert_eq!(
            entry(Mode::Push, &status, Some(PushTarget::Upstream)),
            Entry::Push
        );
        // Without a remote the button still commits in one click.
        assert_eq!(entry(Mode::Commit, &status, None), Entry::Commit);
        status.upstream = None;
        assert_eq!(
            entry(Mode::Commit, &status, Some(PushTarget::NewBranch)),
            Entry::CommitAndPush
        );
        assert_eq!(
            entry(Mode::Push, &status, Some(PushTarget::NewBranch)),
            Entry::Publish
        );
        status.branch = None;
        assert_eq!(
            entry(Mode::Commit, &status, Some(PushTarget::NewBranch)),
            Entry::Commit
        );
    }
}
