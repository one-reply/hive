//! An independent test is identified before development and stays pinned through repairs.
use super::{Error, Phase, Run, workspace::RegressionResult};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

/// Checkpoints of the optional single-lane regression workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegressionStep {
    /// The user selects one of the approved criteria.
    Choose,
    /// A fresh developer-profile session writes only the test.
    Author,
    /// Hive runs the test against the base commit.
    Baseline,
    /// Hive runs exactly the pinned test in the candidate.
    Candidate,
    /// A failed verification or changed test needs a decision.
    Decision,
}

impl RegressionStep {
    /// True at a human decision.
    pub fn needs_user(self) -> bool {
        matches!(self, Self::Choose | Self::Decision)
    }
}

/// Supported local test executables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestRunner {
    /// Stable Rust test harness.
    Cargo,
    /// Project-installed Jest.
    Jest,
    /// Project-installed Vitest.
    Vitest,
}

/// Input to a fixed runner invocation. The agent cannot specify a shell command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegressionTest {
    /// Local runner.
    pub runner: TestRunner,
    /// Repository-relative dedicated test file.
    pub path: String,
    /// Exact, fully qualified test name.
    pub name: String,
    /// Expected diagnostic from the failing assertion on the base commit.
    pub expected: String,
}

impl RegressionTest {
    pub(super) fn validate(&self) -> Result<(), Error> {
        let path = Path::new(&self.path);
        let valid_path = !self.path.is_empty()
            && self.path.len() <= 1024
            && !self.path.starts_with('-')
            && self.path.split('/').all(|s| {
                !s.is_empty()
                    && !s.starts_with('.')
                    && !s.starts_with('-')
                    && s.chars().all(|c| c.is_alphanumeric() || "_-.".contains(c))
            })
            && path.components().all(|c| matches!(c, Component::Normal(_)));
        let file = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let is_test = match self.runner {
            TestRunner::Cargo => {
                path.parent()
                    .and_then(Path::file_name)
                    .is_some_and(|s| s == "tests")
                    && file.ends_with(".rs")
            }
            _ => {
                file.split('.')
                    .rev()
                    .nth(1)
                    .is_some_and(|p| matches!(p, "test" | "spec"))
                    && matches!(
                        path.extension().and_then(|s| s.to_str()),
                        Some("ts" | "js" | "tsx" | "jsx" | "mjs" | "cjs" | "mts" | "cts")
                    )
            }
        };
        if !valid_path
            || !is_test
            || self.name.trim().is_empty()
            || self.name.starts_with('-')
            || self.name.len() > 512
            || self.expected.trim().is_empty()
            || self.expected.len() > 512
            || self
                .name
                .chars()
                .chain(self.expected.chars())
                .any(char::is_control)
        {
            return Err(Error::InvalidInput(
                "Supply a relative test file, exact test name and expected assertion diagnostic.",
            ));
        }
        Ok(())
    }
}

/// The saved authoring and verification record for one approved criterion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Regression {
    pub(super) criterion: String,
    pub(super) test: Option<RegressionTest>,
    pub(super) blob: Option<String>,
    pub(super) baseline: String,
    pub(super) candidate: String,
    pub(super) candidate_revision: Option<String>,
    pub(super) after_development: bool,
    pub(super) blocked_at: Option<RegressionStep>,
    pub(super) changed_blob: Option<String>,
    pub(super) problem: String,
}

impl Regression {
    /// Selected criterion and recorded diagnostics for Checks and Timeline.
    pub fn description(&self) -> String {
        let mut parts = vec![self.criterion.clone()];
        if let Some(t) = &self.test {
            parts.push(format!(
                "{:?} · {} · {}\nExpected failure: {}",
                t.runner, t.path, t.name, t.expected
            ));
        }
        for (label, detail) in [
            ("Base", &self.baseline),
            ("Candidate", &self.candidate),
            ("Needs you", &self.problem),
        ] {
            if !detail.is_empty() {
                parts.push(format!("{label} · {detail}"));
            }
        }
        parts.join("\n")
    }
    /// Candidate test revision requiring explicit approval.
    pub fn changed_blob(&self) -> Option<&str> {
        self.changed_blob.as_deref()
    }
    /// Whether the developer has already worked on this candidate.
    pub fn can_repair(&self) -> bool {
        self.after_development
    }
}

