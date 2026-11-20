//! What comes back from a stage: an agent's typed report, a reviewer's weighted findings and
//! the results of the commands Hive runs itself.
use serde::{Deserialize, Serialize};

/// Planner handoff for one selected lane.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Assignment {
    /// Zero-based index in the run's lane list.
    pub lane: usize,
    /// Concrete development assignment.
    pub task: String,
    /// Testable completion criteria.
    pub criteria: String,
}

/// How much a reviewer finding weighs. Only blocking findings send work back to development.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Breaks an acceptance criterion, the build or the tests, loses data or opens a hole.
    Blocker,
    /// A defect to fix before merging. Also what a missing or unreadable value means: a
    /// finding Hive cannot weigh must not be waved through.
    #[default]
    Major,
    /// An improvement that should not hold the change back; shown to the user instead.
    Minor,
}

impl Severity {
    /// Whether the finding returns the work to development.
    pub fn blocks(self) -> bool {
        self != Self::Minor
    }

    /// Name as the report contract and the UI spell it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Blocker => "blocker",
            Self::Major => "major",
            Self::Minor => "minor",
        }
    }
}

/// Reviewer finding addressed to one lane.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Finding {
    /// Repository owning this finding.
    pub lane: usize,
    /// Weight chosen by the reviewer; reports saved before it existed block as before.
    #[serde(default, deserialize_with = "crate::state::lenient::or_default")]
    pub severity: Severity,
    /// Evidence, expected behavior and reproduction or verification steps.
    pub description: String,
}

impl Finding {
    /// Whether a finding the user already accepted is this one again. Severity is left out:
    /// a reviewer re-rating the same text does not reopen the decision.
    pub(super) fn repeats(&self, accepted: &Self) -> bool {
        self.lane == accepted.lane && self.description == accepted.description
    }
}

/// Evidence reported by a reviewer for one approved criterion.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CriterionEvidence {
    /// Assigned lane.
    pub lane: usize,
    /// Exact approved line of acceptance criteria.
    pub criterion: String,
    /// Named result in this lane's required check, verified by Hive before it counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<String>,
    /// Test or observation the reviewer used to confirm it.
    pub evidence: String,
}

impl CriterionEvidence {
    /// Whether this is a concrete observation for `approved`, one line of a lane's acceptance
    /// criteria. Agents repeat a line without its list marker or the spaces around it.
    pub fn confirms(&self, approved: &str) -> bool {
        let claimed = criterion_text(&self.criterion);
        !claimed.is_empty()
            && !self.evidence.trim().is_empty()
            && claimed == criterion_text(approved)
    }
}

/// A criterion line without the bullet, number or checkbox a planner may have put before it.
fn criterion_text(line: &str) -> &str {
    let line = line.trim();
    let unnumbered = line.trim_start_matches(|c: char| c.is_ascii_digit());
    let marked = if unnumbered.len() < line.len() {
        unnumbered.strip_prefix(['.', ')'])
    } else {
        line.strip_prefix(['-', '*', '+', '•'])
    };
    // A marker stands alone or before a space: "-1 is rejected" and "3.5 s budget" are criteria.
    let Some(text) = marked.filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    else {
        return line;
    };
    let text = text.trim_start();
    ["[ ]", "[x]", "[X]"]
        .iter()
        .find_map(|checkbox| text.strip_prefix(checkbox))
        .map_or(text, str::trim_start)
}

/// Result submitted by the bound agent for a particular attempt.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Report {
    /// Proposed repository-specific work.
    Plan {
        /// One proposed interface agreement shared by every participating lane.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        contract: String,
        /// Ordered lane assignments.
        assignments: Vec<Assignment>,
    },
    /// Independent test selected before implementation.
    Regression {
        /// Identity of the test written in the assigned worktree.
        test: super::RegressionTest,
    },
    /// Implementation or repair completed.
    Developed {
        /// What changed and how it was verified.
        summary: String,
    },
    /// Review completed; empty findings represents a pass.
    Review {
        /// Review evidence.
        summary: String,
        /// Confirmed criteria, absent in reports from earlier versions.
        #[serde(default)]
        evidence: Vec<CriterionEvidence>,
        /// Outstanding issues.
        findings: Vec<Finding>,
    },
}

/// Outcome of a mandatory command, recorded by Hive itself.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandResult {
    /// Lane whose worktree hosted the command.
    pub lane: usize,
    /// Exact user-configured command.
    pub command: String,
    /// Whether the command actually completed successfully.
    pub success: bool,
    /// Process exit status, absent on signal, timeout or spawn failure.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Bounded terminal output or diagnostic.
    pub output: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(criterion: &str, evidence: &str) -> CriterionEvidence {
        CriterionEvidence {
            lane: 0,
            criterion: criterion.into(),
            test: None,
            evidence: evidence.into(),
        }
    }

    #[test]
    fn evidence_names_a_criterion_without_its_list_marker_or_surrounding_spaces() {
        let plain = claim(" Expired session returns 401 ", "auth_expired passed");
        for approved in [
            "Expired session returns 401",
            "- Expired session returns 401",
            "  * Expired session returns 401  ",
            "2. Expired session returns 401",
            "3) Expired session returns 401",
            "- [ ] Expired session returns 401",
            "• [x] Expired session returns 401",
        ] {
            assert!(plain.confirms(approved), "{approved:?}");
        }
        assert!(
            claim("- Expired session returns 401", "seen").confirms("Expired session returns 401")
        );
    }

    #[test]
    fn evidence_for_another_criterion_an_empty_one_or_without_an_observation_confirms_nothing() {
        assert!(!claim("Always 200", "seen").confirms("Expired session returns 401"));
        assert!(
            !claim("Expired session returns 401", "  ").confirms("Expired session returns 401")
        );
        for empty in ["", "  ", "-", "- "] {
            assert!(!claim(empty, "seen").confirms(empty), "{empty:?}");
        }
        // Text that only looks like a marker is part of the criterion.
        assert!(!claim("1 is rejected", "seen").confirms("-1 is rejected"));
        assert!(!claim("5 s budget", "seen").confirms("3.5 s budget"));
        assert!(claim("-1 is rejected", "seen").confirms("-1 is rejected"));
    }
}
