//! What the user weighs at acceptance: every criterion with its proof and the ways to close
//! it, then the reviewer notes that did not block. Criterion actions are bound to the review
//! the user was looking at when opening the dialog.
use super::*;
use gpui_kit::component::scroll::ScrollableElement as _;
use hive_core::pipeline::{CriterionDecision, EvidenceSource};

mod notes;
mod proof;
use proof::{Proof, criterion_card};

/// Leave room for the reviewer notes, the diff and navigation when a plan has many criteria.
const CRITERIA_PREVIEW: usize = 2;
/// Share of the window's height the criteria and notes may take before they scroll: the tabs
/// below keep their room in a small window, and a tall one shows more of the list.
const REVIEW_HEIGHT_SHARE: f32 = 0.42;

/// Whether rows say which lane they belong to. With one assigned lane its name would only
/// repeat on every row.
fn names_lanes(run: &Run) -> bool {
    run.lanes().iter().filter(|lane| lane.is_assigned()).count() > 1
}

/// A criterion of the plan with the evidence it has now.
struct Criterion<'a> {
    /// Place in the plan, which names the criterion's controls wherever it is listed.
    index: usize,
    lane: usize,
    line: &'a str,
    source: EvidenceSource<'a>,
}

/// The run's criteria, those waiting for the user first and each group in the plan's order,
/// so a preview never hides what holds acceptance back.
fn open_first(run: &Run) -> Vec<Criterion<'_>> {
    let mut criteria: Vec<_> = run
        .criteria()
        .enumerate()
        .map(|(index, (lane, line))| Criterion {
            index,
            lane,
            line,
            source: run.criterion_source(lane, line),
        })
        .collect();
    // Stable, so equal keys keep the order they came in.
    criteria.sort_by_key(|criterion| criterion.source.closes());
    criteria
}

fn section_title(icon: Lucide, title: &'static str, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .child(
            Icon::new(icon)
                .size_4()
                .text_color(cx.theme().muted_foreground),
        )
        .child(heading(title))
}

impl RunView {
    /// Criteria and reviewer notes in one region that scrolls on its own.
    pub(super) fn acceptance_review(
        &self,
        run: &Run,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        // The region draws its scrollbar over its right edge; the padding keeps the cards and
        // their status labels clear of it.
        let body = v_flex()
            .pr_4()
            .gap_3()
            .child(self.criteria(run, w, cx))
            .children(self.review_notes(run, w, cx));
        div()
            .max_h(w.viewport_size().height * REVIEW_HEIGHT_SHARE)
            .child(body)
            .overflow_y_scrollbar()
            .id("acceptance-review")
    }

    fn criteria(&self, run: &Run, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let criteria = open_first(run);
        let total = criteria.len();
        let resolved = criteria.iter().filter(|c| c.source.closes()).count();
        let shown = if self.criteria_expanded {
            total
        } else {
            CRITERIA_PREVIEW
        };
        let named = names_lanes(run);
        let mut title = section_title(Lucide::ListChecks, "Acceptance criteria", cx);
        if total > 0 {
            title = title.child(status_pill(
                format!("{resolved} of {total} resolved"),
                if resolved == total {
                    cx.theme().success
                } else {
                    cx.theme().warning
                },
            ));
        }
        if total > CRITERIA_PREVIEW {
            title = title.child(div().flex_1()).child(button_with_icon(
                "expand-criteria",
                if self.criteria_expanded {
                    "Show fewer".into()
                } else {
                    format!("Show all {total}")
                },
                if self.criteria_expanded {
                    Lucide::ChevronUp
                } else {
                    Lucide::ChevronDown
                },
                ButtonLook::Outline,
                w,
                cx,
                |v, _, cx| {
                    v.criteria_expanded = !v.criteria_expanded;
                    cx.notify();
                },
            ));
        }
        let mut list = v_flex().gap_1p5().child(title);
        if total == 0 {
            list = list.child(subtle("The plan has no acceptance criteria.", cx));
        }
        for criterion in criteria.iter().take(shown) {
            let actions = run
                .review_revision()
                .map(|revision| Self::criterion_actions(criterion, revision, w, cx));
            list = list.child(criterion_card(
                criterion.line,
                named.then(|| run.lanes()[criterion.lane].name.as_str()),
                &Proof::from(&criterion.source),
                actions,
                cx,
            ));
        }
        list
    }

