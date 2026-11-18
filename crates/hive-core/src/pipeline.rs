//! Project-independent pipeline recipes and immutable per-task copies of those recipes.
use crate::agent::{AgentKind, launch::AGENTS};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

mod outcome;
mod recipe;
mod record;
mod run;
mod stage;
mod timing;
mod transitions;
pub mod workspace;
pub use outcome::{CheckTally, readable_history};
pub use recipe::unique_name;
pub use run::{Phase, Run, StageTiming, Ticket};
pub use stage::{Stage, StageStatus};
pub use timing::now_ms;

/// Invalid pipeline input or an operation inconsistent with the current checkpoint.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A previously issued invocation was completed, replaced or cancelled.
    #[error("This attempt is no longer active")]
    InactiveAttempt,
    /// A user action was offered for a different stage.
    #[error("{0}")]
    WrongStage(&'static str),
    /// The input cannot be turned into a valid task or report.
    #[error("{0}")]
    InvalidInput(&'static str),
    /// A planner assignment is out of range, repeated or has no task or criteria.
    #[error("Assignment for lane {0} is out of range, repeated or missing its task or criteria")]
    InvalidAssignment(usize),
    /// A reviewer finding names a lane without work in this run.
    #[error("Finding for lane {0} names a lane without an assignment, or has no description")]
    InvalidFinding(usize),
    /// A configured return route has no active development lane.
    #[error("Bind and assign return lane {0} before proceeding")]
    MissingLane(String),
    /// The installed CLI cannot represent a given session profile.
    #[error(transparent)]
    Profile(#[from] crate::agent::launch::LaunchError),
}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}

/// Shared templates and independently resumable task runs on this installation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Library {
    // Records which starter recipes have been offered, so renaming one survives a restart.
    pub(crate) catalog_version: u64,
    /// Editable recipes available in every local project.
    pub templates: Vec<Template>,
    /// Task runs keyed by stable local IDs.
    pub runs: Vec<Run>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            catalog_version: 1,
            templates: Template::presets(),
            runs: Vec::new(),
        }
    }
}

/// Interactive CLI and optional session-local model overrides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Installed agent CLI.
    pub agent: AgentKind,
    /// Empty uses the CLI's configured default.
    pub model: String,
    /// Empty uses the model's default effort.
    pub effort: String,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            agent: AgentKind::Codex,
            model: String::new(),
            effort: String::new(),
        }
    }
}

impl Profile {
    /// Reject unlaunchable agents and values that cannot be literal CLI arguments.
    pub fn validate(&self) -> Result<(), Error> {
        if !AGENTS.contains(&self.agent) {
            return Err(Error::Profile(
                crate::agent::launch::LaunchError::Unsupported,
            ));
        }
        for value in [&self.model, &self.effort] {
            if value.starts_with('-') || value.chars().any(char::is_control) || value.len() > 256 {
                return Err(Error::InvalidInput(
                    "Use a literal model or effort value, up to 256 bytes",
                ));
            }
        }
        Ok(())
    }
}

/// One stage's prompt and execution settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Step {
    /// Name shown on the stage card.
    pub name: String,
    /// Instructions added to the task and handoff.
    #[serde(default)]
    pub instruction: String,
    /// CLI for this stage.
    #[serde(default)]
    pub profile: Profile,
    /// Automatic fix rounds allowed for this review.
    #[serde(default = "default_limit")]
    pub limit: u8,
    /// Fixed destination for reviewer findings; otherwise each finding names its lane.
    #[serde(default)]
    pub return_lane: Option<String>,
}

fn default_limit() -> u8 {
    3
}

impl Step {
    /// A stage using the default model and three automatic repair rounds.
    pub fn new(name: &str, instruction: &str) -> Self {
        Self {
            name: name.into(),
            instruction: instruction.into(),
            profile: Profile::default(),
            limit: default_limit(),
            return_lane: None,
        }
    }
}

/// Named development lane, bound to a local repository at run creation.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LanePreset {
    /// Reusable role, such as Web or API.
    pub name: String,
    /// Developer CLI profile.
    pub profile: Profile,
    /// User-configured mandatory QA command, empty when unused.
    pub command: String,
}

/// Ordered stage recipe shared between projects.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Template {
    /// Library display name.
    pub name: String,
    /// Initial read-only planner.
    pub planner: Step,
    /// Instructions shared by development lanes.
    pub development: Step,
    /// Ordered reviews; the first review owns mandatory QA commands.
    pub reviews: Vec<Step>,
    /// Named development roles, in display and serialization order.
    pub lanes: Vec<LanePreset>,
}

