//! Serializes user mutations per worktree and preserves drafts on failure.
use super::*;

impl Git {
    pub(crate) fn push_action(&self) -> Option<Action> {
        let status = self.status.as_ref()?;
        status.branch.as_ref()?;
        let publish = status.upstream.is_none();
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

    fn mutate(
        &mut self,
        target: Target,
        action: Option<Action>,
        hunk: Option<(DiffRequest, std::sync::Arc<FileDiff>, usize)>,
        cx: &mut Context<Self>,
    ) {
        if self.unverified.contains(&target) || !self.busy.insert(target.clone()) {
            return;
        }
        if self.target.as_ref() == Some(&target) {
            self._refresh = None;
            self.loading = false;
            self.error = None;
        }
        self.failures.remove(&target);
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
                        git::changes::execute(&t.host, &t.path, &action)
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
                let (result, synced) = match result {
                    Ok(synced) => (Ok(()), synced),
                    Err(error) => (Err(error), None),
                };
                this.finish_remote_operation(&target, synced, cx);
                let message = match &action {
                    Some(Action::Commit(message)) if result.is_ok() => Some(message),
                    Some(Action::CommitAndPush { message, .. })
                        if result.is_ok()
                            || matches!(&result, Err(git::GitError::PushAfterCommit { .. })) =>
                    {
                        Some(message)
                    }
                    _ => None,
                };
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
    }
}
