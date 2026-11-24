//! Evidence belongs to the attempt that returned it, including findings later repaired or dismissed.
use super::*;
use hive_core::pipeline::Report;

impl RunView {
    pub(super) fn handoffs(
        &self,
        stage: Stage,
        run: &Run,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut results = v_flex().gap_2();
        let mut reports = run.stage_reports(stage).rev().peekable();
        let mut latest = true;
        while let Some((ticket, report)) = reports.next() {
            let open = latest || self.expanded_reports.contains(&ticket.attempt);
            if latest {
                results = results.child(heading(if reports.peek().is_some() {
                    "Latest report"
                } else {
                    "Agent report"
                }));
            } else {
                results = results.child(h_flex().child(button(
                    ("historical-report", ticket.attempt as usize),
                    format!(
                        "{} report · Attempt {}",
                        if open { "Hide" } else { "Show" },
                        ticket.attempt
                    ),
                    ButtonLook::Ghost,
                    w,
                    cx,
                    move |v, _, cx| {
                        toggle(&mut v.expanded_reports, ticket.attempt);
                        cx.notify();
                    },
                )));
            }
            if open {
                results = results.child(report_body(ticket, report, run, cx));
            }
            latest = false;
        }
        results
    }
}
pub(super) fn report_body(
    ticket: Ticket,
    report: &Report,
    run: &Run,
    cx: &App,
) -> impl IntoElement {
    let mut body = v_flex().gap_1();
    let id = ticket.attempt as usize;
    match report {
        Report::Regression { test } => {
            body = body.child(subtle(format!("{} · {}", test.path, test.name), cx));
        }
        Report::Plan {
            assignments,
            contract,
        } => {
            if !contract.is_empty() {
                body = body
                    .child(heading("Shared contract"))
                    .child(div().text_sm().child(contract.clone()));
            }
            for assignment in assignments {
                if let Some(lane) = run.lanes().get(assignment.lane) {
                    body = body
                        .child(heading(lane.name.clone()))
                        .child(div().text_sm().child(assignment.task.clone()))
                        .child(subtle("Acceptance criteria", cx))
                        .child(div().text_sm().child(assignment.criteria.clone()));
                }
            }
        }
        Report::Developed { summary } | Report::Review { summary, .. } => {
            body = body.child(
                div()
                    .text_sm()
                    .child(SelectableText::new(("agent-summary", id), summary.clone())),
            );
            if let Report::Review { findings, .. } = report {
                if findings.is_empty() {
                    body = body.child(subtle("No findings reported.", cx));
                }
                for f in findings {
                    if let Some(lane) = run.lanes().get(f.lane) {
                        body = body.child(
                            h_flex()
                                .items_start()
                                .gap_2()
                                .child(
                                    Icon::new(Lucide::CircleAlert)
                                        .flex_none()
                                        .size_3p5()
                                        .text_color(if f.severity.blocks() {
                                            cx.theme().warning
                                        } else {
                                            cx.theme().muted_foreground
                                        }),
                                )
                                .child(
                                    // Beside the icon the text is sized along the row, where
                                    // it keeps its one-line width unless it may shrink; then
                                    // it wraps.
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .gap_1()
                                        .child(heading(format!(
                                            "{} · {}",
                                            lane.name,
                                            f.severity.label()
                                        )))
                                        .child(
                                            div()
                                                .debug_selector(|| "reported-finding".into())
                                                .text_sm()
                                                .child(f.description.clone()),
                                        ),
                                ),
                        );
                    }
                }
            }
        }
    }
    div()
        .id(("reported-evidence", id))
        .max_h(rems(15.))
        .overflow_y_scroll()
        .child(body)
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::report_body;
    use gpui_kit as gpui;
    use gpui_kit::prelude::*;
    use gpui_kit::{Context, Pixels, TestAppContext, Window, div, px};
    use hive_core::pipeline::{Finding, Lane, Report, Run, Severity, Template, Ticket};

    const WIDTH: Pixels = px(320.);

    /// A review report in a column `WIDTH` wide.
    struct Column(Run, Report);
    impl Render for Column {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let ticket = Ticket {
                run: self.0.id(),
                attempt: 1,
                lane: None,
                review: None,
            };
            div()
                .w(WIDTH)
                .child(report_body(ticket, &self.1, &self.0, cx))
        }
    }

    #[gpui::test]
    fn a_long_finding_wraps_inside_the_report_instead_of_running_past_it(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let lane = Lane {
            name: "Development".into(),
            project: "/unused-report-test-project".into(),
            base: "HEAD".into(),
            ..Lane::default()
        };
        let run = Run::new(
            7,
            lane.project.clone(),
            "Task".into(),
            Template::default(),
            vec![lane],
        )
        .expect("a run over one lane");
        let report = Report::Review {
            summary: "Summary".into(),
            evidence: Vec::new(),
            findings: vec![Finding {
                lane: 0,
                severity: Severity::Major,
                description: "The check returns false for any address. ".repeat(12),
            }],
        };
        let (_, cx) = cx.add_window_view(|_, _| Column(run, report));
        cx.run_until_parked();

        let finding = cx.debug_bounds("reported-finding").expect("the finding");
        assert!(finding.right() <= WIDTH, "{finding:?}");
        let one_line = cx.update(|window, _| window.line_height());
        assert!(finding.size.height > one_line * 2., "{finding:?}");
    }
}
