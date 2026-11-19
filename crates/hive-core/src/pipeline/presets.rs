//! Starter recipes and the stock instructions each of their stages gives its agent.
//!
//! Hive appends the task, the lane assignments, the handoff and the report contract to these
//! texts, so they only describe how to do the stage's work well, not the output format.
use super::{LanePreset, Step, Template};

const FEATURE_PLAN: &str = "Read the code the task touches in every listed worktree before planning; do not edit files. Give each lane that needs work a self-contained assignment: the modules to change, the expected behavior, and the tests to add or update. Write acceptance criteria as observable checks (a test, a command, a reproducible behavior), not intentions. Leave out lanes that need no changes. Ask the user when the requirements are ambiguous or a decision has real trade-offs instead of guessing.";

const FEATURE_DEVELOPMENT: &str = "Read the code your assignment touches and follow the repository's conventions and agent instructions (AGENTS.md, CLAUDE.md, CONTRIBUTING). Implement the smallest complete change that meets every acceptance criterion and fix root causes, not symptoms. Add or update tests for the new behavior, then run the project's formatter, linter and tests. If the handoff lists findings for your lane, resolve each one and say how. Do not push or switch branches. Report what changed, the commands you ran with their results, and what you could not verify.";

const FEATURE_QA: &str = "Verify, do not fix. In each worktree inspect the actual changes (git status, git diff, the branch's commits) and check every acceptance criterion against the code and tests, not against the developer's summary. Look for unhandled edge cases and error paths, broken callers of changed code, and missing or weak tests. Your sandbox is read-only, so you may be unable to build; Hive runs each lane's configured checks itself. Report only concrete defects, each with its location, a reproduction or failing case, and the expected behavior; skip style preferences.";

const SECURITY_AND_PERFORMANCE: &str = "Review only the changed code and the paths it affects. Security: untrusted input reaching shell commands, queries, file paths or HTML; missing authentication or authorization checks; secrets in code, logs or errors; unsafe deserialization; risky new dependencies. Performance: extra work on hot paths, I/O or queries inside loops, unbounded memory or result sizes, blocking calls on async or UI threads. Report only issues with a concrete exploit or a measurable cost, each with its location and a fix; skip theoretical concerns and code the change does not touch.";

const FIX_PLAN: &str = "Locate the bug by reading the code; do not edit files. Identify the root cause and every caller that shares the faulty path, then assign the smallest complete fix to the lane that owns it. Acceptance criteria must include the original symptom gone and a regression test that fails before the fix and passes after it. Ask the user when you cannot tell what the correct behavior is.";

const FIX_DEVELOPMENT: &str = "Fix the root cause in the shared code path, not only the reported call site. First write a regression test that fails without the fix, then make it pass. Keep the diff focused: no refactoring or unrelated changes. Run the project's formatter, linter and tests. Do not push or switch branches. Report the root cause, the fix, and the test evidence.";

const FIX_QA: &str = "Verify, do not fix. Confirm the change removes the root cause rather than masking the symptom: it covers every caller of the faulty path, and the regression test would fail without the fix. Check that it changes no behavior beyond the fix. Your sandbox is read-only; Hive runs each lane's configured checks itself. Report only concrete defects, each with its location, a reproduction and the expected behavior.";

const CROSS_PLAN: &str = "Read the code on both sides of the boundary in every worktree; do not edit files. Define the shared contract first: endpoints or messages, field names and types, optionality, errors and status values. Repeat it verbatim in every affected assignment so the lanes cannot drift, and state which side stays backward compatible while the other is deployed. Give each lane criteria that verify its side of the contract. Ask the user when the contract involves a product decision.";

const CROSS_DEVELOPMENT: &str = "Implement your side of the contract exactly as the assignment states it. If the contract is wrong or incomplete, stop and report the problem instead of inventing a variant. Keep the backward compatibility the plan requires. Add tests for your side of the contract, including error cases, then run the project's formatter, linter and tests. Do not push or switch branches. Report what changed and how you verified it.";

const CROSS_QA: &str = "Verify, do not fix. Check that every lane implements the same contract: compare names, types, optionality, error codes and status values on both sides, then trace one complete request through the changed code from client to server and back. Your sandbox is read-only; Hive runs each lane's configured checks itself. Report each mismatch against the lane that deviates from the planned contract, with the location on both sides.";

/// Stock instructions shipped before catalog version 2, by stage name. A saved starter recipe
/// still holding one of them was never edited, so it may receive the current text.
const RETIRED: [(&str, &str); 6] = [
    (
        "Plan",
        "Break the task into repository-specific assignments and acceptance criteria. Ask for clarification when needed.",
    ),
    (
        "Plan",
        "Identify the root cause, the smallest complete fix and a regression check.",
    ),
    (
        "Plan",
        "Define the shared contract and assign its changes and acceptance criteria to each participating repository.",
    ),
    (
        "Development",
        "Implement the approved assignment in your worktree. Report what changed and what you verified.",
    ),
    (
        "QA",
        "Verify the acceptance criteria across all participating repositories. Report reproducible findings with the affected lane.",
    ),
    (
        "Security & performance",
        "Review the changes for security and performance regressions. Report specific findings and affected lanes.",
    ),
];

