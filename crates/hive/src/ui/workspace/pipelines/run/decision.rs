//! Explicit user decisions at a repair limit or cancellation.
use super::*;

impl RunView {
    pub(super) fn stop(&self, w: &mut Window, cx: &mut Context<Self>) {
        let id = self.id;
        let view = cx.weak_entity();
        w.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            dialog
                .title("Stop this pipeline?")
                .child("Running work will stop. The worktrees and changes will remain available.")
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Stop run")
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let result = AppState::get(cx)
                                .pipelines
                                .clone()
                                .update(cx, |p, cx| p.cancel(id, cx));
                            let _ = view.update(cx, |this, cx| {
                                this.error = result.err();
                                cx.notify();
                            });
                            true
                        }),
                )
        });
    }
    pub(super) fn decide(&self, r: &Run, w: &mut Window, cx: &mut Context<Self>) {
        let id = self.id;
        let gate = r.phase();
        let findings = r.findings().to_vec();
        let options: Vec<String> = std::iter::once("Allow one more repair".to_owned())
            .chain(
                findings
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("Dismiss finding {}", i + 1)),
            )
            .collect();
        let select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(options.clone()),
                Some(IndexPath::default()),
                w,
                cx,
            )
        });
        let note = area("", "Explain this decision", w, cx);
        let mut evidence = r
            .findings()
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{}. {} · {}", i + 1, r.lanes()[f.lane].name, f.description))
            .collect::<Vec<_>>();
        for command in r.commands().iter().filter(|cmd| !cmd.success) {
            evidence.push(format!(
                "Required check failed: {}\n{}",
                command.command, command.output
            ));
        }
        let history = r.history().join("\n");
        let view = cx.weak_entity();
        w.open_alert_dialog(cx, move |dialog, _, cx| {
            let view = view.clone();
            let note = note.clone();
            let select = select.clone();
            let options = options.clone();
            let findings = findings.clone();
            dialog
                .title("Review needs a decision")
                .child(
                    div()
                        .id("decision-evidence")
                        .max_h(rems(10.))
                        .overflow_y_scroll()
                        .child(SelectableText::new("findings", evidence.join("\n\n"))),
                )
                .child(subtle("Attempt history", cx))
                .child(
                    div()
                        .id("decision-history")
                        .max_h(rems(6.))
                        .overflow_y_scroll()
                        .child(SelectableText::new("attempt-history", history.clone())),
                )
                .child(Select::new(&select).w_full())
                .child(paragraph("Reason", &note, cx))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Apply decision")
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            let reason = note.read(cx).value().to_string();
                            if reason.trim().is_empty() {
                                return false;
                            }
                            let Some(index) = select
                                .read(cx)
                                .selected_value()
                                .and_then(|v| options.iter().position(|o| o == v))
                            else {
                                return false;
                            };
                            let result = AppState::get(cx).pipelines.clone().update(cx, |p, cx| {
                                p.edit(id, cx, |r| {
                                    if r.phase() != gate || r.findings() != findings {
                                        return Err("Findings changed. Reopen the decision.".into());
                                    }
                                    if index == 0 {
                                        r.retry_gate(&reason).map_err(|error| error.to_string())
                                    } else {
                                        r.dismiss_finding(index - 1, &reason)
                                            .map_err(|error| error.to_string())
                                    }
                                })
                            });
                            let _ = view.update(cx, |this, cx| {
                                this.error = result.err();
                                cx.notify();
                            });
                            true
                        }),
                )
        });
    }
}
