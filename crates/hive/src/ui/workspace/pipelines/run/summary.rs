//! An inline completion notice keeps the current tab and reveals its saved reports on demand.
use super::*;
use gpui_kit::base::{
    MotionReveal,
    motion::{Transition, transition},
};

const REVEAL_DURATION: Duration = Duration::from_millis(260);
const REPORTS_MAX_HEIGHT: f32 = 12.;

fn check_outcome(run: &Run) -> String {
    let mut required = 0;
    let mut passed = 0;
    for (ix, lane) in run.lanes.iter().enumerate() {
        if lane.assignment.trim().is_empty() || lane.command.trim().is_empty() {
            continue;
        }
        required += 1;
        if run
            .commands
            .iter()
            .any(|c| c.lane == ix && c.command == lane.command && c.success)
        {
            passed += 1;
        }
    }
    match required {
        0 => "No required commands".into(),
        1 => format!("{passed}/1 required check passed"),
        n => format!("{passed}/{n} required checks passed"),
    }
}

fn reveal_progress(phase: Phase, window: &mut Window, cx: &mut App) -> Option<f32> {
    let accepted = phase == Phase::Accepted;
    let progress = transition(
        "pipeline-completion-progress",
        if accepted { 1.0_f32 } else { 0.0 },
        Transition::new(REVEAL_DURATION).ease(ease_in_out),
        window,
        cx,
    );
    accepted.then_some(progress)
}

impl RunView {
    pub(super) fn completion_summary(
        &self,
        run: &Run,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let progress = reveal_progress(run.phase, w, cx)?;
        let repairs: usize = (0..run.template.reviews.len())
            .map(|i| usize::from(run.cycles(i)))
            .sum();
        let success = cx.theme().success;
        let toggle = button(
            "completion-reports",
            if self.summary_expanded {
                "Hide reports"
            } else {
                "Show reports"
            },
            ButtonLook::Outline,
            w,
            cx,
            |view, _, cx| {
                view.summary_expanded = !view.summary_expanded;
                cx.notify();
            },
        );
        let header = h_flex()
            .gap_2()
            .flex_wrap()
            .child(Icon::new(Lucide::CircleCheck).text_color(success))
            .child(heading("Run complete"))
            .child(subtle(
                format!(
                    "{}   ·   {repairs} automatic repair {}",
                    check_outcome(run),
                    if repairs == 1 { "round" } else { "rounds" }
                ),
                cx,
            ))
            .child(div().flex_1())
            .child(toggle);
        let card = card(cx)
            .border_color(success.opacity(0.35))
            .child(header)
            .when(self.summary_expanded, |card| {
                card.child(
                    div()
                        .id("completion-reports-scroll")
                        .max_h(rems(REPORTS_MAX_HEIGHT))
                        .overflow_y_scroll()
                        .child(reports(run, cx)),
                )
            });
        let block = div()
            .px_5()
            .pt_3()
            .child(card)
            .opacity(progress)
            .into_any_element();
        Some(if progress < 1.0 {
            MotionReveal::new("pipeline-completion-reveal", progress, block).into_any_element()
        } else {
            block
        })
    }
}

fn reports(run: &Run, cx: &App) -> Div {
    let mut body = v_flex().gap_3();
    for stage in (0..run.lanes.len())
        .map(Stage::Develop)
        .chain((0..run.template.reviews.len()).map(Stage::Review))
    {
        let report = handoff::stage_reports(run, stage).next_back();
        let mut section = v_flex()
            .gap_1()
            .pt_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(heading(stage.title(run)));
        if let Stage::Develop(i) = stage {
            let lane = &run.lanes[i];
            if lane.assignment.trim().is_empty() && report.is_none() {
                continue;
            }
            if !lane.worktree.as_os_str().is_empty() {
                section = section.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SelectableText::new(
                            ("summary-worktree", i),
                            lane.worktree.display().to_string(),
                        )),
                );
            }
        }
        section = match report {
            Some((ticket, report)) => section.child(handoff::report_body(ticket, report, run, cx)),
            None => section.child(subtle(
                "No report saved for this stage. See Activity for recorded handoffs.",
                cx,
            )),
        };
        body = body.child(section);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::{Run, check_outcome};
    use gpui_kit as gpui;
    use hive_core::pipeline::{CommandResult, Lane, Template};

    #[gpui::test]
    fn accepting_reveals_the_block_once(cx: &mut gpui::TestAppContext) {
        use super::{Phase, REVEAL_DURATION, reveal_progress};
        use gpui::prelude::*;
        struct Notice(Phase, Option<f32>);
        impl Render for Notice {
            fn render(
                &mut self,
                w: &mut gpui::Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                self.1 = reveal_progress(self.0, w, cx);
                gpui::div()
            }
        }
        cx.update(gpui_kit::init);
        let (notice, cx) = cx.add_window_view(|_, _| Notice(Phase::Acceptance, None));
        cx.update(|w, cx| w.draw(cx).clear(cx));
        assert!(notice.read_with(cx, |n, _| n.1).is_none());
        notice.update(cx, |n, cx| {
            n.0 = Phase::Accepted;
            cx.notify();
        });
        cx.update(|w, cx| w.draw(cx).clear(cx));
        assert_eq!(notice.read_with(cx, |n, _| n.1), Some(0.));
        cx.background_executor.advance_clock(REVEAL_DURATION / 2);
        cx.update(|w, cx| w.draw(cx).clear(cx));
        assert!(notice.read_with(cx, |n, _| n.1.is_some_and(|p| p > 0. && p < 1.)));
        cx.background_executor.advance_clock(REVEAL_DURATION);
        cx.update(|w, cx| w.draw(cx).clear(cx));
        assert_eq!(notice.read_with(cx, |n, _| n.1), Some(1.));
    }

    #[test]
    fn check_summary_requires_a_matching_passing_result_for_assigned_work() {
        let mut run = Run::new(
            1,
            "/repo".into(),
            "Task".into(),
            Template::default(),
            vec![Lane {
                name: "Development".into(),
                project: "/repo".into(),
                command: "cargo test".into(),
                ..Default::default()
            }],
        )
        .unwrap();
        assert_eq!(check_outcome(&run), "No required commands");
        run.lanes[0].assignment = "Implement the task".into();
        assert_eq!(check_outcome(&run), "0/1 required check passed");
        run.commands.push(CommandResult {
            lane: 0,
            command: "different check".into(),
            success: true,
            output: String::new(),
            exit_code: Some(0),
        });
        assert_eq!(check_outcome(&run), "0/1 required check passed");
        run.commands[0].command = "cargo test".into();
        assert_eq!(check_outcome(&run), "1/1 required check passed");
        run.commands[0].success = false;
        assert_eq!(check_outcome(&run), "0/1 required check passed");
    }
}
