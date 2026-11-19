//! A run shown as the recipe's stages, with each stage's observed intervals and status.
use super::{Phase, Run, StageTiming};

/// One row of a run's recipe: an agent step, a human gate or the required checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// The planner agent.
    Planner,
    /// The user approves or edits the plan.
    Approval,
    /// Development in one lane.
    Develop(usize),
    /// A QA review agent.
    Review(usize),
    /// Required checks, run before the first review.
    Commands(usize),
    /// The user decides on findings that exhausted the repair budget.
    Decision(usize),
    /// The user accepts the result.
    Accept,
}

/// Where a stage stands, derived from the run's phase and the stage's timings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageStatus {
    /// The last attempt stopped without an end time (Hive exited).
    Interrupted,
    /// The last attempt ended unsuccessfully.
    Failed,
    /// The plan waits for the user.
    AwaitingApproval,
    /// A decision or acceptance waits for the user.
    NeedsYou,
    /// An agent or check runs now.
    InProgress,
    /// The stage finished.
    Complete,
    /// The plan gave this lane no work.
    Unassigned,
    /// The run was cancelled before this stage.
    Stopped,
    /// The stage has not started.
    Pending,
}

impl StageStatus {
    /// User-facing status text.
    pub fn label(self) -> &'static str {
        match self {
            Self::Interrupted => "Interrupted",
            Self::Failed => "Failed",
            Self::AwaitingApproval => "Awaiting your approval",
            Self::NeedsYou => "Needs you",
            Self::InProgress => "In progress",
            Self::Complete => "Complete",
            Self::Unassigned => "Unassigned",
            Self::Stopped => "Stopped",
            Self::Pending => "Pending",
        }
    }

    /// Whether the stage's last attempt did not succeed.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Interrupted | Self::Failed)
    }
}

impl std::fmt::Display for StageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl Stage {
    /// Stages in recipe order; a decision appears only once the run has reached it.
    pub fn recipe(run: &Run) -> Vec<Self> {
        let mut stages = vec![Self::Planner, Self::Approval];
        stages.extend((0..run.lanes.len()).map(Self::Develop));
        if run.lanes.iter().any(|lane| run.shows_check(lane)) {
            stages.push(Self::Commands(0));
        }
        for i in 0..run.template.reviews.len() {
            stages.push(Self::Review(i));
            if run.phase == Phase::Decision(i)
                || run
                    .timings
                    .iter()
                    .any(|row| row.phase == Phase::Decision(i))
            {
                stages.push(Self::Decision(i));
            }
        }
        stages.push(Self::Accept);
        stages
    }

    /// The stage a phase belongs to; development needs the lane.
    pub fn from_phase(phase: Phase, lane: Option<usize>) -> Option<Self> {
        Some(match phase {
            Phase::Planning => Self::Planner,
            Phase::ApprovePlan => Self::Approval,
            Phase::Development => Self::Develop(lane?),
            Phase::Review(i) => Self::Review(i),
            Phase::Commands(i) => Self::Commands(i),
            Phase::Decision(i) => Self::Decision(i),
            Phase::Acceptance => Self::Accept,
            Phase::Accepted | Phase::Cancelled => return None,
        })
    }

    /// Name from the frozen recipe, or the gate's fixed name.
    pub fn title(self, run: &Run) -> String {
        match self {
            Self::Planner => run.template.planner.name.clone(),
            Self::Approval => "Plan approval".into(),
            Self::Develop(i) => run.lanes.get(i).map(|l| l.name.clone()).unwrap_or_default(),
            Self::Review(i) => run
                .template
                .reviews
                .get(i)
                .map(|r| r.name.clone())
                .unwrap_or_default(),
            Self::Commands(_) => "Required checks".into(),
            Self::Decision(_) => "Repair decision".into(),
            Self::Accept => "Acceptance".into(),
        }
    }

    /// Whether the user, not an agent, completes this stage.
    pub fn is_human(self) -> bool {
        matches!(self, Self::Approval | Self::Decision(_) | Self::Accept)
    }

    /// Whether the run is at this stage now.
    pub fn is_active(self, run: &Run) -> bool {
        if let Self::Develop(i) = self {
            return run.phase == Phase::Development
                && run.pending().iter().any(|t| t.lane == Some(i));
        }
        Some(self) == Self::from_phase(run.phase, None)
    }

