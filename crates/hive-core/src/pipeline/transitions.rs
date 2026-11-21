//! Every transition that may authorize more work lives here. Inputs are validated before mutation.
use super::*;
use std::collections::BTreeSet;

impl Run {
    /// Consume a structured report only for the invocation authorized to produce it.
    pub fn report(&mut self, ticket: &Ticket, report: Report) -> Result<(), Error> {
        if self.step(ticket).is_none() {
            return Err(Error::InactiveAttempt);
        }
        let handoff = (*ticket, self.phase, report.clone());
        match (self.phase, report) {
            (Phase::Regression(RegressionStep::Author), Report::Regression { test }) => {
                self.submit_regression_test(test)?;
            }
            (
                Phase::Planning,
                Report::Plan {
                    assignments,
                    contract,
                },
            ) => {
                if assignments.is_empty() {
                    return Err(Error::InvalidInput(
                        "Provide one task and acceptance criteria per selected lane",
                    ));
                }
                let mut indices = HashSet::new();
                if let Some(invalid) = assignments.iter().find(|a| {
                    a.lane >= self.lanes.len()
                        || !indices.insert(a.lane)
                        || a.task.trim().is_empty()
                        || a.criteria.trim().is_empty()
                }) {
                    return Err(Error::InvalidAssignment(invalid.lane));
                }
                if self.template.shared_contract {
                    if contract.trim().is_empty() {
                        return Err(Error::InvalidInput(
                            "Supply a shared contract for approval.",
                        ));
                    }
                    self.contract = contract.trim().to_owned();
                }
                for lane in &mut self.lanes {
                    lane.assignment.clear();
                    lane.criteria.clear();
                }
                for assignment in assignments {
                    let lane = &mut self.lanes[assignment.lane];
                    lane.assignment = assignment.task;
                    lane.criteria = assignment.criteria;
                }
                self.outstanding.clear();
                self.phase = Phase::ApprovePlan;
                self.history
                    .push("Planner submitted assignments for approval".into());
            }
            (Phase::Development, Report::Developed { summary }) if !summary.trim().is_empty() => {
                let lane = ticket
                    .lane
                    .ok_or(Error::InvalidInput("Missing development lane"))?;
                let handoff = format!("{}: {summary}", self.lanes[lane].name);
                self.history.push(handoff.clone());
                // A lane that shares this worktree starts later and reads what was just done.
                for text in std::iter::once(&mut self.feedback)
                    .chain(self.briefs.iter_mut().map(|(_, brief)| brief))
                {
                    text.push('\n');
                    text.push_str(&handoff);
                }
                self.outstanding.retain(|t| t != ticket);
                if self.outstanding.is_empty() {
                    self.begin_validation();
                }
            }
            (
                Phase::Review(ix),
                Report::Review {
                    summary,
                    findings,
                    evidence,
                },
            ) => {
                for item in &evidence {
                    if self.lanes.get(item.lane).is_none_or(|l| {
                        l.assignment.is_empty()
                            || !l.criteria.lines().any(|line| item.confirms(line))
                    }) {
                        return Err(Error::InvalidInput(
                            "Evidence must identify an approved criterion and a concrete observation",
                        ));
                    }
                }
                if summary.trim().is_empty() {
                    return Err(Error::InvalidInput("Include review evidence"));
                }
                if let Some(invalid) = findings.iter().find(|f| {
                    f.description.trim().is_empty()
                        || self
                            .lanes
                            .get(f.lane)
                            .is_none_or(|l| l.assignment.trim().is_empty())
                }) {
                    return Err(Error::InvalidFinding(invalid.lane));
                }
                self.history
                    .push(format!("{}: {summary}", self.template.reviews[ix].name));
                for finding in &findings {
                    self.history.push(format!(
                        "{} ({}): {}",
                        self.lanes[finding.lane].name,
                        finding.severity.label(),
                        finding.description
                    ));
                }
                let (blocking, notes): (Vec<_>, Vec<_>) = findings
                    .into_iter()
                    .filter(|f| {
                        !self
                            .dismissed
                            .iter()
                            .any(|(gate, accepted)| *gate == ix && f.repeats(accepted))
                    })
                    .partition(|f| f.severity.blocks());
                self.findings = blocking;
                self.notes.extend(notes.into_iter().map(|note| (ix, note)));
                self.outstanding.clear();
                // Only a run saved mid-review by a build that ran the checks afterwards gets
                // here with a check still owed; skipping it would accept untested work.
                if ix == 0 && self.owes_checks() {
                    self.phase = Phase::Commands(ix);
                } else {
                    self.finish_review(ix);
                }
            }
            _ => {
                return Err(Error::InvalidInput(
                    "This report does not match the current stage or has an empty summary",
                ));
            }
        }
        self.reports.push(handoff);
        Ok(())
    }

