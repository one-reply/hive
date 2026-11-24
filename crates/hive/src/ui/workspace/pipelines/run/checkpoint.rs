//! Controls for the gate the run waits at: plan approval, repair decisions and acceptance.
use super::*;
use gpui_kit::component::Disableable as _;

impl RunView {
    pub(super) fn checkpoint_actions(
        &mut self,
        run: &Run,
        sessions: &[LiveSession],
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let mut row = h_flex().gap_2().flex_wrap();
        // What holds the gate shut, said after its buttons.
        let mut blocker = None;
        match run.phase() {
            Phase::ApprovePlan => {
                row = row.child(button_with_icon(
                    "approve-plan",
                    "Approve plan",
                    Lucide::Check,
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
                row = row.child(button_with_icon(
                    "edit-plan",
                    if self.editing_plan {
                        "Discard edits"
                    } else {
                        "Edit plan"
                    },
                    if self.editing_plan {
                        Lucide::Undo2
                    } else {
                        Lucide::Pencil
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
                    row = row.child(button_with_icon(
                        "recheck-current-files",
                        "Recheck current files",
                        Lucide::RefreshCw,
                        ButtonLook::Outline,
                        w,
                        cx,
                        |v, _, cx| v.edit(cx, |r| Ok(r.recheck()?)),
                    ));
                } else if let Some(error) = unreadable {
                    row = row
                        .child(notice(
                            Lucide::CircleX,
                            cx.theme().danger,
                            format!("Changes could not be read: {error}"),
                        ))
                        .child(button_with_icon(
                            "reread-pipeline-changes",
                            "Read changes again",
                            Lucide::RefreshCw,
                            ButtonLook::Outline,
                            w,
                            cx,
                            |v, _, cx| {
                                v.refresh_changes(cx);
                                cx.notify();
                            },
                        ));
                } else if reviewed && !accepting {
                    let reason = run.can_accept().err();
                    row = row.child(Self::accept_button(reason.is_none(), w, cx));
                    blocker = reason.map(|reason| {
                        notice(Lucide::CircleAlert, cx.theme().warning, reason.to_string())
                    });
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
                row = row.child(button_with_icon(
                    "inspect-changes",
                    "Review files",
                    Lucide::FileDiff,
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
                row = row.child(button(
                    "skip-regression",
                    "Continue without this test",
                    ButtonLook::Ghost,
                    w,
                    cx,
                    |_, w, cx| {
                        Self::ask_reason(
                            "Continue without the independent test",
                            "Its criterion will need other evidence at acceptance.".into(),
                            "Why can this run continue without it?",
                            w,
                            cx,
                            |run, reason| Ok(run.skip_regression(&reason)?),
                        );
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
            row = row.child(button_with_icon(
                "request-feedback",
                if self.giving_feedback {
                    "Cancel feedback"
                } else {
                    "Request changes"
                },
                if self.giving_feedback {
                    Lucide::X
                } else {
                    Lucide::MessageSquarePlus
                },
                ButtonLook::Outline,
                w,
                cx,
                |v, _, cx| {
                    v.giving_feedback = !v.giving_feedback;
                    cx.notify();
                },
            ));
        }
        let mut block = v_flex().gap_3().child(row.children(blocker));
        if run.phase() == Phase::Acceptance {
            block = block.child(self.acceptance_review(run, w, cx));
        }
        if self.giving_feedback {
            let gate = run.phase();
            block = block
                .child(paragraph("Feedback (optional)", &self.note, cx))
                .child(h_flex().child(button_with_icon(
                    "send-feedback",
                    "Send request",
                    Lucide::Send,
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

    /// Stays in place while something holds acceptance back, so the gate always shows where
    /// it ends.
    fn accept_button(ready: bool, w: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let hover = Hover::new("accept-pipeline", w, cx).enabled(ready);
        Button::new("accept-pipeline")
            .cursor_pointer()
            .small()
            .icon(Lucide::Check)
            .label("Accept changes")
            .disabled(!ready)
            .on_click(cx.listener(|v, _, _, cx| {
                v.error = AppState::get(cx)
                    .pipelines
                    .clone()
                    .update(cx, |p, cx| p.accept(v.id, cx))
                    .err();
                cx.notify();
            }))
            .smooth_hover(ButtonLook::Primary, &hover, cx)
    }
}

/// Why the gate cannot be passed yet, tinted so it reads apart from the buttons beside it.
fn notice(icon: Lucide, color: Hsla, text: impl Into<SharedString>) -> Div {
    h_flex()
        .min_w_0()
        .gap_1p5()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(color.opacity(STATUS_TINT_OPACITY))
        .child(Icon::new(icon).flex_none().size_3p5().text_color(color))
        .child(div().flex_1().min_w_0().text_xs().child(text.into()))
}
