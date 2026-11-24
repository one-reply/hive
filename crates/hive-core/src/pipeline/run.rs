//! One task's immutable recipe, gate decisions and outstanding stage attempts.
use super::*;
use crate::agent::launch::ResumeSession;
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Current execution gate; a run has exactly one gate even when lanes run in parallel.
/// No catch-all variant: this build cannot drive a gate it does not know, so loading drops
/// such a run alone and reports it in the recovery notice.
pub enum Phase {
    /// Waiting for the planner to report.
    Planning,
    /// The proposed plan is available for human editing and approval.
    ApprovePlan,
    /// Independently authored test and its two executions.
    Regression(RegressionStep),
    /// Independent repositories may develop simultaneously.
    Development,
    /// One read-only reviewer sees every repository.
    Review(usize),
    /// Mandatory commands. The index names the review to continue when they pass.
    /// A failing command uses the required QA stage's repair budget.
    Commands(usize),
    /// Automatic repair budget exhausted.
    Decision(usize),
    /// Final diffs and results await human acceptance.
    Acceptance,
    /// Human accepted the changes.
    Accepted,
    /// User stopped this run.
    Cancelled,
}

/// Identity of exactly one agent invocation, never reused within a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ticket {
    /// Run identity.
    pub run: u64,
    /// Monotonic invocation number.
    pub attempt: u64,
    /// Developer lane, absent for aggregate stages.
    pub lane: Option<usize>,
}

/// Measured wall-clock interval of one agent invocation or checkpoint, in Unix milliseconds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageTiming {
    /// Stage being measured.
    pub phase: Phase,
    /// Bound development lane, when individual work is timed.
    pub lane: Option<usize>,
    /// Ticket number for an actual CLI session.
    pub attempt: Option<u64>,
    /// When work or a human checkpoint started.
    pub start_ms: u64,
    /// When the stage stopped; absent while it is active.
    pub end_ms: Option<u64>,
    /// The recorded interval ended unsuccessfully.
    #[serde(default)]
    pub failed: bool,
}

/// Serializable state machine; process and UI handles belong to the application.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// Stable run identity.
    pub(super) id: u64,
    /// Local project that owns this task in the sidebar.
    #[serde(with = "crate::path_encoding::persisted")]
    pub(super) owner: PathBuf,
    #[serde(default)]
    pub(super) direct_start: bool,
    /// Original user request.
    pub(super) task: String,
    /// Recipe frozen at launch.
    pub(super) template: Template,
    /// Approved repository bindings.
    pub(super) lanes: Vec<Lane>,
    #[serde(default)]
    pub(super) contract: String,
    #[serde(default)]
    pub(super) regression: Option<Regression>,
    /// Current gate.
    pub(super) phase: Phase,
    /// Chronological handoffs, decisions and test outcomes.
    pub(super) history: Vec<String>,
    /// Accepted handoffs tied to their actual agent invocation and phase. A report kind or
    /// phase from a newer Hive drops that entry only, not the run.
    #[serde(default, deserialize_with = "crate::state::lenient::elements")]
    pub(super) reports: Vec<(Ticket, Phase, Report)>,
    /// Observed stage intervals; older runs contain only their handoff history.
    #[serde(default, deserialize_with = "crate::state::lenient::elements")]
    pub(super) timings: Vec<StageTiming>,
    /// Open findings for the most recent review.
    pub(super) findings: Vec<Finding>,
    /// Most recent QA command results.
    pub(super) commands: Vec<CommandResult>,
    /// Content fingerprint taken before QA, checked again on final acceptance.
    #[serde(default)]
    pub(super) review_revision: Option<String>,
    /// Permanent addresses of delivered requests, keyed by representative lane.
    #[serde(default)]
    pub(super) requests: Vec<(usize, String)>,
    #[serde(default)]
    pub(super) review_trees: Vec<String>,
    #[serde(default)]
    pub(super) feedback_trees: Vec<String>,
    /// Unsent line notes with their receiving lane and reviewed tree.
    #[serde(default, deserialize_with = "crate::state::lenient::elements")]
    pub(super) comments: Vec<(usize, String, crate::git::review_notes::Note)>,
    #[serde(default)]
    pub(super) decisions: Vec<acceptance::RecordedDecision>,
    #[serde(default)]
    pub(super) qa_focus: Option<(usize, String)>,
    #[serde(default)]
    pub(super) review_brief: String,
    #[serde(default)]
    pub(super) check_revision: Option<String>,
    #[serde(default)]
    pub(super) first_review_attempt: u64,
    /// Known CLI session IDs, linked to the exact attempts that created them.
    #[serde(default)]
    pub(super) sessions: Vec<(Ticket, ResumeSession)>,
    pub(super) outstanding: Vec<Ticket>,
    pub(super) last_attempt: u64,
    pub(super) rounds: Vec<u8>,
    #[serde(default)]
    pub(super) dismissed: Vec<(usize, Finding)>,
    /// Non-blocking findings of the current QA pass, with the review that reported each.
    #[serde(default)]
    pub(super) notes: Vec<(usize, Finding)>,
    /// Reviews the current QA pass leaves out: their path condition matched nothing the pass
    /// reviews. Decided together with the pass's fingerprint.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) skipped: Vec<usize>,
    #[serde(default)]
    pub(super) feedback: String,
    /// Repair instructions addressed to one lane each; a lane without one reads `feedback`.
    #[serde(default)]
    pub(super) briefs: Vec<(usize, String)>,
    /// Development attempts that continue their lane's earlier CLI conversation instead of
    /// starting a new one. A retried attempt gets a new ticket and so starts fresh.
    #[serde(default)]
    pub(super) continuations: Vec<(Ticket, ResumeSession)>,
}

