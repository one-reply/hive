//! Criterion provenance at the reviewed revision, shared by the gate, the UI and delivery.
use super::{Error, Phase, Report, Run};
use serde::{Deserialize, Serialize};
use std::fmt;

/// An explicit user decision for one criterion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CriterionDecision {
    /// The user exercised the scenario and recorded the observation.
    Manual(String),
    /// The user accepted the outstanding risk for this revision.
    Exception(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RecordedDecision {
    lane: usize,
    criterion: String,
    revision: String,
    decision: CriterionDecision,
}

/// The strongest current source for a criterion. A reviewer assessment alone leaves it open.
pub enum EvidenceSource<'a> {
    /// Named test observed in Hive's own execution.
    Test(&'a str),
    /// Explicit decision on this reviewed revision.
    User(&'a CriterionDecision),
    /// Evidence claimed by a named reviewer.
    Reviewer(&'a str, &'a str),
    /// No observation available.
    Missing,
}

impl EvidenceSource<'_> {
    /// Whether this evidence satisfies the human acceptance gate.
    pub fn closes(&self) -> bool {
        matches!(self, Self::Test(_) | Self::User(_))
    }
}

impl fmt::Display for EvidenceSource<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Test(test) => write!(f, "Hive test · {test}"),
            Self::User(CriterionDecision::Manual(note)) => write!(f, "Manually verified · {note}"),
            Self::User(CriterionDecision::Exception(note)) => {
                write!(f, "Accepted exception · {note}")
            }
            Self::Reviewer(role, note) => write!(f, "Reviewer assessment · {role} · {note}"),
            Self::Missing => f.write_str("Unverified"),
        }
    }
}

impl Run {
    /// Nonempty criteria in plan order with their owning lane.
    pub fn criteria(&self) -> impl Iterator<Item = (usize, &str)> {
        self.lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_assigned())
            .flat_map(|(i, l)| {
                l.criteria
                    .lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(move |s| (i, s))
            })
    }

    /// Resolve current evidence. Old decisions stay in the run history after a repair.
    pub fn criterion_source<'a>(&'a self, lane: usize, line: &str) -> EvidenceSource<'a> {
        if let Some(decision) = self.decisions.iter().rev().find(|d| {
            d.lane == lane
                && d.criterion == line
                && Some(d.revision.as_str()) == self.review_revision()
        }) {
            return EvidenceSource::User(&decision.decision);
        }
        if let Some(r) = self.regression()
            && r.criterion == line
            && lane == 0
            && r.candidate_revision.is_some()
            && r.candidate_revision == self.review_revision
            && let Some(test) = &r.test
        {
            return EvidenceSource::Test(&test.name);
        }
        let mut assessment = EvidenceSource::Missing;
        for (ticket, phase, report) in self.reports.iter().rev() {
            let (Phase::Review(ix), Report::Review { evidence, .. }) = (phase, report) else {
                continue;
            };
            if ticket.attempt < self.first_review_attempt {
                continue;
            }
            for claim in evidence
                .iter()
                .filter(|e| e.lane == lane && e.confirms(line))
            {
                if let Some(test) = &claim.test
                    && self.check_revision.is_some()
                    && self.check_revision == self.review_revision
                    && self
                        .commands
                        .iter()
                        .find(|c| c.lane == lane)
                        .is_some_and(|c| c.passed_test(test))
                {
                    return EvidenceSource::Test(test);
                }
                if matches!(assessment, EvidenceSource::Missing)
                    && let Some(step) = self.template.reviews.get(*ix)
                {
                    assessment = EvidenceSource::Reviewer(&step.name, &claim.evidence);
                }
            }
        }
        assessment
    }

    fn check_criterion(&self, lane: usize, line: &str, revision: &str) -> Result<(), Error> {
        if self.phase != Phase::Acceptance
            || self.review_revision() != Some(revision)
            || !self.criteria().any(|c| c == (lane, line))
        {
            return Err(Error::InvalidInput(
                "The criterion or its reviewed revision has changed.",
            ));
        }
        Ok(())
    }

    /// Record the user's observation or explicit exception for the entire reviewed revision.
    pub fn decide_criterion(
        &mut self,
        lane: usize,
        line: &str,
        revision: &str,
        decision: CriterionDecision,
    ) -> Result<(), Error> {
        self.check_criterion(lane, line, revision)?;
        let (CriterionDecision::Manual(note) | CriterionDecision::Exception(note)) = &decision;
        if note.trim().is_empty() {
            return Err(Error::InvalidInput("Enter an observation or reason."));
        }
        self.history
            .push(format!("{line} — {}", EvidenceSource::User(&decision)));
        self.decisions.push(RecordedDecision {
            lane,
            criterion: line.into(),
            revision: revision.into(),
            decision,
        });
        Ok(())
    }

    /// Ask QA to check a criterion again without scheduling a developer.
    pub fn verify_criterion(
        &mut self,
        lane: usize,
        line: &str,
        revision: &str,
    ) -> Result<(), Error> {
        self.check_criterion(lane, line, revision)?;
        self.history
            .push(format!("Requested QA verification · {line}"));
        self.qa_focus = Some((lane, line.into()));
        // QA alone, also when it otherwise runs together with other reviews.
        let qa = self.template.qa_index();
        self.phase = Phase::Review(qa);
        self.schedule_review(qa);
        Ok(())
    }
}
