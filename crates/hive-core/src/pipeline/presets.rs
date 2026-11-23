//! Starter recipes, and the upgrade of the ones saved with retired stock instructions.
use super::stock::{DEVELOPER, FEATURE_PLANNER, QA_REVIEWER, SECURITY_AND_PERFORMANCE_REVIEWER};
use super::{LanePreset, Template};

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
            shared_contract: false,
            planner: FEATURE_PLANNER.stage("Plan"),
            development: DEVELOPER.stage("Development"),
            reviews: vec![
                QA_REVIEWER.stage("QA"),
                SECURITY_AND_PERFORMANCE_REVIEWER.stage("Security & performance"),
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
            reviews: vec![QA_REVIEWER.stage("QA")],
            ..feature.clone()
        };
        let cross = Self {
            name: "Cross-repository".into(),
            shared_contract: true,
            lanes: ["API", "Web"]
                .map(|name| LanePreset {
                    name: name.into(),
                    ..Default::default()
                })
                .into(),
            ..feature.clone()
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
        for step in template.steps_mut() {
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
    use crate::pipeline::Library;
    use crate::pipeline::stock::{link_stock_prompts, unlinked};

    /// Feature delivery as a catalog-version-1 file holds it: retired texts, no links.
    fn retired_feature_delivery() -> Template {
        let mut template = unlinked(Template::default());
        template.planner.instruction = RETIRED[0].1.into();
        template.development.instruction = RETIRED[3].1.into();
        template.reviews[0].instruction = RETIRED[4].1.into();
        template.reviews[1].instruction = RETIRED[5].1.into();
        template
    }

    #[test]
    fn every_preset_stage_follows_a_current_built_in_prompt() {
        let library = Library::default();
        for preset in Template::presets() {
            for step in preset.steps() {
                let prompt = step.prompt.and_then(|id| library.prompt(id));
                assert_eq!(
                    prompt.map(|p| &p.text),
                    Some(&step.instruction),
                    "{} / {}",
                    preset.name,
                    step.name
                );
                assert!(
                    !RETIRED.iter().any(|&(_, text)| text == step.instruction),
                    "{} still ships a retired text",
                    step.name
                );
            }
        }
    }

    #[test]
    fn refresh_then_link_brings_an_untouched_recipe_to_the_current_preset() {
        let mut templates = [retired_feature_delivery()];
        refresh_stock_instructions(&mut templates);
        assert_eq!(templates[0], unlinked(Template::default()));
        link_stock_prompts(&mut templates);
        assert_eq!(templates[0], Template::default());
    }

    #[test]
    fn refresh_links_quick_fix_to_the_shared_role_prompts() {
        let mut fix = unlinked(Template::presets().remove(1));
        fix.planner.instruction = RETIRED[1].1.into();
        fix.development.instruction = RETIRED[3].1.into();
        fix.reviews[0].instruction = RETIRED[4].1.into();
        let mut templates = [fix];
        refresh_stock_instructions(&mut templates);
        assert_eq!(templates[0].development.instruction, DEVELOPER.text);
        assert_eq!(templates[0].reviews[0].instruction, QA_REVIEWER.text);
        link_stock_prompts(&mut templates);
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
        assert_eq!(templates[0].development.instruction, DEVELOPER.text);
        assert_eq!(templates[1], renamed);
    }

    #[test]
    fn refresh_keeps_a_retired_text_moved_to_another_stage() {
        let mut template = unlinked(Template::default());
        template.reviews[1].instruction = RETIRED[4].1.into();
        let expected = template.clone();
        let mut templates = [template];
        refresh_stock_instructions(&mut templates);
        assert_eq!(templates[0], expected);
    }
}