impl Run {
    /// Create a task from explicit local repository bindings.
    pub fn new(
        id: u64,
        owner: PathBuf,
        task: String,
        template: Template,
        lanes: Vec<Lane>,
    ) -> Result<Self, Error> {
        template.validate()?;
        if !owner.is_absolute() || task.trim().is_empty() || id == 0 || lanes.is_empty() {
            return Err(Error::InvalidInput(
                "Choose a local project, enter a task and bind at least one lane",
            ));
        }
        let mut names = HashSet::new();
        for lane in &lanes {
            // Trimmed like `Template::validate_structure`, so " Web" and "Web" are one name.
            if !lane.project.is_absolute()
                || lane.name.trim().is_empty()
                || !names.insert(lane.name.trim())
            {
                return Err(Error::InvalidInput(
                    "Bind every lane to a local repository and give it a unique name",
                ));
            }
            crate::agent::launch::validate_pipeline_profile(&lane.profile)?;
        }
        for step in &template.reviews {
            if let Some(lane) = &step.return_lane
                && !names.contains(lane.trim())
            {
                return Err(Error::MissingLane(lane.clone()));
            }
        }
        let reviews = template.reviews.len();
        let mut run = Self {
            id,
            owner,
            task,
            template,
            lanes,
            phase: Phase::Planning,
            contract: String::new(),
            regression: None,
            history: vec![],
            timings: vec![],
            reports: vec![],
            findings: vec![],
            commands: vec![],
            sessions: vec![],
            review_revision: None,
            requests: vec![],
            direct_start: false,
            review_trees: vec![],
            feedback_trees: vec![],
            comments: vec![],
            decisions: vec![],
            qa_focus: None,
            review_brief: String::new(),
            check_revision: None,
            first_review_attempt: 0,
            outstanding: vec![],
            last_attempt: 0,
            rounds: vec![0; reviews],
            dismissed: vec![],
            notes: vec![],
            skipped: vec![],
            feedback: String::new(),
            briefs: vec![],
            continuations: vec![],
        };
        run.schedule(None);
        Ok(run)
    }

    pub(super) fn schedule(&mut self, lane: Option<usize>) -> Ticket {
        // One ticket per invocation, bounded by state.json size even after manual retries.
        self.last_attempt += 1;
        let ticket = Ticket {
            run: self.id,
            attempt: self.last_attempt,
            lane,
        };
        self.outstanding.push(ticket);
        ticket
    }

    /// Attempts runnable now. Within a repository the earliest unfinished lane owns the worktree.
    pub fn pending(&self) -> Vec<Ticket> {
        let mut repos = HashSet::new();
        self.outstanding
            .iter()
            .copied()
            .filter(|t| match t.lane {
                None => true,
                Some(ix) => self.lanes.get(ix).is_some_and(|lane| {
                    repos.insert(if lane.worktree.as_os_str().is_empty() {
                        &lane.project
                    } else {
                        &lane.worktree
                    })
                }),
            })
            .collect()
    }