    fn criterion_actions(
        criterion: &Criterion,
        revision: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Criterion {
            index,
            lane,
            line,
            ref source,
        } = *criterion;
        let mut actions = h_flex().gap_1p5().flex_wrap().pt_0p5();
        if let EvidenceSource::Test(name) = source {
            let name = name.to_string();
            actions = actions.child(button_with_icon(
                ("see-test", index),
                "View test",
                Lucide::FlaskConical,
                ButtonLook::Outline,
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
        for (manual, label, icon, id) in [
            (true, "Record observation", Lucide::Eye, "manual-proof"),
            (
                false,
                "Accept exception",
                Lucide::ShieldAlert,
                "waive-proof",
            ),
        ] {
            let (line, revision) = (line.to_owned(), revision.to_owned());
            actions = actions.child(button_with_icon(
                (id, index),
                label,
                icon,
                ButtonLook::Outline,
                w,
                cx,
                move |_, w, cx| {
                    let (criterion, revision) = (line.clone(), revision.clone());
                    Self::ask_reason(
                        label,
                        line.clone(),
                        if manual {
                            "What did you observe?"
                        } else {
                            "Why accept this risk?"
                        },
                        w,
                        cx,
                        move |run, note| {
                            let decision = if manual {
                                CriterionDecision::Manual(note)
                            } else {
                                CriterionDecision::Exception(note)
                            };
                            Ok(run.decide_criterion(lane, &criterion, &revision, decision)?)
                        },
                    );
                },
            ));
        }
        let (line, revision) = (line.to_owned(), revision.to_owned());
        actions.child(button_with_icon(
            ("verify-criterion", index),
            "Return to QA",
            Lucide::RotateCcw,
            ButtonLook::Outline,
            w,
            cx,
            move |v, _, cx| {
                v.edit(cx, |r| Ok(r.verify_criterion(lane, &line, &revision)?));
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::{names_lanes, open_first};
    use hive_core::pipeline::{Lane, Phase, Run, Template};

    fn lane(name: &str, criteria: &str) -> Lane {
        Lane {
            name: name.into(),
            project: "/unused-acceptance-test-project".into(),
            base: "HEAD".into(),
            assignment: "Do the work".into(),
            criteria: criteria.into(),
            ..Lane::default()
        }
    }

    /// A saved run at the acceptance gate, as the state file would hold it; the transitions
    /// to get there need real worktrees.
    fn at_acceptance(lanes: Vec<Lane>, decided: &[&str]) -> Run {
        let owner = lanes[0].project.clone();
        let run = Run::new(7, owner, "Task".into(), Template::default(), lanes).expect("a run");
        let mut saved = serde_json::to_value(&run).expect("a run serializes");
        saved["phase"] = serde_json::to_value(Phase::Acceptance).expect("a phase serializes");
        saved["review_revision"] = "reviewed".into();
        saved["decisions"] = decided
            .iter()
            .map(|criterion| {
                serde_json::json!({
                    "lane": 0,
                    "criterion": criterion,
                    "revision": "reviewed",
                    "decision": { "Manual": "Checked by hand" },
                })
            })
            .collect();
        serde_json::from_value(saved).expect("the patched run loads")
    }

    #[test]
    fn criteria_waiting_for_the_user_come_first_and_each_group_keeps_the_plan_order() {
        let run = at_acceptance(
            vec![lane("Web", "First\nSecond\nThird\nFourth")],
            &["First", "Third"],
        );
        let listed: Vec<_> = open_first(&run).iter().map(|c| (c.index, c.line)).collect();
        assert_eq!(
            listed,
            [(1, "Second"), (3, "Fourth"), (0, "First"), (2, "Third")]
        );
    }

    #[test]
    fn a_run_without_criteria_lists_nothing() {
        assert!(open_first(&at_acceptance(vec![lane("Web", " \n")], &[])).is_empty());
    }

    #[test]
    fn rows_name_their_lane_only_when_more_than_one_lane_has_work() {
        assert!(!names_lanes(&at_acceptance(vec![lane("Web", "One")], &[])));
        let mut idle = lane("Api", "");
        idle.assignment.clear();
        let one_assigned = at_acceptance(vec![lane("Web", "One"), idle], &[]);
        assert!(!names_lanes(&one_assigned));
        let both = at_acceptance(vec![lane("Web", "One"), lane("Api", "Two")], &[]);
        assert!(names_lanes(&both));
    }
}
