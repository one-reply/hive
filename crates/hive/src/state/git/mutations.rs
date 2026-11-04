//! Serializes user mutations per worktree and preserves drafts on failure.
use super::*;

impl Git {
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
        if !self.busy.insert(target.clone()) {
            return;
        }
        if self.target.as_ref() == Some(&target) {
            self._refresh = None;
            self.loading = false;
            self.error = None;
        }
        self.failures.remove(&target);
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
                    } else {
                        Ok(())
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy.remove(&target);
                match result {
                    Ok(()) => {
                        if let Some(Action::Commit(message)) = action
                            && this.draft(&target) == message
                        {
                            this.drafts.remove(&target);
                        }
                        this.projects.update(cx, |p, cx| p.rescan(cx)).detach();
                    }
                    Err(error) => this.report(&target, error, cx),
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
