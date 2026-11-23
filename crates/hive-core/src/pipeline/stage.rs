//! A run shown as the recipe's stages, with each stage's observed intervals and status.
use super::{Phase, Run, StageTiming};

/// One row of a run's recipe: an agent step, a human gate or the required checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// The planner agent.
    Planner,
    /// The user approves or edits the plan.
    Approval,
    /// Independent regression workflow.
    Regression,
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

/// How far a run is through its recipe, for a summary that has no room for the timeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunProgress {
    /// Stages that finished.
    pub done: usize,
    /// Stages the run has work in.
    pub total: usize,
}

impl Run {
    /// Finished stages out of the recipe's stages. A lane the plan gave no work is left out,
    /// and a decision counts only once the run has reached it, so the total can grow.
    pub fn progress(&self) -> RunProgress {
        let mut progress = RunProgress::default();
        for stage in Stage::recipe(self) {
            match stage.status(self, &stage.observations(self)) {
                StageStatus::Unassigned => continue,
                StageStatus::Complete => progress.done += 1,
                _ => {}
            }
            progress.total += 1;
        }
        progress
    }
}

impl Stage {
    /// Stages in recipe order; a decision appears only once the run has reached it.
    pub fn recipe(run: &Run) -> Vec<Self> {
        let mut stages = if run.direct_start() {
            vec![]
        } else {
            vec![Self::Planner, Self::Approval]
        };
        if run.regression().is_some() {
            stages.push(Self::Regression);
        }
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
            Phase::Regression(_) => Self::Regression,
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
            Self::Regression => "Independent regression test".into(),
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
                Self::Regression if matches!(run.phase,Phase::Regression(step) if step.needs_user()) => {
                    StageStatus::NeedsYou
                }
                Self::Decision(_) | Self::Accept => StageStatus::NeedsYou,
                _ => StageStatus::InProgress,
            };
        }
        if !records.is_empty() {
            return StageStatus::Complete;
        }
        if let Self::Develop(i) = self
            && run.lanes.get(i).is_some_and(|l| !l.is_assigned())
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
    use super::{RunProgress, Stage, StageStatus};
    use crate::pipeline::{Assignment, Lane, Phase, Report, Run, StageTiming, Template};

    fn lane(name: &str, project: &str) -> Lane {
        Lane {
            name: name.into(),
            project: project.into(),
            command: "cargo test".into(),
            ..Default::default()
        }
    }

    fn sample() -> Run {
        Run::new(
            1,
            "/api".into(),
            "Task".into(),
            Template::default(),
            vec![lane("API", "/api")],
        )
        .unwrap()
    }

    /// Applies a transition and records its stage interval, as the application does.
    fn advance(run: &mut Run, at: u64, change: impl FnOnce(&mut Run)) {
        let previous = run.phase;
        change(run);
        run.record_stage(Some(previous), at);
    }

    #[test]
    fn a_run_that_just_started_has_finished_none_of_its_stages() {
        let mut run = sample();
        run.record_stage(None, 1);
        assert_eq!(run.progress(), RunProgress { done: 0, total: 7 });
    }

    #[test]
    fn progress_counts_finished_stages_and_leaves_out_a_lane_without_work() {
        let mut run = Run::new(
            1,
            "/api".into(),
            "Task".into(),
            Template::default(),
            vec![lane("API", "/api"), lane("Web", "/web")],
        )
        .unwrap();
        run.record_stage(None, 1);
        advance(&mut run, 2, |run| {
            let planner = run.pending()[0];
            let plan = Report::Plan {
                assignments: vec![Assignment {
                    lane: 0,
                    task: "Add the endpoint".into(),
                    criteria: "Tests pass".into(),
                }],
                contract: String::new(),
            };
            run.report(&planner, plan).unwrap();
        });
        advance(&mut run, 3, |run| run.approve().unwrap());
        // Planner and approval are done; Web has no task, so six stages remain to count.
        assert_eq!(run.progress(), RunProgress { done: 2, total: 7 });
        assert_eq!(Stage::Develop(1).status(&run, &[]), StageStatus::Unassigned);
    }

    #[test]
    fn an_accepted_run_has_finished_every_stage_it_had_work_in() {
        let mut run = sample();
        run.lanes[0].assignment = "Add the endpoint".into();
        run.phase = Phase::Accepted;
        assert_eq!(run.progress(), RunProgress { done: 7, total: 7 });
    }

    #[test]
    fn a_cancelled_run_keeps_the_stages_it_never_finished() {
        let mut run = sample();
        run.record_stage(None, 1);
        advance(&mut run, 2, |run| run.cancel().unwrap());
        let progress = run.progress();
        assert_eq!(progress.done, 0);
        assert!(progress.total > 0);
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
