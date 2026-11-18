//! Read access to a run, and the facts the application records outside the transitions:
//! checkout paths, CLI session IDs and the reviewed content fingerprint.
use super::{
    CommandResult, Error, Finding, Lane, Phase, Report, Run, Stage, StageTiming, Template, Ticket,
};
use crate::agent::launch::ResumeSession;
use std::path::{Path, PathBuf};

impl Run {
    /// Stable run identity.
    pub fn id(&self) -> u64 {
        self.id
    }
    /// Local project that owns this task in the sidebar.
    pub fn owner(&self) -> &Path {
        &self.owner
    }
    /// Original user request.
    pub fn task(&self) -> &str {
        &self.task
    }
    /// Recipe frozen at launch.
    pub fn template(&self) -> &Template {
        &self.template
    }
    /// Approved repository bindings.
    pub fn lanes(&self) -> &[Lane] {
        &self.lanes
    }
    /// Current gate.
    pub fn phase(&self) -> Phase {
        self.phase
    }
    /// Chronological handoffs, decisions and test outcomes.
    pub fn history(&self) -> &[String] {
        &self.history
    }
    /// Accepted handoffs with the attempt and phase that produced them.
    pub fn reports(&self) -> &[(Ticket, Phase, Report)] {
        &self.reports
    }
    /// Observed stage intervals.
    pub fn timings(&self) -> &[StageTiming] {
        &self.timings
    }
    /// Open findings of the most recent review.
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }
    /// Most recent QA command results.
    pub fn commands(&self) -> &[CommandResult] {
        &self.commands
    }
    /// Content fingerprint taken before QA.
    pub fn review_revision(&self) -> Option<&str> {
        self.review_revision.as_deref()
    }
    /// Known CLI sessions, linked to the attempts that created them.
    pub fn sessions(&self) -> &[(Ticket, ResumeSession)] {
        &self.sessions
    }

    /// Reports of one stage, oldest first, matched by phase and lane rather than by name.
    pub fn stage_reports(
        &self,
        stage: Stage,
    ) -> impl DoubleEndedIterator<Item = (Ticket, &Report)> {
        self.reports
            .iter()
            .filter(move |(ticket, phase, _)| Stage::from_phase(*phase, ticket.lane) == Some(stage))
            .map(|(ticket, _, report)| (*ticket, report))
    }

    /// Records the checkout Git created for a lane. A recorded path is never replaced: a
    /// late result after cancellation must not redirect work that already uses it.
    pub fn record_worktree(&mut self, lane: usize, path: PathBuf) {
        if let Some(lane) = self.lanes.get_mut(lane)
            && lane.worktree.as_os_str().is_empty()
        {
            lane.worktree = path;
        }
    }

    /// Links a CLI session ID to the pending attempt that created it, replacing an older one.
    pub fn record_session(&mut self, ticket: Ticket, session: ResumeSession) {
        if !self.outstanding.contains(&ticket) {
            return;
        }
        self.sessions.retain(|(t, _)| *t != ticket);
        self.sessions.push((ticket, session));
    }

    /// Records the fingerprint of the changes the first QA review looks at.
    pub fn record_review_revision(&mut self, revision: String) -> Result<(), Error> {
        if self.phase != Phase::Review(0) {
            return Err(Error::WrongStage(
                "The changes are fingerprinted before the first review",
            ));
        }
        self.review_revision = Some(revision);
        Ok(())
    }

    /// Approves the plan with the user's edits, one `(task, criteria)` per lane in order.
    pub fn approve_with(&mut self, assignments: Vec<(String, String)>) -> Result<(), Error> {
        if self.phase != Phase::ApprovePlan || assignments.len() != self.lanes.len() {
            return Err(Error::WrongStage("The proposed plan has changed."));
        }
        let mut edited = self.clone();
        for (lane, (task, criteria)) in edited.lanes.iter_mut().zip(assignments) {
            lane.assignment = task;
            lane.criteria = criteria;
        }
        edited.approve()?;
        *self = edited;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentKind;

    fn run() -> Run {
        Run::new(
            12,
            "/qa".into(),
            "Task".into(),
            Template::default(),
            vec![Lane {
                name: "QA".into(),
                project: "/qa".into(),
                ..Default::default()
            }],
        )
        .unwrap()
    }

    #[test]
    fn histories_with_colliding_names_resolve_handoffs_only_by_phase_lane_and_ticket() {
        let mut run = run();
        run.history.push("QA: unrelated free text".into());
        assert!(run.stage_reports(Stage::Review(0)).next().is_none());
        let developer = Ticket {
            run: 12,
            attempt: 2,
            lane: Some(0),
        };
        let review = Ticket {
            run: 12,
            attempt: 3,
            lane: None,
        };
        let developed = Report::Developed {
            summary: "Build complete".into(),
        };
        run.reports.push((developer, Phase::Development, developed));
        let reviewed = Report::Review {
            summary: "QA complete".into(),
            findings: vec![],
        };
        run.reports.push((review, Phase::Review(0), reviewed));
        let last = |stage| run.stage_reports(stage).last().map(|(t, _)| t);
        assert_eq!(last(Stage::Develop(0)), Some(developer));
        assert_eq!(last(Stage::Review(0)), Some(review));
        assert_eq!(run.stage_reports(Stage::Review(1)).count(), 0);
    }

    #[test]
    fn a_recorded_worktree_is_never_redirected_by_a_late_result() {
        let mut run = run();
        run.record_worktree(0, "/qa/.worktrees/a".into());
        run.record_worktree(0, "/elsewhere".into());
        run.record_worktree(9, "/missing-lane".into());
        assert_eq!(run.lanes()[0].worktree, Path::new("/qa/.worktrees/a"));
    }

    #[test]
    fn sessions_and_fingerprints_attach_only_to_current_work() {
        let mut run = run();
        let planner = run.pending()[0];
        let stale = Ticket {
            attempt: planner.attempt + 1,
            ..planner
        };
        let session = |id: &str| ResumeSession::new(AgentKind::ClaudeCode, id).unwrap();
        let second = session("6f1c2d3e-0000-4000-8000-000000000002");
        run.record_session(planner, session("6f1c2d3e-0000-4000-8000-000000000001"));
        run.record_session(planner, second.clone());
        run.record_session(stale, second.clone());
        assert_eq!(run.sessions(), [(planner, second)]);
        assert!(run.record_review_revision("abc".into()).is_err());
        assert_eq!(run.review_revision(), None);
    }

    #[test]
    fn edited_plan_is_applied_only_when_it_can_be_approved() {
        let mut run = run();
        run.phase = Phase::ApprovePlan;
        let before = run.clone();
        assert!(
            run.approve_with(vec![("Task".into(), String::new())])
                .is_err()
        );
        assert_eq!(run, before);
        assert!(run.approve_with(vec![]).is_err());
        run.approve_with(vec![("Fix QA".into(), "Tests pass".into())])
            .unwrap();
        assert_eq!(run.phase(), Phase::Development);
        assert_eq!(run.lanes()[0].assignment, "Fix QA");
    }
}
