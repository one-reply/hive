//! Actions are bound to the review the user was looking at when opening the dialog.
use super::*;
use hive_core::pipeline::{CriterionDecision, EvidenceSource};

impl RunView {
    pub(super) fn criterion_row(
        &self,
        run: &Run,
        lane: usize,
        line: &str,
        index: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let source = run.criterion_source(lane, line);
        let mut row = v_flex()
            .child(
                div()
                    .text_sm()
                    .child(format!("{} · {line}", run.lanes()[lane].name)),
            )
            .child(subtle(source.to_string(), cx));
        let Some(revision) = run.review_revision() else {
            return row;
        };
        let mut actions = h_flex().gap_1().flex_wrap();
        if let EvidenceSource::Test(name) = &source {
            let name = name.to_string();
            actions = actions.child(button(
                ("see-test", index),
                "View test",
                ButtonLook::Ghost,
                w,
                cx,
                move |v, _, cx| {
                    v.tab = RunTab::Checks;
                    v.expanded_checks.insert(lane);
                    v.selected_test = Some((lane, name.clone()));
                    cx.notify();
                },
            ));
        }
        for (manual, label, id) in [
            (true, "Record observation", "manual-proof"),
            (false, "Accept exception", "waive-proof"),
        ] {
            let (line, revision) = (line.to_owned(), revision.to_owned());
            actions = actions.child(button(
                (id, index),
                label,
                ButtonLook::Ghost,
                w,
                cx,
                move |_, w, cx| {
                    let input = area(
                        "",
                        if manual {
                            "What did you observe?"
                        } else {
                            "Why accept this risk?"
                        },
                        w,
                        cx,
                    );
                    let view = cx.weak_entity();
                    let (line, revision) = (line.clone(), revision.clone());
                    w.open_alert_dialog(cx, move |d, _, cx| {
                        let (line, revision, input, view) =
                            (line.clone(), revision.clone(), input.clone(), view.clone());
                        d.title(label)
                            .child(subtle(line.clone(), cx))
                            .child(paragraph("Details", &input, cx))
                            .button_props(
                                DialogButtonProps::default()
                                    .ok_text("Save")
                                    .show_cancel(true)
                                    .on_ok(move |_, _, cx| {
                                        let note = input.read(cx).value().trim().to_owned();
                                        if note.is_empty() {
                                            return false;
                                        }
                                        let decision = if manual {
                                            CriterionDecision::Manual(note)
                                        } else {
                                            CriterionDecision::Exception(note)
                                        };
                                        let _ = view.update(cx, |v, cx| {
                                            v.edit(cx, |r| {
                                                Ok(r.decide_criterion(
                                                    lane, &line, &revision, decision,
                                                )?)
                                            })
                                        });
                                        true
                                    }),
                            )
                    });
                },
            ));
        }
        let (line, revision) = (line.to_owned(), revision.to_owned());
        actions = actions.child(button(
            ("verify-criterion", index),
            "Return to QA",
            ButtonLook::Ghost,
            w,
            cx,
            move |v, _, cx| {
                v.edit(cx, |r| Ok(r.verify_criterion(lane, &line, &revision)?));
            },
        ));
        row = row.child(actions);
        row
    }
}
