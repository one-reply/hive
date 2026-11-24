//! Explicit user decisions at a repair limit or cancellation.
use super::*;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::dialog::{AlertDialog, DialogFooter};
use hive_core::pipeline::Severity;

/// Wide enough for a finding to read as a paragraph, not a column of words.
const DIALOG_WIDTH: Pixels = px(560.);
/// Lines of a failed check's output the dialog shows; the Checks tab has all of it.
const OUTPUT_TAIL_LINES: usize = 12;

type Picker = Entity<SelectState<SearchableVec<String>>>;
/// Applies a choice with its reason and says whether the dialog closes.
type Apply = Rc<dyn Fn(Choice, String, &mut App) -> bool>;

/// What the user can decide at the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Repair,
    /// Index into the run's open findings.
    Dismiss(usize),
}

impl Choice {
    fn label(self) -> String {
        match self {
            Self::Repair => "Send back for one more repair".into(),
            Self::Dismiss(ix) => format!("Dismiss finding {}", ix + 1),
        }
    }

    /// What applying the choice does, shown under the picker.
    fn consequence(self, failed_checks: bool) -> String {
        match self {
            Self::Repair => "The developer gets the open issues with your reason, \
                             then the checks and the review run again."
                .into(),
            Self::Dismiss(ix) if failed_checks => format!(
                "Finding {} is accepted as it is. The failed check still has to be repaired.",
                ix + 1
            ),
            Self::Dismiss(ix) => format!(
                "Finding {} is accepted as it is, and the reviewer is asked not to raise it again.",
                ix + 1
            ),
        }
    }
}

struct FindingCard {
    title: String,
    severity: Severity,
    description: String,
}

/// The heading of a finding: its number in the list, the review that raised it when the
/// gate holds findings of several, the lane it is about and how severe it is.
fn finding_title(number: usize, reviewer: Option<&str>, lane: &str, severity: &str) -> String {
    match reviewer {
        Some(reviewer) => format!("{number} · {reviewer} · {lane} · {severity}"),
        None => format!("{number} · {lane} · {severity}"),
    }
}

struct FailedCheck {
    command: String,
    /// Repository and exit status.
    origin: String,
    output: String,
    /// Whether `output` leaves out earlier lines.
    clipped: bool,
}

/// The gate as the dialog shows it, read from the run when the dialog opens.
struct Brief {
    summary: String,
    findings: Vec<FindingCard>,
    failed: Vec<FailedCheck>,
}

impl Brief {
    fn read(run: &Run, review: usize) -> Self {
        let lane = |ix: usize| run.lanes().get(ix);
        // Reviews that ran together bring their findings to one gate: say whose each is.
        let reviewers: Vec<_> = run.findings().iter().map(|f| run.reviewer(f)).collect();
        let several = reviewers.iter().any(|reviewer| *reviewer != reviewers[0]);
        let reviewer = |i: usize| {
            let step = reviewers[i].and_then(|review| run.template().reviews.get(review));
            step.filter(|_| several).map(|step| step.name.as_str())
        };
        let findings: Vec<_> = run
            .findings()
            .iter()
            .enumerate()
            .map(|(i, f)| FindingCard {
                title: finding_title(
                    i + 1,
                    reviewer(i),
                    lane(f.lane).map_or("Unknown lane", |l| &l.name),
                    f.severity.label(),
                ),
                severity: f.severity,
                description: f.description.clone(),
            })
            .collect();
        let failed: Vec<_> = run
            .commands()
            .iter()
            .filter(|c| !c.success)
            .map(|c| {
                let (output, clipped) = tail(&c.output, OUTPUT_TAIL_LINES);
                let mut origin = lane(c.lane).map_or_else(String::new, |l| path_name(&l.project));
                if let Some(code) = c.exit_code {
                    origin = format!("{origin} · exit {code}");
                }
                FailedCheck {
                    command: c.command.clone(),
                    origin,
                    output: output.to_owned(),
                    clipped,
                }
            })
            .collect();
        let step = run.template().reviews.get(review);
        Self {
            summary: summary(
                step.map_or("The review", |s| &s.name),
                findings.len(),
                failed.len(),
                run.cycles(review),
                step.map_or(0, |s| s.limit),
            ),
            findings,
            failed,
        }
    }

    fn choices(&self) -> Vec<Choice> {
        std::iter::once(Choice::Repair)
            .chain((0..self.findings.len()).map(Choice::Dismiss))
            .collect()
    }
}

/// One sentence on why the run stopped here.
fn summary(review: &str, findings: usize, failed: usize, rounds: u8, limit: u8) -> String {
    let mut open = Vec::new();
    if findings > 0 {
        open.push(count(findings, "open finding", "open findings"));
    }
    if failed > 0 {
        open.push(count(failed, "failed check", "failed checks"));
    }
    if open.is_empty() {
        open.push("open issues".into());
    }
    format!(
        "{review} still has {} after {rounds} of {limit} repair rounds. Decide what happens next.",
        open.join(" and ")
    )
}

/// The last `lines` lines of `output`, and whether anything above them was left out. Bounded
/// by lines alone: one very long line shows whole, and the dialog scrolls.
fn tail(output: &str, lines: usize) -> (&str, bool) {
    let output = output.trim_end();
    let start = output
        .rmatch_indices('\n')
        .nth(lines.saturating_sub(1))
        .map_or(0, |(newline, _)| newline + 1);
    (&output[start..], start > 0)
}

