//! The recipe drawn as the stages a run will go through.
use super::*;

impl LaunchForm {
    /// The stages this recipe runs, in order, so the user sees where their approval comes.
    pub(super) fn route(&self, cx: &App) -> Div {
        let mut route = h_flex().flex_wrap().items_center().gap_1p5();
        let planning = !(self.direct && self.can_start_direct());
        let regression = self.regression && self.can_start_direct();
        let checks = self
            .bindings
            .iter()
            .filter_map(|b| Some(b.command(b.base_preview(cx)?.as_ref().ok()?, cx)))
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ");
        let checks = (!checks.is_empty()).then(|| format!("Required checks · {checks}"));
        let steps = planning
            .then_some((Lucide::Map, self.template.planner.name.as_str()))
            .into_iter()
            .chain(planning.then_some((Lucide::UserRoundCheck, "Your approval")))
            .chain(regression.then_some((Lucide::Terminal, "Reproduce on base")))
            .chain(std::iter::once((
                Lucide::GitBranch,
                self.template.development.name.as_str(),
            )))
            .chain(regression.then_some((Lucide::Terminal, "Verify fix")))
            .chain(checks.as_deref().map(|label| (Lucide::Terminal, label)))
            .chain(
                self.template
                    .reviews
                    .iter()
                    .map(|s| (Lucide::ShieldCheck, s.name.as_str())),
            )
            .chain(std::iter::once((Lucide::Check, "Acceptance")));
        for (i, (icon, name)) in steps.enumerate() {
            if i > 0 {
                route = route.child(
                    Icon::new(Lucide::ChevronRight)
                        .size_3()
                        .text_color(cx.theme().muted_foreground),
                );
            }
            route = route.child(stage_chip(icon, name.to_owned(), cx));
        }
        route
    }
}

fn stage_chip(icon: Lucide, name: String, cx: &App) -> Div {
    h_flex()
        .flex_none()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .text_xs()
        .child(
            Icon::new(icon)
                .size_3()
                .text_color(cx.theme().muted_foreground),
        )
        .child(name)
}
