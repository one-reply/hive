//! Taking an accepted run's branches to the hosting service, one explicit request at a time.
use super::*;
use hive_core::git::pull_request::{self, Draft};

/// Where a lane's pull request stands while the app is open. A request opened earlier is
/// found again through the worktree's own PR card, so none of this is saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    Opening,
    /// The new request's web address.
    Opened(String),
    Failed(String),
}

impl Pipelines {
    pub(crate) fn delivery(&self, id: u64, lane: usize) -> Option<&Delivery> {
        self.runtime(id)?.deliveries.get(&lane)
    }

    /// Opens a pull or merge request for the branch in `lane`'s worktree. Hive never pushes
    /// for this: an unpublished branch comes back as the hosting CLI's own error.
    pub(crate) fn open_pull_request(
        &mut self,
        id: u64,
        lane: usize,
        draft: Draft,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let run = self.run(id, cx)?;
        if run.phase() != Phase::Accepted {
            return Err("Accept the changes before opening a pull request.".into());
        }
        let worktree = run
            .lanes()
            .get(lane)
            .map(|lane| lane.worktree.clone())
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or("This lane has no worktree.")?;
        if self.delivery(id, lane) == Some(&Delivery::Opening) {
            return Ok(());
        }
        self.runtime_mut(id)
            .deliveries
            .insert(lane, Delivery::Opening);
        let work = cx.background_executor().spawn(async move {
            pull_request::create(&Host::Local, &worktree, &draft).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pipelines, cx| {
                pipelines.finish_pull_request(id, lane, result, cx)
            });
        })
        .detach();
        cx.notify();
        Ok(())
    }

    pub(super) fn finish_pull_request(
        &mut self,
        id: u64,
        lane: usize,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        // A deleted run dropped its runtime; its late answer has nowhere to go.
        let Some(runtime) = self.runtime.get_mut(&id) else {
            return;
        };
        if let Err(error) = &result {
            tracing::warn!(run = id, lane, %error, "pull request was not opened");
        }
        runtime.deliveries.insert(
            lane,
            match result {
                Ok(url) => Delivery::Opened(url),
                Err(error) => Delivery::Failed(error),
            },
        );
        cx.notify();
    }
}
