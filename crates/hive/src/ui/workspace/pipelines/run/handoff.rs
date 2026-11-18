//! Evidence belongs to the attempt that returned it, including findings later repaired or dismissed.
use super::*;
use hive_core::pipeline::Report;

pub(super) fn stage_reports(
    run: &Run,
    stage: Stage,
) -> impl DoubleEndedIterator<Item = (Ticket, &Report)> {
    run.reports
        .iter()
        .filter(move |(ticket, phase, _)| Stage::from_phase(*phase, ticket.lane) == Some(stage))
        .map(|(ticket, _, report)| (*ticket, report))
}
impl RunView {
    pub(super) fn handoffs(
        &self,
        stage: Stage,
        run: &Run,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut results = v_flex().gap_2();
        let mut reports = stage_reports(run, stage).rev().peekable();
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
fn report_body(ticket: Ticket, report: &Report, run: &Run, cx: &App) -> impl IntoElement {
    let mut body = v_flex().gap_1();
    let id = ticket.attempt as usize;
    match report {
        Report::Plan { assignments } => {
            for assignment in assignments {
                if let Some(lane) = run.lanes.get(assignment.lane) {
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
                    if let Some(lane) = run.lanes.get(f.lane) {
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

#[cfg(test)]
mod tests {
    use super::{Stage, stage_reports};
    use hive_core::pipeline::{Lane, Phase, Report, Run, Template, Ticket};

    #[test]
    fn histories_with_colliding_names_resolve_handoffs_only_by_phase_lane_and_ticket() {
        let mut run = Run::new(
            12,
            "/qa".into(),
            "Task".into(),
            Template::default(),
            vec![Lane {
                name: "QA".into(),
                project: "/qa".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        run.history.push("QA: unrelated free text".into());
        assert!(stage_reports(&run, Stage::Review(0)).next().is_none());
        let developer = Ticket {
            run: 12,
            attempt: 2,
            lane: Some(0),
        };
        let review = Ticket {
            run: 12,
            attempt: 3,
            lane: None,
        };
        run.reports.push((
            developer,
            Phase::Development,
            Report::Developed {
                summary: "Build complete".into(),
            },
        ));
        run.reports.push((
            review,
            Phase::Review(0),
            Report::Review {
                summary: "QA complete".into(),
                findings: vec![],
            },
        ));
        assert_eq!(
            stage_reports(&run, Stage::Develop(0)).last().unwrap().0,
            developer
        );
        assert_eq!(
            stage_reports(&run, Stage::Review(0)).last().unwrap().0,
            review
        );
        assert_eq!(stage_reports(&run, Stage::Review(1)).count(), 0);
    }
}
