//! Display stages in recipe order and group repeated, authenticated stage invocations.
use super::super::map::RoleKind;
use super::*;
use hive_core::pipeline::StageTiming;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Planner,
    Approval,
    Develop(usize),
    Review(usize),
    Commands(usize),
    Decision(usize),
    Accept,
}

impl Stage {
    pub(super) fn recipe(run: &Run) -> Vec<Self> {
        let mut stages = vec![Self::Planner, Self::Approval];
        stages.extend((0..run.lanes.len()).map(Self::Develop));
        for i in 0..run.template.reviews.len() {
            stages.push(Self::Review(i));
            if i == 0 && run.lanes.iter().any(|lane| !lane.command.trim().is_empty()) {
                stages.push(Self::Commands(i));
            }
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
    pub(super) fn from_phase(phase: Phase, lane: Option<usize>) -> Option<Self> {
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
    pub(super) fn title(self, r: &Run) -> String {
        match self {
            Self::Planner => r.template.planner.name.clone(),
            Self::Approval => "Plan approval".into(),
            Self::Develop(i) => r.lanes[i].name.clone(),
            Self::Review(i) => r.template.reviews[i].name.clone(),
            Self::Commands(_) => "Required checks".into(),
            Self::Decision(_) => "Repair decision".into(),
            Self::Accept => "Acceptance".into(),
        }
    }
    pub(super) fn icon(self, run: &Run) -> Lucide {
        match self {
            Self::Planner => RoleKind::Plan.icon(),
            Self::Approval | Self::Decision(_) => RoleKind::Human.icon(),
            Self::Develop(_) => RoleKind::Build.icon(),
            Self::Review(i) => RoleKind::review(i, &run.template.reviews[i]).icon(),
            Self::Commands(_) => Lucide::Terminal,
            Self::Accept => Lucide::ClipboardCheck,
        }
    }
    pub(super) fn human(self) -> bool {
        matches!(self, Self::Approval | Self::Decision(_) | Self::Accept)
    }
    pub(super) fn active(self, r: &Run) -> bool {
        if let Self::Develop(i) = self {
            return r.phase == Phase::Development && r.pending().iter().any(|t| t.lane == Some(i));
        }
        Some(self) == Self::from_phase(r.phase, None)
    }
    pub(super) fn observations(self, r: &Run) -> Vec<&StageTiming> {
        let rows = r
            .timings
            .iter()
            .filter(|t| Self::from_phase(t.phase, t.lane) == Some(self));
        // Plan and review each have an aggregate stage clock and a more precise PTY clock.
        let use_attempt = matches!(self, Self::Planner | Self::Review(_))
            && rows.clone().any(|t| t.attempt.is_some());
        rows.filter(|t| !use_attempt || t.attempt.is_some())
            .collect()
    }
    pub(super) fn state(self, run: &Run, records: &[&StageTiming]) -> &'static str {
        if records
            .last()
            .is_some_and(|t| t.failed && t.end_ms.is_none())
        {
            return "Interrupted";
        }
        if records.last().is_some_and(|t| t.failed) {
            return "Failed";
        }
        if self.active(run) {
            return if self.human() {
                "Needs you"
            } else {
                "In progress"
            };
        }
        if !records.is_empty() {
            return "Complete";
        }
        if let Self::Develop(i) = self
            && run.lanes[i].assignment.is_empty()
            && !matches!(run.phase, Phase::Planning | Phase::ApprovePlan)
        {
            return "Unassigned";
        }
        if run.phase == Phase::Accepted {
            return "Complete";
        }
        if run.phase == Phase::Cancelled {
            return "Stopped";
        }
        if self == Self::Planner && run.phase != Phase::Planning {
            return "Complete";
        }
        "Pending"
    }
}

#[cfg(test)]
mod tests {
    use super::Stage;
    use hive_core::pipeline::{Lane, Phase, Run, StageTiming, Template};

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
                Stage::Review(0),
                Stage::Commands(0),
                Stage::Review(1),
                Stage::Accept
            ]
        );
        run.phase = Phase::Decision(1);
        assert!(Stage::recipe(&run).contains(&Stage::Decision(1)));
        assert_eq!(Stage::Decision(1).state(&run, &[]), "Needs you");
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
        assert_eq!(Stage::Planner.state(&run, &rows), "Failed");
    }
}
