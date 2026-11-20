//! Controls for the gate the run waits at: plan approval, repair decisions and acceptance.
use super::*;

/// Leave room for the diff and navigation when a plan has many criteria.
const CRITERIA_PREVIEW: usize = 3;
/// The expanded list scrolls inside this height instead of pushing the decision off screen.
const CRITERIA_MAX_HEIGHT: Rems = rems(9.);

impl RunView {
    pub(super) fn checkpoint_actions(
        &mut self,
        run: &Run,
        sessions: &[LiveSession],
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let mut row = h_flex().gap_2().flex_wrap();
        match run.phase() {
            Phase::ApprovePlan => {
                row = row.child(button(
                    "approve-plan",
                    "Approve plan",
                    ButtonLook::Primary,
                    w,
                    cx,
                    |v, _, cx| v.approve(cx),
                ));
                let contract = run.shared_contract().to_owned();
                let plan = run
                    .lanes()
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
                            v.contract = area(&contract, "Shared contract", w, cx);
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
                let accepting = p.accepting(run.id());
                let reviewed = matches!(p.review(run.id()), Some(Ok(_)));
                let unreadable = match p.review(run.id()) {
                    Some(Err(error)) if !accepting => Some(error.clone()),
                    _ => None,
                };
                if run.review_revision().is_none() && !accepting {
                    row = row.child(button(
                        "recheck-current-files",
                        "Recheck current files",
                        ButtonLook::Outline,
                        w,
                        cx,
                        |v, _, cx| v.edit(cx, |r| Ok(r.recheck()?)),
                    ));
                } else if let Some(error) = unreadable {
                    row = row
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().danger)
                                .child(format!("Changes could not be read: {error}")),
                        )
                        .child(button(
                            "reread-pipeline-changes",
                            "Read changes again",
                            ButtonLook::Outline,
                            w,
                            cx,
                            |v, _, cx| {
                                v.refresh_changes(cx);
                                cx.notify();
                            },
                        ));
                } else if reviewed
                    && !accepting
                    && let Err(reason) = run.can_accept()
                {
                    row = row.child(subtle(reason.to_string(), cx));
                } else if reviewed && !accepting {
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
            Phase::Regression(step) if step.needs_user() => {
                use hive_core::pipeline::RegressionStep;
                let regression = run.regression()?;
                row = row.child(subtle(regression.description(), cx));
                if step == RegressionStep::Choose {
                    for (ix, (_, line)) in run.criteria().enumerate() {
                        let line = line.to_owned();
                        row = row.child(button(
                            ("regression-criterion", ix),
                            line.clone(),
                            ButtonLook::Outline,
                            w,
                            cx,
                            move |v, _, cx| v.edit(cx, |r| Ok(r.choose_regression(&line)?)),
                        ));
                    }
                } else {
                    if let Some(blob) = regression.changed_blob() {
                        let blob = blob.to_owned();
                        row = row.child(button(
                            "approve-test",
                            "Approve revised test and recheck base",
                            ButtonLook::Outline,
                            w,
                            cx,
                            move |v, _, cx| v.edit(cx, |r| Ok(r.retry_regression(Some(&blob))?)),
                        ));
                    }
                    row = row
                        .child(button(
                            "retry-test",
                            "Retry check",
                            ButtonLook::Outline,
                            w,
                            cx,
                            |v, _, cx| v.edit(cx, |r| Ok(r.retry_regression(None)?)),
                        ))
                        .child(button(
                            "repair-test",
                            if regression.can_repair() {
                                "Return to developer"
                            } else {
                                "Revise reproducer"
                            },
                            ButtonLook::Outline,
                            w,
                            cx,
                            |v, _, cx| v.edit(cx, |r| Ok(r.repair_regression()?)),
                        ));
                }
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
                            && matches!(run.phase(), Phase::Decision(_))
                        {
                            v.decide(&run, w, cx);
                        }
                    },
                ));
            }
            _ => {
                let mut requests = sessions
                    .iter()
                    .filter_map(|session| Some((session, session.request_action()?)))
                    .peekable();
                if requests.peek().is_none() && !self.giving_feedback {
                    return None;
                }
                for (session, action) in requests {
                    let ticket = session.ticket;
                    let mut title = session.stage.title(run);
                    if let Stage::Develop(i) = session.stage {
                        let repo = path_name(&run.lanes()[i].project);
                        if title != repo {
                            title = format!("{title} · {repo}");
                        }
                    }
                    row = row.child(button(
                        ("open-agent-request", ticket.attempt as usize),
                        format!("{action} · {title}"),
                        ButtonLook::Primary,
                        w,
                        cx,
                        move |v, w, cx| v.terminal(ticket, w, cx),
                    ));
                }
            }
        }
        if matches!(run.phase(), Phase::ApprovePlan | Phase::Acceptance) {
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
        let mut block = v_flex().gap_2().child(row);
        if run.phase() == Phase::Acceptance {
            let mut rows = v_flex().gap_1();
            let mut total = 0;
            for (ix, line) in run.criteria() {
                total += 1;
                if total <= CRITERIA_PREVIEW || self.criteria_expanded {
                    rows = rows.child(self.criterion_row(run, ix, line, total, w, cx));
                }
            }
            let mut title = h_flex().gap_2().child(heading("Acceptance criteria"));
            if total > CRITERIA_PREVIEW {
                title = title.child(button(
                    "expand-criteria",
                    if self.criteria_expanded {
                        "Show fewer".into()
                    } else {
                        format!("Show all {total}")
                    },
                    ButtonLook::Ghost,
                    w,
                    cx,
                    |v, _, cx| {
                        v.criteria_expanded = !v.criteria_expanded;
                        cx.notify();
                    },
                ));
            }
            let rows = div()
                .id("acceptance-criteria")
                .max_h(CRITERIA_MAX_HEIGHT)
                .overflow_y_scroll()
                .child(rows);
            block = block.child(v_flex().gap_1().child(title).child(rows));
        }
        if run.phase() == Phase::Acceptance
            && let Some(notes) = review_notes(run, cx)
        {
            block = block.child(notes);
        }
        if self.giving_feedback {
            let gate = run.phase();
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
        Some(block)
    }
}

/// Reviewer findings that did not block the run, listed where the user decides to accept.
fn review_notes(run: &Run, cx: &App) -> Option<Div> {
    let notes = run.notes();
    if notes.is_empty() {
        return None;
    }
    let mut list = v_flex().gap_1().child(subtle(
        format!(
            "{} did not block acceptance",
            count(notes.len(), "reviewer note", "reviewer notes")
        ),
        cx,
    ));
    for (i, (review, note)) in notes.iter().enumerate() {
        let (Some(review), Some(lane)) = (
            run.template().reviews.get(*review),
            run.lanes().get(note.lane),
        ) else {
            continue;
        };
        list = list.child(div().text_sm().child(SelectableText::new(
            ("review-note", i),
            format!("{} · {}: {}", review.name, lane.name, note.description),
        )));
    }
    Some(list)
}
