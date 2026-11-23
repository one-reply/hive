//! The built-in prompts: what a new prompt library holds and what starter stages follow.
//!
//! Hive appends the task, the lane assignments, the handoff and the report contract to these
//! texts, so they only describe how to do the stage's work well, not the output format.
use super::{Library, Prompt, ReviewRole, Step, Template, unique_name};

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
        let mut stage = Step::linked(name, &self.prompt());
        stage.review_role = if self.id == CODE_REVIEWER.id {
            ReviewRole::Code
        } else if self.id == QA_REVIEWER.id {
            ReviewRole::Qa
        } else {
            ReviewRole::Unspecified
        };
        stage
    }
}

pub(super) const FEATURE_PLANNER: Stock = Stock {
    id: 1,
    name: "Feature planner",
    text: include_str!("prompts/feature-planner.md").trim_ascii_end(),
};

pub(super) const DEVELOPER: Stock = Stock {
    id: 2,
    name: "Developer",
    text: include_str!("prompts/developer.md").trim_ascii_end(),
};

pub(super) const QA_REVIEWER: Stock = Stock {
    id: 3,
    name: "QA reviewer",
    text: include_str!("prompts/qa-reviewer.md").trim_ascii_end(),
};

pub(super) const SECURITY_AND_PERFORMANCE_REVIEWER: Stock = Stock {
    id: 4,
    name: "Security & performance reviewer",
    text: include_str!("prompts/security-and-performance-reviewer.md").trim_ascii_end(),
};

// IDs 5–14 belonged to retired prompts and remain reserved for saved libraries.
const LEGACY_CODE_REVIEWER: u64 = 11;
pub(super) const CODE_REVIEWER: Stock = Stock {
    id: 15,
    name: "Code reviewer",
    text: include_str!("prompts/code-reviewer.md").trim_ascii_end(),
};

const STOCK: [&Stock; 5] = [
    &FEATURE_PLANNER,
    &DEVELOPER,
    &CODE_REVIEWER,
    &QA_REVIEWER,
    &SECURITY_AND_PERFORMANCE_REVIEWER,
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
}

impl Library {
    /// A new review stage following its library prompt, or using the built-in text if deleted.
    pub fn review_step(&self, kind: ReviewKind) -> Step {
        // Older libraries may still have a customized prompt dedicated to this menu entry.
        let (legacy_id, stock) = match kind {
            ReviewKind::Code => (LEGACY_CODE_REVIEWER, &CODE_REVIEWER),
            ReviewKind::Security => (12, &SECURITY_AND_PERFORMANCE_REVIEWER),
            ReviewKind::Performance => (13, &SECURITY_AND_PERFORMANCE_REVIEWER),
            ReviewKind::Acceptance => (14, &QA_REVIEWER),
        };
        let mut step = match self.prompt(legacy_id).or_else(|| self.prompt(stock.id)) {
            Some(prompt) => Step::linked(kind.stage_name(), prompt),
            None => Step::new(kind.stage_name(), stock.text),
        };
        if kind == ReviewKind::Code {
            step.review_role = ReviewRole::Code;
        }
        step
    }
}

impl Prompt {
    /// Whether this is the built-in code reviewer, including its original specialized version.
    pub fn is_code_reviewer(&self) -> bool {
        self.id == CODE_REVIEWER.id || self.id == LEGACY_CODE_REVIEWER
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

impl Library {
    pub(crate) fn upgrade_code_review(&mut self) {
        if !self.prompts.is_empty() && !self.prompts.iter().any(Prompt::is_code_reviewer) {
            let mut prompt = CODE_REVIEWER.prompt();
            prompt.name = unique_name(&prompt.name, self.prompts.iter().map(|p| p.name.as_str()));
            let index = self
                .prompts
                .iter()
                .position(|p| p.id == QA_REVIEWER.id)
                .unwrap_or(self.prompts.len());
            self.prompts.insert(index, prompt);
        }
        let code = self.review_step(ReviewKind::Code);
        for template in &mut self.templates {
            if ["Feature delivery", "Cross-repository"]
                .iter()
                .any(|n| template.name.eq_ignore_ascii_case(n))
                && let [qa, security] = template.reviews.as_slice()
                && qa.name.eq_ignore_ascii_case("QA")
                && security.name.eq_ignore_ascii_case("Security & performance")
            {
                template.add_review(code.clone());
            }
        }
    }

    pub(crate) fn upgrade_shared_contract(&mut self) {
        const CONTRACT_PLANNER_ID: u64 = 8;
        const SHARED_CONTRACT_TEXT: &str = "Read the code on both sides of the boundary in every worktree; do not edit files. Define the shared contract first: endpoints or messages, field names and types, optionality, errors and status values. Follow the shared contract format supplied by Hive, and state which side stays backward compatible while the other is deployed. Give each lane criteria that verify its side of the contract. Ask the user when the contract involves a product decision.";
        const PREVIOUS: &str = "Read the code on both sides of the boundary in every worktree; do not edit files. Define the shared contract first: endpoints or messages, field names and types, optionality, errors and status values. Repeat it verbatim in every affected assignment so the lanes cannot drift, and state which side stays backward compatible while the other is deployed. Give each lane criteria that verify its side of the contract. Ask the user when the contract involves a product decision.";
        for template in &mut self.templates {
            if template.planner.instruction == PREVIOUS && template.planner.prompt.is_none() {
                template.planner.prompt = Some(CONTRACT_PLANNER_ID);
            }
            if template.name == "Cross-repository"
                && (template.planner.prompt == Some(CONTRACT_PLANNER_ID)
                    || (template.planner.prompt == Some(FEATURE_PLANNER.id)
                        && template.planner.instruction == FEATURE_PLANNER.text))
            {
                template.shared_contract = true;
            }
        }
        if let Some(prompt) = self
            .prompts
            .iter_mut()
            .find(|p| p.id == CONTRACT_PLANNER_ID && p.text == PREVIOUS)
        {
            prompt.text = SHARED_CONTRACT_TEXT.into();
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
        for (kind, stock) in ReviewKind::ALL.into_iter().zip([
            CODE_REVIEWER,
            SECURITY_AND_PERFORMANCE_REVIEWER,
            SECURITY_AND_PERFORMANCE_REVIEWER,
            QA_REVIEWER,
        ]) {
            let step = library.review_step(kind);
            assert_eq!(step.name, kind.stage_name());
            assert_eq!(step.prompt, Some(stock.id));
            assert_eq!(step.instruction, stock.text);
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
    fn adding_a_review_keeps_the_saved_specialized_prompt_when_present() {
        let mut library = Library::default();
        library.prompts.push(Prompt {
            id: 12,
            name: "Security reviewer".into(),
            text: "House security review".into(),
        });
        let step = library.review_step(ReviewKind::Security);
        assert_eq!(step.prompt, Some(12));
        assert_eq!(step.instruction, "House security review");
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
