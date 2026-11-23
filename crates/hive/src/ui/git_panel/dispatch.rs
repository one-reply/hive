//! Turns a click, a shortcut or a menu row into the Git action it names, when the worktree
//! allows that action right now.
use super::*;

impl GitPanel {
    /// Reads what decides the availability of the panel's actions, as of this frame.
    pub(super) fn with_facts<R>(&self, cx: &App, read: impl FnOnce(&Facts) -> R) -> R {
        let git = self.git.read(cx);
        let message = self.message.read(cx).value();
        let push = PushTarget::of(git.push_action().as_ref(), git.status.as_ref());
        read(&Facts {
            status: git.status.as_ref(),
            message: &message,
            has_remote: git.remote.is_some(),
            push,
            operation_unfinished: git.operation().is_some(),
            base: git.base.as_deref(),
            request: self.request(git, cx),
        })
    }

    pub(super) fn blocker(&self, entry: Entry, cx: &App) -> Option<Blocker> {
        self.with_facts(cx, |facts| availability::blocker(entry, facts))
    }

    /// Runs `entry`, unless the worktree is busy or the action is unavailable by now.
    pub(super) fn perform(&self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        if self.git.read(cx).is_busy() || self.blocker(entry, cx).is_some() {
            return;
        }
        match entry {
            Entry::StageAll => self.stage_all(cx),
            Entry::Commit => self.commit(cx),
            Entry::CommitAndPush => self.commit_and_push(window, cx),
            Entry::CommitAndSync => self.commit_and_sync(cx),
            Entry::Push => self.push(false, window, cx),
            Entry::Publish => self.push(true, window, cx),
            Entry::ForcePush => self.force_push(window, cx),
            Entry::CreatePullRequest => self.create_pull_request(false, window, cx),
            Entry::PushBeforePullRequest => self.create_pull_request(true, window, cx),
            Entry::Sync => self.sync(cx),
            Entry::Rebase => self.rebase(window, cx),
            Entry::Pull => self.git.update(cx, |git, cx| git.perform(Action::Pull, cx)),
            Entry::Fetch => self.fetch(cx),
        }
    }

    /// Stages every unstaged and untracked file, whatever the list filter shows.
    fn stage_all(&self, cx: &mut Context<Self>) {
        let Some(status) = &self.git.read(cx).status else {
            return;
        };
        let paths = status
            .files
            .iter()
            .filter(|file| file.unstaged.is_some())
            .map(|file| file.path.clone())
            .collect();
        self.git
            .update(cx, |git, cx| git.perform(Action::Stage(paths), cx));
    }

    fn commit(&self, cx: &mut Context<Self>) {
        let message = self.message.read(cx).value().to_string();
        self.git
            .update(cx, |git, cx| git.perform(Action::Commit(message), cx));
    }