    /// Measured intervals of this stage, oldest first.
    pub fn observations(self, run: &Run) -> Vec<&StageTiming> {
        let rows = run
            .timings
            .iter()
            .filter(|t| Self::from_phase(t.phase, t.lane) == Some(self));
        // Plan and review each have an aggregate stage clock and a more precise PTY clock.
        let use_attempt = matches!(self, Self::Planner | Self::Review(_))
            && rows.clone().any(|t| t.attempt.is_some());
        rows.filter(|t| !use_attempt || t.attempt.is_some())
            .collect()
    }

    /// Status from the run and this stage's `records` (its [`Self::observations`]).
    pub fn status(self, run: &Run, records: &[&StageTiming]) -> StageStatus {
        match records.last() {
            Some(t) if t.failed && t.end_ms.is_none() => return StageStatus::Interrupted,
            Some(t) if t.failed => return StageStatus::Failed,
            _ => {}
        }
        if self.is_active(run) {
            return match self {
                Self::Approval => StageStatus::AwaitingApproval,
                Self::Decision(_) | Self::Accept => StageStatus::NeedsYou,
                _ => StageStatus::InProgress,
            };
        }
        if !records.is_empty() {
            return StageStatus::Complete;
        }
        if let Self::Develop(i) = self
            && run.lanes.get(i).is_some_and(|l| l.assignment.is_empty())
            && !matches!(run.phase, Phase::Planning | Phase::ApprovePlan)
        {
            return StageStatus::Unassigned;
        }
        match run.phase {
            Phase::Accepted => StageStatus::Complete,
            Phase::Cancelled => StageStatus::Stopped,
            phase if self == Self::Planner && phase != Phase::Planning => StageStatus::Complete,
            _ => StageStatus::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Stage, StageStatus};
    use crate::pipeline::{Lane, Phase, Run, StageTiming, Template};

    fn sample() -> Run {
        Run::new(
            1,
            "/api".into(),
            "Task".into(),
            Template::default(),
            vec![Lane {
                name: "API".into(),
                project: "/api".into(),
                command: "cargo test".into(),
                ..Default::default()
            }],
        )
        .unwrap()
    }
    #[test]
    fn generated_rows_follow_the_recipe_and_only_include_decisions_actually_reached() {
        let mut run = sample();
        assert_eq!(
            Stage::recipe(&run),
            vec![
                Stage::Planner,
                Stage::Approval,
                Stage::Develop(0),
                Stage::Commands(0),
                Stage::Review(0),
                Stage::Review(1),
                Stage::Accept
            ]
        );
        run.phase = Phase::Decision(1);
        assert!(Stage::recipe(&run).contains(&Stage::Decision(1)));
        assert_eq!(Stage::Decision(1).status(&run, &[]), StageStatus::NeedsYou);
    }
    #[test]
    fn approval_is_requested_only_after_planning_and_until_development_starts() {
        let mut run = sample();
        assert_eq!(Stage::Approval.status(&run, &[]), StageStatus::Pending);
        run.phase = Phase::ApprovePlan;
        assert_eq!(
            Stage::Approval.status(&run, &[]),
            StageStatus::AwaitingApproval
        );
        let approval = StageTiming {
            phase: Phase::ApprovePlan,
            lane: None,
            attempt: None,
            start_ms: 1,
            end_ms: Some(2),
            failed: false,
        };
        run.phase = Phase::Development;
        assert_eq!(
            Stage::Approval.status(&run, &[&approval]),
            StageStatus::Complete
        );
    }
    #[test]
    fn plan_uses_its_actual_pty_interval_and_a_failed_attempt_takes_precedence_over_the_current_phase()
     {
        let mut run = sample();
        let mut timing = StageTiming {
            phase: Phase::Planning,
            lane: None,
            attempt: None,
            start_ms: 100,
            end_ms: None,
            failed: false,
        };
        run.timings.push(timing.clone());
        timing.attempt = Some(1);
        timing.start_ms = 200;
        timing.end_ms = Some(250);
        timing.failed = true;
        run.timings.push(timing);
        let rows = Stage::Planner.observations(&run);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].start_ms, 200);
        assert_eq!(Stage::Planner.status(&run, &rows), StageStatus::Failed);
    }
}