impl Run {
    /// Optional regression verification record.
    pub fn regression(&self) -> Option<&Regression> {
        self.regression.as_ref()
    }

    /// Enable the workflow before the new run starts.
    pub fn enable_regression(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Planning || !self.history.is_empty() || self.lanes.len() != 1 {
            return Err(Error::InvalidInput(
                "Independent regression testing requires a new single-lane run.",
            ));
        }
        self.regression = Some(Regression::default());
        Ok(())
    }

    pub(super) fn start_development(&mut self, lanes: Vec<usize>) {
        if self.regression.is_none() {
            self.develop(lanes);
            return;
        }
        self.phase = Phase::Regression(RegressionStep::Choose);
        let mut criteria = self.criteria().map(|(_, line)| line.to_owned());
        let first = criteria.next();
        let second = criteria.next();
        drop(criteria);
        if let (Some(line), None) = (first, second) {
            self.start_author(&line);
        }
    }

    fn start_author(&mut self, line: &str) {
        if let Some(r) = &mut self.regression {
            r.criterion = line.into();
        }
        self.phase = Phase::Regression(RegressionStep::Author);
        self.schedule(None);
    }

    /// Select the single regression scenario from the approved criteria.
    pub fn choose_regression(&mut self, line: &str) -> Result<(), Error> {
        if self.phase != Phase::Regression(RegressionStep::Choose)
            || !self.criteria().any(|(_, l)| l == line)
        {
            return Err(Error::InvalidInput("Select an approved criterion."));
        }
        self.start_author(line);
        Ok(())
    }

    pub(super) fn regression_prompt(&self) -> Result<String, Error> {
        let r = self.regression.as_ref().ok_or(Error::InactiveAttempt)?;
        let mut prompt = format!(
            "Independently write one regression test before the fix. User task: {}\nApproved criterion: {}\nWork only on one dedicated integration test file; do not implement the fix, edit other files or change checks. Use a Cargo tests/<target>.rs file or a Jest/Vitest *.test.* or *.spec.* file. Include a distinctive expected assertion failure. Report {{\"kind\":\"regression\",\"test\":{{\"runner\":\"cargo|jest|vitest\",\"path\":\"relative test path\",\"name\":\"exact full test name\",\"expected\":\"assertion diagnostic substring\"}}}}.",
            self.task, r.criterion
        );
        if !r.problem.is_empty() {
            prompt.push_str("\nPrevious baseline check: ");
            prompt.push_str(&r.problem);
        }
        Ok(prompt)
    }

    pub(super) fn submit_regression_test(&mut self, test: RegressionTest) -> Result<(), Error> {
        test.validate()?;
        let r = self.regression.as_mut().ok_or(Error::InactiveAttempt)?;
        r.test = Some(test);
        r.problem.clear();
        r.blocked_at = None;
        r.changed_blob = None;
        r.blob = None;
        r.baseline.clear();
        r.candidate.clear();
        self.outstanding.clear();
        self.phase = Phase::Regression(RegressionStep::Baseline);
        Ok(())
    }

    pub(super) fn begin_validation(&mut self) {
        if let Some(r) = &mut self.regression {
            r.after_development = true;
            r.candidate_revision = None;
            r.candidate.clear();
            r.problem.clear();
            r.blocked_at = None;
            r.changed_blob = None;
            self.phase = Phase::Regression(if r.baseline.is_empty() {
                RegressionStep::Baseline
            } else {
                RegressionStep::Candidate
            });
        } else {
            self.begin_qa();
        }
    }

    /// True when a saved phase requires a background command instead of an agent.
    pub fn has_command_work(&self) -> bool {
        matches!(
            self.phase,
            Phase::Commands(_)
                | Phase::Regression(RegressionStep::Baseline | RegressionStep::Candidate)
        )
    }

