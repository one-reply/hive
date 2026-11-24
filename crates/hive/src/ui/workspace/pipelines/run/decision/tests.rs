// Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
use super::{Apply, Brief, Choice, FailedCheck, FindingCard, Picker, form, summary, tail};
use crate::ui::destructive::testing::{is_open, settle, show, window};
use gpui_kit as gpui;
use gpui_kit::component::IndexPath;
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::SelectState;
use gpui_kit::prelude::*;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use hive_core::pipeline::Severity;
use std::cell::RefCell;
use std::rc::Rc;

fn brief(findings: usize, description: &str) -> Brief {
    Brief {
        summary: "QA still has open findings.".into(),
        findings: (0..findings)
            .map(|i| FindingCard {
                title: format!("{} · API · major", i + 1),
                severity: Severity::Major,
                description: description.into(),
            })
            .collect(),
        failed: vec![FailedCheck {
            command: "cargo test".into(),
            origin: "api · exit 101".into(),
            output: "test failed".into(),
            clipped: false,
        }],
    }
}

type Applied = Rc<RefCell<Vec<(Choice, String)>>>;

/// The decision dialog over `brief` in a test window, with what it applied.
fn open(
    brief: Brief,
    cx: &mut TestAppContext,
) -> (
    &mut VisualTestContext,
    Picker,
    Entity<TextareaState>,
    Applied,
) {
    let cx = window(cx);
    let labels: Vec<_> = brief.choices().into_iter().map(Choice::label).collect();
    let (picker, note) = cx.update(|window, cx| {
        let picker = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(labels),
                Some(IndexPath::default()),
                window,
                cx,
            )
        });
        let note = cx.new(|cx| TextareaState::new(window, cx).rows(3));
        (picker, note)
    });
    let applied = Applied::default();
    let sink = applied.clone();
    let apply: Apply = Rc::new(move |choice, reason, _| {
        sink.borrow_mut().push((choice, reason));
        true
    });
    let (shown_picker, shown_note) = (picker.clone(), note.clone());
    show(cx, move |alert, _, cx| {
        form(alert, &brief, &shown_picker, &shown_note, apply.clone(), cx)
    });
    (cx, picker, note, applied)
}

fn click_apply(cx: &mut VisualTestContext) {
    let apply = cx
        .debug_bounds("decision-apply")
        .expect("the Apply decision button");
    cx.simulate_click(apply.center(), Default::default());
    settle(cx);
}

#[test]
fn a_finding_names_its_review_only_when_the_gate_holds_findings_of_several() {
    assert_eq!(
        super::finding_title(2, None, "API", "major"),
        "2 · API · major"
    );
    assert_eq!(
        super::finding_title(1, Some("Security review"), "API", "blocker"),
        "1 · Security review · API · blocker"
    );
}

#[test]
fn the_summary_names_what_is_open_and_the_rounds_spent() {
    assert_eq!(
        summary("QA", 2, 0, 3, 3),
        "QA still has 2 open findings after 3 of 3 repair rounds. Decide what happens next."
    );
    assert!(summary("QA", 1, 1, 3, 3).contains("1 open finding and 1 failed check after"));
    assert!(summary("QA", 0, 2, 1, 1).contains("still has 2 failed checks after"));
    // A gate read from a damaged state file still makes a sentence.
    assert!(summary("QA", 0, 0, 0, 0).contains("still has open issues after"));
}

#[test]
fn the_tail_keeps_the_last_lines_and_says_when_it_cut() {
    assert_eq!(tail("a\nb\nc\n", 2), ("b\nc", true));
    assert_eq!(tail("a\nb\nc", 3), ("a\nb\nc", false));
    assert_eq!(tail("one line", 12), ("one line", false));
    assert_eq!(tail("", 12), ("", false));
    assert_eq!(tail("я\nї\nє", 1), ("є", true));
}

#[test]
fn repair_comes_first_and_every_open_finding_can_be_dismissed() {
    assert_eq!(
        brief(2, "x").choices(),
        [Choice::Repair, Choice::Dismiss(0), Choice::Dismiss(1)]
    );
    assert_eq!(brief(0, "x").choices(), [Choice::Repair]);
    assert_eq!(Choice::Dismiss(1).label(), "Dismiss finding 2");
}

#[test]
fn dismissing_beside_a_failed_check_says_the_check_still_blocks() {
    assert!(
        Choice::Dismiss(0)
            .consequence(true)
            .contains("failed check still")
    );
    assert!(
        Choice::Dismiss(0)
            .consequence(false)
            .contains("not to raise it again")
    );
}

#[gpui::test]
fn long_evidence_scrolls_while_the_cards_keep_their_height_and_apply_stays_in_view(
    cx: &mut TestAppContext,
) {
    let (cx, ..) = open(
        brief(12, &"A finding that takes a few lines. ".repeat(20)),
        cx,
    );
    let viewport = cx.update(|window, _| window.viewport_size());
    let cards: Vec<_> = (0..12)
        .map(|i| {
            cx.debug_bounds(Box::leak(format!("decision-finding-{i}").into_boxed_str()))
                .expect("a card for every finding")
        })
        .collect();
    let apply = cx.debug_bounds("decision-apply").expect("the Apply button");

    // More evidence than fits: it runs past the window, where only a scroller reaches it.
    assert!(cards[11].bottom() > viewport.height);
    // No card is squeezed to make room: each is as tall as the first, and none overlaps
    // the one before it.
    for pair in cards.windows(2) {
        assert_eq!(pair[1].size.height, pair[0].size.height);
        assert!(pair[1].top() >= pair[0].bottom());
    }
    assert!(cards[0].size.height > gpui::px(60.));
    // The decision is not pushed out by the evidence.
    assert!(apply.bottom() <= viewport.height);
    assert!(cards[0].top() < apply.top());
}

#[gpui::test]
fn apply_waits_for_a_reason_and_then_passes_the_chosen_finding(cx: &mut TestAppContext) {
    let (cx, picker, note, applied) = open(brief(2, "Something is off."), cx);

    click_apply(cx);
    cx.simulate_keystrokes("enter");
    settle(cx);
    assert!(applied.borrow().is_empty());
    assert!(is_open(cx));

    cx.update(|window, cx| {
        picker.update(cx, |picker, cx| {
            picker.set_selected_index(Some(IndexPath::default().row(2)), window, cx);
        });
        note.update(cx, |note, cx| {
            note.set_value("  Out of scope  ", window, cx)
        });
    });
    settle(cx);
    click_apply(cx);

    assert_eq!(
        *applied.borrow(),
        [(Choice::Dismiss(1), "Out of scope".to_owned())]
    );
    assert!(!is_open(cx));
}
