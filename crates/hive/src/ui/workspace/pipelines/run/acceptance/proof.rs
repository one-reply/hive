//! A criterion's evidence as its card shows it: how far it is from closing, who vouches for
//! it and with what.
use super::*;

/// How far a criterion is from letting the run be accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    /// Proven by a test Hive ran or by the user's own observation.
    Verified,
    /// The user accepted the risk without proof.
    Excepted,
    /// Waits for the user's decision.
    Open,
}

impl Standing {
    fn color(self, cx: &App) -> Hsla {
        match self {
            Self::Verified => cx.theme().success,
            Self::Excepted => cx.theme().info,
            Self::Open => cx.theme().warning,
        }
    }
}

/// A criterion's evidence as its card words it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Proof<'a> {
    standing: Standing,
    icon: Lucide,
    label: &'static str,
    /// The reviewer vouching for the criterion.
    by: Option<&'a str>,
    /// The test's name, the user's note or the reviewer's claim.
    detail: Option<&'a str>,
}

impl<'a> From<&EvidenceSource<'a>> for Proof<'a> {
    fn from(source: &EvidenceSource<'a>) -> Self {
        let (standing, icon, label, by, detail) = match *source {
            EvidenceSource::Test(name) => (
                Standing::Verified,
                Lucide::CircleCheck,
                "Hive test",
                None,
                Some(name),
            ),
            EvidenceSource::User(CriterionDecision::Manual(note)) => (
                Standing::Verified,
                Lucide::UserRoundCheck,
                "Manually verified",
                None,
                Some(note.as_str()),
            ),
            EvidenceSource::User(CriterionDecision::Exception(note)) => (
                Standing::Excepted,
                Lucide::ShieldAlert,
                "Accepted exception",
                None,
                Some(note.as_str()),
            ),
            EvidenceSource::Reviewer(role, note) => (
                Standing::Open,
                Lucide::ScanEye,
                "Reviewer assessment",
                Some(role),
                Some(note),
            ),
            EvidenceSource::Missing => (
                Standing::Open,
                Lucide::CircleDashed,
                "Unverified",
                None,
                None,
            ),
        };
        Self {
            standing,
            icon,
            label,
            by,
            detail,
        }
    }
}

/// One criterion: what was promised, what proves it and the user's ways to close it.
pub(super) fn criterion_card(
    criterion: &str,
    lane: Option<&str>,
    proof: &Proof,
    actions: Option<Div>,
    cx: &App,
) -> Div {
    let color = proof.standing.color(cx);
    let mut body = v_flex().flex_1().min_w_0().gap_1().child(
        h_flex()
            .items_start()
            .gap_3()
            .child(
                // Beside the icon and the label the text keeps its one-line width unless it
                // may shrink; then it wraps.
                div()
                    .debug_selector(|| "criterion-text".into())
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(criterion.to_owned()),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(color)
                    .child(proof.label),
            ),
    );
    if lane.is_some() || proof.by.is_some() || proof.detail.is_some() {
        let mut evidence = h_flex().items_start().gap_1p5().text_xs();
        if let Some(lane) = lane {
            evidence = evidence.child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded_sm()
                    .bg(cx.theme().muted)
                    .text_color(cx.theme().muted_foreground)
                    .child(lane.to_owned()),
            );
        }
        if let Some(by) = proof.by {
            evidence = evidence.child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(by.to_owned()),
            );
        }
        if let Some(detail) = proof.detail {
            evidence = evidence.child(subtle(detail.to_owned(), cx).flex_1().min_w_0());
        }
        body = body.child(evidence);
    }
    h_flex()
        .items_start()
        .gap_2p5()
        .px_3()
        .py_2()
        .border_1()
        .border_color(cx.theme().border)
        .rounded_md()
        .bg(cx.theme().sidebar)
        .child(
            Icon::new(proof.icon)
                .flex_none()
                .size_4()
                .mt_0p5()
                .text_color(color),
        )
        .child(body.children(actions))
}

#[cfg(test)]
mod tests {
    // Not a glob import: the parent's `gpui_kit::*` brings in GPUI's `test` attribute.
    use super::{Proof, Standing, criterion_card};
    use gpui_kit as gpui;
    use gpui_kit::prelude::*;
    use gpui_kit::{Context, Pixels, TestAppContext, Window, div, px};
    use hive_core::pipeline::{CriterionDecision, EvidenceSource};

    #[test]
    fn only_a_test_or_the_users_own_decision_takes_a_criterion_off_the_open_list() {
        let standing = |source: EvidenceSource| Proof::from(&source).standing;
        let manual = CriterionDecision::Manual("Clicked through it".into());
        let exception = CriterionDecision::Exception("Out of scope".into());
        assert_eq!(standing(EvidenceSource::Test("loads")), Standing::Verified);
        assert_eq!(standing(EvidenceSource::User(&manual)), Standing::Verified);
        assert_eq!(
            standing(EvidenceSource::User(&exception)),
            Standing::Excepted
        );
        assert_eq!(
            standing(EvidenceSource::Reviewer("QA", "Read the code")),
            Standing::Open
        );
        assert_eq!(standing(EvidenceSource::Missing), Standing::Open);
    }

    #[test]
    fn a_card_names_the_reviewer_apart_from_the_claim_and_says_nothing_without_evidence() {
        let claim = EvidenceSource::Reviewer("QA", "Verified in configs.test.js");
        let proof = Proof::from(&claim);
        assert_eq!(
            (proof.label, proof.by, proof.detail),
            (
                "Reviewer assessment",
                Some("QA"),
                Some("Verified in configs.test.js")
            )
        );
        let exception = CriterionDecision::Exception("Out of scope".into());
        let accepted = EvidenceSource::User(&exception);
        let proof = Proof::from(&accepted);
        assert_eq!(
            (proof.label, proof.by, proof.detail),
            ("Accepted exception", None, Some("Out of scope"))
        );
        let proof = Proof::from(&EvidenceSource::Missing);
        assert_eq!(
            (proof.label, proof.by, proof.detail),
            ("Unverified", None, None)
        );
    }

    const WIDTH: Pixels = px(320.);

    /// One criterion card in a column `WIDTH` wide.
    struct Column;
    impl Render for Column {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let claim = "Verified in source and passed. ".repeat(6);
            let source = EvidenceSource::Reviewer("Security & performance", &claim);
            div().w(WIDTH).child(criterion_card(
                &"loadConfigs rejects a missing key. ".repeat(8),
                Some("Development"),
                &Proof::from(&source),
                None,
                cx,
            ))
        }
    }

    #[gpui::test]
    fn a_long_criterion_wraps_inside_its_card_instead_of_running_past_it(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| Column);
        cx.run_until_parked();

        let text = cx.debug_bounds("criterion-text").expect("the criterion");
        assert!(text.right() <= WIDTH, "{text:?}");
        let one_line = cx.update(|window, _| window.line_height());
        assert!(text.size.height > one_line * 2., "{text:?}");
    }
}