    /// Recipe stage for this live ticket.
    pub fn step(&self, ticket: &Ticket) -> Option<&Step> {
        if !self.pending().contains(ticket) {
            return None;
        }
        match self.phase {
            Phase::Planning => Some(&self.template.planner),
            Phase::Regression(RegressionStep::Author) => Some(&self.template.development),
            Phase::Development => Some(&self.template.development),
            Phase::Review(ix) => self.template.reviews.get(ix),
            _ => None,
        }
    }

    /// Effective CLI profile, including per-lane development overrides.
    pub fn profile(&self, ticket: &Ticket) -> Option<&Profile> {
        let step = self.step(ticket)?;
        if self.phase == Phase::Regression(RegressionStep::Author) {
            return self.lanes.first().map(|l| &l.profile);
        }
        Some(
            ticket
                .lane
                .and_then(|ix| self.lanes.get(ix))
                .map_or(&step.profile, |l| &l.profile),
        )
    }

    /// Number of repair rounds already charged to this review.
    pub fn cycles(&self, review: usize) -> u8 {
        self.rounds.get(review).copied().unwrap_or_default()
    }

    /// Prompt for an invocation, including the current plan and cross-repository context.
    /// Images pasted into the task are named by their files on this machine.
    pub fn prompt(&self, ticket: &Ticket) -> Result<String, Error> {
        let prompt = self.stage_prompt(ticket)?;
        Ok(
            match super::images::for_agent(&prompt, &super::images::directory()) {
                std::borrow::Cow::Owned(expanded) => expanded,
                std::borrow::Cow::Borrowed(_) => prompt,
            },
        )
    }

    fn stage_prompt(&self, ticket: &Ticket) -> Result<String, Error> {
        let step = self.step(ticket).ok_or(Error::InactiveAttempt)?;
        if self.phase == Phase::Regression(RegressionStep::Author) {
            return self.regression_prompt();
        }
        let mut prompt = format!("{}\n\nUser task:\n{}\n", step.instruction, self.task);
        if self.template.shared_contract {
            if self.phase == Phase::Planning {
                prompt.push_str("\nSupply one top-level contract string in the plan report for all lanes: interface, errors, compatibility, rollout where applicable. Reference it from each assignment.\n");
            }
            if !self.contract.is_empty() {
                let _ = writeln!(prompt, "\nShared contract:\n{}", self.contract);
            }
        }
        let reviewing = matches!(self.phase, Phase::Review(_));
        for (i, lane) in self.lanes.iter().enumerate() {
            // A reviewer may only file findings against lanes that have work to review.
            if ticket.lane.is_some_and(|index| index != i) || (reviewing && !lane.is_assigned()) {
                continue;
            }
            // The base commit bounds the lane's changes: without it a reviewer of committed
            // work sees an empty `git diff`.
            let _ = writeln!(
                prompt,
                "\nLane {i}: {}\nWorktree: {}\nBase commit: {}\nAssignment: {}\nCriteria: {}",
                lane.name,
                lane.worktree.display(),
                lane.base,
                lane.assignment,
                lane.criteria
            );
        }
        let handoff = self.handoff(ticket);
        if !reviewing && !handoff.is_empty() {
            let _ = writeln!(prompt, "\nHandoff:\n{handoff}");
        }
        if self.phase == Phase::Development {
            self.write_developer_constraints(&mut prompt);
        }
        if let Phase::Review(ix) = self.phase {
            self.write_review_context(ix, &mut prompt);
        }
        let contract = match self.phase {
            Phase::Planning if self.template.shared_contract => {
                r#"Report {"kind":"plan","contract":"the proposed API, compatibility and rollout","assignments":[{"lane":0,"task":"...","criteria":"..."}]}. Include each required lane and put each testable criterion on its own line."#
            }
            Phase::Planning => {
                r#"Plan only. Ask the user if necessary. Report {"kind":"plan","assignments":[{"lane":0,"task":"...","criteria":"..."}]} with one entry per required lane. Put each separately testable acceptance criterion on its own line."#
            }
            Phase::Development => super::handoff::DEVELOPED_CONTRACT,
            Phase::Review(_) => {
                r#"Inspect the participating worktrees together. A lane's changes are everything after its base commit: `git diff <base commit>` shows committed and uncommitted edits together, `git status` lists new files. Report {"kind":"review","summary":"evidence","evidence":[{"lane":0,"criterion":"exact approved line","evidence":"specific test or observation","test":"optional exact passing test name from Hive output"}],"findings":[{"lane":0,"severity":"major","description":"issue, reproduction and expected behavior"}]}. Severity is "blocker" (breaks an acceptance criterion, the build or the tests, or loses data), "major" (a defect to fix before merging) or "minor" (an improvement that should not hold the change back). Blocker and major findings return the work to development; minor ones are shown to the user. List only criteria you actually confirmed. Unverified criteria have no evidence entry. Use an empty findings list to pass."#
            }
            _ => return Err(Error::WrongStage("This gate has no agent")),
        };
        let _ = writeln!(prompt, "\n{contract}");
        Ok(prompt)
    }

