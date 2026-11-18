use super::*;

impl Pipelines {
    pub(super) fn clear_review(&mut self, id: u64) {
        self.reviews.remove(&id);
        self.patches.remove(&id);
        self.review_jobs.remove(&id);
        self.patch_jobs.remove(&id);
    }

    pub(crate) fn load_review(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.review_jobs.contains_key(&id) {
            return;
        }
        let Ok(run) = self.run(id, cx) else { return };
        let lanes = run.lanes.clone();
        self.clear_review(id);
        self.next_read = self.next_read.wrapping_add(1);
        let read = self.next_read;
        self.review_jobs.insert(id, read);
        let work = cx
            .background_executor()
            .spawn(async move { workspace::review(id, &lanes).map_err(|e| e.to_string()) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |p, cx| {
                if p.review_jobs.get(&id) == Some(&read) {
                    p.review_jobs.remove(&id);
                    p.reviews.insert(id, result);
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
            .reviews
            .get(&id)
            .and_then(|r| r.as_ref().ok())
            .and_then(|repos| repos.iter().find(|r| r.lane == lane))
            .and_then(|repo| repo.files.iter().find(|f| f.path == path))
            .cloned();
        let binding = self
            .run(id, cx)
            .ok()
            .and_then(|r| r.lanes.get(lane))
            .cloned();
        self.next_read = self.next_read.wrapping_add(1);
        let read = self.next_read;
        self.patch_jobs.insert(id, read);
        self.patches.remove(&id);
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
                if p.patch_jobs.get(&id) == Some(&read) {
                    p.patch_jobs.remove(&id);
                    p.patches.insert(id, (lane, path, result));
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    pub(crate) fn accepting(&self, id: u64) -> bool {
        self.accept_jobs.contains_key(&id)
    }

    /// Accept only the worktree contents that entered this QA pass. A manual edit returns
    /// all participating repositories to QA before another acceptance decision.
    pub(crate) fn accept(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        let run = self.run(id, cx)?;
        if run.phase != Phase::Acceptance {
            return Err("This run is not awaiting acceptance.".into());
        }
        if self.accepting(id) {
            return Ok(());
        }
        let lanes = run.lanes.clone();
        let generation = self.generations.get(&id).copied().unwrap_or(0);
        self.accept_jobs.insert(id, generation);
        self.errors.remove(&id);
        let stop = Arc::new(Cancellation::default());
        if let Some(previous) = self.workers.insert(id, stop.clone()) {
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
        if self.accept_jobs.get(&id) != Some(&generation) {
            return;
        }
        self.accept_jobs.remove(&id);
        cx.notify();
        if self.stopped || self.generations.get(&id).copied().unwrap_or(0) != generation {
            return;
        }
        self.workers.remove(&id);
        let result = revision.and_then(|revision| {
            self.edit(id, cx, |run| {
                if run.review_revision.as_ref() == Some(&revision) {
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
