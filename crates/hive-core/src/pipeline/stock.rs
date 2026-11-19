//! The built-in prompts: what a new prompt library holds and what starter stages follow.
//!
//! Hive appends the task, the lane assignments, the handoff and the report contract to these
//! texts, so they only describe how to do the stage's work well, not the output format.
use super::{Library, Prompt, Step, Template};

/// A built-in prompt. Saved links hold its id, so an id is never changed or reused; a new
/// built-in takes the next free one below `prompts::FIRST_USER_ID`.
pub(super) struct Stock {
    id: u64,
    name: &'static str,
    pub(super) text: &'static str,
}

impl Stock {
    fn prompt(&self) -> Prompt {
        Prompt {
            id: self.id,
            name: self.name.into(),
            text: self.text.into(),
        }
    }

    /// A starter stage following this prompt.
    pub(super) fn stage(&self, name: &str) -> Step {
        Step::linked(name, &self.prompt())
    }
}

pub(super) const FEATURE_PLANNER: Stock = Stock {
    id: 1,
    name: "Feature planner",
    text: "Read the code the task touches in every listed worktree before planning; do not edit files. Give each lane that needs work a self-contained assignment: the modules to change, the expected behavior, and the tests to add or update. Write acceptance criteria as observable checks (a test, a command, a reproducible behavior), not intentions. Leave out lanes that need no changes. Ask the user when the requirements are ambiguous or a decision has real trade-offs instead of guessing.",
};

pub(super) const DEVELOPER: Stock = Stock {
    id: 2,
    name: "Developer",
    text: "Read the code your assignment touches and follow the repository's conventions and agent instructions (AGENTS.md, CLAUDE.md, CONTRIBUTING). Implement the smallest complete change that meets every acceptance criterion and fix root causes, not symptoms. Add or update tests for the new behavior, then run the project's formatter, linter and tests. If the handoff lists findings for your lane, resolve each one and say how. Do not push or switch branches. Report what changed, the commands you ran with their results, and what you could not verify.",
};

pub(super) const QA_REVIEWER: Stock = Stock {
    id: 3,
    name: "QA reviewer",
    text: "Verify, do not fix. In each worktree inspect the actual changes (git status, git diff, the branch's commits) and check every acceptance criterion against the code and tests, not against the developer's summary. Look for unhandled edge cases and error paths, broken callers of changed code, and missing or weak tests. Your sandbox is read-only, so you may be unable to build; Hive runs each lane's configured checks itself. Report only concrete defects, each with its location, a reproduction or failing case, and the expected behavior; skip style preferences.",
};

pub(super) const SECURITY_AND_PERFORMANCE_REVIEWER: Stock = Stock {
    id: 4,
    name: "Security & performance reviewer",
    text: "Review only the changed code and the paths it affects. Security: untrusted input reaching shell commands, queries, file paths or HTML; missing authentication or authorization checks; secrets in code, logs or errors; unsafe deserialization; risky new dependencies. Performance: extra work on hot paths, I/O or queries inside loops, unbounded memory or result sizes, blocking calls on async or UI threads. Report only issues with a concrete exploit or a measurable cost, each with its location and a fix; skip theoretical concerns and code the change does not touch.",
};

pub(super) const FIX_PLANNER: Stock = Stock {
    id: 5,
    name: "Bug-fix planner",
    text: "Locate the bug by reading the code; do not edit files. Identify the root cause and every caller that shares the faulty path, then assign the smallest complete fix to the lane that owns it. Acceptance criteria must include the original symptom gone and a regression test that fails before the fix and passes after it. Ask the user when you cannot tell what the correct behavior is.",
};

pub(super) const FIX_DEVELOPER: Stock = Stock {
    id: 6,
    name: "Bug-fix developer",
    text: "Fix the root cause in the shared code path, not only the reported call site. First write a regression test that fails without the fix, then make it pass. Keep the diff focused: no refactoring or unrelated changes. Run the project's formatter, linter and tests. Do not push or switch branches. Report the root cause, the fix, and the test evidence.",
};