impl Default for Template {
    fn default() -> Self {
        Self {
            name: "Feature delivery".into(),
            planner: Step::new("Plan", FEATURE_PLAN),
            development: Step::new("Development", FEATURE_DEVELOPMENT),
            reviews: vec![
                Step::new("QA", FEATURE_QA),
                Step::new("Security & performance", SECURITY_AND_PERFORMANCE),
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
        let fix = Self {
            name: "Quick fix".into(),
            planner: Step::new("Plan", FIX_PLAN),
            development: Step::new("Development", FIX_DEVELOPMENT),
            reviews: vec![Step::new("QA", FIX_QA)],
            ..feature.clone()
        };
        let cross = Self {
            name: "Cross-repository".into(),
            lanes: ["API", "Web"]
                .map(|name| LanePreset {
                    name: name.into(),
                    ..Default::default()
                })
                .into(),
            planner: Step::new("Plan", CROSS_PLAN),
            development: Step::new("Development", CROSS_DEVELOPMENT),
            reviews: vec![
                Step::new("QA", CROSS_QA),
                Step::new("Security & performance", SECURITY_AND_PERFORMANCE),
            ],
        };
        vec![feature, fix, cross]
    }
}

/// Gives saved starter recipes the current stock instructions wherever the user kept the
/// retired default, leaving edited stages, renamed recipes and run snapshots untouched.
pub(crate) fn refresh_stock_instructions(templates: &mut [Template]) {
    let presets = Template::presets();
    for template in templates {
        let Some(preset) = presets
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(&template.name))
        else {
            continue;
        };
        let steps = [&mut template.planner, &mut template.development]
            .into_iter()
            .chain(&mut template.reviews);
        for step in steps {
            let retired = RETIRED
                .iter()
                .any(|&(name, text)| name == step.name && text == step.instruction);
            if let Some(current) = preset.steps().find(|s| s.name == step.name)
                && retired
            {
                step.instruction.clone_from(&current.instruction);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retired_feature_delivery() -> Template {
        let mut template = Template::default();
        template.planner.instruction = RETIRED[0].1.into();
        template.development.instruction = RETIRED[3].1.into();
        template.reviews[0].instruction = RETIRED[4].1.into();
        template.reviews[1].instruction = RETIRED[5].1.into();
        template
    }

    #[test]
    fn every_preset_stage_has_its_own_stock_instruction() {
        for preset in Template::presets() {
            for step in preset.steps() {
                assert!(!step.instruction.trim().is_empty(), "{}", step.name);
                assert!(
                    !RETIRED.iter().any(|&(_, text)| text == step.instruction),
                    "{} still ships a retired text",
                    step.name
                );
            }
        }
    }

    #[test]
    fn refresh_replaces_untouched_retired_instructions() {
        let mut templates = [retired_feature_delivery()];
        refresh_stock_instructions(&mut templates);
        assert_eq!(templates[0], Template::default());
    }

    #[test]
    fn refresh_uses_the_matching_recipe_for_shared_retired_texts() {
        let mut fix = Template::presets().remove(1);
        fix.planner.instruction = RETIRED[1].1.into();
        fix.development.instruction = RETIRED[3].1.into();
        fix.reviews[0].instruction = RETIRED[4].1.into();
        let mut templates = [fix];
        refresh_stock_instructions(&mut templates);
        assert_eq!(templates[0].development.instruction, FIX_DEVELOPMENT);
        assert_eq!(templates[0].reviews[0].instruction, FIX_QA);
        assert_eq!(templates[0], Template::presets()[1]);
    }

    #[test]
    fn refresh_keeps_edited_stages_and_renamed_recipes() {
        let mut edited = retired_feature_delivery();
        edited.planner.instruction = "Custom planning".into();
        let mut renamed = retired_feature_delivery();
        renamed.name = "My delivery".into();
        let mut templates = [edited.clone(), renamed.clone()];
        refresh_stock_instructions(&mut templates);
        assert_eq!(templates[0].planner.instruction, "Custom planning");
        assert_eq!(templates[0].development.instruction, FEATURE_DEVELOPMENT);
        assert_eq!(templates[1], renamed);
    }

    #[test]
    fn refresh_keeps_a_retired_text_moved_to_another_stage() {
        let mut template = Template::default();
        template.reviews[1].instruction = RETIRED[4].1.into();
        let expected = template.clone();
        let mut templates = [template];
        refresh_stock_instructions(&mut templates);
        assert_eq!(templates[0], expected);
    }
}