    /// Invalidate a disconnected invocation and issue a fresh ticket for the same work.
    pub fn restart_attempt(&mut self, ticket: &Ticket) -> Result<Ticket, Error> {
        if !self.pending().contains(ticket) {
            return Err(Error::InactiveAttempt);
        }
        // Preserve the position: a second lane in the same repository is still waiting.
        self.last_attempt += 1;
        let replacement = Ticket {
            attempt: self.last_attempt,
            ..*ticket
        };
        let slot = self
            .outstanding
            .iter_mut()
            .find(|t| *t == ticket)
            .ok_or(Error::InactiveAttempt)?;
        *slot = replacement;
        // Whatever stopped the attempt may be the conversation itself: the retry starts fresh.
        self.continuations.retain(|(t, _)| t != ticket);
        self.history.push(format!(
            "Retried attempt {} as {}",
            ticket.attempt, replacement.attempt
        ));
        Ok(replacement)
    }

    /// Check indices and identities before rendering or resuming an on-disk snapshot.
    pub(crate) fn valid_snapshot(&self) -> bool {
        if self.id == 0
            || !self.owner.is_absolute()
            || self.lanes.is_empty()
            || [self.review_trees(), self.feedback_trees()]
                .iter()
                .any(|trees| {
                    !trees.is_empty()
                        && (trees.len() != self.lanes.len()
                            || trees.iter().any(|oid| !workspace::is_object_name(oid)))
                })
            || self.template.validate_structure().is_err()
            || self.rounds.len() != self.template.reviews.len()
            || self.last_attempt > u64::MAX / 2
            || self.lanes.iter().any(|l| !l.project.is_absolute())
            || self.findings.iter().any(|f| f.lane >= self.lanes.len())
            || self.commands.iter().any(|c| c.lane >= self.lanes.len())
            || self
                .dismissed
                .iter()
                .any(|(_, f)| f.lane >= self.lanes.len())
            || self
                .notes
                .iter()
                .any(|(review, f)| *review >= self.rounds.len() || f.lane >= self.lanes.len())
            // Only a review with a condition can be left out; anything else would be a
            // review that silently never ran.
            || self
                .skipped
                .iter()
                .any(|review| self.template.review_condition(*review).is_none())
            || self
                .briefs
                .iter()
                .any(|(lane, _)| *lane >= self.lanes.len())
        {
            return false;
        }
        let mut attempts = HashSet::new();
        let mut lanes = HashSet::new();
        if self.outstanding.iter().any(|t| {
            t.run != self.id
                || t.attempt == 0
                || t.attempt > self.last_attempt
                || !attempts.insert(t.attempt)
                || !lanes.insert(t.lane)
        }) {
            return false;
        }
        let aggregate = self.outstanding.len() == 1 && self.outstanding[0].lane.is_none();
        match self.phase {
            Phase::Regression(step) => {
                self.regression_valid(step)
                    && if step == RegressionStep::Author {
                        aggregate
                    } else {
                        self.outstanding.is_empty()
                    }
            }
            Phase::Planning => aggregate,
            Phase::Review(ix) => ix < self.rounds.len() && aggregate,
            Phase::Development => {
                !self.outstanding.is_empty()
                    && self
                        .outstanding
                        .iter()
                        .all(|t| t.lane.is_some_and(|ix| ix < self.lanes.len()))
            }
            Phase::Decision(ix) | Phase::Commands(ix) => {
                ix < self.rounds.len() && self.outstanding.is_empty()
            }
            _ => self.outstanding.is_empty(),
        }
    }
}
