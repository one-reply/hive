//! One generation per target, sharing the same mutation lock as staging and commits.
use super::*;
use hive_core::commit_message::{self, Recipe, cli::Cancellation};
use std::sync::Arc;

impl Git {
    pub(crate) fn generate_message(&mut self, recipe: Recipe, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        if !self.busy.insert(target.clone()) {
            return;
        }
        self._refresh = None;
        self.loading = false;
        self.error = None;
        self.failures.remove(&target);
        let cancellation = Arc::new(Cancellation::default());
        self.generations
            .insert(target.clone(), cancellation.clone());
        let key = target.clone();
        let task = cx.spawn(async move |this, cx| {
            let query = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    commit_message::generate(&query.host, &query.path, &recipe, &cancellation)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy.remove(&target);
                this.generations.remove(&target);
                match result {
                    Ok(message) => {
                        this.drafts.insert(target.clone(), message);
                    }
                    Err(error) => {
                        tracing::warn!(%error, "commit generation failed");
                        let message = error.to_string();
                        this.failures.insert(target.clone(), message.clone());
                        if this.target.as_ref() == Some(&target) {
                            this.error = Some(message);
                        }
                    }
                }
                if this.target.as_ref() == Some(&target) {
                    this.refresh(true, cx);
                }
                cx.notify();
            });
        });
        self.operations.insert(key, task);
        cx.notify();
    }

    pub(crate) fn stop_generations(&self) {
        for cancellation in self.generations.values() {
            cancellation.cancel();
        }
    }
}

impl Drop for Git {
    fn drop(&mut self) {
        self.stop_generations();
    }
}
