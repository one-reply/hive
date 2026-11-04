//! Commit composer and its staged-change controls.
use super::*;

mod actions;

impl GitPanel {
    fn generate_message(&self, cx: &mut Context<Self>) {
        let git = self.git.read(cx);
        if git.is_busy()
            || git
                .status
                .as_ref()
                .is_none_or(|s| s.has_conflicts() || staged_count(s) == 0)
        {
            return;
        }
        let recipe = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .commit_message
            .clone();
        if !recipe.is_configured() {
            crate::ui::settings::open_git(cx);
            return;
        }
        self.git
            .update(cx, |git, cx| git.generate_message(recipe, cx));
    }

    pub(super) fn commit(&self, cx: &mut Context<Self>) {
        let state = self.git.read(cx);
        let message = self.message.read(cx).value();
        if state.is_busy() || commit_blocker(state.status.as_ref(), &message).is_some() {
            return;
        }
        self.git.update(cx, |g, cx| {
            g.perform(Action::Commit(message.to_string()), cx)
        });
    }

    fn commit_and_push(&self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.git.read(cx);
        let message = self.message.read(cx).value();
        if state.is_busy() || commit_blocker(state.status.as_ref(), &message).is_some() {
            return;
        }
        let Some(Action::Push { remote, publish }) = state.push_action() else {
            return;
        };
        let description = format!(
            "Commit staged changes and publish this branch to {remote}, setting its upstream."
        );
        let action = Action::CommitAndPush {
            message: message.to_string(),
            remote,
            publish,
        };
        if publish {
            git_controls::confirm(
                self.git.clone(),
                action,
                git_controls::Confirmation {
                    title: "Commit & Publish Branch",
                    description,
                    ok_text: "Commit & Push",
                    danger: false,
                },
                window,
                cx,
            );
        } else {
            self.git.update(cx, |git, cx| git.perform(action, cx));
        }
    }

    pub(super) fn footer(
        &self,
        status: Option<&Status>,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let staged = status.map(staged_count).unwrap_or(0);
        let generating = self.git.read(cx).is_generating();
        let conflicts = status.is_some_and(Status::has_conflicts);
        let generate_hover = Hover::new("generate-commit-message", window, cx)
            .enabled(!busy && !conflicts && staged > 0);
        v_flex()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .relative()
                    .child(TextField::textarea(
                        Textarea::new(&self.message).pr_8().disabled(generating),
                        &self.message,
                        cx,
                    ))
                    .child(
                        div().absolute().top_1().right_1().child(
                            Button::new("generate-commit-message")
                                .cursor_pointer()
                                .small()
                                .icon(Lucide::Sparkles)
                                .loading(generating)
                                .accessibility_label("Generate commit message with AI")
                                .disabled(busy || conflicts || staged == 0)
                                .tooltip(if conflicts {
                                    "Resolve conflicts first"
                                } else if staged == 0 {
                                    "Stage changes to enable AI generation"
                                } else {
                                    "Generate a new commit message · configure in Git settings"
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.generate_message(cx)))
                                .smooth_hover(ButtonLook::Ghost, &generate_hover, cx),
                        ),
                    ),
            )
            .when(generating, |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Generating…"),
                )
            })
            .child(self.commit_actions(status, busy, window, cx))
    }
}

fn staged_count(status: &Status) -> usize {
    status
        .files
        .iter()
        .filter(|f| f.staged.is_some_and(|c| c != Change::Conflicted))
        .count()
}

fn commit_blocker(status: Option<&Status>, message: &str) -> Option<&'static str> {
    if status.is_some_and(Status::has_conflicts) {
        Some("Resolve conflicts first")
    } else if status.is_none_or(|s| staged_count(s) == 0) {
        Some("Stage changes first")
    } else if message.trim().is_empty() {
        Some("Write a commit message")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{commit_blocker, staged_count};
    use hive_core::git::status::{Change, FileChange, Status};

    #[test]
    fn commit_requires_staged_files_message_and_no_conflicts() {
        let mut status = Status::default();
        assert_eq!(commit_blocker(None, "message"), Some("Stage changes first"));
        assert_eq!(
            commit_blocker(Some(&status), "message"),
            Some("Stage changes first")
        );
        status.files.push(FileChange {
            path: "file.rs".into(),
            old_path: None,
            staged: None,
            unstaged: Some(Change::Modified),
            submodule: false,
            staged_stats: None,
            unstaged_stats: None,
        });
        assert_eq!(staged_count(&status), 0);
        assert_eq!(
            commit_blocker(Some(&status), "message"),
            Some("Stage changes first")
        );
        status.files[0].staged = Some(Change::Modified);
        assert_eq!(staged_count(&status), 1);
        for message in ["", " \n\t"] {
            assert_eq!(
                commit_blocker(Some(&status), message),
                Some("Write a commit message")
            );
        }
        assert_eq!(commit_blocker(Some(&status), "message"), None);
        let mut conflict = status.files[0].clone();
        conflict.path = "conflict.rs".into();
        conflict.staged = Some(Change::Conflicted);
        status.files.push(conflict);
        assert_eq!(staged_count(&status), 1);
        assert_eq!(
            commit_blocker(Some(&status), "message"),
            Some("Resolve conflicts first")
        );
    }
}
