//! Where a review sends its findings, and when it stops retrying and asks the user.
use super::*;

impl TemplateEditor {
    pub(super) fn review_routing(&self, cx: &mut Context<Self>) -> Div {
        let form = &self.inspector;
        let mut routes = vec![(None, "Affected lanes".into())];
        routes.extend(
            self.template
                .lanes
                .iter()
                .map(|l| (Some(l.name.clone()), l.name.clone())),
        );
        let mut limits = vec![1, 2, 3, 4, 5];
        if !limits.contains(&form.limit) {
            limits.push(form.limit);
        }
        let options = limits
            .into_iter()
            .map(|n| {
                (
                    n,
                    format!("{n} {}", if n == 1 { "round" } else { "rounds" }),
                )
            })
            .collect();
        let target = form
            .route
            .clone()
            .unwrap_or_else(|| "the lanes they affect".into());
        section(cx)
            .child(section_title(Lucide::CornerUpLeft, "Findings", cx))
            .child(subtle(findings_summary(&target, form.limit), cx))
            .child(setting(
                "Send findings to",
                choice(
                    "stage-route",
                    &form.route,
                    routes,
                    cx,
                    |s: &mut Self, r, _, _| {
                        if let Item::Review(i) = s.selected {
                            s.template.reviews[i].return_lane = r.clone();
                        }
                        s.inspector.route = r
                    },
                ),
                cx,
            ))
            .child(setting(
                "Ask me after",
                choice(
                    "stage-limit",
                    &form.limit,
                    options,
                    cx,
                    |s: &mut Self, n, _, _| s.inspector.limit = n,
                ),
                cx,
            ))
    }
}

/// What happens to a review's findings, in the words of the two settings below it.
fn findings_summary(target: &str, rounds: u8) -> String {
    let rounds = if rounds == 1 {
        "the first round".to_owned()
    } else {
        format!("{rounds} rounds")
    };
    format!(
        "Blocker and major findings go back to {target} to be fixed. If they remain after {rounds}, Hive asks you. Minor findings wait for you at acceptance."
    )
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::findings_summary;

    #[test]
    fn findings_summary_names_the_target_and_the_round_limit() {
        assert_eq!(
            findings_summary("the lanes they affect", 3),
            "Blocker and major findings go back to the lanes they affect to be fixed. If they remain after 3 rounds, Hive asks you. Minor findings wait for you at acceptance."
        );
        assert_eq!(
            findings_summary("Web", 1),
            "Blocker and major findings go back to Web to be fixed. If they remain after the first round, Hive asks you. Minor findings wait for you at acceptance."
        );
    }
}
