//! Serializes user mutations per worktree and preserves drafts on failure.
use super::*;

/// The message of the commit `action` created, which its draft no longer has to hold: also
/// when the push or sync that followed the commit failed, since the commit itself stays.
fn committed_message<'a>(
    action: Option<&'a Action>,
    result: &Result<(), git::GitError>,
) -> Option<&'a str> {
    use git::GitError::{PushAfterCommit, SyncAfterCommit};
    match action? {
        Action::Commit(message) if result.is_ok() => Some(message),
        Action::CommitAndPush { message, .. }
            if result.is_ok() || matches!(result, Err(PushAfterCommit { .. })) =>
        {
            Some(message)
        }
        Action::CommitAndSync { message, .. }
            if result.is_ok() || matches!(result, Err(SyncAfterCommit { .. })) =>
        {
            Some(message)
        }
        _ => None,
    }
}

impl Git {
    pub(crate) fn push_action(&self) -> Option<Action> {
        let status = self.status.as_ref()?;
        status.branch.as_ref()?;
        // An upstream whose remote-tracking ref is gone has nothing to push to until the
        // branch is published again; Git counts no commits ahead of it either.
        let publish = status.tracked_upstream().is_none();
        let remote = if publish {
            self.remote.as_ref()?
        } else {
            self.upstream_remote.as_ref()?
        };
        Some(Action::Push {
            remote: remote.clone(),
            publish,
        })
    }

    pub fn perform(&mut self, action: Action, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        self.mutate(target, Some(action), None, cx);
    }

    pub fn perform_at(&mut self, target: Target, action: Action, cx: &mut Context<Self>) {
        self.mutate(target, Some(action), None, cx);
    }

    /// Runs `action` like [`Git::perform_at`] for a caller that continues once it is done:
    /// the receiver gets whether it succeeded. A failure is reported to the user as usual.
    ///
    /// `None` when nothing was started, because the worktree is busy or awaits verification.
    pub(crate) fn perform_awaited(
        &mut self,
        target: Target,
        action: Action,
        cx: &mut Context<Self>,
    ) -> Option<async_channel::Receiver<bool>> {
        if !self.mutate(target.clone(), Some(action), None, cx) {
            return None;
        }
        let (done, outcome) = async_channel::bounded(1);
        self.outcomes.insert(target, done);
        Some(outcome)
    }

    pub fn apply_hunk(
        &mut self,
        request: DiffRequest,
        diff: std::sync::Arc<FileDiff>,
        hunk: usize,
        cx: &mut Context<Self>,
    ) {
        self.mutate(
            request.target.clone(),
            None,
            Some((request, diff, hunk)),
            cx,
        );
    }

