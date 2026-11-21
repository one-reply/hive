//! The evidence and attempt controls revealed by selecting a stage.
use super::*;

impl RunView {
    pub(super) fn stage_detail(
        &mut self,
        stage: Stage,
        r: &Run,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut detail = v_flex()
            .mx_2()
            .mb_2()
            .px_3()
            .py_2()
            .gap_2()
            .border_l_2()
            .border_color(cx.theme().border);
        let step = match stage {
            Stage::Planner => Some(&r.template().planner),
            Stage::Develop(_) => Some(&r.template().development),
            Stage::Review(i) => r.template().reviews.get(i),
            _ => None,
        };
        if let Some(step) = step {
            let profile = match stage {
                Stage::Develop(i) => &r.lanes()[i].profile,
                _ => &step.profile,
            };
            let mut meta = vec![profile.agent.name()];
            if !profile.model.is_empty() {
                meta.push(&profile.model);
            }
            if !profile.effort.is_empty() {
                meta.push(&profile.effort);
            }
            detail = detail.child(subtle(meta.join(" · "), cx));
            if !step.instruction.trim().is_empty() {
                detail = detail.child(div().text_sm().child(step.instruction.clone()));
            }
        }
        let now = elapsed::now_ms();
        if let Some((start, _)) = elapsed::range(r.timings(), now) {
            for timing in stage.observations(r) {
                let action = timing
                    .attempt
                    .map_or_else(|| "Checkpoint".to_string(), |n| format!("Attempt {n}"));
                let length = if timing.failed && timing.end_ms.is_none() {
                    "end unknown".to_owned()
                } else {
                    elapsed::duration(
                        elapsed::observed_end(timing, now).saturating_sub(timing.start_ms),
                    )
                };
                let when = elapsed::duration(timing.start_ms.saturating_sub(start));
                detail = detail.child(subtle(
                    format!("{action} · started at +{when} · {length}"),
                    cx,
                ));
            }
        }
        match stage {
            Stage::Approval => {
                if r.template().shared_contract {
                    detail = if self.editing_plan {
                        detail.child(paragraph("Shared contract", &self.contract, cx))
                    } else {
                        detail
                            .child(heading("Shared contract"))
                            .child(div().text_sm().child(r.shared_contract().to_owned()))
                    };
                }
                let mut assigned = 0;
                for (i, lane) in r.lanes().iter().enumerate() {
                    let fields = self.editing_plan.then(|| self.plan.get(i)).flatten();
                    if fields.is_none() && lane.assignment.is_empty() {
                        continue;
                    }
                    assigned += 1;
                    detail = detail.child(heading(format!(
                        "{} · {}",
                        lane.name,
                        path_name(&lane.project)
                    )));
                    if let Some((task, criteria)) = fields {
                        detail = detail
                            .child(paragraph("Assignment", task, cx))
                            .child(paragraph("Acceptance criteria", criteria, cx));
                    } else {
                        detail = detail
                            .child(div().text_sm().child(lane.assignment.clone()))
                            .child(subtle("Acceptance criteria", cx))
                            .child(div().text_sm().child(lane.criteria.clone()));
                    }
                }
                if assigned == 0 {
                    detail =
                        detail.child(subtle("The planner has not submitted assignments yet.", cx));
                }
                if r.phase() == Phase::ApprovePlan {
                    detail = detail.child(h_flex().pt_1().child(button(
                        "approve-plan-inline",
                        "Approve plan and start development",
                        ButtonLook::Primary,
                        w,
                        cx,
                        |view, _, cx| view.approve(cx),
                    )));
                }
            }
            Stage::Develop(i) => {
                let lane = &r.lanes()[i];
                detail = detail.child(heading(path_name(&lane.project)));
                if !lane.assignment.is_empty() {
                    detail = detail
                        .child(div().text_sm().child(lane.assignment.clone()))
                        .child(subtle(lane.criteria.clone(), cx));
                }
            }
            Stage::Review(i) | Stage::Decision(i) => {
                if let Some(step) = r.template().reviews.get(i) {
                    detail = detail.child(subtle(
                        format!("Repair rounds · {} of {}", r.cycles(i), step.limit),
                        cx,
                    ));
                }
                if r.phase() == Phase::Decision(i) || r.phase() == Phase::Commands(i) {
                    for f in r.findings() {
                        if let Some(lane) = r.lanes().get(f.lane) {
                            detail = detail.child(
                                div()
                                    .text_sm()
                                    .child(format!("{} · {}", lane.name, f.description)),
                            );
                        }
                    }
                }
            }
            Stage::Commands(_) => {
                detail = detail.child(h_flex().child(button(
                    "inspect-stage-checks",
                    "View checks",
                    ButtonLook::Outline,
                    w,
                    cx,
                    |v, _, cx| {
                        v.tab = RunTab::Checks;
                        cx.notify();
                    },
                )))
            }
            Stage::Accept => {
                detail = detail.child(h_flex().child(button(
                    "inspect-stage-changes",
                    "View changes",
                    ButtonLook::Outline,
                    w,
                    cx,
                    |v, _, cx| {
                        v.tab = RunTab::Changes;
                        v.refresh_changes(cx);
                        cx.notify();
                    },
                )))
            }
            Stage::Regression => {
                if let Some(test) = r.regression() {
                    detail = detail.child(subtle(test.description(), cx));
                }
            }
            Stage::Planner => {}
        }
        if r.stage_reports(stage).next().is_some() {
            detail = detail.child(self.handoffs(stage, r, w, cx));
        }
        for ticket in r
            .pending()
            .into_iter()
            .filter(|t| Stage::from_phase(r.phase(), t.lane) == Some(stage))
        {
            let p = AppState::get(cx).pipelines.read(cx);
            if p.session(&ticket).is_some() {
                detail = detail.child(h_flex().child(button(
                    ("open-pipeline-attempt", ticket.attempt as usize),
                    "Open session",
                    ButtonLook::Outline,
                    w,
                    cx,
                    move |v, w, cx| v.terminal(ticket, w, cx),
                )));
            } else if p.error(r.id()).is_some() && r.sessions().iter().any(|(t, _)| *t == ticket) {
                detail = detail.child(h_flex().child(button(
                    ("resume-pipeline-attempt", ticket.attempt as usize),
                    "Resume conversation",
                    ButtonLook::Outline,
                    w,
                    cx,
                    move |v, _, cx| {
                        v.error = AppState::get(cx)
                            .pipelines
                            .clone()
                            .update(cx, |p, cx| p.resume(&ticket, cx))
                            .err();
                        cx.notify();
                    },
                )));
            }
        }
        detail
    }
}
