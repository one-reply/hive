//! What Hive holds for one run while the app is open. None of it is saved, so cancelling a run
//! drops all of it in one step instead of one cleanup per field.
use super::*;
use hive_core::terminal::TerminalControl;

#[derive(Default)]
pub(super) struct RunRuntime {
    pub(super) reports: HashMap<Ticket, Vec<super::reports::Reply>>,
    /// Stamp of the scheduled step; results carrying an older stamp are dropped.
    pub(super) generation: u64,
    /// Why the run stopped and waits for the user.
    pub(super) error: Option<String>,
    /// Cancels the background step in flight.
    pub(super) worker: Option<Arc<Cancellation>>,
    /// Terminals of reported attempts that are still shutting down.
    pub(super) closing: Vec<TerminalControl>,
    pub(super) review: Option<Result<Vec<ReviewRepo>, String>>,
    pub(super) patch: Option<PatchPreview>,
    pub(super) review_job: Option<u64>,
    pub(super) patch_job: Option<u64>,
    /// The generation an acceptance check was started for.
    pub(super) accept_job: Option<u64>,
}

impl Pipelines {
    pub(super) fn runtime(&self, id: u64) -> Option<&RunRuntime> {
        self.runtime.get(&id)
    }

    pub(super) fn runtime_mut(&mut self, id: u64) -> &mut RunRuntime {
        self.runtime.entry(id).or_default()
    }

    /// A fresh stamp for a step or read; unique for the app's lifetime, so a result from
    /// before a cancellation can never match a later step.
    pub(super) fn next_job(&mut self) -> u64 {
        self.next_job = self.next_job.wrapping_add(1);
        self.next_job
    }

    /// The changed files of the run's worktrees, once read.
    pub(crate) fn review(&self, id: u64) -> Option<&Result<Vec<ReviewRepo>, String>> {
        self.runtime(id)?.review.as_ref()
    }

    /// The diff of the file selected in the run's change list, once read.
    pub(crate) fn patch(&self, id: u64) -> Option<&PatchPreview> {
        self.runtime(id)?.patch.as_ref()
    }

    pub(super) fn clear_error(&mut self, id: u64) {
        if let Some(runtime) = self.runtime.get_mut(&id) {
            runtime.error = None;
        }
    }

    /// Forgets everything held for a run and stops its background step.
    pub(super) fn drop_runtime(&mut self, id: u64) {
        if let Some(worker) = self.runtime.remove(&id).and_then(|r| r.worker) {
            worker.cancel();
        }
    }
}
