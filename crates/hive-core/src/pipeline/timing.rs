//! Observed stage and attempt boundaries; a crash has no trustworthy end timestamp.
use super::{Phase, Run, StageTiming, Ticket};
use std::time::{SystemTime, UNIX_EPOCH};

/// Current wall-clock time in Unix milliseconds, the unit [`StageTiming`] stores.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn executing(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::Planning | Phase::Development | Phase::Review(_) | Phase::Commands(_)
    )
}

fn open(row: &StageTiming) -> bool {
    row.end_ms.is_none() && !row.failed
}

impl Run {
    /// Closes the previous stage interval and opens one for the current phase. `previous`
    /// decides whether the closed interval failed (findings or a red check).
    pub fn record_stage(&mut self, previous: Option<Phase>, at: u64) {
        if self
            .timings
            .iter()
            .any(|row| row.phase == self.phase && row.attempt.is_none() && open(row))
        {
            return;
        }
        let cancelled = self.phase == Phase::Cancelled;
        let failed = cancelled
            || match previous {
                Some(Phase::Review(_)) => !self.findings.is_empty(),
                Some(Phase::Commands(_)) => self.commands.iter().any(|c| !c.success),
                _ => false,
            };
        for row in self
            .timings
            .iter_mut()
            .filter(|row| open(row) && (row.attempt.is_none() || cancelled))
        {
            row.end_ms = Some(at.max(row.start_ms));
            row.failed = failed;
        }
        let terminal = matches!(self.phase, Phase::Accepted | Phase::Cancelled);
        self.timings.push(StageTiming {
            phase: self.phase,
            lane: None,
            attempt: None,
            start_ms: at,
            end_ms: terminal.then_some(at),
            failed: cancelled,
        });
    }

    /// Whether the attempt's interval is still running.
    pub fn has_open_attempt(&self, ticket: &Ticket) -> bool {
        self.timings
            .iter()
            .any(|row| row.attempt == Some(ticket.attempt) && open(row))
    }

    /// Starts timing a launched agent session, once per attempt.
    pub fn start_attempt_at(&mut self, ticket: Ticket, at: u64) {
        if !self.has_open_attempt(&ticket) {
            self.timings.push(StageTiming {
                phase: self.phase,
                lane: ticket.lane,
                attempt: Some(ticket.attempt),
                start_ms: at,
                end_ms: None,
                failed: false,
            });
        }
    }

    /// Ends the attempt's open interval; a clock moved backwards cannot reverse it.
    pub fn finish_attempt_at(&mut self, ticket: Ticket, failed: bool, at: u64) {
        if let Some(row) = self
            .timings
            .iter_mut()
            .rev()
            .find(|r| r.attempt == Some(ticket.attempt) && open(r))
        {
            row.end_ms = Some(at.max(row.start_ms));
            row.failed = failed;
        }
    }

    /// Pending attempts whose session has been running for at least `budget_ms`.
    pub fn attempts_running_over(&self, budget_ms: u64, now: u64) -> Vec<Ticket> {
        self.pending()
            .into_iter()
            .filter(|ticket| {
                self.timings.iter().any(|row| {
                    row.attempt == Some(ticket.attempt)
                        && open(row)
                        && now.saturating_sub(row.start_ms) >= budget_ms
                })
            })
            .collect()
    }

    /// Whether agent work or a check is being timed, as opposed to a human gate.
    pub fn is_interruptible(&self) -> bool {
        executing(self.phase) && self.timings.iter().any(open)
    }

    /// Marks running work as interrupted; `None` when the end time is unknown (a crash).
    pub fn interrupt_timings(&mut self, at: Option<u64>) {
        if executing(self.phase) {
            for row in self.timings.iter_mut().filter(|r| open(r)) {
                row.end_ms = at.map(|t| t.max(row.start_ms));
                row.failed = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{Lane, Template};

    #[test]
    fn parallel_attempts_have_independent_intervals_and_crashes_keep_unknown_end_times() {
        let mut run = Run::new(
            1,
            "/api".into(),
            "Task".into(),
            Template::default(),
            vec![
                Lane {
                    name: "API".into(),
                    project: "/api".into(),
                    ..Default::default()
                },
                Lane {
                    name: "Web".into(),
                    project: "/web".into(),
                    ..Default::default()
                },
            ],
        )
        .unwrap();
        run.phase = Phase::Development;
        run.record_stage(None, 10);
        let a = Ticket {
            run: 1,
            attempt: 2,
            lane: Some(0),
        };
        let b = Ticket {
            run: 1,
            attempt: 3,
            lane: Some(1),
        };
        run.start_attempt_at(a, 11);
        run.start_attempt_at(b, 12);
        run.outstanding = vec![a, b];
        // Both attempts are pending, but only one has been running long enough to mention.
        assert_eq!(run.attempts_running_over(10, 21), [a]);
        assert_eq!(run.attempts_running_over(10, 22), [a, b]);
        assert!(run.attempts_running_over(10, 12).is_empty());
        run.finish_attempt_at(a, false, 25);
        assert_eq!(run.attempts_running_over(10, 100), [b]);
        assert_eq!(run.timings[1].end_ms, Some(25));
        assert_eq!(run.timings[2].end_ms, None);
        run.interrupt_timings(None);
        assert!(run.timings[2].failed);
        assert_eq!(run.timings[2].end_ms, None);
        run.record_stage(None, 40);
        run.start_attempt_at(b, 42);
        run.finish_attempt_at(b, false, 50);
        run.phase = Phase::Review(0);
        run.record_stage(Some(Phase::Development), 51);
        assert_eq!(run.timings[3].end_ms, Some(51));
        assert_eq!(run.timings[4].start_ms, 42);
        assert_eq!(run.timings[4].end_ms, Some(50));
        assert_eq!(run.timings.len(), 6);
    }

    #[test]
    fn waiting_gates_keep_elapsed_time_across_relaunch_and_clock_changes_cannot_reverse_intervals()
    {
        let mut run = Run::new(
            1,
            "/api".into(),
            "Task".into(),
            Template::default(),
            vec![Lane {
                name: "API".into(),
                project: "/api".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        run.phase = Phase::ApprovePlan;
        run.record_stage(None, 100);
        assert!(!run.is_interruptible());
        run.interrupt_timings(Some(120));
        assert_eq!(run.timings[0].end_ms, None);
        run.phase = Phase::Development;
        run.record_stage(Some(Phase::ApprovePlan), 90);
        assert_eq!(run.timings[0].end_ms, Some(100));
        run.interrupt_timings(Some(95));
        assert_eq!(run.timings[1].end_ms, Some(95));
        assert!(run.timings[1].failed);
    }
}

#[cfg(test)]
mod check_outcomes {
    use super::*;
    use crate::pipeline::{CommandResult, Finding, Lane, Template};
    #[test]
    fn passing_commands_stay_green_when_a_reviewer_requests_changes() {
        let mut run = Run::new(
            1,
            "/api".into(),
            "Fix".into(),
            Template::default(),
            vec![Lane {
                name: "API".into(),
                project: "/api".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        run.phase = Phase::Commands(0);
        run.record_stage(None, 0);
        run.findings.push(Finding {
            lane: 0,
            severity: Default::default(),
            description: "Review finding".into(),
        });
        run.commands.push(CommandResult {
            lane: 0,
            command: "npm test".into(),
            success: true,
            exit_code: Some(0),
            output: String::new(),
        });
        run.phase = Phase::Development;
        run.record_stage(Some(Phase::Commands(0)), 1);
        assert!(!run.timings[0].failed);
    }
}
