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
                        if !v.expanded_reports.remove(&ticket.attempt) {
                            v.expanded_reports.insert(ticket.attempt);
                        }
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
        Report::Plan { assignments } => {
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
                                        .size_3p5()
                                        .text_color(cx.theme().warning),
                                )
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(heading(lane.name.clone()))
                                        .child(div().text_sm().child(f.description.clone())),
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