    /// Starts the mutation unless the worktree is busy or awaits verification; says whether
    /// it started.
    fn mutate(
        &mut self,
        target: Target,
        action: Option<Action>,
        hunk: Option<(DiffRequest, std::sync::Arc<FileDiff>, usize)>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.unverified.contains(&target) || !self.busy.insert(target.clone()) {
            return false;
        }
        if self.target.as_ref() == Some(&target) {
            self._refresh = None;
            self.refresh_generation += 1;
            self.loading = false;
            self.error = None;
        }
        self.failures.remove(&target);
        let stop = std::sync::Arc::new(hive_core::command::Cancellation::default());
        if action.as_ref().is_some_and(Action::is_stoppable) {
            self.stops.insert(target.clone(), stop.clone());
        }
        self.start_remote_operation(&target, action.as_ref());
        tracing::info!(path = ?target.path, ?action, hunk = hunk.is_some(), "source control mutation");
        cx.notify();
        let key = target.clone();
        let task = cx.spawn(async move |this, cx| {
            let t = target.clone();
            let operation = action.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(action) = operation {
                        git::changes::execute(&t.host, &t.path, &action, &stop)
                    } else if let Some((request, diff, index)) = hunk {
                        git::diff::apply_hunk(
                            &t.host,
                            &t.path,
                            &request.path,
                            &request.source,
                            &diff,
                            index,
                        )
                        .map(|()| None)
                    } else {
                        Ok(None)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy.remove(&target);
                this.stops.remove(&target);
                let (result, synced) = match result {
                    Ok(synced) => (Ok(()), synced),
                    Err(error) => (Err(error), None),
                };
                if let Some(done) = this.outcomes.remove(&target) {
                    // The waiting side may have gone away; then nobody needs the answer.
                    let _ = done.try_send(result.is_ok());
                }
                this.finish_remote_operation(&target, synced, cx);
                let message = committed_message(action.as_ref(), &result);
                if let Some(message) = message
                    && this.draft(&target) == message
                {
                    this.drafts.remove(&target);
                }
                if result.is_ok() || message.is_some() {
                    this.projects.update(cx, |p, cx| p.rescan(cx)).detach();
                }
                if let Err(error) = result {
                    if error.outcome_unknown() {
                        this.unverified.insert(target.clone());
                        this.interrupted_actions
                            .insert(target.clone(), action.clone());
                    }
                    this.report(&target, error, cx);
                }
                if this.target.as_ref() == Some(&target) {
                    this.refresh(true, cx);
                }
                cx.notify();
            });
        });
        self.operations.insert(key, task);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::committed_message;
    use crate::state::git::tests::source_control;
    use gpui_kit as gpui;
    use gpui_kit::TestAppContext;
    use hive_core::git::{GitError, changes::Action, status::Status};

    #[gpui::test]
    fn a_push_publishes_a_branch_whose_upstream_is_missing_or_gone(cx: &mut TestAppContext) {
        let (_, git) = source_control(cx);
        git.update(cx, |git, _| {
            git.remote = Some("origin".into());
            git.upstream_remote = Some("origin".into());
            let mut status = Status {
                oid: Some("head".into()),
                branch: Some("feature".into()),
                upstream: Some("origin/feature".into()),
                ..Status::default()
            };
            let mut publishes = |status: &Status| {
                git.status = Some(status.clone());
                match git.push_action() {
                    Some(Action::Push { remote, publish }) if remote == "origin" => publish,
                    other => panic!("unexpected push action: {other:?}"),
                }
            };
            assert!(!publishes(&status));
            // Deleted on the remote and pruned here: the configuration still names it.
            status.upstream_gone = true;
            assert!(publishes(&status));
            status.upstream = None;
            assert!(publishes(&status));
        });
    }

    fn failed() -> GitError {
        GitError::Invalid("the hook refused".into())
    }

    #[test]
    fn a_draft_is_spent_once_its_commit_exists_whatever_followed_it() {
        let commit = Action::Commit("message".into());
        let push = Action::CommitAndPush {
            message: "message".into(),
            remote: "origin".into(),
            publish: false,
        };
        let sync = Action::CommitAndSync {
            message: "message".into(),
            remote: "origin".into(),
        };
        for action in [&commit, &push, &sync] {
            assert_eq!(committed_message(Some(action), &Ok(())), Some("message"));
            // The commit itself failed: the text is still the user's draft.
            assert_eq!(committed_message(Some(action), &Err(failed())), None);
        }
        let after_commit = |wrap: fn(Box<GitError>) -> GitError| Err(wrap(Box::new(failed())));
        let pushed = after_commit(|source| GitError::PushAfterCommit { source });
        let synced = after_commit(|source| GitError::SyncAfterCommit { source });
        assert_eq!(committed_message(Some(&push), &pushed), Some("message"));
        assert_eq!(committed_message(Some(&sync), &synced), Some("message"));
        // Each wrapper belongs to its own action.
        assert_eq!(committed_message(Some(&push), &synced), None);
        assert_eq!(committed_message(Some(&commit), &pushed), None);
    }

    #[test]
    fn mutations_that_commit_nothing_leave_the_draft() {
        let sync = Action::Sync {
            remote: "origin".into(),
        };
        for action in [Some(&sync), Some(&Action::Pull), None] {
            assert_eq!(committed_message(action, &Ok(())), None);
        }
    }
}
