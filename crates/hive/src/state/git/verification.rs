//! Quarantine uncertain writes until a fresh inspection and explicit acknowledgement.
use super::*;

impl Git {
    pub(crate) fn needs_verification(&self) -> bool {
        self.target
            .as_ref()
            .is_some_and(|t| self.unverified.contains(t))
    }

    pub(crate) fn acknowledge_outcome(&mut self, cx: &mut Context<Self>) {
        if !self.result_inspected() {
            return;
        }
        if let Some(target) = &self.target {
            self.unverified.remove(target);
            self.inspected.remove(target);
            self.interrupted_actions.remove(target);
        }
        self.clear_error(cx);
    }

    pub(crate) fn result_inspected(&self) -> bool {
        self.target
            .as_ref()
            .is_some_and(|t| self.inspected.contains(t))
    }

    pub(crate) fn verify_result(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone().filter(|t| self.unverified.contains(t)) else {
            return;
        };
        if !self.busy.insert(target.clone()) {
            return;
        }
        let action = self.interrupted_actions.get(&target).cloned().flatten();
        let task_target = target.clone();
        let task = cx.spawn(async move |this, cx| {
            let t = task_target.clone();
            let result = cx
                .background_executor()
                .spawn(async move { git::verification::inspect(&t.host, &t.path, action.as_ref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy.remove(&task_target);
                match result {
                    Ok(report) => {
                        this.inspected.insert(task_target.clone());
                        this.failures.insert(task_target.clone(), report.clone());
                        if this.target.as_ref() == Some(&task_target) {
                            this.error = Some(report);
                        }
                    }
                    Err(error) => this.report(&task_target, error, cx),
                }
                cx.notify();
            });
        });
        self.operations.insert(target, task);
        cx.notify();
    }
}
