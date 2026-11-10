//! The worktree's unfinished merge, rebase, cherry-pick or revert, and its open conflicts.
use super::*;
use hive_core::git::conflicts::Operation;

const BANNER_BACKGROUND_OPACITY: f32 = 0.06;
const BANNER_BORDER_OPACITY: f32 = 0.35;

/// What the banner above the file list says and which actions it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Banner {
    pub title: String,
    /// The operation Abort and Continue act on; a stash or checkout can leave conflicts
    /// without one.
    pub operation: Option<Operation>,
    pub conflicts: usize,
}

impl Banner {
    /// `None` when the worktree is neither mid-operation nor holding unmerged files.
    pub(super) fn new(operation: Option<Operation>, conflicts: usize) -> Option<Self> {
        let unresolved = match conflicts {
            0 => None,
            1 => Some("1 conflict".to_owned()),
            count => Some(format!("{count} conflicts")),
        };
        let title = match (operation, unresolved) {
            (None, None) => return None,
            (None, Some(unresolved)) => format!("{unresolved} to resolve"),
            (Some(operation), None) => {
                format!("{} in progress · ready to continue", name(operation))
            }
            (Some(operation), Some(unresolved)) => {
                format!("{} in progress · {unresolved}", name(operation))
            }
        };
        Some(Self {
            title,
            operation,
            conflicts,
        })
    }

    /// Why Continue cannot run yet.
    pub(super) fn continue_blocker(&self) -> Option<&'static str> {
        (self.conflicts > 0).then_some("Resolve every conflict first")
    }
}

fn name(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => "Merge",
        Operation::Rebase => "Rebase",
        Operation::CherryPick => "Cherry-pick",
        Operation::Revert => "Revert",
    }
}

fn continue_tooltip(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => "Commit the merge with Git's default message",
        Operation::Rebase => "Continue the rebase with the staged resolution",
        Operation::CherryPick => "Commit the cherry-pick and continue",
        Operation::Revert => "Commit the revert and continue",
    }
}

fn abort_confirmation(operation: Operation) -> git_controls::Confirmation {
    let (title, ok_text) = match operation {
        Operation::Merge => ("Abort Merge", "Abort Merge"),
        Operation::Rebase => ("Abort Rebase", "Abort Rebase"),
        Operation::CherryPick => ("Abort Cherry-pick", "Abort Cherry-pick"),
        Operation::Revert => ("Abort Revert", "Abort Revert"),
    };
    git_controls::Confirmation {
        title,
        description: format!(
            "The {} is abandoned and the worktree returns to where it was before it started. Conflict resolutions made since then are lost and cannot be recovered.",
            name(operation).to_lowercase()
        ),
        ok_text,
        danger: true,
    }
}

impl GitPanel {
    pub(super) fn operation(
        &self,
        banner: Banner,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let busy = self.git.read(cx).is_busy();
        let blocker = banner.continue_blocker();
        let can_continue = !busy && blocker.is_none();
        let agent_hover = Hover::new("operation-agent", window, cx);
        let abort_hover = Hover::new("operation-abort", window, cx).enabled(!busy);
        let continue_hover = Hover::new("operation-continue", window, cx).enabled(can_continue);
        let warning = cx.theme().warning;
        let button = |id: &'static str| Button::new(id).cursor_pointer().xsmall();
        let mut actions = h_flex().flex_wrap().gap_1();
        if banner.conflicts > 0 {
            actions = actions.child(
                button("operation-agent")
                    .label("Resolve with Agent")
                    .tooltip("Draft a request to resolve every conflict for the worktree's agent")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.git.update(cx, |git, cx| {
                            if let Some(target) = git.target.clone() {
                                git.prepare_conflict_draft(target, &git.conflicted_paths(), cx);
                            }
                        })
                    }))
                    .smooth_hover(ButtonLook::Outline, &agent_hover, cx),
            );
        }
        if let Some(operation) = banner.operation {
            actions = actions
                .child(
                    button("operation-abort")
                        .label("Abort")
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            git_controls::confirm(
                                this.git.clone(),
                                Action::AbortOperation,
                                abort_confirmation(operation),
                                window,
                                cx,
                            )
                        }))
                        .smooth_hover(ButtonLook::Outline, &abort_hover, cx),
                )
                .child(
                    button("operation-continue")
                        .label("Continue")
                        .tooltip(blocker.unwrap_or_else(|| continue_tooltip(operation)))
                        .disabled(!can_continue)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.git
                                .update(cx, |git, cx| git.perform(Action::ContinueOperation, cx))
                        }))
                        .smooth_hover(ButtonLook::Primary, &continue_hover, cx),
                );
        }
        div().px_3().pb_2().flex_shrink_0().child(
            v_flex()
                .p_2()
                .gap_2()
                .rounded_md()
                .border_1()
                .border_color(warning.opacity(BANNER_BORDER_OPACITY))
                .bg(warning.opacity(BANNER_BACKGROUND_OPACITY))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(Lucide::GitMerge).small().text_color(warning))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(META_TEXT + px(1.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(banner.title),
                        ),
                )
                .child(actions),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{Banner, abort_confirmation};
    use hive_core::git::conflicts::Operation;

    #[test]
    fn banner_names_the_operation_and_what_is_left() {
        assert_eq!(Banner::new(None, 0), None);
        let title = |operation, conflicts| Banner::new(operation, conflicts).unwrap().title;
        assert_eq!(
            title(Some(Operation::Merge), 2),
            "Merge in progress · 2 conflicts"
        );
        assert_eq!(
            title(Some(Operation::Rebase), 1),
            "Rebase in progress · 1 conflict"
        );
        assert_eq!(
            title(Some(Operation::CherryPick), 0),
            "Cherry-pick in progress · ready to continue"
        );
        // A stash or checkout leaves unmerged files with nothing to abort or continue.
        let stash = Banner::new(None, 3).unwrap();
        assert_eq!(stash.title, "3 conflicts to resolve");
        assert_eq!(stash.operation, None);
    }

    #[test]
    fn continue_waits_for_every_conflict() {
        let blocked = Banner::new(Some(Operation::Rebase), 1).unwrap();
        assert_eq!(
            blocked.continue_blocker(),
            Some("Resolve every conflict first")
        );
        let ready = Banner::new(Some(Operation::Rebase), 0).unwrap();
        assert_eq!(ready.continue_blocker(), None);
    }

    #[test]
    fn abort_is_confirmed_as_destructive_by_name() {
        let confirmation = abort_confirmation(Operation::CherryPick);
        assert!(confirmation.danger);
        assert_eq!(confirmation.title, "Abort Cherry-pick");
        assert!(
            confirmation
                .description
                .contains("The cherry-pick is abandoned")
        );
    }
}
