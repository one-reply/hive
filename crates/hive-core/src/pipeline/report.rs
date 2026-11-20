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
    /// Test or observation the reviewer used to confirm it.
    pub evidence: String,
}

/// Result submitted by the bound agent for a particular attempt.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Report {
    /// Proposed repository-specific work.
    Plan {
        /// Ordered lane assignments.
        assignments: Vec<Assignment>,
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
