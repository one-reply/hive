//! How the run starts: with which version of a repeated recipe, and with or without a planner.
use super::*;
use gpui_kit::base::motion::{MotionReveal, Transition, transition};
use gpui_kit::component::switch::Switch;
use std::time::Duration;

/// How long the acceptance criteria take to unfold under the quick start switch.
const CRITERIA_UNFOLD: Duration = Duration::from_millis(220);
/// The reveal clips to its bounds; this much room around the field keeps its focus ring,
/// drawn outside the field, from being cut off.
const RING_ROOM: Pixels = px(4.);

/// Quick start hands the whole task to one developer, so exactly one repository is bound and
/// every review returns its findings to that lane.
fn direct_start_allowed<'a>(
    bound: &[&str],
    mut return_lanes: impl Iterator<Item = &'a str>,
) -> bool {
    bound.len() == 1 && return_lanes.all(|name| bound.contains(&name))
}

impl LaunchForm {
    /// Whether the run skips planning: the user asked for it and the recipe still allows it.
    pub(super) fn is_direct(&self) -> bool {
        self.direct && self.can_start_direct()
    }

    pub(super) fn can_start_direct(&self) -> bool {
        let bound: Vec<_> = self
            .bindings
            .iter()
            .filter(|b| b.project.is_some())
            .map(|b| b.preset.name.as_str())
            .collect();
        !self.template.shared_contract
            && direct_start_allowed(
                &bound,
                self.template
                    .reviews
                    .iter()
                    .filter_map(|s| s.return_lane.as_deref()),
            )
    }

    /// Offered when the library's recipe has changed since the run being repeated.
    pub(super) fn recipe_version(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let (saved, latest) = self.previous_recipe.clone()?;
        Some(
            h_flex()
                .gap_2()
                .child(subtle(
                    if self.frozen {
                        "Using the saved recipe. The library version has changed."
                    } else {
                        "Using the updated library recipe."
                    },
                    cx,
                ))
                .child(button(
                    "choose-recipe-version",
                    if self.frozen {
                        "Use library version"
                    } else {
                        "Use saved version"
                    },
                    ButtonLook::Outline,
                    window,
                    cx,
                    move |v, w, cx| {
                        let direct = v.direct;
                        v.frozen = !v.frozen;
                        v.apply_template(
                            if v.frozen {
                                saved.clone()
                            } else {
                                latest.clone()
                            },
                            w,
                            cx,
                        );
                        v.direct = direct && v.can_start_direct();
                    },
                )),
        )
    }

    /// The switches a single-repository run offers; other runs always plan first.
    pub(super) fn options(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<Div> {
        if !self.can_start_direct() {
            return None;
        }
        let direct = self.is_direct();
        let unfolded = transition(
            "quick-start-criteria-unfolded",
            if direct { 1.0_f32 } else { 0. },
            Transition::new(CRITERIA_UNFOLD).ease(ease_in_out),
            window,
            cx,
        );
        let quick_start = option_row(
            "Quick start",
            "Skip the planner: you write the acceptance criteria and development starts right away.",
            Switch::new("quick-start-mode")
                .accessibility_label("Quick start")
                .checked(direct)
                .on_click(cx.listener(|form, on: &bool, window, cx| {
                    form.direct = *on;
                    if *on {
                        form.criteria.focus_handle(cx).focus(window, cx);
                    }
                    cx.notify();
                })),
            cx,
        );
        let regression = option_row(
            "Independent regression test",
            "A fresh session writes one test before development. Hive runs it on the base, \
             where it must fail, and again after the change.",
            Switch::new("independent-regression")
                .accessibility_label("Independent regression test")
                .checked(self.regression)
                .on_click(cx.listener(|form, on: &bool, _, cx| {
                    form.regression = *on;
                    cx.notify();
                })),
            cx,
        );
        // Once folded the field leaves the tree, so Tab never lands on something invisible.
        // The reveal lays its child out as its own root, where an auto width shrinks to
        // content, hence the full width.
        let criteria = if direct || unfolded > 0. {
            v_flex()
                .w_full()
                .px(RING_ROOM)
                .pt_3()
                .pb(RING_ROOM)
                .gap_1()
                .opacity(unfolded)
                .child(
                    h_flex()
                        .gap_0p5()
                        .child(subtle("Acceptance criteria", cx))
                        .child(required_mark(cx)),
                )
                .child(TextField::textarea(
                    Textarea::new(&self.criteria),
                    &self.criteria,
                    cx,
                ))
        } else {
            div()
        };
        // The negative margin gives back the ring's room, so the field lines up with the card.
        let quick_start = v_flex()
            .child(quick_start)
            .child(div().mx(-RING_ROOM).child(MotionReveal::new(
                "quick-start-criteria",
                unfolded,
                criteria.into_any_element(),
            )));
        Some(
            v_flex().gap_2().child(heading("Options")).child(
                card(cx).w_full().gap_3().child(quick_start).child(
                    regression
                        .pt_3()
                        .border_t_1()
                        .border_color(cx.theme().border),
                ),
            ),
        )
    }
}

/// A named option with one line on what it changes, and its switch on the right.
fn option_row(title: &'static str, description: &'static str, switch: Switch, cx: &App) -> Div {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_6()
        .child(
            v_flex()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().child(title))
                .child(subtle(description, cx)),
        )
        .child(div().flex_none().child(switch))
}

#[cfg(test)]
mod tests {
    use super::direct_start_allowed;

    #[test]
    fn quick_start_needs_exactly_one_repository_that_receives_every_review() {
        let none: [&str; 0] = [];
        assert!(direct_start_allowed(&["Web"], none.into_iter()));
        assert!(direct_start_allowed(&["Web"], ["Web", "Web"].into_iter()));
        assert!(!direct_start_allowed(&["Web"], ["Web", "API"].into_iter()));
        assert!(!direct_start_allowed(&[], none.into_iter()));
        assert!(!direct_start_allowed(&["Web", "API"], ["Web"].into_iter()));
    }
}
