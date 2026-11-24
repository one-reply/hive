//! What one stage tells the next beyond the task itself: the repair instructions a review or
//! a red check sends back, and what a reviewer should not have to rediscover.
use super::transitions::check_excerpt;
use super::{Phase, Run, Ticket};
use crate::agent::launch::ResumeSession;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Passing tests named per lane in a reviewer's prompt; a large suite would crowd out the task.
const LISTED_TESTS: usize = 128;

/// How a developer finishes a round, whether the round starts or continues a conversation.
pub(super) const DEVELOPED_CONTRACT: &str = r#"Work only in your assigned worktree. Report {"kind":"developed","summary":"..."} with changes and verification evidence."#;

impl Run {
    /// What the previous stage left for this attempt: the lane's own repair brief when it
    /// has one, otherwise the handoff every role reads.
    pub(super) fn handoff(&self, ticket: &Ticket) -> &str {
        ticket
            .lane
            .and_then(|lane| self.briefs.iter().find(|(to, _)| *to == lane))
            .map_or(&self.feedback, |(_, brief)| brief)
    }

    /// Links each development attempt just scheduled to its lane's latest conversation with
    /// the same CLI, so another round costs a follow-up message, not a second reading of
    /// code the agent wrote itself.
    pub(super) fn continue_conversations(&mut self) {
        self.continuations = self
            .outstanding
            .iter()
            .filter_map(|ticket| {
                let agent = self.lanes.get(ticket.lane?)?.profile.agent;
                let (_, session) = self.sessions.iter().rev().find(|(earlier, session)| {
                    earlier.lane == ticket.lane && session.agent() == agent
                })?;
                Some((*ticket, session.clone()))
            })
            .collect();
    }

    /// The conversation a pending development attempt continues and the message that
    /// continues it. `None` means the attempt starts a new conversation with [`Run::prompt`].
    pub fn continuation(&self, ticket: &Ticket) -> Option<(&ResumeSession, String)> {
        if self.phase != Phase::Development || !self.pending().contains(ticket) {
            return None;
        }
        let (_, session) = self.continuations.iter().find(|(t, _)| t == ticket)?;
        let mut message = format!(
            "Continue your earlier work in this worktree.\n\n{}\n\n{DEVELOPED_CONTRACT}",
            self.handoff(ticket)
        );
        if !self.contract.is_empty() {
            let _ = writeln!(message, "\nShared contract:\n{}", self.contract);
        }
        self.write_developer_constraints(&mut message);
        Some((session, message))
    }

    pub(super) fn write_developer_constraints(&self, prompt: &mut String) {
        if let Some(r) = self.regression()
            && let Some(test) = &r.test
        {
            let _ = writeln!(
                prompt,
                "\nThe independent test for ‘{}’ is already written: {} in {}. Preserve this exact test while implementing the fix.",
                r.criterion, test.name, test.path
            );
        }
    }

    /// The record of a repair request for the reviewers and the history, and one brief per
    /// lane that has to repair, in lane order. Each lane gets only what was routed to it.
    pub(super) fn repair_handoff(
        &self,
        review: usize,
        note: &str,
    ) -> (String, Vec<(usize, String)>) {
        // Each finding goes where its own review sends findings: reviews that ran together
        // may route differently.
        let fixed = |asker: usize| {
            let route = self.template.reviews[asker].return_lane.as_ref();
            route.and_then(|name| self.lanes.iter().position(|l| l.name == *name))
        };
        // Red checks stop a pass before its first review, so no reviewer asked for anything.
        let cause = if self.findings.is_empty() {
            "Required checks failed.".to_owned()
        } else {
            let names: Vec<_> = self
                .askers(review)
                .into_iter()
                .map(|asker| self.template.reviews[asker].name.as_str())
                .collect();
            format!("{} requested fixes.", names.join(" and "))
        };
        let header = format!("{cause} {note}").trim_end().to_owned();
        let mut routed: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        let mut record = vec![header.clone()];
        for f in &self.findings {
            let item = format!("{}: {}", self.lanes[f.lane].name, f.description);
            let asker = self.reviewer(f).unwrap_or(review);
            routed
                .entry(fixed(asker).unwrap_or(f.lane))
                .or_default()
                .push(item.clone());
            record.push(item);
        }
        if review == self.template.qa_index() {
            for result in self.commands.iter().filter(|c| !c.success) {
                let item = format!(
                    "{} failed: {}\n{}",
                    self.lanes[result.lane].name,
                    result.command,
                    check_excerpt(&result.output)
                );
                routed.entry(result.lane).or_default().push(item.clone());
                record.push(item);
            }
        }
        let briefs = routed
            .into_iter()
            .map(|(lane, items)| {
                let brief = format!(
                    "{header}\nResolve every item below in your worktree:\n{}",
                    items.join("\n")
                );
                (lane, brief)
            })
            .collect();
        (record.join("\n"), briefs)
    }

    /// Appends what a reviewer starts with: the checks Hive already ran on these changes and
    /// the findings the user accepted at this review.
    pub(super) fn write_review_context(&self, review: usize, prompt: &mut String) {
        if !self.review_brief.is_empty() {
            let _ = writeln!(prompt, "Previous review request:\n{}", self.review_brief);
            for (lane, (old, new)) in self
                .feedback_trees
                .iter()
                .zip(&self.review_trees)
                .enumerate()
            {
                let _ = writeln!(prompt, "Lane {lane} repaired changes: git diff {old} {new}");
            }
        }
        if let Some((lane, line)) = &self.qa_focus {
            let _ = writeln!(
                prompt,
                "Recheck this acceptance criterion in lane {lane}: {line}"
            );
        }
        if !self.commands.is_empty() {
            prompt.push_str("\nRequired checks Hive ran on these changes:\n");
            for result in &self.commands {
                let _ = writeln!(
                    prompt,
                    "- {} · {} · {}",
                    self.lanes[result.lane].name,
                    result.command,
                    if result.success { "passed" } else { "failed" }
                );
            }
        }
        for check in self.commands.iter().filter(|check| check.success) {
            for name in check.passed.iter().take(LISTED_TESTS) {
                let _ = writeln!(prompt, "Lane {} passed test: {name}", check.lane);
            }
            // Without this line a reviewer reads a test past the list as one that did not run.
            if let Some(more) = check
                .passed
                .len()
                .checked_sub(LISTED_TESTS)
                .filter(|n| *n > 0)
            {
                let _ = writeln!(
                    prompt,
                    "Lane {} also passed {more} tests not listed here; a test you cite by its exact name still counts.",
                    check.lane
                );
            }
        }
        let mut accepted = self
            .dismissed
            .iter()
            .filter(|(gate, _)| *gate == review)
            .peekable();
        if accepted.peek().is_some() {
            prompt.push_str(
                "\nThe user accepted these findings at this review. Do not report them again:\n",
            );
            for (_, finding) in accepted {
                let _ = writeln!(
                    prompt,
                    "- {}: {}",
                    self.lanes[finding.lane].name, finding.description
                );
            }
        }
    }
}