pub(super) const FIX_QA_REVIEWER: Stock = Stock {
    id: 7,
    name: "Bug-fix QA reviewer",
    text: "Verify, do not fix. Confirm the change removes the root cause rather than masking the symptom: it covers every caller of the faulty path, and the regression test would fail without the fix. Check that it changes no behavior beyond the fix. Your sandbox is read-only; Hive runs each lane's configured checks itself. Report only concrete defects, each with its location, a reproduction and the expected behavior.",
};

pub(super) const CONTRACT_PLANNER: Stock = Stock {
    id: 8,
    name: "Contract planner",
    text: "Read the code on both sides of the boundary in every worktree; do not edit files. Define the shared contract first: endpoints or messages, field names and types, optionality, errors and status values. Repeat it verbatim in every affected assignment so the lanes cannot drift, and state which side stays backward compatible while the other is deployed. Give each lane criteria that verify its side of the contract. Ask the user when the contract involves a product decision.",
};

pub(super) const CONTRACT_DEVELOPER: Stock = Stock {
    id: 9,
    name: "Contract developer",
    text: "Implement your side of the contract exactly as the assignment states it. If the contract is wrong or incomplete, stop and report the problem instead of inventing a variant. Keep the backward compatibility the plan requires. Add tests for your side of the contract, including error cases, then run the project's formatter, linter and tests. Do not push or switch branches. Report what changed and how you verified it.",
};

pub(super) const CONTRACT_QA_REVIEWER: Stock = Stock {
    id: 10,
    name: "Contract QA reviewer",
    text: "Verify, do not fix. Check that every lane implements the same contract: compare names, types, optionality, error codes and status values on both sides, then trace one complete request through the changed code from client to server and back. Your sandbox is read-only; Hive runs each lane's configured checks itself. Report each mismatch against the lane that deviates from the planned contract, with the location on both sides.",
};

pub(super) const CODE_REVIEWER: Stock = Stock {
    id: 11,
    name: "Code reviewer",
    text: "Review the changed code for correctness and maintainability: logic errors, unhandled edge cases and error paths, broken callers of changed functions, logic that duplicates code already in the repository, and missing or weak tests. Report only actionable defects, each with its location, the problem and the expected fix; skip style that the formatter or linter already enforces.",
};

pub(super) const SECURITY_REVIEWER: Stock = Stock {
    id: 12,
    name: "Security reviewer",
    text: "Review the changed code and the paths it touches for vulnerabilities: untrusted input reaching shell commands, queries, file paths or HTML; missing authentication or authorization checks; secrets in code, logs or errors; unsafe deserialization; risky new dependencies. Report each issue with its location, how it can be exploited and a defensive fix; skip theoretical concerns and code the change does not touch.",
};

pub(super) const PERFORMANCE_REVIEWER: Stock = Stock {
    id: 13,
    name: "Performance reviewer",
    text: "Inspect the changed paths for performance regressions: extra work on hot paths, I/O or queries inside loops, unbounded memory or result sizes, blocking calls on async or UI threads, missing limits or pagination. Report each issue with its location, its expected cost at realistic input sizes and a fix; do not flag micro-optimizations without evidence.",
};

pub(super) const ACCEPTANCE_REVIEWER: Stock = Stock {
    id: 14,
    name: "Acceptance reviewer",
    text: "Check the changes against every approved acceptance criterion in each worktree, using the code and tests rather than the developers' summaries. Report each unmet criterion with its evidence and the expected behavior.",
};

const STOCK: [&Stock; 14] = [
    &FEATURE_PLANNER,
    &DEVELOPER,
    &QA_REVIEWER,
    &SECURITY_AND_PERFORMANCE_REVIEWER,
    &FIX_PLANNER,
    &FIX_DEVELOPER,
    &FIX_QA_REVIEWER,
    &CONTRACT_PLANNER,
    &CONTRACT_DEVELOPER,
    &CONTRACT_QA_REVIEWER,
    &CODE_REVIEWER,
    &SECURITY_REVIEWER,
    &PERFORMANCE_REVIEWER,
    &ACCEPTANCE_REVIEWER,
];

/// Review stages the template editor offers to add.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewKind {
    /// Correctness and maintainability of the change.
    Code,
    /// Vulnerabilities in the changed code.
    Security,
    /// Performance regressions in the changed paths.
    Performance,
    /// The approved acceptance criteria, as a starting point for a custom review.
    Acceptance,
}