    /// Consume the bounded execution result and checkpoint before releasing the next stage.
    pub fn record_regression(&mut self, result: RegressionResult) -> Result<(), Error> {
        let Phase::Regression(step @ (RegressionStep::Baseline | RegressionStep::Candidate)) =
            self.phase
        else {
            return Err(Error::InactiveAttempt);
        };
        let r = self.regression.as_mut().ok_or(Error::InactiveAttempt)?;
        match result {
            RegressionResult::Matched {
                blob,
                revision,
                detail,
            } => {
                if !super::workspace::is_object_name(&blob)
                    || r.blob.as_ref().is_some_and(|pinned| pinned != &blob)
                    || (step == RegressionStep::Candidate && r.blob.is_none())
                {
                    return Err(Error::InvalidInput("Test revision changed."));
                }
                self.history.push(format!("Regression {step:?} · {detail}"));
                if step == RegressionStep::Baseline {
                    r.blob = Some(blob);
                    r.baseline = detail;
                    if r.after_development {
                        self.phase = Phase::Regression(RegressionStep::Candidate);
                    } else {
                        self.develop([0]);
                    }
                } else {
                    r.candidate = detail;
                    r.candidate_revision = Some(revision);
                    self.begin_qa();
                }
            }
            RegressionResult::NeedsDecision {
                detail,
                changed_blob,
            } => {
                self.history
                    .push(format!("Regression {step:?} needs you · {detail}"));
                r.problem = detail;
                r.changed_blob = changed_blob;
                r.blocked_at = Some(step);
                self.phase = Phase::Regression(RegressionStep::Decision);
            }
        }
        Ok(())
    }

    /// Retry the failed execution, or authorize a specific changed test and repeat both runs.
    pub fn retry_regression(&mut self, approve_blob: Option<&str>) -> Result<(), Error> {
        if self.phase != Phase::Regression(RegressionStep::Decision) {
            return Err(Error::InactiveAttempt);
        }
        let r = self.regression.as_mut().ok_or(Error::InactiveAttempt)?;
        let next = if let Some(blob) = approve_blob {
            if r.changed_blob.as_deref() != Some(blob) {
                return Err(Error::InvalidInput("The proposed test version changed."));
            }
            r.blob = Some(blob.into());
            r.candidate_revision = None;
            r.baseline.clear();
            r.candidate.clear();
            self.history
                .push(format!("User approved revised regression test {blob}"));
            RegressionStep::Baseline
        } else {
            r.blocked_at.ok_or(Error::InactiveAttempt)?
        };
        r.problem.clear();
        r.blocked_at = None;
        r.changed_blob = None;
        self.phase = Phase::Regression(next);
        Ok(())
    }

    /// Ask the author for another reproducer, or the developer for a candidate repair.
    pub fn repair_regression(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Regression(RegressionStep::Decision) {
            return Err(Error::InactiveAttempt);
        }
        let r = self.regression.as_ref().ok_or(Error::InactiveAttempt)?;
        if r.can_repair() {
            self.feedback = format!(
                "Fix the regression without changing the approved test. {}",
                r.description()
            );
            self.briefs.clear();
            self.develop([0]);
            self.continue_conversations();
        } else {
            let line = r.criterion.clone();
            self.start_author(&line);
        }
        Ok(())
    }

    pub(super) fn regression_valid(&self, step: RegressionStep) -> bool {
        self.lanes.len() == 1
            && self.regression.as_ref().is_some_and(|r| {
                (step == RegressionStep::Choose || self.criteria().any(|(_, s)| s == r.criterion))
                    && (matches!(step, RegressionStep::Choose | RegressionStep::Author)
                        || r.test.as_ref().is_some_and(|t| t.validate().is_ok()))
                    && r.blob
                        .as_deref()
                        .is_none_or(super::workspace::is_object_name)
                    && r.changed_blob
                        .as_deref()
                        .is_none_or(super::workspace::is_object_name)
                    && (step != RegressionStep::Candidate
                        || (r.blob.is_some() && !r.baseline.is_empty()))
                    && (step != RegressionStep::Decision
                        || matches!(
                            r.blocked_at,
                            Some(RegressionStep::Candidate | RegressionStep::Baseline)
                        ))
            })
    }
}