impl Default for Template {
    fn default() -> Self {
        Self {
            name: "Feature delivery".into(),
            planner: Step::new(
                "Plan",
                "Break the task into repository-specific assignments and acceptance criteria. Ask for clarification when needed.",
            ),
            development: Step::new(
                "Development",
                "Implement the approved assignment in your worktree. Report what changed and what you verified.",
            ),
            reviews: vec![
                Step::new(
                    "QA",
                    "Verify the acceptance criteria across all participating repositories. Report reproducible findings with the affected lane.",
                ),
                Step::new(
                    "Security & performance",
                    "Review the changes for security and performance regressions. Report specific findings and affected lanes.",
                ),
            ],
            lanes: vec![LanePreset {
                name: "Development".into(),
                ..Default::default()
            }],
        }
    }
}

impl Template {
    /// Starter recipes for a full feature, a targeted fix or an API/client change.
    pub fn presets() -> Vec<Self> {
        let feature = Self::default();
        let mut fix = Self {
            name: "Quick fix".into(),
            ..feature.clone()
        };
        fix.reviews.truncate(1);
        fix.planner.instruction =
            "Identify the root cause, the smallest complete fix and a regression check.".into();
        let cross = Self {
            name: "Cross-repository".into(),
            lanes: ["API", "Web"]
                .map(|name| LanePreset {
                    name: name.into(),
                    ..Default::default()
                })
                .into(),
            planner: Step::new(
                "Plan",
                "Define the shared contract and assign its changes and acceptance criteria to each participating repository.",
            ),
            ..feature.clone()
        };
        vec![feature, fix, cross]
    }

    /// Checks a draft before it becomes an executable recipe.
    pub fn validate(&self) -> Result<(), Error> {
        self.validate_structure()?;
        for profile in self.profiles() {
            crate::agent::launch::validate_pipeline_profile(profile)?;
        }
        Ok(())
    }

    /// Checks names and references without the agent catalog, which changes between releases:
    /// a saved recipe stays loadable after an agent or effort level is retired, and its launch
    /// reports the unsupported profile instead.
    pub fn validate_structure(&self) -> Result<(), Error> {
        if self.name.trim().is_empty() || self.lanes.is_empty() || self.reviews.is_empty() {
            return Err(Error::InvalidInput(
                "Give the template a name, a development lane and a QA review",
            ));
        }
        let mut names = HashSet::new();
        for lane in &self.lanes {
            if lane.name.trim().is_empty() || !names.insert(lane.name.trim()) {
                return Err(Error::InvalidInput("Each lane needs a distinct name"));
            }
        }
        for step in self.steps() {
            if step.name.trim().is_empty() {
                return Err(Error::InvalidInput("Give every stage a name"));
            }
            if let Some(name) = &step.return_lane
                && !names.contains(name.trim())
            {
                return Err(Error::MissingLane(name.clone()));
            }
        }
        Ok(())
    }

    fn steps(&self) -> impl Iterator<Item = &Step> {
        [&self.planner, &self.development]
            .into_iter()
            .chain(&self.reviews)
    }

    fn profiles(&self) -> impl Iterator<Item = &Profile> {
        let lanes = self.lanes.iter().map(|lane| &lane.profile);
        lanes.chain(self.steps().map(|step| &step.profile))
    }
}

/// One development role bound to a Git repository and an isolated worktree.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Lane {
    /// Name used for assignment and review.
    pub name: String,
    /// Local project root.
    #[serde(with = "crate::path_encoding::persisted")]
    pub project: PathBuf,
    /// Starting ref, resolved by preparation to an immutable commit.
    pub base: String,
    /// Actual task worktree; empty until preparation succeeds.
    #[serde(with = "crate::path_encoding::persisted")]
    pub worktree: PathBuf,
    /// Mandatory command as confirmed by the user.
    pub command: String,
    /// Approved task for this lane, empty when omitted from the plan.
    pub assignment: String,
    /// Acceptance criteria for this assignment.
    pub criteria: String,
    /// Effective development profile.
    pub profile: Profile,
}

impl Lane {
    /// Whether this assigned lane contributes a mandatory QA command.
    pub fn requires_check(&self) -> bool {
        !self.assignment.trim().is_empty() && !self.command.trim().is_empty()
    }
}

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

/// Reviewer finding addressed to one lane.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Finding {
    /// Repository owning this finding.
    pub lane: usize,
    /// Evidence, expected behavior and reproduction or verification steps.
    pub description: String,
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
mod tests;
