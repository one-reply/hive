//! Controls for the gate the run waits at: plan approval, repair decisions and acceptance.
use super::*;

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
                if let Some(error) = unreadable {
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
