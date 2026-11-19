//! Which saved runs the library keeps.
use super::*;

impl Pipelines {
    /// Removes a finished run from the library. Its worktrees and branches stay where they
    /// are: the run is a record of the work, not the work.
    pub(crate) fn delete_run(&mut self, id: u64, cx: &mut Context<Self>) -> Result<(), String> {
        let run = self.run(id, cx)?;
        if !matches!(run.phase(), Phase::Accepted | Phase::Cancelled) {
            return Err("Stop the run before deleting it.".into());
        }
        self.store.update(cx, |store, cx| {
            store.edit_pipelines(cx, |library| library.runs.retain(|run| run.id() != id))
        });
        self.suspended.retain(|ticket| ticket.run != id);
        self.session_errors.retain(|ticket, _| ticket.run != id);
        self.resumes.retain(|ticket, _| ticket.run != id);
        self.drop_runtime(id);
        cx.notify();
        Ok(())
    }
}
