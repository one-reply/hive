//! Tells the user when a stage's agent has been at work for longer than a stage usually takes.
//! Nothing is stopped: a long stage may be legitimate, a hung one is the user's call.
use super::*;
use crate::state::terminal_session::SessionStatus;
use hive_core::pipeline::Stage;
use std::time::Duration;

/// How long one agent attempt may run before the user hears about it, once.
const ATTEMPT_BUDGET: Duration = Duration::from_secs(30 * 60);
/// How often running attempts are compared with the budget.
const WATCH_INTERVAL: Duration = Duration::from_secs(60);

impl Pipelines {
    /// Checks the running attempts once a minute for as long as the entity lives.
    pub(super) fn watch(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WATCH_INTERVAL).await;
                let alive = this.update(cx, |pipelines, cx| {
                    pipelines.announce_slow_attempts(now_ms(), cx)
                });
                if alive.is_err() {
                    return;
                }
            }
        })
    }

    /// Announces, once per attempt, each live agent that is past the budget and is not
    /// already waiting for the user (that request has its own notification).
    pub(super) fn announce_slow_attempts(&mut self, now: u64, cx: &mut Context<Self>) {
        self.overdue.retain(|ticket| self.live.contains_key(ticket));
        if self.live.is_empty() || self.stopped {
            return;
        }
        let budget = u64::try_from(ATTEMPT_BUDGET.as_millis()).unwrap_or(u64::MAX);
        let mut slow = Vec::new();
        for run in &self.store.read(cx).state.pipelines.runs {
            for ticket in run.attempts_running_over(budget, now) {
                let working = self.live.get(&ticket).is_some_and(|live| {
                    let session = live.session.read(cx);
                    !session.exited && !matches!(session.status, SessionStatus::Failed(_))
                });
                if !working
                    || self.overdue.contains(&ticket)
                    || self.session_errors.contains_key(&ticket)
                    || self.agent_waits(&ticket, cx)
                {
                    continue;
                }
                let Some(stage) = Stage::from_phase(run.ticket_phase(&ticket), ticket.lane) else {
                    continue;
                };
                slow.push((ticket, stage.title(run)));
            }
        }
        for (ticket, stage) in slow {
            self.overdue.insert(ticket);
            cx.emit(RunWaits {
                run: ticket.run,
                reason: Waiting::Slow {
                    stage,
                    minutes: ATTEMPT_BUDGET.as_secs() / 60,
                },
            });
        }
    }
}