impl ReviewKind {
    /// Every kind, in menu order.
    pub const ALL: [Self; 4] = [
        Self::Code,
        Self::Security,
        Self::Performance,
        Self::Acceptance,
    ];

    /// Name a newly added stage of this kind starts with.
    pub fn stage_name(self) -> &'static str {
        match self {
            Self::Code => "Code review",
            Self::Security => "Security review",
            Self::Performance => "Performance review",
            Self::Acceptance => "Custom review",
        }
    }

    fn stock(self) -> &'static Stock {
        match self {
            Self::Code => &CODE_REVIEWER,
            Self::Security => &SECURITY_REVIEWER,
            Self::Performance => &PERFORMANCE_REVIEWER,
            Self::Acceptance => &ACCEPTANCE_REVIEWER,
        }
    }
}

impl Library {
    /// A new review stage of `kind`: following its built-in prompt while the library still has
    /// it, otherwise carrying the built-in text as the stage's own instructions.
    pub fn review_step(&self, kind: ReviewKind) -> Step {
        let stock = kind.stock();
        match self.prompt(stock.id) {
            Some(prompt) => Step::linked(kind.stage_name(), prompt),
            None => Step::new(kind.stage_name(), stock.text),
        }
    }
}

/// The built-in prompt library, in display order.
pub(crate) fn stock_prompts() -> Vec<Prompt> {
    STOCK.iter().map(|stock| stock.prompt()).collect()
}

/// Links stages that carry a built-in text word for word to that built-in prompt. Meant for a
/// library just seeded with [`stock_prompts`]; stages with their own wording or an existing
/// link are left alone.
pub(crate) fn link_stock_prompts(templates: &mut [Template]) {
    for template in templates {
        for step in template.steps_mut() {
            if step.prompt.is_none() {
                step.prompt = STOCK
                    .iter()
                    .find(|stock| stock.text == step.instruction)
                    .map(|stock| stock.id);
            }
        }
    }
}

/// A template the way a file saved before prompt links holds it.
#[cfg(test)]
pub(super) fn unlinked(mut template: Template) -> Template {
    for step in template.steps_mut() {
        step.prompt = None;
    }
    template
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::prompts::FIRST_USER_ID;
    use std::collections::HashSet;

    #[test]
    fn stock_prompts_have_unique_ids_names_and_texts() {
        let prompts = stock_prompts();
        let count = prompts.len();
        assert!(
            prompts
                .iter()
                .all(|p| p.is_complete() && p.id < FIRST_USER_ID)
        );
        let ids: HashSet<_> = prompts.iter().map(|p| p.id).collect();
        let names: HashSet<_> = prompts
            .iter()
            .map(|p| p.name.to_ascii_lowercase())
            .collect();
        let texts: HashSet<_> = prompts.iter().map(|p| &p.text).collect();
        assert_eq!((ids.len(), names.len(), texts.len()), (count, count, count));
    }

    #[test]
    fn review_step_links_only_while_the_library_has_the_prompt() {
        let mut library = Library::default();
        for kind in ReviewKind::ALL {
            let step = library.review_step(kind);
            assert_eq!(step.name, kind.stage_name());
            assert_eq!(step.prompt, Some(kind.stock().id));
            assert_eq!(step.instruction, kind.stock().text);
        }
        let id = CODE_REVIEWER.id;
        library
            .save_prompt(Some(id), "Code reviewer", "House rules")
            .unwrap();
        assert_eq!(
            library.review_step(ReviewKind::Code).instruction,
            "House rules"
        );
        library.delete_prompt(id).unwrap();
        let step = library.review_step(ReviewKind::Code);
        assert_eq!(step.prompt, None);
        assert_eq!(step.instruction, CODE_REVIEWER.text);
    }

    #[test]
    fn linking_skips_edited_and_already_linked_stages() {
        let mut template = unlinked(Template::default());
        template.planner.instruction.push_str(" Mine.");
        template.development.prompt = Some(FIRST_USER_ID);
        let mut templates = [template.clone()];
        link_stock_prompts(&mut templates);
        assert_eq!(templates[0].planner, template.planner);
        assert_eq!(templates[0].development, template.development);
        assert_eq!(templates[0].reviews, Template::default().reviews);
    }
}