    fn commit_and_push(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Action::Push { remote, publish }) = self.git.read(cx).push_action() else {
            return;
        };
        let description = format!(
            "Commit staged changes and publish this branch to {remote}, setting its upstream."
        );
        let action = Action::CommitAndPush {
            message: self.message.read(cx).value().to_string(),
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

    /// Pushes to the upstream, or publishes the branch under its own name once the user
    /// confirms where to; publishing a branch that tracks another name moves its upstream.
    fn push(&self, publish: bool, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.git.read(cx);
        let Some(Action::Push { remote, .. }) = state.push_action() else {
            return;
        };
        let Some(branch) = state
            .status
            .as_ref()
            .and_then(|status| status.branch.as_ref())
        else {
            return;
        };
        let description =
            format!("{branch} will be pushed to {remote} and start tracking {remote}/{branch}.");
        let action = Action::Push { remote, publish };
        if publish {
            git_controls::confirm(
                self.git.clone(),
                action,
                git_controls::Confirmation {
                    title: "Publish Branch",
                    description,
                    ok_text: "Publish",
                    danger: false,
                },
                window,
                cx,
            );
        } else {
            self.git.update(cx, |git, cx| git.perform(action, cx));
        }
    }

    fn fetch(&self, cx: &mut Context<Self>) {
        let Some(remote) = self.git.read(cx).remote.clone() else {
            return;
        };
        self.git
            .update(cx, |git, cx| git.perform(Action::Fetch(remote), cx));
    }

    /// The remote the branch tracks, which a sync or a force push talks to.
    fn upstream_remote(&self, cx: &App) -> Option<String> {
        match self.git.read(cx).push_action() {
            Some(Action::Push {
                remote,
                publish: false,
            }) => Some(remote),
            _ => None,
        }
    }

    fn sync(&self, cx: &mut Context<Self>) {
        let Some(remote) = self.upstream_remote(cx) else {
            return;
        };
        self.git
            .update(cx, |git, cx| git.perform(Action::Sync { remote }, cx));
    }

    fn commit_and_sync(&self, cx: &mut Context<Self>) {
        let Some(remote) = self.upstream_remote(cx) else {
            return;
        };
        let message = self.message.read(cx).value().to_string();
        self.git.update(cx, |git, cx| {
            git.perform(Action::CommitAndSync { message, remote }, cx)
        });
    }

    /// Reads what the remote branch would lose, then asks before replacing it.
    fn force_push(&self, window: &mut Window, cx: &mut Context<Self>) {
        let git = self.git.clone();
        let Some(request) = git.update(cx, |git, cx| git.preview_force_push(cx)) else {
            return;
        };
        let (target, remote, preview) = (request.target, request.remote, request.preview);
        window
            .spawn(cx, async move |cx| {
                // The sender goes away only with source control itself.
                let Ok(preview) = preview.recv().await else {
                    return;
                };
                cx.update(|window, cx| match preview {
                    Ok(preview) => {
                        let confirmation = git_controls::Confirmation {
                            title: "Force Push",
                            description: force_push_warning(&preview),
                            ok_text: "Force Push",
                            danger: true,
                        };
                        let action = Action::ForcePush {
                            remote,
                            expected: preview.expected,
                            head: preview.head,
                        };
                        git_controls::confirm_at(git, target, action, confirmation, window, cx);
                    }
                    Err(error) => {
                        tracing::warn!(%error, path = ?target.path, "force push preview failed");
                        git.update(cx, |git, cx| {
                            git.report_message(&target, error.to_string(), cx)
                        });
                    }
                })
                .ok();
            })
            .detach();
    }

    fn rebase(&self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.git.read(cx);
        let (Some(base), Some(branch)) = (
            state.base.clone(),
            state
                .status
                .as_ref()
                .and_then(|status| status.branch.clone()),
        ) else {
            return;
        };
        let published = state
            .status
            .as_ref()
            .is_some_and(|status| status.tracked_upstream().is_some());
        git_controls::confirm(
            self.git.clone(),
            Action::Rebase { onto: base.clone() },
            git_controls::Confirmation {
                title: "Rebase Branch",
                description: rebase_warning(&branch, &base, published),
                ok_text: "Rebase",
                danger: false,
            },
            window,
            cx,
        );
    }
}

/// What the confirmation of a force push says it discards.
fn force_push_warning(preview: &ForcePushPreview) -> String {
    let upstream = &preview.upstream;
    let mut warning = match preview.total {
        0 => format!("{upstream} will be replaced by your local branch."),
        1 => format!(
            "{upstream} will be replaced by your local branch. 1 commit on it that your branch does not contain will be discarded:"
        ),
        total => format!(
            "{upstream} will be replaced by your local branch. {total} commits on it that your branch does not contain will be discarded:"
        ),
    };
    for commit in &preview.discarded {
        warning.push_str("\n• ");
        warning.push_str(commit);
    }
    let unlisted = preview.total.saturating_sub(preview.discarded.len());
    if unlisted > 0 {
        warning.push_str(&format!("\n…and {unlisted} more"));
    }
    warning.push_str(
        "\n\nThe push is refused if the remote branch or your local branch has moved since this was read.",
    );
    warning
}

/// What the confirmation of a rebase says will happen to `branch`.
fn rebase_warning(branch: &str, base: &str, published: bool) -> String {
    let name = branches::short_name(base);
    let fetched = if base.starts_with("refs/remotes/") {
        format!("Hive fetches {name} first, then replays")
    } else {
        "Hive replays".to_owned()
    };
    let mut warning = format!(
        "{fetched} the commits of {branch} on top of {name}. They get new ids; a conflict pauses the rebase until you continue or abort it."
    );
    if published {
        warning.push_str(" The published branch then needs Force Push.");
    }
    warning
}

#[cfg(test)]
mod tests {
    use super::{force_push_warning, rebase_warning};
    use hive_core::git::changes::ForcePushPreview;

    fn preview(total: usize, discarded: &[&str]) -> ForcePushPreview {
        ForcePushPreview {
            upstream: "origin/feature".into(),
            expected: "0123abcd".into(),
            head: "4567ef01".into(),
            discarded: discarded.iter().map(|commit| commit.to_string()).collect(),
            total,
        }
    }

    #[test]
    fn a_force_push_names_every_commit_it_discards_or_how_many_it_leaves_out() {
        let one = force_push_warning(&preview(1, &["abc1234 fix the parser"]));
        assert!(one.starts_with("origin/feature will be replaced by your local branch. 1 commit"));
        assert!(one.contains("\n• abc1234 fix the parser"));
        assert!(!one.contains("more"));

        let many = force_push_warning(&preview(10, &["a first", "b second"]));
        assert!(many.contains("10 commits on it"));
        assert!(many.contains("\n• a first\n• b second\n…and 8 more"));
        assert!(many.ends_with("has moved since this was read."));

        // Local commits were dropped; the remote has nothing the branch lacks.
        let none = force_push_warning(&preview(0, &[]));
        assert!(none.starts_with("origin/feature will be replaced by your local branch."));
        assert!(!none.contains("discarded"));
    }

    #[test]
    fn a_rebase_says_what_it_fetches_and_what_a_published_branch_needs_next() {
        let remote = rebase_warning("feature", "refs/remotes/origin/develop", true);
        assert!(remote.starts_with("Hive fetches origin/develop first, then replays"));
        assert!(remote.contains("the commits of feature on top of origin/develop"));
        assert!(remote.ends_with("The published branch then needs Force Push."));

        let local = rebase_warning("feature", "refs/heads/main", false);
        assert!(local.starts_with("Hive replays the commits of feature on top of main."));
        assert!(!local.contains("Force Push"));
    }
}
