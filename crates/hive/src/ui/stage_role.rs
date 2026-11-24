//! The roles of pipeline stages and how each is drawn: one icon and one color per role, the
//! same on the recipe canvas, in a run and in the prompt library.
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex};
use gpui_kit::*;
use hive_core::pipeline::{Prompt, ReviewRole, Template};

/// A role's surface is washed out so the icon or text on top keeps its contrast.
pub(crate) const TINT_OPACITY: f32 = 0.12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RoleKind {
    Plan,
    Build,
    CodeReview,
    Qa,
    Security,
    Performance,
    Review,
    Human,
}

impl RoleKind {
    /// The role of the review at `index` of `template`. QA is where the recipe says it is, not
    /// whichever review comes first: reviews stand in any order.
    pub(crate) fn review(template: &Template, index: usize) -> Self {
        let Some(step) = template.reviews.get(index) else {
            return Self::Review;
        };
        if step.review_role == ReviewRole::Code {
            return Self::CodeReview;
        }
        if index == template.qa_index() {
            return Self::Qa;
        }
        let name = step.name.to_lowercase();
        if name.contains("code review") {
            Self::CodeReview
        } else if name.contains("security") {
            Self::Security
        } else if name.contains("performance") {
            Self::Performance
        } else {
            Self::Review
        }
    }

    pub(crate) fn library_prompt(prompt: Option<&Prompt>, name: &str) -> Option<Self> {
        if prompt.is_some_and(Prompt::is_code_reviewer) {
            Some(Self::CodeReview)
        } else {
            Self::of_prompt(name)
        }
    }

    /// The role a library prompt is written for, read from its name. `None` when the name
    /// says nothing: the prompt is then drawn without a role.
    fn of_prompt(name: &str) -> Option<Self> {
        let name = name.to_lowercase();
        let mentions = |words: &[&str]| words.iter().any(|word| name.contains(word));
        // "Security & performance reviewer" and "QA reviewer" are reviewers of a kind, so the
        // kinds are checked before the general words.
        if mentions(&["plan"]) {
            Some(Self::Plan)
        } else if mentions(&["security"]) {
            Some(Self::Security)
        } else if mentions(&["performance"]) {
            Some(Self::Performance)
        } else if mentions(&["qa", "quality", "acceptance", "test"]) {
            Some(Self::Qa)
        } else if mentions(&["code review"]) {
            Some(Self::CodeReview)
        } else if mentions(&["review", "audit"]) {
            Some(Self::Review)
        } else if mentions(&["develop", "engineer", "implement", "build"]) {
            Some(Self::Build)
        } else {
            None
        }
    }

    pub(crate) fn icon(self) -> Lucide {
        match self {
            Self::Plan => Lucide::BrainCircuit,
            Self::Build => Lucide::CodeXml,
            Self::CodeReview => Lucide::FileSearch,
            Self::Qa => Lucide::ListChecks,
            Self::Security => Lucide::ShieldCheck,
            Self::Performance => Lucide::Activity,
            Self::Review => Lucide::ScanEye,
            Self::Human => Lucide::UserRoundCheck,
        }
    }

    pub(crate) fn color(self, cx: &App) -> Hsla {
        match self {
            Self::Plan => cx.theme().magenta,
            Self::Build => cx.theme().blue,
            Self::Qa => cx.theme().cyan,
            Self::CodeReview | Self::Review | Self::Security | Self::Performance => {
                cx.theme().yellow
            }
            Self::Human => cx.theme().green,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Plan => "Planning agent",
            Self::Build => "Development",
            Self::CodeReview => "Code review",
            Self::Qa => "Quality check",
            Self::Security => "Security review",
            Self::Performance => "Performance review",
            Self::Review => "Review agent",
            Self::Human => "Human decision",
        }
    }

    /// The role's icon on its tinted tile.
    pub(crate) fn tile(self, cx: &App) -> Div {
        tile(self.icon(), self.color(cx))
    }
}

/// An icon on a tile tinted with its own color.
pub(crate) fn tile(icon: Lucide, color: Hsla) -> Div {
    h_flex()
        .size_7()
        .flex_none()
        .justify_center()
        .rounded_md()
        .bg(color.opacity(TINT_OPACITY))
        .child(Icon::new(icon).size_4().text_color(color))
}

#[cfg(test)]
mod tests {
    use super::RoleKind;
    use hive_core::pipeline::{ReviewRole, Template};

    #[test]
    fn qa_is_the_marked_review_or_the_first_of_an_unmarked_recipe() {
        let roles = |t: &Template| -> Vec<_> {
            (0..t.reviews.len())
                .map(|i| RoleKind::review(t, i))
                .collect()
        };
        let mut recipe = Template::default();
        assert_eq!(
            roles(&recipe),
            [RoleKind::CodeReview, RoleKind::Qa, RoleKind::Security]
        );
        recipe.move_review(2, 0).unwrap();
        assert_eq!(
            roles(&recipe),
            [RoleKind::Security, RoleKind::CodeReview, RoleKind::Qa]
        );
        // Saved before roles: QA is the first review and carries no marker.
        recipe.remove_review(1).unwrap();
        recipe.move_review(1, 0).unwrap();
        recipe.reviews[0].review_role = ReviewRole::Unspecified;
        assert_eq!(roles(&recipe), [RoleKind::Qa, RoleKind::Security]);
        assert_eq!(RoleKind::review(&recipe, 9), RoleKind::Review);
    }

    #[test]
    fn built_in_prompt_names_map_to_their_roles() {
        let cases = [
            ("Feature planner", RoleKind::Plan),
            ("Contract planner", RoleKind::Plan),
            ("Developer", RoleKind::Build),
            ("Bug-fix developer", RoleKind::Build),
            ("QA reviewer", RoleKind::Qa),
            ("Contract QA reviewer", RoleKind::Qa),
            ("Acceptance reviewer", RoleKind::Qa),
            ("Security & performance reviewer", RoleKind::Security),
            ("Security reviewer", RoleKind::Security),
            ("Performance reviewer", RoleKind::Performance),
            ("Code reviewer", RoleKind::CodeReview),
        ];
        for (name, role) in cases {
            assert_eq!(RoleKind::of_prompt(name), Some(role), "{name}");
        }
    }

    #[test]
    fn a_name_without_a_role_word_has_no_role() {
        assert_eq!(RoleKind::of_prompt("House rules"), None);
        assert_eq!(RoleKind::of_prompt(""), None);
        assert_eq!(
            RoleKind::of_prompt("SENIOR ENGINEER"),
            Some(RoleKind::Build)
        );
    }
}
