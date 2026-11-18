//! Record observed boundaries; a crash has no trustworthy end timestamp.
use hive_core::pipeline::{Phase, Run, StageTiming, Ticket};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn now() -> u64 {
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

pub(super) fn stage(run: &mut Run, previous: Option<Phase>, at: u64) {
    if run
        .timings
        .iter()
        .any(|row| row.phase == run.phase && row.attempt.is_none() && open(row))
    {
        return;
    }
    let cancelled = run.phase == Phase::Cancelled;
    for row in run
        .timings
        .iter_mut()
        .filter(|row| open(row) && (row.attempt.is_none() || cancelled))
    {
        let failed = cancelled
            || match previous {
                Some(Phase::Review(_)) => !run.findings.is_empty(),
                Some(Phase::Commands(_)) => run.commands.iter().any(|c| !c.success),
                _ => false,
            };
        row.end_ms = Some(at.max(row.start_ms));
        row.failed = failed;
    }
    let terminal = matches!(run.phase, Phase::Accepted | Phase::Cancelled);
    run.timings.push(StageTiming {
        phase: run.phase,
        lane: None,
        attempt: None,
        start_ms: at,
        end_ms: terminal.then_some(at),
        failed: cancelled,
    });
}

pub(super) fn attempt_open(run: &Run, ticket: Ticket) -> bool {
    run.timings
        .iter()
        .any(|row| row.attempt == Some(ticket.attempt) && open(row))
}

pub(super) fn start_attempt(run: &mut Run, ticket: Ticket, at: u64) {
    if !attempt_open(run, ticket) {
        run.timings.push(StageTiming {
            phase: run.phase,
            lane: ticket.lane,
            attempt: Some(ticket.attempt),
            start_ms: at,
            end_ms: None,
            failed: false,
        });
    }
}

pub(super) fn finish_attempt(run: &mut Run, ticket: Ticket, failed: bool, at: u64) {
    if let Some(row) = run
        .timings
        .iter_mut()
        .rev()
        .find(|r| r.attempt == Some(ticket.attempt) && open(r))
    {
        row.end_ms = Some(at.max(row.start_ms));
        row.failed = failed;
    }
}

pub(super) fn interruptible(run: &Run) -> bool {
    executing(run.phase) && run.timings.iter().any(open)
}

pub(super) fn interrupt(run: &mut Run, at: Option<u64>) {
    if executing(run.phase) {
        for row in run.timings.iter_mut().filter(|r| open(r)) {
            row.end_ms = at.map(|t| t.max(row.start_ms));
            row.failed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::pipeline::{Lane, Template};

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
        stage(&mut run, None, 10);
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
        start_attempt(&mut run, a, 11);
        start_attempt(&mut run, b, 12);
        finish_attempt(&mut run, a, false, 25);
        assert_eq!(run.timings[1].end_ms, Some(25));
        assert_eq!(run.timings[2].end_ms, None);
        interrupt(&mut run, None);
        assert!(run.timings[2].failed);
        assert_eq!(run.timings[2].end_ms, None);
        stage(&mut run, None, 40);
        start_attempt(&mut run, b, 42);
        finish_attempt(&mut run, b, false, 50);
        run.phase = Phase::Review(0);
        stage(&mut run, Some(Phase::Development), 51);
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
        stage(&mut run, None, 100);
        assert!(!interruptible(&run));
        interrupt(&mut run, Some(120));
        assert_eq!(run.timings[0].end_ms, None);
        run.phase = Phase::Development;
        stage(&mut run, Some(Phase::ApprovePlan), 90);
        assert_eq!(run.timings[0].end_ms, Some(100));
        interrupt(&mut run, Some(95));
        assert_eq!(run.timings[1].end_ms, Some(95));
        assert!(run.timings[1].failed);
    }
}

#[cfg(test)]
mod check_outcomes {
    use super::*;
    use hive_core::pipeline::{CommandResult, Finding, Lane, Template};
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
        stage(&mut run, None, 0);
        run.findings.push(Finding {
            lane: 0,
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
        stage(&mut run, Some(Phase::Commands(0)), 1);
        assert!(!run.timings[0].failed);
    }
}
