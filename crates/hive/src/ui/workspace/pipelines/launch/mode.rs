//! How the run starts: with which version of a repeated recipe, and with or without a planner.
use super::*;

/// Quick start hands the whole task to one developer, so exactly one repository is bound and
/// every review returns its findings to that lane.
fn direct_start_allowed<'a>(
    bound: &[&str],
    mut return_lanes: impl Iterator<Item = &'a str>,
) -> bool {
    bound.len() == 1 && return_lanes.all(|name| bound.contains(&name))
}

impl LaunchForm {
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

    pub(super) fn quick_start_toggle(
        &self,
        direct: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        self.can_start_direct().then(|| {
            button(
                "quick-start-mode",
                if direct {
                    "Use planning and approval"
                } else {
                    "Quick start with my criteria"
                },
                ButtonLook::Outline,
                window,
                cx,
                |v, _, cx| {
                    v.direct = !v.direct;
                    cx.notify();
                },
            )
        })
    }
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
