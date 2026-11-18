//! User-configured checks and chronological evidence, with long output disclosed on demand.
use super::*;
use crate::ui::hover::SmoothHoverBg as _;

impl RunView {
    pub(super) fn checks(&self, r: &Run, w: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut list = v_flex().gap_1().child(heading("Required checks"));
        let mut count = 0;
        for (i, lane) in r
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.command.trim().is_empty())
        {
            count += 1;
            let result = r.commands.iter().find(|c| c.lane == i);
            let (label, color) = match result {
                Some(c) if c.success => ("Passed", cx.theme().success),
                Some(_) => ("Failed", cx.theme().danger),
                None => ("Pending", cx.theme().muted_foreground),
            };
            let open = self.expanded_checks.contains(&i);
            let hover = Hover::new(("check-hover", i), w, cx);
            let mut row = h_flex()
                .id(("pipeline-check", i))
                .px_2()
                .py_2()
                .gap_3()
                .rounded_sm()
                .child(
                    Icon::new(if result.is_some_and(|c| c.success) {
                        Lucide::CircleCheck
                    } else if result.is_some() {
                        Lucide::CircleX
                    } else {
                        Lucide::Clock
                    })
                    .size_4()
                    .text_color(color),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(lane.command.clone()))
                        .child(subtle(path_name(&lane.project), cx)),
                )
                .child(div().text_xs().text_color(color).child(label));
            if let Some(check) = result {
                if let Some(code) = check.exit_code {
                    row = row.child(subtle(format!("exit {code}"), cx));
                }
                row = row
                    .cursor_pointer()
                    .hover_bg(&hover, cx.theme().background, cx.theme().muted)
                    .child(
                        Icon::new(if open {
                            Lucide::ChevronDown
                        } else {
                            Lucide::ChevronRight
                        })
                        .size_3(),
                    )
                    .map(|row| {
                        actionable(
                            row,
                            format!("{} · {label} · {}", lane.command, path_name(&lane.project)),
                            Some(open),
                            cx,
                            move |v, cx| {
                                if !v.expanded_checks.remove(&i) {
                                    v.expanded_checks.insert(i);
                                }
                                cx.notify();
                            },
                        )
                    });
            }
            list = list.child(row);
            if open && let Some(check) = result {
                list = list.child(
                    div()
                        .id(("check-output", i))
                        .max_h(rems(12.))
                        .overflow_y_scroll()
                        .rounded_md()
                        .p_2()
                        .bg(cx.theme().muted)
                        .text_xs()
                        .child(SelectableText::new(
                            ("check-log", i),
                            if check.output.is_empty() {
                                "No output".into()
                            } else {
                                check.output.clone()
                            },
                        )),
                );
            }
        }
        if count == 0 {
            list =
                list.child(subtle("No required commands were configured for this run.", cx).py_3());
        }
        list = list.child(heading("Agent reviews").mt_3());
        for (i, review) in r.template.reviews.iter().enumerate() {
            let stage = Stage::Review(i);
            let state = stage.state(r, &stage.observations(r));
            let latest = handoff::stage_reports(r, stage)
                .next_back()
                .map(|(_, report)| report);
            let mut label = state.to_owned();
            let mut color = match state {
                "Complete" => cx.theme().success,
                "Failed" | "Interrupted" => cx.theme().danger,
                "In progress" => cx.theme().info,
                _ => cx.theme().muted_foreground,
            };
            if !matches!(state, "In progress" | "Interrupted")
                && let Some(hive_core::pipeline::Report::Review { findings, .. }) = latest
            {
                if findings.is_empty() {
                    label = "Passed".into();
                    color = cx.theme().success;
                } else {
                    label = format!(
                        "{} {}",
                        findings.len(),
                        if findings.len() == 1 {
                            "finding"
                        } else {
                            "findings"
                        }
                    );
                    color = cx.theme().warning;
                }
            }
            let expanded = self.expanded_reviews.contains(&i);
            let hover = Hover::new(("review-check-hover", i), w, cx);
            list = list.child(
                h_flex()
                    .id(("agent-check", i))
                    .px_2()
                    .py_2()
                    .gap_3()
                    .rounded_sm()
                    .cursor_pointer()
                    .hover_bg(&hover, cx.theme().background, cx.theme().muted)
                    .child(
                        Icon::new(super::super::map::RoleKind::review(i, review).icon())
                            .size_4()
                            .text_color(color),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(review.name.clone()))
                            .child(subtle(review.profile.agent.name(), cx)),
                    )
                    .child(div().text_xs().text_color(color).child(label))
                    .child(
                        Icon::new(if expanded {
                            Lucide::ChevronDown
                        } else {
                            Lucide::ChevronRight
                        })
                        .size_3(),
                    )
                    .map(|row| {
                        actionable(
                            row,
                            format!("{} · {state}", review.name),
                            Some(expanded),
                            cx,
                            move |v, cx| {
                                if !v.expanded_reviews.remove(&i) {
                                    v.expanded_reviews.insert(i);
                                }
                                cx.notify();
                            },
                        )
                    }),
            );
            if expanded {
                let mut evidence = v_flex()
                    .pl_4()
                    .py_2()
                    .gap_1()
                    .border_l_1()
                    .border_color(cx.theme().border);
                if latest.is_some() {
                    evidence = evidence.child(self.handoffs(stage, r, w, cx));
                } else {
                    evidence = evidence.child(subtle(
                        "No structured report is available for this stage.",
                        cx,
                    ));
                }
                list = list.child(evidence);
            }
        }
        scroll("run-checks", list)
    }
    pub(super) fn activity(&self, r: &Run, w: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut list = v_flex().gap_1().child(heading("Activity"));
        if r.history.is_empty() {
            list = list.child(subtle("No handoffs have been recorded yet.", cx).py_3());
        }
        for (i, event) in r.history.iter().enumerate() {
            let text = event.trim();
            let first = text.lines().next().unwrap_or_default();
            let open = self.expanded_events.contains(&i);
            let expandable = first.len() != text.len() || first.chars().count() > 85;
            let hover = Hover::new(("event-hover", i), w, cx);
            let label = human_exit(first);
            let mut row = h_flex()
                .id(("activity-entry", i))
                .gap_2()
                .px_2()
                .py_2()
                .child(
                    div()
                        .size_1p5()
                        .rounded_full()
                        .bg(cx.theme().muted_foreground),
                )
                .child(div().flex_1().min_w_0().text_sm().truncate().child(label));
            if expandable {
                row = row
                    .cursor_pointer()
                    .rounded_md()
                    .hover_bg(&hover, cx.theme().background, cx.theme().muted)
                    .child(
                        Icon::new(if open {
                            Lucide::ChevronDown
                        } else {
                            Lucide::ChevronRight
                        })
                        .size_3(),
                    )
                    .map(|row| {
                        actionable(row, first.to_owned(), Some(open), cx, move |v, cx| {
                            if !v.expanded_events.remove(&i) {
                                v.expanded_events.insert(i);
                            }
                            cx.notify();
                        })
                    });
            }
            list = list.child(row);
            if open {
                list = list.child(
                    div()
                        .id(("history-output", i))
                        .p_2()
                        .ml_4()
                        .max_h(rems(12.))
                        .overflow_y_scroll()
                        .border_l_1()
                        .border_color(cx.theme().border)
                        .text_sm()
                        .child(SelectableText::new(("history-text", i), human_exit(text))),
                );
            }
        }
        scroll("run-activity", list)
    }
}
// Historical command events persisted a Rust Option in their first line.
fn human_exit(text: &str) -> String {
    if let Some((prefix, rest)) = text.split_once("(Some(")
        && let Some((code, suffix)) = rest.split_once("))")
        && code.parse::<i32>().is_ok()
    {
        return format!("{prefix}(exit {code}){suffix}");
    }
    text.to_owned()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[::core::prelude::v1::test]
    fn old_structured_exit_codes_are_readable_without_rewriting_raw_test_output() {
        assert_eq!(
            human_exit("API · cargo test · passed (Some(0))"),
            "API · cargo test · passed (exit 0)"
        );
        assert_eq!(human_exit("app: Some(value)"), "app: Some(value)");
    }
}