fn chosen(picker: &Picker, choices: &[Choice], cx: &App) -> Option<Choice> {
    let label = picker.read(cx).selected_value()?;
    choices.iter().copied().find(|c| &c.label() == label)
}

/// The findings and failed checks, one card each.
fn evidence(brief: &Brief, cx: &App) -> Div {
    // The dialog's body is the only scroller. A scroller nested in it is a flex child that
    // shrinks below its text, which then paints over whatever follows; this column keeps
    // its height and overflows the body instead (`wiki/gotchas/gpui-scroll-content-shrink`).
    let mut column = v_flex().flex_shrink_0().gap_2();
    for (i, finding) in brief.findings.iter().enumerate() {
        let tint = if finding.severity == Severity::Blocker {
            cx.theme().danger
        } else {
            cx.theme().warning
        };
        column = column.child(
            card(cx)
                .debug_selector(move || format!("decision-finding-{i}"))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(Lucide::CircleAlert).size_3p5().text_color(tint))
                        .child(heading(finding.title.clone())),
                )
                .child(div().text_sm().child(SelectableText::new(
                    ("decision-finding", i),
                    finding.description.clone(),
                ))),
        );
    }
    for (i, check) in brief.failed.iter().enumerate() {
        let mut failed = card(cx)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Icon::new(Lucide::CircleX)
                            .size_3p5()
                            .text_color(cx.theme().danger),
                    )
                    .child(heading(format!(
                        "Required check failed · {}",
                        check.command
                    ))),
            )
            .child(subtle(check.origin.clone(), cx));
        if check.clipped {
            failed = failed.child(subtle("Earlier output is on the Checks tab.", cx));
        }
        column = column.child(
            failed.child(
                div()
                    .p_2()
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(SelectableText::new(
                        ("decision-check-log", i),
                        if check.output.is_empty() {
                            "No output".to_owned()
                        } else {
                            check.output.clone()
                        },
                    )),
            ),
        );
    }
    column
}

/// Lays the decision out on `alert`: the evidence scrolls in the dialog's body, while the
/// choice, its reason and the buttons stay in view below it. `apply` gets the choice once
/// it has a reason and says whether the dialog closes.
fn form(
    alert: AlertDialog,
    brief: &Brief,
    picker: &Picker,
    note: &Entity<TextareaState>,
    apply: Apply,
    cx: &App,
) -> AlertDialog {
    let choices = brief.choices();
    let choice = chosen(picker, &choices, cx);
    let ready = choice.is_some() && !note.read(cx).value().trim().is_empty();
    let submit = Rc::new({
        let (picker, note) = (picker.clone(), note.clone());
        move |cx: &mut App| {
            let reason = note.read(cx).value().trim().to_owned();
            match chosen(&picker, &choices, cx) {
                Some(choice) if !reason.is_empty() => apply(choice, reason, cx),
                _ => false,
            }
        }
    });
    let by_button = submit.clone();
    let fields = v_flex()
        .gap_3()
        .pt_3()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(
            v_flex()
                .gap_1()
                .child(subtle("Decision", cx))
                .child(Select::new(picker).w_full())
                .children(choice.map(|c| subtle(c.consequence(!brief.failed.is_empty()), cx))),
        )
        .child(
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_0p5()
                        .child(subtle("Reason", cx))
                        .child(required_mark(cx)),
                )
                .child(TextField::textarea(Textarea::new(note), note, cx)),
        )
        .child(
            DialogFooter::new()
                .child(
                    Button::new("decision-cancel")
                        .cursor_pointer()
                        .label("Cancel")
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                )
                .child(
                    Button::new("decision-apply")
                        .debug_selector(|| "decision-apply".into())
                        .cursor_pointer()
                        .primary()
                        .label("Apply decision")
                        .disabled(!ready)
                        .on_click(move |_, window, cx| {
                            if by_button(cx) {
                                window.close_dialog(cx);
                            }
                        }),
                ),
        );
    alert
        .width(DIALOG_WIDTH)
        .title("Review needs a decision")
        .description(brief.summary.clone())
        .child(evidence(brief, cx))
        // What Enter triggers, wherever the focus is.
        .on_ok(move |_, _, cx| submit(cx))
        .footer(fields)
}

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
        let gate = r.phase();
        let Phase::Decision(review) = gate else {
            return;
        };
        let id = self.id;
        let findings = r.findings().to_vec();
        let brief = Brief::read(r, review);
        let labels: Vec<_> = brief.choices().into_iter().map(Choice::label).collect();
        let picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(labels),
                Some(IndexPath::default()),
                w,
                cx,
            )
        });
        let note = area("", "Explain the decision. The agents will read it.", w, cx);
        let view = cx.weak_entity();
        let apply: Apply = Rc::new(move |choice, reason, cx| {
            let result = AppState::get(cx).pipelines.clone().update(cx, |p, cx| {
                p.edit(id, cx, |r| {
                    if r.phase() != gate || r.findings() != findings {
                        return Err("Findings changed. Reopen the decision.".into());
                    }
                    match choice {
                        Choice::Repair => r.retry_gate(&reason),
                        Choice::Dismiss(index) => r.dismiss_finding(index, &reason),
                    }
                    .map_err(|error| error.to_string())
                })
            });
            let _ = view.update(cx, |this, cx| {
                this.error = result.err();
                cx.notify();
            });
            true
        });
        w.open_alert_dialog(cx, move |alert, _, cx| {
            form(alert, &brief, &picker, &note, apply.clone(), cx)
        });
    }
}

#[cfg(test)]
mod tests;
