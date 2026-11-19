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

impl Pipelines {
    pub(crate) fn status(&self, run: &Run, cx: &App) -> RunStatus {
        RunStatus::of(
            run.phase(),
            self.error(run.id()).is_some(),
            self.agent_request(run, cx).is_some(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not a glob import: `gpui_kit::*` brings in GPUI's `test` attribute, which shadows `#[test]`.
    use super::RunStatus;
    use hive_core::pipeline::Phase;

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
