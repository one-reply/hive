//! The recipe drawn as the stages a run will go through.
use super::*;
use crate::ui::stage_role::{RoleKind, TINT_OPACITY};

/// What performs a stage of the route: an agent or the user in a role, a command Hive runs,
/// or the recipe itself where it lets the run through a gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Performer {
    Role(RoleKind),
    Command,
    Recipe,
}

/// The stages of `template` in the order a run reaches them. `planning` is off for a quick
/// start, `regression` brackets development with the independent test, and `checks` names
/// the commands that must pass before the first review.
fn stops(
    template: &Template,
    planning: bool,
    regression: bool,
    checks: &str,
) -> Vec<(Performer, String)> {
    let role = |kind, name: &str| (Performer::Role(kind), name.to_owned());
    let command = |name: &str| (Performer::Command, name.to_owned());
    let mut stops = Vec::new();
    if planning {
        stops.push(role(RoleKind::Plan, &template.planner.name));
        stops.push(if template.auto_approves_plan() {
            (Performer::Recipe, "Automatic approval".to_owned())
        } else {
            role(RoleKind::Human, "Your approval")
        });
    }
    if regression {
        stops.push(command("Reproduce on base"));
    }
    stops.push(role(RoleKind::Build, &template.development.name));
    if regression {
        stops.push(command("Verify fix"));
    }
    if !checks.is_empty() {
        stops.push(command(&format!("Required checks · {checks}")));
    }
    for (index, review) in template.reviews.iter().enumerate() {
        // A review limited to some paths may not run at all; the route says so up front.
        let name = match template.review_condition(index) {
            Some(_) => format!("{} · if paths match", review.name),
            None => review.name.clone(),
        };
        stops.push(role(RoleKind::review(template, index), &name));
    }
    stops.push(role(RoleKind::Human, "Acceptance"));
    stops
}

impl LaunchForm {
    /// The stages this recipe runs, in order, so the user sees where their approval comes.
    pub(super) fn route(&self, cx: &App) -> Div {
        let checks = self
            .bindings
            .iter()
            .filter_map(|b| Some(b.command(b.ready(cx)?, cx)))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        let regression = self.regression && self.can_start_direct();
        let mut route = h_flex().flex_wrap().items_center().gap_1p5();
        let stops = stops(&self.template, !self.is_direct(), regression, &checks);
        for (i, (performer, name)) in stops.into_iter().enumerate() {
            // The arrow travels with the stage it leads to, so a wrapped row never ends in one.
            let arrow = (i > 0).then(|| {
                Icon::new(Lucide::ChevronRight)
                    .size_3()
                    .text_color(cx.theme().muted_foreground)
            });
            route = route.child(
                h_flex()
                    .flex_none()
                    .gap_1p5()
                    .children(arrow)
                    .child(stage_chip(performer, name, cx)),
            );
        }
        route
    }
}

/// Every stage carries its role's icon and color; the user's own decisions are tinted as a
/// whole, so the two places the run waits for them stand out from the agents' work.
fn stage_chip(performer: Performer, name: String, cx: &App) -> Div {
    let (icon, color) = match performer {
        Performer::Role(role) => (role.icon(), role.color(cx)),
        Performer::Command => (Lucide::Terminal, cx.theme().muted_foreground),
        Performer::Recipe => (Lucide::FastForward, cx.theme().muted_foreground),
    };
    let human = performer == Performer::Role(RoleKind::Human);
    h_flex()
        .flex_none()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .border_1()
        .text_xs()
        .map(|chip| {
            if human {
                chip.border_color(color.opacity(TINT_OPACITY))
                    .bg(color.opacity(TINT_OPACITY))
            } else {
                chip.border_color(cx.theme().border)
                    .bg(cx.theme().background)
            }
        })
        .child(Icon::new(icon).size_3().text_color(color))
        .child(name)
}

#[cfg(test)]
mod tests {
    use super::{Performer, RoleKind, Template, stops};

    fn names(stops: &[(Performer, String)]) -> Vec<&str> {
        stops.iter().map(|(_, name)| name.as_str()).collect()
    }

    #[test]
    fn a_planned_run_shows_both_decisions_of_the_user() {
        let template = Template::default();
        let stops = stops(&template, true, false, "");
        let human: Vec<_> = stops
            .iter()
            .filter(|(performer, _)| *performer == Performer::Role(RoleKind::Human))
            .map(|(_, name)| name.as_str())
            .collect();
        assert_eq!(human, ["Your approval", "Acceptance"]);
        assert_eq!(
            stops.first(),
            Some(&(
                Performer::Role(RoleKind::Plan),
                template.planner.name.clone()
            ))
        );
        assert_eq!(stops.len(), 4 + template.reviews.len());
    }

    #[test]
    fn an_auto_approving_recipe_leaves_acceptance_as_the_users_only_decision() {
        let mut template = Template::default();
        template.toggle_plan_approval();
        let stops = stops(&template, true, false, "");
        assert_eq!(stops[1], (Performer::Recipe, "Automatic approval".into()));
        let human = |stops: &[(Performer, String)]| {
            let decisions = stops
                .iter()
                .filter(|(performer, _)| *performer == Performer::Role(RoleKind::Human));
            decisions.map(|(_, name)| name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(human(&stops), ["Acceptance"]);
        // A shared contract is read by the user whatever the recipe says.
        template.plan_approval = hive_core::pipeline::PlanApproval::Auto;
        template.shared_contract = true;
        let stops = super::stops(&template, true, false, "");
        assert_eq!(human(&stops), ["Your approval", "Acceptance"]);
    }

    #[test]
    fn a_review_limited_to_some_paths_is_marked_and_qa_never_is() {
        // Code review, QA, Security & performance.
        let mut template = Template::default();
        for review in &mut template.reviews {
            review.only_paths = vec!["crates/auth/".into()];
        }
        let stops = stops(&template, false, false, "");
        assert_eq!(
            names(&stops)[1..],
            [
                "Code review · if paths match",
                "QA",
                "Security & performance · if paths match",
                "Acceptance",
            ]
        );
    }

    #[test]
    fn a_quick_start_begins_with_development() {
        let template = Template::default();
        let stops = stops(&template, false, false, "");
        assert_eq!(
            stops.first(),
            Some(&(
                Performer::Role(RoleKind::Build),
                template.development.name.clone()
            ))
        );
        assert!(!names(&stops).contains(&"Your approval"));
        assert_eq!(names(&stops).last(), Some(&"Acceptance"));
    }

    #[test]
    fn the_regression_test_and_required_checks_surround_development() {
        let template = Template::default();
        let stops = stops(&template, false, true, "cargo test");
        assert_eq!(
            names(&stops)[..4],
            [
                "Reproduce on base",
                template.development.name.as_str(),
                "Verify fix",
                "Required checks · cargo test",
            ]
        );
        assert_eq!(stops[0].0, Performer::Command);
    }
}
