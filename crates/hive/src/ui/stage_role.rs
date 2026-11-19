//! The roles of pipeline stages and how each is drawn: one icon and one color per role, the
//! same on the recipe canvas, in a run and in the prompt library.
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex};
use gpui_kit::*;
use hive_core::pipeline::Step;

/// A role's surface is washed out so the icon or text on top keeps its contrast.
pub(crate) const TINT_OPACITY: f32 = 0.12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RoleKind {
    Plan,
    Build,
    Qa,
    Security,
    Performance,
    Review,
    Human,
}

impl RoleKind {
    pub(crate) fn review(index: usize, step: &Step) -> Self {
        if index == 0 {
            return Self::Qa;
        }
        let name = step.name.to_lowercase();
        if name.contains("security") {
            Self::Security
        } else if name.contains("performance") {
            Self::Performance
        } else {
            Self::Review
        }
    }

    /// The role a library prompt is written for, read from its name. `None` when the name
    /// says nothing: the prompt is then drawn without a role.
    pub(crate) fn of_prompt(name: &str) -> Option<Self> {
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
            Self::Review | Self::Security | Self::Performance => cx.theme().yellow,
            Self::Human => cx.theme().green,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Plan => "Planning agent",
            Self::Build => "Development",
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
            ("Code reviewer", RoleKind::Review),
        ];
        for (name, role) in cases {
            assert_eq!(RoleKind::of_prompt(name), Some(role), "{name}");
        }
    }

    #[test]
    fn a_name_without_a_role_word_has_no_role() {
        assert_eq!(RoleKind::of_prompt("House rules"), None);
        assert_eq!(RoleKind::of_prompt(""), None);
        assert_eq!(RoleKind::of_prompt("SENIOR ENGINEER"), Some(RoleKind::Build));
    }
}
