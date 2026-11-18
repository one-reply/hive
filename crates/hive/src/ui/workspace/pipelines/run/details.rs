//! Checkpoint controls and the evidence revealed by selecting a stage.
use super::*;

impl RunView {
    pub(super) fn actions(&mut self, run: &Run, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut row = h_flex().gap_2().flex_wrap();
        match run.phase {
            Phase::ApprovePlan => {
                row = row.child(button(
                    "approve-plan",
                    "Approve plan",
                    ButtonLook::Primary,
                    w,
                    cx,
                    |v, _, cx| v.approve(cx),
                ));
                let plan = run
                    .lanes
                    .iter()
                    .map(|l| (l.assignment.clone(), l.criteria.clone()))
                    .collect::<Vec<_>>();
                row = row.child(button(
                    "edit-plan",
                    if self.editing_plan {
                        "Discard edits"
                    } else {
                        "Edit plan"
                    },
                    ButtonLook::Outline,
                    w,
                    cx,
                    move |v, w, cx| {
                        v.editing_plan = !v.editing_plan;
                        v.tab = RunTab::Timeline;
                        v.selected_stage = Some(Stage::Approval);
                        if v.editing_plan {
                            v.plan = plan
                                .iter()
                                .map(|(task, criteria)| {
                                    (
                                        area(task, "Assignment", w, cx),
                                        area(criteria, "Acceptance criteria", w, cx),
                                    )
                                })
                                .collect();
                        }
                        cx.notify();
                    },
                ));
            }
            Phase::Acceptance => {
                let p = AppState::get(cx).pipelines.read(cx);
                let accepting = p.accepting(run.id);
                let reviewed = matches!(p.reviews.get(&run.id), Some(Ok(_)));
                if reviewed && !accepting {
                    row = row.child(button(
                        "accept-pipeline",
                        "Accept changes",
                        ButtonLook::Primary,
                        w,
                        cx,
                        |v, _, cx| {
                            v.error = AppState::get(cx)
                                .pipelines
                                .clone()
                                .update(cx, |p, cx| p.accept(v.id, cx))
                                .err();
                            cx.notify();
                        },
                    ));
                } else {
                    row = row.child(subtle(
                        if accepting {
                            "Verifying changes…"
                        } else {
                            "Reading changes for acceptance…"
                        },
                        cx,
                    ));
                }
                row = row.child(button(
                    "inspect-changes",
                    "Review files",
                    ButtonLook::Outline,
                    w,
                    cx,
                    |v, _, cx| {
                        v.tab = RunTab::Changes;
                        cx.notify();
                    },
                ));
            }
            Phase::Decision(_) => {
                row = row.child(button(
                    "review-findings",
                    "Review findings",
                    ButtonLook::Primary,
                    w,
                    cx,
                    |v, w, cx| {
                        if let Some(run) = v.run.clone()
                            && matches!(run.phase, Phase::Decision(_))
                        {
                            v.decide(&run, w, cx);
                        }
                    },
                ));
            }
            _ => {
                let mut active = run
                    .pending()
                    .into_iter()
                    .filter(|t| AppState::get(cx).pipelines.read(cx).session(t).is_some());
                if let Some(ticket) = active.next()
                    && active.next().is_none()
                {
                    row = row.child(button(
                        "open-active-session",
                        "Open agent session",
                        ButtonLook::Outline,
                        w,
                        cx,
                        move |v, w, cx| v.terminal(ticket, w, cx),
                    ));
                }
            }
        }
        if matches!(run.phase, Phase::ApprovePlan | Phase::Acceptance) {
            row = row.child(button(
                "request-feedback",
                if self.giving_feedback {
                    "Cancel feedback"
                } else {
                    "Request changes"
                },
                ButtonLook::Ghost,
                w,
                cx,
                |v, _, cx| {
                    v.giving_feedback = !v.giving_feedback;
                    cx.notify();
                },
            ));
        }
        row = row.child(div().flex_1()).child(button(
            "cancel-pipeline",
            "Stop run",
            ButtonLook::Ghost,
            w,
            cx,
            |v, w, cx| v.stop(w, cx),
        ));
        let mut block = v_flex().gap_2().child(row);
        if self.giving_feedback {
            let gate = run.phase;
            block = block
                .child(paragraph("Feedback (optional)", &self.note, cx))
                .child(h_flex().child(button(
                    "send-feedback",
                    "Send request",
                    ButtonLook::Primary,
                    w,
                    cx,
                    move |v, _, cx| {
                        let note = v.note.read(cx).value().to_string();
                        v.edit(cx, |r| {
                            match gate {
                                Phase::ApprovePlan => r.replan(&note),
                                _ => r.request_changes(&note),
                            }
                            .map_err(Into::into)
                        });
                    },
                )));
        }
        block
    }

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
            Stage::Planner => Some(&r.template.planner),
            Stage::Develop(_) => Some(&r.template.development),
            Stage::Review(i) => r.template.reviews.get(i),
            _ => None,
        };
        if let Some(step) = step {
            let profile = match stage {
                Stage::Develop(i) => &r.lanes[i].profile,
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
        if let Some((start, _)) = elapsed::range(&r.timings, now) {
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
                let mut assigned = 0;
                for (i, lane) in r.lanes.iter().enumerate() {
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
            }
            Stage::Develop(i) => {
                let lane = &r.lanes[i];
                detail = detail.child(heading(path_name(&lane.project)));
                if !lane.assignment.is_empty() {
                    detail = detail
                        .child(div().text_sm().child(lane.assignment.clone()))
                        .child(subtle(lane.criteria.clone(), cx));
                }
            }
            Stage::Review(i) | Stage::Decision(i) => {
                if let Some(step) = r.template.reviews.get(i) {
                    detail = detail.child(subtle(
                        format!("Repair rounds · {} of {}", r.cycles(i), step.limit),
                        cx,
                    ));
                }
                if r.phase == Phase::Decision(i) || r.phase == Phase::Commands(i) {
                    for f in &r.findings {
                        if let Some(lane) = r.lanes.get(f.lane) {
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
            Stage::Planner => {}
        }
        if handoff::stage_reports(r, stage).next().is_some() {
            detail = detail.child(self.handoffs(stage, r, w, cx));
        }
        for ticket in r
            .pending()
            .into_iter()
            .filter(|t| Stage::from_phase(r.phase, t.lane) == Some(stage))
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
            } else if p.error(r.id).is_some() && r.sessions.iter().any(|(t, _)| *t == ticket) {
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
