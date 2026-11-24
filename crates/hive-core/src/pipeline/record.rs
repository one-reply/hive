//! Read access to a run, and the facts the application records outside the transitions:
//! checkout paths, CLI session IDs and the reviewed content fingerprint.
use super::{
    CommandResult, Error, Finding, Lane, Phase, Report, Run, Stage, StageTiming, Template, Ticket,
};
use crate::agent::launch::ResumeSession;
use std::path::{Path, PathBuf};

impl Run {
    /// Whether this run started from the user's criteria without a planner.
    pub fn direct_start(&self) -> bool {
        self.direct_start
    }
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
    /// Open blocking findings of the most recent review.
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }
    /// Non-blocking findings of the current QA pass, each with the index of its review.
    pub fn notes(&self) -> &[(usize, Finding)] {
        &self.notes
    }
    /// Required-check results of the current QA pass.
    pub fn commands(&self) -> &[CommandResult] {
        &self.commands
    }
    /// Content fingerprint taken after the required checks, before the first review.
    pub fn review_revision(&self) -> Option<&str> {
        self.review_revision.as_deref()
    }
    /// Immutable trees for the most recent QA pass, in lane order.
    pub fn review_trees(&self) -> &[String] {
        &self.review_trees
    }
    /// Trees the user saw before asking for the last repair.
    pub fn feedback_trees(&self) -> &[String] {
        &self.feedback_trees
    }
    /// Reviews the current QA pass leaves out because nothing it reviews matches their paths.
    pub fn skipped_reviews(&self) -> &[usize] {
        &self.skipped
    }

    /// Bind the QA fingerprint and the corresponding Git trees to the same review, and decide
    /// which conditional reviews this pass leaves out. `changed` lists the repository-relative
    /// paths the trees change; `None` when they could not be listed, which skips nothing.
    ///
    /// One transition, so a saved run never holds the fingerprint without the decision: a
    /// review that should have been skipped could otherwise start in between.
    pub fn record_review_trees(
        &mut self,
        revision: String,
        trees: Vec<String>,
        changed: Option<&[String]>,
    ) -> Result<(), Error> {
        if trees.len() != self.lanes.len()
            || !trees
                .iter()
                .all(|tree| super::workspace::is_object_name(tree))
        {
            return Err(Error::InvalidInput("Invalid review trees"));
        }
        if self.phase == Phase::Review(0)
            && let Some(r) = self.regression.as_mut()
            && r.candidate_revision.as_ref() != Some(&revision)
        {
            self.outstanding.clear();
            r.problem = "The candidate changed between the regression check and QA. Retry the candidate test.".into();
            r.blocked_at = Some(super::RegressionStep::Candidate);
            self.phase = Phase::Regression(super::RegressionStep::Decision);
            return Ok(());
        }
        self.record_review_revision(revision)?;
        self.review_trees = trees;
        if let Some(changed) = changed {
            self.skip_reviews(changed);
        }
        Ok(())
    }

    /// Leaves out the reviews whose condition matches none of `changed`. A review that has
    /// already reported in this run keeps running: a repair that moved the code out of its
    /// paths must not escape the check of its own finding.
    fn skip_reviews(&mut self, changed: &[String]) {
        let reported = |review: usize| {
            let phase = Phase::Review(review);
            self.reports.iter().any(|(_, at, _)| *at == phase)
        };
        let skipped: Vec<_> = (0..self.template.reviews.len())
            .filter(|&review| {
                self.template
                    .review_condition(review)
                    .is_some_and(|patterns| !super::condition::applies(patterns, changed))
                    && !reported(review)
            })
            .collect();
        for &review in &skipped {
            let step = &self.template.reviews[review];
            self.history.push(format!(
                "{} skipped: no changed file matches {}",
                step.name,
                step.only_paths.join(", ")
            ));
        }
        self.skipped = skipped;
        // The first group was scheduled before its conditions could be judged: the reviewers
        // of the reviews now left out are withdrawn, and a group left out whole is passed.
        if self.phase == Phase::Review(0) {
            let skipped = &self.skipped;
            self.outstanding
                .retain(|ticket| !skipped.contains(&ticket.review.unwrap_or(0)));
            if self.outstanding.is_empty() {
                self.enter_review(self.template.review_group(0).end);
            }
        }
    }
    /// Pending feedback anchored to an exact checkout tree, grouped by receiving lane.
    pub fn comments(&self) -> &[(usize, String, crate::git::review_notes::Note)] {
        &self.comments
    }

    /// Queue a note only while the same reviewed tree is awaiting acceptance.
    pub fn add_comment(
        &mut self,
        lane: usize,
        tree: String,
        note: crate::git::review_notes::Note,
    ) -> Result<(), Error> {
        if self.phase != Phase::Acceptance
            || self.review_trees.get(lane) != Some(&tree)
            || self.lanes.get(lane).is_none_or(|l| !l.is_assigned())
            || note.text.trim().is_empty()
        {
            return Err(Error::InvalidInput(
                "This comment no longer matches the review.",
            ));
        }
        self.comments.push((lane, tree, note));
        Ok(())
    }

    /// Remove the exact queued note, including when its position has become outdated.
    pub fn remove_comment(&mut self, comment: &(usize, String, crate::git::review_notes::Note)) {
        if let Some(i) = self.comments.iter().position(|n| n == comment) {
            self.comments.remove(i);
        }
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

    /// Records the fingerprint of the changes the first QA review looks at, taken once the
    /// required checks have stopped writing to the worktree.
    pub fn record_review_revision(&mut self, revision: String) -> Result<(), Error> {
        if self.phase != Phase::Review(0) {
            return Err(Error::WrongStage(
                "The changes are fingerprinted before the first review",
            ));
        }
        self.review_revision = Some(revision);
        Ok(())
    }

    /// Proposed or approved contract in this frozen run.
    pub fn shared_contract(&self) -> &str {
        &self.contract
    }

    /// Approves the plan with the user's edits, one `(task, criteria)` per lane in order.
    pub fn approve_with(&mut self, assignments: Vec<(String, String)>) -> Result<(), Error> {
        self.approve_plan(assignments, None)
    }

    /// Approves edited assignments and the single shared contract together.
    pub fn approve_with_contract(
        &mut self,
        assignments: Vec<(String, String)>,
        contract: &str,
    ) -> Result<(), Error> {
        self.approve_plan(assignments, Some(contract))
    }

    fn approve_plan(
        &mut self,
        assignments: Vec<(String, String)>,
        contract: Option<&str>,
    ) -> Result<(), Error> {
        if self.phase != Phase::ApprovePlan || assignments.len() != self.lanes.len() {
            return Err(Error::WrongStage("The proposed plan has changed."));
        }
        let mut edited = self.clone();
        if edited.template.shared_contract
            && let Some(contract) = contract
        {
            edited.contract = contract.trim().into();
        }
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
            review: None,
        };
        let review = Ticket {
            run: 12,
            attempt: 3,
            lane: None,
            review: None,
        };
        let developed = Report::Developed {
            summary: "Build complete".into(),
        };
        run.reports.push((developer, Phase::Development, developed));
        let reviewed = Report::Review {
            evidence: vec![],
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
