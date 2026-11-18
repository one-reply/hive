//! Live CLI requests belong to the current attempt, including runs without timing data.
use super::*;

pub(super) struct LiveSession {
    pub(super) ticket: Ticket,
    pub(super) stage: Stage,
    pub(super) status: Option<AgentStatus>,
}

impl LiveSession {
    pub(super) fn for_run(run: &Run, cx: &App) -> Vec<Self> {
        let app = AppState::get(cx);
        let pipelines = app.pipelines.read(cx);
        let agents = app.agents.read(cx);
        run.pending()
            .into_iter()
            .filter_map(|ticket| {
                let stage = Stage::from_phase(run.phase(), ticket.lane)?;
                let session = pipelines.session(&ticket)?;
                let status = (!session.read(cx).exited)
                    .then(|| agents.recipient(session.entity_id()))
                    .and_then(|observed| observed_status(observed.status, observed.exact));
                Some(Self {
                    ticket,
                    stage,
                    status,
                })
            })
            .collect()
    }

    pub(super) fn request_action(&self) -> Option<&'static str> {
        match self.status? {
            AgentStatus::WaitingApproval => Some("Review approval"),
            // A pending step whose turn ended has not reported yet: the agent waits for a reply.
            AgentStatus::WaitingInput | AgentStatus::Done => Some("Answer request"),
            _ => None,
        }
    }
}

/// The run no longer waits for this attempt, so its terminal shows a closed process.
pub(super) fn attempt_ended(run: Option<&Run>, ticket: &Ticket) -> bool {
    run.is_none_or(|run| !run.pending().contains(ticket))
}

fn observed_status(status: Option<AgentStatus>, exact: bool) -> Option<AgentStatus> {
    // Quiet output alone cannot distinguish an agent thinking from one waiting for input.
    status.filter(|s| exact || *s != AgentStatus::Idle)
}

#[cfg(test)]
mod tests {
    use super::{AgentStatus, LiveSession, Stage, Ticket, attempt_ended, observed_status};
    use hive_core::pipeline::{Lane, Run, Template};

    #[test]
    fn a_live_session_view_closes_once_its_attempt_is_no_longer_pending() {
        let lane = Lane {
            name: "Web".into(),
            project: "/web".into(),
            base: "HEAD".into(),
            ..Lane::default()
        };
        let run = Run::new(
            7,
            "/web".into(),
            "Task".into(),
            Template::default(),
            vec![lane],
        )
        .unwrap();
        let [planner] = run.pending()[..] else {
            panic!("a new run waits for its planner");
        };
        assert!(!attempt_ended(Some(&run), &planner));
        let stale = Ticket {
            attempt: planner.attempt + 1,
            ..planner
        };
        assert!(attempt_ended(Some(&run), &stale));
        assert!(attempt_ended(None, &planner));
    }

    #[test]
    fn agent_feedback_distinguishes_silent_tools_from_confirmed_waits() {
        use AgentStatus::*;
        assert_eq!(observed_status(None, false), None);
        assert_eq!(observed_status(Some(Idle), false), None);
        assert_eq!(observed_status(Some(Idle), true), Some(Idle));
        for status in [Working, WaitingInput, WaitingApproval, Done, Failed] {
            assert_eq!(observed_status(Some(status), true), Some(status));
        }
        assert_eq!(observed_status(Some(Working), false), Some(Working));
    }

    #[test]
    fn only_outstanding_requests_offer_a_response() {
        use AgentStatus::*;
        let mut session = LiveSession {
            ticket: Ticket {
                run: 1,
                attempt: 3,
                lane: None,
            },
            stage: Stage::Review(0),
            status: None,
        };
        for (status, action) in [
            (Some(WaitingApproval), Some("Review approval")),
            (Some(Working), None),
            (Some(WaitingInput), Some("Answer request")),
            (Some(Idle), None),
            (Some(Done), Some("Answer request")),
            (Some(Failed), None),
            (None, None),
        ] {
            session.status = status;
            assert_eq!(session.request_action(), action);
        }
    }
}