    /// Begin a fresh single-lane run with the user's own assignment and acceptance criteria.
    pub fn start_direct(&mut self, criteria: &str) -> Result<(), Error> {
        if self.phase != Phase::Planning
            || !self.history.is_empty()
            || self.template.shared_contract
            || self.lanes.len() != 1
            || criteria.trim().is_empty()
            || self
                .template
                .reviews
                .iter()
                .filter_map(|s| s.return_lane.as_ref())
                .any(|name| name != &self.lanes[0].name)
        {
            return Err(Error::InvalidInput(
                "Quick start needs one lane and explicit acceptance criteria",
            ));
        }
        self.direct_start = true;
        self.outstanding.clear();
        self.lanes[0].assignment = self.task.clone();
        self.lanes[0].criteria = criteria.trim().to_owned();
        self.history
            .push("User started development with explicit acceptance criteria".into());
        self.start_development(vec![0]);
        Ok(())
    }

    /// Confirm the human-edited assignments and release the development lanes.
    pub fn approve(&mut self) -> Result<(), Error> {
        if self.phase != Phase::ApprovePlan {
            return Err(Error::WrongStage("The plan is not awaiting approval"));
        }
        if self.template.shared_contract && self.contract.trim().is_empty() {
            return Err(Error::InvalidInput(
                "Enter a shared contract before approving.",
            ));
        }
        let assigned: Vec<_> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.assignment.trim().is_empty())
            .collect();
        if assigned.is_empty() || assigned.iter().any(|(_, l)| l.criteria.trim().is_empty()) {
            return Err(Error::InvalidInput(
                "Provide an assignment and acceptance criteria for each active lane",
            ));
        }
        if let Some(missing) = self
            .template
            .reviews
            .iter()
            .filter_map(|s| s.return_lane.as_ref())
            .find(|name| !assigned.iter().any(|(_, l)| l.name == **name))
        {
            return Err(Error::MissingLane(missing.clone()));
        }
        let indices: Vec<_> = assigned.into_iter().map(|(ix, _)| ix).collect();
        self.history.push("User approved the plan".into());
        self.feedback.clear();
        self.briefs.clear();
        self.continuations.clear();
        self.start_development(indices);
        Ok(())
    }

    /// Request a revised plan, preserving the previously proposed assignments as context.
    pub fn replan(&mut self, note: &str) -> Result<(), Error> {
        if self.phase != Phase::ApprovePlan {
            return Err(Error::WrongStage("There is no plan awaiting approval"));
        }
        self.feedback = if note.trim().is_empty() {
            "User requested a revised plan. Reassess the assignments and acceptance criteria against the original task.".into()
        } else {
            format!("User requested a revised plan: {note}")
        };
        self.history.push(self.feedback.clone());
        self.phase = Phase::Planning;
        self.schedule(None);
        Ok(())
    }

    /// Apply only a complete set of results for the currently configured mandatory checks.
    pub fn command_results(&mut self, results: Vec<CommandResult>) -> Result<(), Error> {
        let Phase::Commands(ix) = self.phase else {
            return Err(Error::WrongStage("No commands are pending"));
        };
        let expected: BTreeSet<_> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.requires_check())
            .map(|(ix, _)| ix)
            .collect();
        let actual: BTreeSet<_> = results.iter().map(|r| r.lane).collect();
        if actual != expected
            || actual.len() != results.len()
            || results.iter().any(|r| {
                self.lanes
                    .get(r.lane)
                    .is_none_or(|l| r.command != l.command)
                    || (r.success && r.exit_code != Some(0))
            })
        {
            return Err(Error::InvalidInput(
                "The results do not match the required commands",
            ));
        }
        // `commands` keeps this round's full output; history only needs the diagnostic tail,
        // so a long QA loop does not grow the saved state by the size of every log.
        for r in &results {
            self.history.push(format!(
                "{} · {} · {} ({})\n{}",
                self.lanes[r.lane].name,
                r.command,
                if r.success { "Passed" } else { "Failed" },
                r.exit_code
                    .map_or_else(|| "no exit code".into(), |code| format!("exit {code}")),
                check_excerpt(&r.output)
            ));
        }
        self.commands = results;
        // The fingerprint is taken when the first review starts, so having one here means
        // that review already saw this pass: a run saved by a build that ran the checks
        // after it. There the checks conclude the review instead of preceding it.
        let reviewed = self.review_revision.is_some();
        if reviewed || self.commands.iter().any(|r| !r.success) {
            self.finish_review(ix);
        } else {
            self.enter_review(ix);
        }
        Ok(())
    }

    /// Records the revision which stayed unchanged throughout command execution.
    pub fn checked_at(
        &mut self,
        results: Vec<CommandResult>,
        revision: Option<String>,
    ) -> Result<(), Error> {
        self.command_results(results)?;
        self.check_revision = revision;
        Ok(())
    }

    /// Whether an assigned lane's required command has no result in this QA pass.
    fn owes_checks(&self) -> bool {
        self.lanes.iter().enumerate().any(|(ix, lane)| {
            lane.requires_check()
                && !self
                    .commands
                    .iter()
                    .any(|c| c.lane == ix && c.command == lane.command)
        })
    }

    fn finish_review(&mut self, ix: usize) {
        let issues =
            !self.findings.is_empty() || (ix == 0 && self.commands.iter().any(|r| !r.success));
        if !issues {
            if self.qa_focus.take().is_some() {
                self.phase = Phase::Acceptance;
            } else {
                self.enter_review(ix + 1);
            }
        } else if self.rounds[ix] >= self.template.reviews[ix].limit {
            self.phase = Phase::Decision(ix);
            self.history.push(format!(
                "{} needs a decision",
                self.template.reviews[ix].name
            ));
        } else {
            self.rounds[ix] += 1;
            self.repair(ix, "");
        }
    }

    /// Starts a QA pass over the current changes: the required checks first, because a red
    /// test is cheaper to find than a reviewer's turn, then the reviews in order.
    pub(super) fn begin_qa(&mut self) {
        self.first_review_attempt = self.last_attempt.saturating_add(1);
        self.qa_focus = None;
        self.check_revision = None;
        self.review_revision = None;
        self.review_trees.clear();
        self.findings.clear();
        self.commands.clear();
        self.notes.clear();
        self.briefs.clear();
        self.continuations.clear();
        if self.lanes.iter().any(Lane::requires_check) {
            self.phase = Phase::Commands(0);
        } else {
            self.enter_review(0);
        }
    }

    fn enter_review(&mut self, ix: usize) {
        self.findings.clear();
        if ix == self.template.reviews.len() {
            self.phase = Phase::Acceptance;
        } else {
            self.phase = Phase::Review(ix);
            self.schedule(None);
        }
    }

    pub(super) fn develop(&mut self, lanes: impl IntoIterator<Item = usize>) {
        self.phase = Phase::Development;
        for lane in lanes {
            self.schedule(Some(lane));
        }
    }

    fn repair(&mut self, ix: usize, note: &str) {
        let (feedback, briefs) = self.repair_handoff(ix, note);
        self.history.push(feedback.clone());
        self.review_brief.clone_from(&feedback);
        if !self.review_trees.is_empty() {
            self.feedback_trees.clone_from(&self.review_trees);
        }
        self.feedback = feedback;
        let destinations: Vec<_> = briefs.iter().map(|(lane, _)| *lane).collect();
        self.briefs = briefs;
        self.develop(destinations);
        self.continue_conversations();
    }

    /// Authorize one additional repair round after the automatic budget is exhausted.
    pub fn retry_gate(&mut self, note: &str) -> Result<(), Error> {
        let Phase::Decision(ix) = self.phase else {
            return Err(Error::WrongStage("No review is awaiting a decision"));
        };
        self.history
            .push(format!("User authorized another repair: {note}"));
        self.repair(ix, note);
        Ok(())
    }

    /// Dismiss one identified reviewer finding. Mandatory test failures stay blocking.
    pub fn dismiss_finding(&mut self, index: usize, reason: &str) -> Result<(), Error> {
        let Phase::Decision(ix) = self.phase else {
            return Err(Error::WrongStage("No review is awaiting a decision"));
        };
        if index >= self.findings.len() || reason.trim().is_empty() {
            return Err(Error::InvalidInput("Select a finding and enter a reason"));
        }
        let finding = self.findings.remove(index);
        self.history.push(format!(
            "User dismissed “{}”: {reason}",
            finding.description
        ));
        self.dismissed.push((ix, finding));
        if self.findings.is_empty() && (ix != 0 || self.commands.iter().all(|r| r.success)) {
            self.finish_review(ix);
        }
        Ok(())
    }

    /// Reopen QA when the worktree changed outside the pipeline after the last review.
    pub fn recheck(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Acceptance {
            return Err(Error::WrongStage("The run is not ready for acceptance"));
        }
        self.history
            .push("The worktree changed since QA. Checking the current files again.".into());
        self.begin_validation();
        Ok(())
    }

    /// Whether the user may accept now. A line comment still queued blocks it: accepting ends
    /// the review it was written for, so it could never be sent.
    pub fn can_accept(&self) -> Result<(), Error> {
        if self.phase != Phase::Acceptance {
            return Err(Error::WrongStage("The run is not ready for acceptance"));
        }
        if !self.comments.is_empty() {
            return Err(Error::InvalidInput(
                "Send or remove your line comments before accepting.",
            ));
        }
        if self.commands.iter().any(|c| !c.success) || self.owes_checks() {
            return Err(Error::InvalidInput(
                "Pass the required checks before accepting.",
            ));
        }
        if self
            .criteria()
            .any(|(lane, line)| !self.criterion_source(lane, line).closes())
        {
            return Err(Error::InvalidInput(
                "Verify each criterion or record a reason to accept it.",
            ));
        }
        Ok(())
    }

    /// Accept the current cross-repository changes after all checks pass.
    pub fn accept(&mut self) -> Result<(), Error> {
        self.can_accept()?;
        self.phase = Phase::Accepted;
        self.history.push("User accepted the changes".into());
        Ok(())
    }

    /// Invalidate all live attempts without removing worktrees or changes.
    pub fn cancel(&mut self) -> Result<(), Error> {
        if matches!(self.phase, Phase::Accepted | Phase::Cancelled) {
            return Err(Error::WrongStage("The run has already ended"));
        }
        self.phase = Phase::Cancelled;
        self.outstanding.clear();
        self.history.push("User stopped the run".into());
        Ok(())
    }
}

// Full command output stays in the Checks view. Keep large logs out of CLI launch arguments.
pub(super) fn check_excerpt(output: &str) -> String {
    const LIMIT: usize = 4096;
    if output.len() <= LIMIT {
        return output.to_owned();
    }
    let tail = output.ceil_char_boundary(output.len() - LIMIT);
    format!("[Last 4 KiB of check output]\n{}", &output[tail..])
}
