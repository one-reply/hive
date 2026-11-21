//! User-configured checks and chronological evidence, with long output disclosed on demand.
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use hive_core::pipeline::{Finding, readable_history};

/// A review's result as the Checks tab words it, and whether it let the run continue: only
/// blocking findings hold a run, the rest are notes for the user.
fn review_outcome(findings: &[Finding]) -> (String, bool) {
    let blocking = findings.iter().filter(|f| f.severity.blocks()).count();
    let notes = findings.len() - blocking;
    match (blocking, notes) {
        (0, 0) => ("Passed".into(), true),
        (0, _) => (format!("Passed · {}", count(notes, "note", "notes")), true),
        _ => (count(blocking, "finding", "findings"), false),
    }
}

impl RunView {
    pub(super) fn checks(&self, r: &Run, w: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut list = v_flex().gap_1().child(heading("Required checks"));
        if let Some((lane, test)) = &self.selected_test
            && let Some(check) = r
                .commands()
                .iter()
                .find(|c| c.lane == *lane && c.passed_test(test))
        {
            list = list.child(subtle(format!("{} · {test} · passed", check.command), cx));
        }
        if let Some(regression) = r.regression() {
            list = list
                .child(heading("Independent regression test"))
                .child(subtle(regression.description(), cx));
        }
        let mut count = 0;
        for (i, lane) in r
            .lanes()
            .iter()
            .enumerate()
            .filter(|(_, l)| r.shows_check(l))
        {
            count += 1;
            let result = r
                .commands()
                .iter()
                .find(|c| c.lane == i && c.command == lane.command);
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
                                toggle(&mut v.expanded_checks, i);
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
        for (i, review) in r.template().reviews.iter().enumerate() {
            let stage = Stage::Review(i);
            let state = stage.status(r, &stage.observations(r));
            let latest = r.stage_reports(stage).next_back().map(|(_, report)| report);
            let mut label = state.label().to_owned();
            let mut color = match state {
                StageStatus::Complete => cx.theme().success,
                StageStatus::Failed | StageStatus::Interrupted => cx.theme().danger,
                StageStatus::InProgress => cx.theme().info,
                _ => cx.theme().muted_foreground,
            };
            if !matches!(state, StageStatus::InProgress | StageStatus::Interrupted)
                && let Some(hive_core::pipeline::Report::Review { findings, .. }) = latest
            {
                let passed;
                (label, passed) = review_outcome(findings);
                color = if passed {
                    cx.theme().success
                } else {
                    cx.theme().warning
                };
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
                                toggle(&mut v.expanded_reviews, i);
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
        if r.history().is_empty() {
            list = list.child(subtle("No handoffs have been recorded yet.", cx).py_3());
        }
        for (i, event) in r.history().iter().enumerate() {
            let text = event.trim();
            let first = text.lines().next().unwrap_or_default();
            let open = self.expanded_events.contains(&i);
            let expandable = first.len() != text.len() || first.chars().count() > 85;
            let hover = Hover::new(("event-hover", i), w, cx);
            let label = readable_history(first);
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
                            toggle(&mut v.expanded_events, i);
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
                        .child(SelectableText::new(
                            ("history-text", i),
                            readable_history(text),
                        )),
                );
            }
        }
        scroll("run-activity", list)
    }
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::review_outcome;
    use hive_core::pipeline::{Finding, Severity};

    fn finding(severity: Severity) -> Finding {
        Finding {
            lane: 0,
            severity,
            description: "Something".into(),
        }
    }

    #[test]
    fn a_review_with_only_notes_passed_and_blocking_findings_are_counted_alone() {
        assert_eq!(review_outcome(&[]), ("Passed".into(), true));
        let notes = [finding(Severity::Minor), finding(Severity::Minor)];
        assert_eq!(review_outcome(&notes), ("Passed · 2 notes".into(), true));
        let mixed = [finding(Severity::Blocker), finding(Severity::Minor)];
        assert_eq!(review_outcome(&mixed), ("1 finding".into(), false));
        let blocking = [finding(Severity::Major), finding(Severity::Blocker)];
        assert_eq!(review_outcome(&blocking), ("2 findings".into(), false));
    }
}
