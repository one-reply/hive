//! Keep the reporting PTY alive until its handoff reaches disk.
use super::*;

pub(super) type Reply = Box<dyn FnOnce(Result<(), String>)>;

impl Pipelines {
    pub(super) fn ensure_reports_saved(&self, id: u64) -> Result<(), String> {
        if self.runtime(id).is_some_and(|r| !r.reports.is_empty()) {
            return Err("The agent's report is waiting for its checkpoint.".into());
        }
        Ok(())
    }

    pub(crate) fn report(
        &mut self,
        from: u64,
        ticket: Ticket,
        report: Report,
        reply: impl FnOnce(Result<(), String>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.stopped
            || self
                .session(&ticket)
                .is_none_or(|s| s.entity_id().as_u64() != from)
        {
            reply(Err(
                "The report did not come from an active pipeline session.".into(),
            ));
            return;
        }
        let id = ticket.run;
        let saved = if self
            .runtime(id)
            .is_some_and(|r| r.reports.contains_key(&ticket))
        {
            // A failed write can be retried, but the same ticket must never change its report.
            let matches = self.run(id, cx).is_ok_and(|run| {
                run.reports()
                    .iter()
                    .any(|(t, _, r)| t == &ticket && r == &report)
            });
            if !matches {
                reply(Err(
                    "This attempt already submitted a different report.".into()
                ));
                return;
            }
            self.runtime(id)
                .and_then(|r| r.error.as_ref())
                .map(|_| self.store.read(cx).pipeline_checkpoint())
        } else {
            let failed = matches!(&report, Report::Review { findings, .. } if !findings.is_empty());
            match self.apply_change(id, cx, |run| {
                run.report(&ticket, report)?;
                run.finish_attempt_at(ticket, failed, now_ms());
                Ok(())
            }) {
                Ok(saved) => Some(saved),
                Err(error) => {
                    reply(Err(error));
                    return;
                }
            }
        };
        self.runtime_mut(id)
            .reports
            .entry(ticket)
            .or_default()
            .push(Box::new(reply));
        if let Some(saved) = saved {
            self.clear_error(id);
            self.schedule(id, saved, cx);
        }
    }

    /// The current generation's snapshot includes every report submitted before it.
    pub(super) fn settle_reports(
        &mut self,
        id: u64,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.runtime.get_mut(&id) else {
            return;
        };
        if runtime.reports.is_empty() {
            return;
        }
        let mut reports = std::mem::take(&mut runtime.reports);
        for (ticket, replies) in &mut reports {
            for reply in replies.drain(..) {
                reply(result.clone());
            }
            if result.is_ok() {
                self.session_errors.remove(ticket);
                self.stop_session(ticket, cx);
            }
        }
        if result.is_err() {
            self.runtime_mut(id).reports = reports;
        }
        cx.notify();
    }
}
