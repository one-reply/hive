//! One answer to "what is this run doing?" for every place that summarizes a run.
use super::*;

/// The state of a run as a person scanning a list of runs needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunStatus {
    /// An agent or Hive's own checks are working; nothing is asked of the user.
    Running,
    /// A gate or an agent is waiting on the user.
    NeedsYou,
    /// A step stopped with an error and needs a retry or a resume.
    Failed,
    Completed,
    Cancelled,
}

impl RunStatus {
    /// A finished run keeps its outcome even if a stale error is still recorded for it, and a
    /// failure outranks a waiting gate: the gate cannot be passed until the step is recovered.
    pub(crate) fn of(phase: Phase, failed: bool, agent_waits: bool) -> Self {
        match phase {
            Phase::Accepted => Self::Completed,
            Phase::Cancelled => Self::Cancelled,
            _ if failed => Self::Failed,
            _ if agent_waits => Self::NeedsYou,
            Phase::ApprovePlan | Phase::Decision(_) | Phase::Acceptance => Self::NeedsYou,
            Phase::Regression(step) => {
                if step.needs_user() {
                    Self::NeedsYou
                } else {
                    Self::Running
                }
            }
            Phase::Planning | Phase::Development | Phase::Review(_) | Phase::Commands(_) => {
                Self::Running
            }
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::NeedsYou => "Needs you",
            Self::Failed => "Failed",
            Self::Completed => "Completed",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// A phase that only the user can move the run out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    Plan,
    Decision,
    Acceptance,
    Regression,
}

impl Gate {
    pub(crate) fn at(phase: Phase) -> Option<Self> {
        match phase {
            Phase::Regression(step) if step.needs_user() => Some(Self::Regression),
            Phase::ApprovePlan => Some(Self::Plan),
            Phase::Decision(_) => Some(Self::Decision),
            Phase::Acceptance => Some(Self::Acceptance),
            _ => None,
        }
    }

    /// What the user is asked to do, as the attention list words it.
    pub(crate) fn action(self) -> &'static str {
        match self {
            Self::Regression => "Review regression test",
            Self::Plan => "Approve plan",
            Self::Decision => "Review findings",
            Self::Acceptance => "Accept changes",
        }
    }

    fn headline(self) -> &'static str {
        match self {
            Self::Regression => "Regression test needs your decision",
            Self::Plan => "Plan ready for approval",
            Self::Decision => "Review needs your decision",
            Self::Acceptance => "Changes ready to accept",
        }
    }
}

/// Why a run stopped for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Waiting {
    Gate(Gate),
    Failed(String),
    /// A stage's agent is still at work after its time budget; nothing is asked of the user.
    Slow {
        stage: String,
        minutes: u64,
    },
    /// The recipe approved the plan and development started; nothing is asked of the user,
    /// who would otherwise never learn there was a plan to read.
    PlanAutoApproved,
}

/// A notification body names the run by the first line of its task, cut to this many characters.
const NOTICE_TASK_CHARS: usize = 110;

impl Waiting {
    /// Title and body of the notification for a run with this task.
    pub(crate) fn notice(&self, task: &str) -> (String, String) {
        let line = task.trim().lines().next().unwrap_or_default();
        let mut name: String = line.chars().take(NOTICE_TASK_CHARS).collect();
        if name.len() < line.len() {
            name.push('…');
        }
        match self {
            Self::Gate(gate) => (gate.headline().to_owned(), name),
            Self::Failed(error) => (
                "Pipeline step failed".to_owned(),
                format!("{name} · {error}"),
            ),
            Self::Slow { stage, minutes } => (
                format!("{stage} is still running after {minutes} min"),
                name,
            ),
            Self::PlanAutoApproved => ("Plan approved automatically".to_owned(), name),
        }
    }
}

impl Pipelines {
    pub(crate) fn status(&self, run: &Run, cx: &App) -> RunStatus {
        RunStatus::of(
            run.phase(),
            self.error(run.id()).is_some(),
            self.agent_request(run, cx).is_some(),
        )
    }

