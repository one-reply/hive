//! Project-independent pipeline recipes and immutable per-task copies of those recipes.
use crate::agent::{AgentKind, launch::AGENTS};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

mod acceptance;
pub use acceptance::{CriterionDecision, EvidenceSource};
mod delivery;
pub mod draft;
mod feedback;
mod handoff;
pub mod images;
mod outcome;
mod presets;
pub(crate) mod prompts;
mod recipe;
mod record;
mod regression;
pub use regression::{Regression, RegressionStep, RegressionTest, TestRunner};
mod report;
mod run;
mod stage;
mod stock;
mod test_output;
mod timing;
mod transitions;
pub mod workspace;
pub use outcome::{CheckTally, readable_history};
pub(crate) use presets::refresh_stock_instructions;
pub use prompts::Prompt;
pub use recipe::unique_name;
pub use report::{Assignment, CommandResult, CriterionEvidence, Finding, Report, Severity};
pub use run::{Phase, Run, StageTiming, Ticket};
pub use stage::{RunProgress, Stage, StageStatus};
pub use stock::ReviewKind;
pub(crate) use stock::{link_stock_prompts, stock_prompts};
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

/// Version of the starter catalog: 1 added the extra starter recipes, 2 rewrote their stock
/// instructions, 3 added the shared contract, 4 introduced code review before QA, and 5
/// updated the code reviewer's instructions.
pub(crate) const CATALOG_VERSION: u64 = 5;

/// Shared templates and independently resumable task runs on this installation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Library {
    // Records which starter recipes have been offered, so renaming one survives a restart.
    pub(crate) catalog_version: u64,
    /// Editable recipes available in every local project.
    pub templates: Vec<Template>,
    // Unique ids and names, mirrored into linked stages: changed only through `prompts.rs`.
    pub(crate) prompts: Vec<Prompt>,
    /// Unsubmitted forms, one per project.
    pub drafts: Vec<draft::LaunchDraft>,
    /// Task runs keyed by stable local IDs.
    pub runs: Vec<Run>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            catalog_version: CATALOG_VERSION,
            templates: Template::presets(),
            prompts: stock_prompts(),
            runs: Vec::new(),
            drafts: Vec::new(),
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
    /// `None` uses the role's own: read-only for the planner and reviewers, the CLI's
    /// configuration for developers. Recipes saved before modes existed have none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<PermissionMode>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            agent: AgentKind::Codex,
            model: String::new(),
            effort: String::new(),
            mode: None,
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

/// How much a stage's CLI may do without asking. Each CLI offers a subset, listed by
/// [`crate::agent::launch::pipeline_modes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Hive passes nothing: the CLI's own configuration decides.
    CliDefault,
    /// File edits inside the worktree go through without a prompt. Everything else the CLI
    /// guards (commands outside its sandbox, network, other folders) still asks.
    AcceptEdits,
    /// The CLI's own automatic review approves or refuses each action instead of the user.
    Auto,
    /// No permission checks and no sandbox.
    Bypass,
    /// Reads and plans without changing files. Also what an unknown saved value means, so a
    /// newer build's choice never widens what an older one allows; serde needs it last.
    #[serde(other)]
    Plan,
}

impl PermissionMode {
    /// Every mode, from the CLI's own configuration to the widest.
    pub const ALL: [Self; 5] = [
        Self::CliDefault,
        Self::Plan,
        Self::AcceptEdits,
        Self::Auto,
        Self::Bypass,
    ];
}

/// Review stages whose identity must survive renaming and prompt customization.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewRole {
    /// Inspects the implementation before QA.
    Code,
    /// The required stage that handles targeted acceptance checks.
    Qa,
    /// An unmarked stage, including those saved before roles were introduced.
    #[default]
    #[serde(other)]
    Unspecified,
}

impl ReviewRole {
    fn is_unspecified(&self) -> bool {
        *self == Self::Unspecified
    }
}

/// One stage's prompt and execution settings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Step {
    /// Name shown on the stage card.
    pub name: String,
    /// Absent in older recipes, whose first review is QA.
    #[serde(default, skip_serializing_if = "ReviewRole::is_unspecified")]
    pub review_role: ReviewRole,
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
    /// Library prompt this stage follows; `instruction` then mirrors that prompt's text.
    /// `None` means the instructions are the stage's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<u64>,
}

fn default_limit() -> u8 {
    3
}

impl Step {
    /// A stage using the default model and three automatic repair rounds.
    pub fn new(name: &str, instruction: &str) -> Self {
        Self {
            name: name.into(),
            review_role: ReviewRole::Unspecified,
            instruction: instruction.into(),
            profile: Profile::default(),
            limit: default_limit(),
            return_lane: None,
            prompt: None,
        }
    }

    /// A stage that follows a library prompt.
    pub fn linked(name: &str, prompt: &Prompt) -> Self {
        Self {
            prompt: Some(prompt.id),
            ..Self::new(name, &prompt.text)
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
    /// Require one shared, human-approved interface contract for all stages.
    pub shared_contract: bool,
    /// Library display name.
    pub name: String,
    /// Initial read-only planner.
    pub planner: Step,
    /// Instructions shared by development lanes.
    pub development: Step,
    /// Reviews in execution order, after the required commands.
    #[serde(default = "presets::legacy_reviews")]
    pub reviews: Vec<Step>,
    /// Named development roles, in display and serialization order.
    pub lanes: Vec<LanePreset>,
}

impl Template {
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
        // Reviews stand in any order: QA is the marked one, or the first of an unmarked recipe.
        let qa = self.qa_index();
        if self.reviews[qa].review_role == ReviewRole::Code
            || self.reviews[qa + 1..]
                .iter()
                .any(|s| s.review_role == ReviewRole::Qa)
        {
            return Err(Error::InvalidInput(
                "A template needs one designated QA review.",
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

    fn steps_mut(&mut self) -> impl Iterator<Item = &mut Step> {
        [&mut self.planner, &mut self.development]
            .into_iter()
            .chain(&mut self.reviews)
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
    /// Branch the lane commits to: an existing local branch at `base`, or a new one that
    /// preparation creates there. Empty in runs saved before the user chose it; those use
    /// the generated `pipeline-<run id>`.
    pub branch: String,
    /// Branch a new `branch` was started from, as the target of its pull request; empty
    /// when the lane works on an existing branch or started from a tag or commit.
    pub base_branch: String,
    /// Actual task worktree; empty until preparation succeeds.
    #[serde(with = "crate::path_encoding::persisted")]
    pub worktree: PathBuf,
    /// Mandatory command as confirmed by the user.
    pub command: String,
    /// Command run once in the new worktree before any agent, empty when unused.
    pub setup: String,
    /// Approved task for this lane, empty when omitted from the plan.
    pub assignment: String,
    /// Acceptance criteria for this assignment.
    pub criteria: String,
    /// Effective development profile.
    pub profile: Profile,
}

impl Lane {
    /// Whether the approved plan gave this lane work. A task of only whitespace is no task:
    /// the plan editor can leave one, and every rule about assigned lanes must agree on it.
    pub fn is_assigned(&self) -> bool {
        !self.assignment.trim().is_empty()
    }

    /// Whether this assigned lane contributes a mandatory QA command.
    pub fn requires_check(&self) -> bool {
        self.is_assigned() && !self.command.trim().is_empty()
    }
}

#[cfg(test)]
mod tests;
