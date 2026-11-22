//! One generation per target, sharing the same mutation lock as staging and commits.
use super::*;
use hive_core::commit_message::{self, Recipe, cli::Cancellation};
use std::sync::Arc;

impl Git {
    pub(crate) fn generate_message(&mut self, recipe: Recipe, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let (host, path) = (target.host.clone(), target.path.clone());
        self.generate(target, cx, move |cancellation| {
            commit_message::generate(&host, &path, &recipe, cancellation)
        });
    }

    /// Runs `generator` off the foreground thread as the generation of `target`, unless the
    /// worktree is busy. Its message replaces the draft; its error is shown and keeps it.
    fn generate(
        &mut self,
        target: Target,
        cx: &mut Context<Self>,
        generator: impl FnOnce(&Cancellation) -> Result<String, commit_message::Error> + Send + 'static,
    ) {
        if self.is_busy_at(&target) || !self.busy.insert(target.clone()) {
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
            let result = cx
                .background_executor()
                .spawn(async move { generator(&cancellation) })
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

    /// Ends the generation running for `target` and frees the worktree at once.
    ///
    /// The generator only reads the repository, so nothing has to be waited for or verified:
    /// its task is dropped with whatever it would still return, the draft stays as the user
    /// left it, and no error is shown for a stop they asked for. The cancellation ends the
    /// CLI behind it; a request already sent to an API runs out unheard.
    pub(crate) fn stop_generation(&mut self, target: &Target, cx: &mut Context<Self>) {
        let Some(cancellation) = self.generations.remove(target) else {
            return;
        };
        tracing::info!(path = ?target.path, "commit generation stopped by the user");
        cancellation.cancel();
        // The worktree was busy with the generation alone, so this is its task.
        self.operations.remove(target);
        self.busy.remove(target);
        if self.target.as_ref() == Some(target) {
            self.refresh(true, cx);
        }
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

#[cfg(test)]
mod tests {
    use crate::state::git::tests::{repository, source_control};
    use gpui_kit as gpui;
    use gpui_kit::TestAppContext;
    use hive_core::commit_message::Error;
    use hive_core::{host::Host, projects::Target};

    fn worktree(repo: &std::path::Path) -> Target {
        Target {
            host: Host::Local,
            project: repo.to_path_buf(),
            path: repo.to_path_buf(),
        }
    }

    #[gpui::test]
    fn a_stopped_generation_frees_the_worktree_and_leaves_the_draft_and_no_error(
        cx: &mut TestAppContext,
    ) {
        let repo = repository("hive-git-stop-generation");
        let target = worktree(&repo);
        let (_, git) = source_control(cx);
        // The empty project list settles first; its notification would clear the target.
        cx.run_until_parked();
        let cancellation = git.update(cx, |git, cx| {
            git.target = Some(target.clone());
            git.set_draft(target.clone(), "my own words".into(), cx);
            // What a generator answers when it is asked too late to notice the stop.
            git.generate(target.clone(), cx, |_| Ok("generated text".into()));
            assert!(git.is_generating() && git.is_busy());
            assert!(git.can_stop(), "a generation offers Stop");
            // The field is locked while the message is being written.
            git.set_draft(target.clone(), "typed meanwhile".into(), cx);
            assert_eq!(git.draft(&target), "my own words");
            let cancellation = git.generations.get(&target).cloned();

            git.stop(cx);
            assert!(!git.is_generating() && !git.is_running());
            assert!(!git.can_stop());
            cancellation.expect("a running generation")
        });
        assert!(cancellation.is_cancelled(), "the CLI behind it is ended");
        cx.run_until_parked();
        git.update(cx, |git, cx| {
            assert!(!git.is_busy());
            assert_eq!(
                git.error, None,
                "a stop the user asked for is not a failure"
            );
            assert_eq!(git.draft(&target), "my own words");
            assert!(!git.needs_verification());
            // The worktree takes the next generation at once.
            git.generate(target.clone(), cx, |_| Ok("second attempt".into()));
            assert!(git.is_generating());
        });
        cx.run_until_parked();
        git.read_with(cx, |git, _| {
            assert!(!git.is_generating() && !git.can_stop());
            assert_eq!(git.draft(&target), "second attempt");
        });
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[gpui::test]
    fn a_failed_generation_keeps_the_draft_and_says_why(cx: &mut TestAppContext) {
        let repo = repository("hive-git-failed-generation");
        let target = worktree(&repo);
        let (_, git) = source_control(cx);
        cx.run_until_parked();
        git.update(cx, |git, cx| {
            git.target = Some(target.clone());
            git.set_draft(target.clone(), "my own words".into(), cx);
            git.generate(target.clone(), cx, |_| {
                Err(Error::Invalid("The generator returned nothing."))
            });
        });
        cx.run_until_parked();
        git.read_with(cx, |git, _| {
            assert!(!git.is_busy());
            assert_eq!(git.draft(&target), "my own words");
            assert_eq!(
                git.error.as_deref(),
                Some("The generator returned nothing.")
            );
        });
        let _ = std::fs::remove_dir_all(&repo);
    }
}
