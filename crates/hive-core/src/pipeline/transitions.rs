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
            (Phase::Planning, Report::Plan { assignments }) => {
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
                self.feedback.push('\n');
                self.feedback.push_str(&handoff);
                self.outstanding.retain(|t| t != ticket);
                if self.outstanding.is_empty() {
                    self.enter_review(0);
                }
            }
            (Phase::Review(ix), Report::Review { summary, findings }) => {
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
                        "{}: {}",
                        self.lanes[finding.lane].name, finding.description
                    ));
                }
                self.findings = findings
                    .into_iter()
                    .filter(|f| {
                        !self
                            .dismissed
                            .iter()
                            .any(|(gate, accepted)| *gate == ix && accepted == f)
                    })
                    .collect();
                self.outstanding.clear();
                if ix == 0 && self.lanes.iter().any(Lane::requires_check) {
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

    /// Confirm the human-edited assignments and release the development lanes.
    pub fn approve(&mut self) -> Result<(), Error> {
        if self.phase != Phase::ApprovePlan {
            return Err(Error::WrongStage("The plan is not awaiting approval"));
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
        self.develop(indices);
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
        self.finish_review(ix);
        Ok(())
    }

    fn finish_review(&mut self, ix: usize) {
        let issues =
            !self.findings.is_empty() || (ix == 0 && self.commands.iter().any(|r| !r.success));
        if !issues {
            self.enter_review(ix + 1);
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

    fn enter_review(&mut self, ix: usize) {
        self.findings.clear();
        if ix == 0 {
            self.review_revision = None;
            self.commands.clear();
        }
        if ix == self.template.reviews.len() {
            self.phase = Phase::Acceptance;
        } else {
            self.phase = Phase::Review(ix);
            self.schedule(None);
        }
    }

    fn develop(&mut self, lanes: impl IntoIterator<Item = usize>) {
        self.phase = Phase::Development;
        for lane in lanes {
            self.schedule(Some(lane));
        }
    }

    fn repair(&mut self, ix: usize, note: &str) {
        let step = &self.template.reviews[ix];
        let fixed = step
            .return_lane
            .as_ref()
            .and_then(|name| self.lanes.iter().position(|l| l.name == *name));
        let mut destinations = BTreeSet::new();
        let mut feedback = vec![format!("{} requested fixes. {note}", step.name)];
        for f in &self.findings {
            destinations.insert(fixed.unwrap_or(f.lane));
            feedback.push(format!("{}: {}", self.lanes[f.lane].name, f.description));
        }
        if ix == 0 {
            for result in self.commands.iter().filter(|c| !c.success) {
                destinations.insert(result.lane);
                feedback.push(format!(
                    "{} failed: {}\n{}",
                    self.lanes[result.lane].name,
                    result.command,
                    check_excerpt(&result.output)
                ));
            }
        }
        self.feedback = feedback.join("\n");
        self.history.push(self.feedback.clone());
        self.develop(destinations);
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
            self.enter_review(ix + 1);
        }
        Ok(())
    }

    /// Reopen QA when the worktree changed outside the pipeline after the last review.
    pub fn recheck(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Acceptance {
            return Err(Error::WrongStage("The run is not ready for acceptance"));
        }
        self.history
            .push("The worktree changed since QA. Reviewing the current files again.".into());
        self.enter_review(0);
        Ok(())
    }

    /// Accept the current cross-repository changes after all checks pass.
    pub fn accept(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Acceptance {
            return Err(Error::WrongStage("The run is not ready for acceptance"));
        }
        self.phase = Phase::Accepted;
        self.history.push("User accepted the changes".into());
        Ok(())
    }

    /// Return a final result to its active development lanes for another review cycle.
    pub fn request_changes(&mut self, note: &str) -> Result<(), Error> {
        if self.phase != Phase::Acceptance {
            return Err(Error::WrongStage("There is no result awaiting acceptance"));
        }
        self.feedback = if note.trim().is_empty() {
            "User requested another pass. Compare the implementation with the approved assignment and address any remaining gaps.".into()
        } else {
            format!("User requested changes: {note}")
        };
        self.history.push(self.feedback.clone());
        let active: Vec<_> = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.assignment.trim().is_empty())
            .map(|(i, _)| i)
            .collect();
        self.develop(active);
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
