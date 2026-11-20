use super::*;

impl Pipelines {
    pub(super) fn clear_review(&mut self, id: u64) {
        if let Some(runtime) = self.runtime.get_mut(&id) {
            runtime.review = None;
            runtime.patch = None;
            runtime.review_job = None;
            runtime.patch_job = None;
        }
    }

    pub(crate) fn load_review(&mut self, id: u64, since_feedback: bool, cx: &mut Context<Self>) {
        if self
            .runtime(id)
            .is_some_and(|r| r.review_job.is_some() && r.since_feedback == since_feedback)
        {
            return;
        }
        let Ok(run) = self.run(id, cx) else { return };
        let run = run.clone();
        self.clear_review(id);
        self.runtime_mut(id).since_feedback = since_feedback;
        let read = self.next_job();
        self.runtime_mut(id).review_job = Some(read);
        let work = cx.background_executor().spawn(async move {
            workspace::review_round(&run, since_feedback).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |p, cx| {
                // A cancelled run dropped its runtime; its late read is discarded.
                if let Some(runtime) = p.runtime.get_mut(&id)
                    && runtime.review_job == Some(read)
                {
                    runtime.review_job = None;
                    runtime.review = Some(result);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn load_patch(
        &mut self,
        id: u64,
        lane: usize,
        path: String,
        cx: &mut Context<Self>,
    ) {
        let file = self
            .review(id)
            .and_then(|r| r.as_ref().ok())
            .and_then(|repos| repos.iter().find(|r| r.lane == lane))
            .and_then(|repo| repo.files.iter().find(|f| f.path == path))
            .cloned();
        let binding = self
            .run(id, cx)
            .ok()
            .and_then(|r| r.lanes().get(lane))
            .cloned();
        let read = self.next_job();
        let runtime = self.runtime_mut(id);
        runtime.patch_job = Some(read);
        runtime.patch = None;
        let work = cx.background_executor().spawn(async move {
            let (lane, file) = binding
                .zip(file)
                .ok_or("The file list changed. Refresh the review.")?;
            let patch = workspace::patch(id, &lane, &file).map_err(|e| e.to_string())?;
            if patch
                .lines()
                .any(|line| line.starts_with("Binary files ") || line == "GIT binary patch")
            {
                return Err("Binary file changed. Text preview is unavailable.".into());
            }
            crate::state::git_diff::parse_patch(&patch)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |p, cx| {
                if let Some(runtime) = p.runtime.get_mut(&id)
                    && runtime.patch_job == Some(read)
                {
                    runtime.patch_job = None;
                    runtime.patch = Some((lane, path, result));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    pub(crate) fn edit_comments(
        &mut self,
        id: u64,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut Run) -> Result<(), hive_core::pipeline::Error>,
    ) -> Result<(), String> {
        self.store.update(cx, |s, cx| {
            s.try_edit_pipelines(cx, |lib| {
                let run = lib
                    .runs
                    .iter_mut()
                    .find(|r| r.id() == id)
                    .ok_or("Run unavailable")?;
                edit(run).map_err(|e| e.to_string())
            })
        })
    }

    pub(crate) fn send_comments(
        &mut self,
        id: u64,
        lane: usize,
        tree: String,
        batch: Vec<hive_core::git::review_notes::Note>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let Ok(run) = self.run(id, cx) else {
            return Task::ready(Err("Run is unavailable".into()));
        };
        let expected = run.review_revision().map(str::to_owned);
        let lanes = run.lanes().to_vec();
        let work = cx.background_executor().spawn(async move {
            workspace::revision(id, &lanes, &Cancellation::default()).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this,cx| {
            let actual = work.await?;
            this.update(cx,|p,cx| {
                if p.run(id,cx)?.review_revision() != expected.as_deref() {return Err("Outdated position. Review the current diff.".into());}
                if expected.as_deref() != Some(&actual) {
                    p.edit(id,cx,|r| Ok(r.recheck()?))?;
                    return Err("Outdated comment position. The files changed and QA is checking them again.".into());
                }
                p.edit(id,cx,|r| Ok(r.submit_comments(lane,&tree,&batch)?))
            }).map_err(|e|e.to_string())?
        })
    }

    pub(crate) fn accepting(&self, id: u64) -> bool {
        self.runtime(id).is_some_and(|r| r.accept_job.is_some())
    }

    /// Accept only the worktree contents that entered this QA pass. A manual edit returns
    /// all participating repositories to QA before another acceptance decision.
    pub(crate) fn accept(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        self.ensure_reports_saved(id)?;
        let run = self.run(id, cx)?;
        if run.phase() != Phase::Acceptance {
            return Err("This run is not awaiting acceptance.".into());
        }
        if self.accepting(id) {
            return Ok(());
        }
        let lanes = run.lanes().to_vec();
        let stop = Arc::new(Cancellation::default());
        let runtime = self.runtime_mut(id);
        let generation = runtime.generation;
        runtime.accept_job = Some(generation);
        runtime.error = None;
        if let Some(previous) = runtime.worker.replace(stop.clone()) {
            previous.cancel();
        }

        let work = cx.background_executor().spawn(async move {
            workspace::revision(id, &lanes, &stop).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let revision = work.await;
            let _ = this.update(cx, |p, cx| {
                p.finish_acceptance(id, generation, revision, cx)
            });
        })
        .detach();
        cx.notify();
        Ok(())
    }

    pub(super) fn finish_acceptance(
        &mut self,
        id: u64,
        generation: u64,
        revision: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.runtime.get_mut(&id) else {
            return;
        };
        if runtime.accept_job != Some(generation) {
            return;
        }
        runtime.accept_job = None;
        cx.notify();
        if self.stopped || runtime.generation != generation {
            return;
        }
        runtime.worker = None;
        let result = revision.and_then(|revision| {
            self.edit(id, cx, |run| {
                if run.review_revision() == Some(revision.as_str()) {
                    Ok(run.accept()?)
                } else {
                    Ok(run.recheck()?)
                }
            })
        });
        if let Err(error) = result {
            self.fail(id, error, cx);
        }
    }
}