    /// Title and body of the notification for `event`; `None` once the run is gone.
    pub(crate) fn notice(&self, event: &RunWaits, cx: &App) -> Option<(String, String)> {
        let run = self.run(event.run, cx).ok()?;
        Some(event.reason.notice(run.task()))
    }

    /// Announces a gate the run reached through an agent's report or a check. The user's own
    /// decisions never call this: they are looking at the run they just moved.
    pub(super) fn announce_gate(&mut self, id: u64, before: Phase, cx: &mut Context<Self>) {
        let Ok(run) = self.run(id, cx) else { return };
        if run.phase() == before {
            return;
        }
        let reason = match Gate::at(run.phase()) {
            Some(gate) => Waiting::Gate(gate),
            None if before == Phase::Planning && run.plan_auto_approved() => {
                Waiting::PlanAutoApproved
            }
            None => return,
        };
        cx.emit(RunWaits { run: id, reason });
    }
}

#[cfg(test)]
mod tests {
    // Not a glob import: `gpui_kit::*` brings in GPUI's `test` attribute, which shadows `#[test]`.
    use super::{Gate, RunStatus, Waiting};
    use hive_core::pipeline::Phase;

    #[test]
    fn only_phases_waiting_on_the_user_are_gates() {
        assert_eq!(Gate::at(Phase::ApprovePlan), Some(Gate::Plan));
        assert_eq!(Gate::at(Phase::Decision(1)), Some(Gate::Decision));
        assert_eq!(Gate::at(Phase::Acceptance), Some(Gate::Acceptance));
        for phase in [
            Phase::Planning,
            Phase::Development,
            Phase::Review(0),
            Phase::Commands(0),
            Phase::Accepted,
            Phase::Cancelled,
        ] {
            assert_eq!(Gate::at(phase), None);
            assert_ne!(
                RunStatus::of(phase, false, false),
                RunStatus::NeedsYou,
                "{phase:?}"
            );
        }
    }

    #[test]
    fn a_notice_names_the_run_by_the_first_line_of_its_task() {
        let gate = Waiting::Gate(Gate::Plan);
        assert_eq!(
            gate.notice("  Add login\nwith details below"),
            ("Plan ready for approval".into(), "Add login".into())
        );
        let (_, body) = gate.notice(&"я".repeat(300));
        assert_eq!(body.chars().count(), 111);
        assert!(body.ends_with('…'));
        let failed = Waiting::Failed("Agent exited before reporting a result.".into());
        assert_eq!(
            failed.notice("Add login"),
            (
                "Pipeline step failed".into(),
                "Add login · Agent exited before reporting a result.".into()
            )
        );
        assert_eq!(
            Waiting::PlanAutoApproved.notice("Add login"),
            ("Plan approved automatically".into(), "Add login".into())
        );
    }

    #[test]
    fn working_phases_are_running() {
        for phase in [
            Phase::Planning,
            Phase::Development,
            Phase::Review(0),
            Phase::Commands(1),
        ] {
            assert_eq!(RunStatus::of(phase, false, false), RunStatus::Running);
        }
    }

    #[test]
    fn human_gates_need_the_user() {
        for phase in [Phase::ApprovePlan, Phase::Decision(0), Phase::Acceptance] {
            assert_eq!(RunStatus::of(phase, false, false), RunStatus::NeedsYou);
        }
    }

    #[test]
    fn waiting_agent_needs_the_user_during_a_working_phase() {
        assert_eq!(
            RunStatus::of(Phase::Development, false, true),
            RunStatus::NeedsYou
        );
    }

    #[test]
    fn error_outranks_a_waiting_gate_and_agent() {
        assert_eq!(
            RunStatus::of(Phase::ApprovePlan, true, true),
            RunStatus::Failed
        );
        assert_eq!(
            RunStatus::of(Phase::Development, true, false),
            RunStatus::Failed
        );
    }

    #[test]
    fn finished_runs_keep_their_outcome_despite_a_stale_error() {
        assert_eq!(
            RunStatus::of(Phase::Accepted, true, true),
            RunStatus::Completed
        );
        assert_eq!(
            RunStatus::of(Phase::Cancelled, true, false),
            RunStatus::Cancelled
        );
    }
}
