//! Commit composer and its staged-change controls.
use super::*;
use gpui_kit::component::kbd::Kbd;

impl GitPanel {
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

    pub(super) fn footer(
        &self,
        status: Option<&Status>,
        busy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let staged = status.map(staged_count).unwrap_or(0);
        let conflicts = status.is_some_and(Status::has_conflicts);
        let stage_paths: Vec<_> = status
            .into_iter()
            .flat_map(|s| &s.files)
            .filter(|file| file.unstaged.is_some_and(|c| c != Change::Conflicted))
            .map(|file| file.path.clone())
            .collect();
        let blocker = if staged > 0 {
            commit_blocker(status, &self.message.read(cx).value())
        } else if conflicts {
            Some("Resolve conflicts first")
        } else if status.is_none() {
            Some("Waiting for changes")
        } else if stage_paths.is_empty() {
            Some("No changes to stage")
        } else {
            None
        };
        let generate_hover = Hover::new("generate-commit-message", window, cx)
            .enabled(!busy && !conflicts && staged > 0);
        let commit_hover =
            Hover::new("commit-staged", window, cx).enabled(!busy && blocker.is_none());
        let commit_look = if staged == 0 {
            ButtonLook::Outline
        } else {
            ButtonLook::Primary
        };
        let shortcut = (staged > 0)
            .then(|| Kbd::binding_for_action(&CommitChanges, Some("SourceControl"), window))
            .flatten();
        v_flex()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .relative()
                    .child(TextField::textarea(
                        Textarea::new(&self.message).pr_8(),
                        &self.message,
                        cx,
                    ))
                    .child(
                        div().absolute().top_1().right_1().child(
                            // shortcut: UI only; attach generation when an AI runner is available.
                            Button::new("generate-commit-message")
                                .cursor_pointer()
                                .small()
                                .icon(Lucide::Sparkles)
                                .accessibility_label("Generate commit message with AI")
                                .disabled(busy || conflicts || staged == 0)
                                .tooltip(if conflicts {
                                    "Resolve conflicts first"
                                } else if staged == 0 {
                                    "Stage changes to enable AI generation"
                                } else {
                                    "Generate commit message with AI — coming soon"
                                })
                                .smooth_hover(ButtonLook::Ghost, &generate_hover, cx),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("commit-staged")
                            .cursor_pointer()
                            .small()
                            .flex_1()
                            .icon(if staged == 0 {
                                Lucide::Plus
                            } else {
                                Lucide::GitCommitHorizontal
                            })
                            .label(if staged == 0 { "Stage All" } else { "Commit" })
                            .loading(busy)
                            .disabled(busy || blocker.is_some())
                            .when_some(blocker, |b, reason| b.tooltip(reason))
                            .when(blocker.is_none() && staged == 0, |b| {
                                b.tooltip("Stage all changes and untracked files")
                            })
                            .when(blocker.is_none() && staged > 0, |b| {
                                b.tooltip_with_action(
                                    "Commit staged changes",
                                    &CommitChanges,
                                    Some("SourceControl"),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if staged == 0 {
                                    this.git.update(cx, |g, cx| {
                                        g.perform(Action::Stage(stage_paths.clone()), cx)
                                    });
                                } else {
                                    this.commit(cx);
                                }
                            }))
                            .smooth_hover(commit_look, &commit_hover, cx)
                            .flex_1(),
                    )
                    .when_some(shortcut, |row, kbd| row.child(kbd.outline())),
            )
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
