//! Unsubmitted, project-local launch choices. Restoring these values never starts work.
use super::{LanePreset, Template};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Editable launch saved across navigation and app restarts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchDraft {
    /// Project owning the composer.
    #[serde(with = "crate::path_encoding::persisted")]
    pub owner: PathBuf,
    /// Recipe as displayed in the form.
    pub template: Template,
    /// Earlier run's recipe, when switching versions on a repeat.
    #[serde(default)]
    pub original: Option<Template>,
    /// Retain the original instructions at launch.
    #[serde(default)]
    pub frozen: bool,
    /// Task entered by the user.
    pub task: String,
    /// Bypass planning with explicit criteria.
    #[serde(default)]
    pub direct: bool,
    /// Criteria for the short route.
    #[serde(default)]
    pub criteria: String,
    /// Ordered repository fields, including temporarily unbound lanes.
    pub lanes: Vec<DraftLane>,
}

/// Repository choices in an unsubmitted launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftLane {
    /// Role and agent choices.
    pub preset: LanePreset,
    /// Selected project, if any.
    #[serde(default, with = "crate::path_encoding::persisted::optional")]
    pub project: Option<PathBuf>,
    /// Fully qualified base ref, tag, or commit to preview again.
    pub reference: String,
    /// Work branch selection.
    pub branch: WorkBranch,
    /// Required check selection.
    pub check: DraftCheck,
    /// Text entered for a custom check.
    pub custom_command: String,
    /// Command for preparing the checkout.
    pub setup: String,
}

/// Source of a required command before the base preview is available.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DraftCheck {
    /// Discover the default at the selected base.
    Suggested,
    /// Explicit choice, including an empty command.
    Command(String),
    /// Use the text from the custom command field.
    Custom,
}

/// Local branch the user has chosen for a task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkBranch {
    /// Branch has yet to be selected.
    Unchosen,
    /// Use an available local branch.
    Existing(String),
    /// Create a new branch from the selected base.
    New(String),
}
impl WorkBranch {
    /// Chosen short name, used when checking or starting this lane.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Unchosen => None,
            Self::Existing(s) | Self::New(s) => Some(s),
        }
    }
}
