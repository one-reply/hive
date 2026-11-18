//! A common elapsed-time scale keeps parallel attempts and repair rounds comparable.
use super::elapsed::{duration, observed_end, span, stopwatch};
use super::*;
use crate::ui::hover::{SmoothHoverBg as _, WIDE_HOVER_DURATION};
use crate::ui::stepped_spinner::SteppedSpinner;
use gpui_kit::base::animation::Lerp as _;
use gpui_kit::base::motion::{MotionReveal, Transition, transition};
use std::time::Duration;

const LABEL_FRACTION: f32 = 0.30;
const LABEL_MIN: f32 = 142.;
const LABEL_MAX: f32 = 260.;
/// A sub-second attempt still gets a visible bar.
const BAR_MIN_WIDTH: f32 = 3.;
/// Waiting and live bars are washed out so their stripes or playhead stay readable.
const BAR_PENDING_OPACITY: f32 = 0.21;
const BAR_DONE_OPACITY: f32 = 0.84;
const HUMAN_STRIPE_OPACITY: f32 = 0.61;
/// Narrower bars have no room for their duration label.
const BAR_LABEL_MIN_FRACTION: f32 = 0.24;
fn label_column() -> Div {
    h_flex()
        .flex_none()
        .w(relative(LABEL_FRACTION))
        .min_w(px(LABEL_MIN))
        .max_w(px(LABEL_MAX))
}
fn live_color(cx: &App) -> Hsla {
    cx.theme().magenta_light.lerp(&cx.theme().blue_light, 0.55)
}

const ROW_HEIGHT: f32 = 47.;
const TRACK_HEIGHT: f32 = 23.;
const TRACK_OPACITY: f32 = 0.65;
/// A hovered row lifts only partway to `muted`, and its track sinks only partway to the
/// background, so the two separate without a hard jump in contrast.
const ROW_HOVER_OPACITY: f32 = 0.6;
const TRACK_HOVER_OPACITY: f32 = 0.55;
const GRID_PARTS: usize = 4;
const UNFOLD_DURATION: Duration = Duration::from_millis(220);
const QUARTER_TURN: f32 = std::f32::consts::FRAC_PI_2;

struct WorkingIcon(SteppedSpinner);

impl Render for WorkingIcon {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = self.0.icon(cx).size_4().text_color(live_color(cx));
        self.0.keep_ticking(cx);
        icon
    }
}

fn stripes(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, w, _| {
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            let mut x = 0.;
            let mut line = PathBuilder::stroke(px(2.));
            while x < width + height {
                let left = (x - height).max(0.);
                let right = x.min(width);
                line.move_to(bounds.origin + point(px(left), px(x - left)));
                line.line_to(bounds.origin + point(px(right), px(x - right)));
                x += 7.;
            }
            if let Ok(path) = line.build() {
                w.paint_path(path, color);
            }
        },
    )
    .absolute()
    .size_full()
}

impl RunView {
    pub(super) fn timeline(
        &mut self,
        run: &Run,
        sessions: &[LiveSession],
        now: u64,
        scale: Option<(u64, u64)>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut table = v_flex().min_w_0().w_full();
        for (i, stage) in Stage::recipe(run).into_iter().enumerate() {
            let rows = stage.observations(run);
            let active = stage.is_active(run);
            let current = rows
                .last()
                .copied()
                .filter(|r| active && r.end_ms.is_none() && !r.failed);
            let agent = sessions
                .iter()
                .find(|s| s.stage == stage)
                .and_then(|s| s.status);
            let stage_status = stage.status(run, &rows);
            let status = agent.map_or(stage_status.label(), AgentStatus::label);
            let working = current.is_some()
                && !stage.is_human()
                && agent.is_none_or(|s| s == AgentStatus::Working);
            let open = self.selected_stage == Some(stage);
            // Sampled for every stage on every frame: keyed state that a frame skips is dropped,
            // and a closing detail must keep its progress until it has folded away.
            let unfolded = transition(
                (("stage-unfold", i), "progress"),
                if open { 1.0_f32 } else { 0. },
                Transition::new(UNFOLD_DURATION).ease(ease_in_out),
                w,
                cx,
            );
            let hover = Hover::with_duration(("stage-hover", i), WIDE_HOVER_DURATION, w, cx);
            let failed = match agent {
                Some(agent) => agent == AgentStatus::Failed,
                None => stage_status.is_failure(),
            };
            let ink = if failed {
                cx.theme().danger
            } else if active && stage.is_human() || agent.is_some_and(AgentStatus::needs_attention)
            {
                cx.theme().warning
            } else if active {
                live_color(cx)
            } else {
                cx.theme().muted_foreground
            };
            let subtitle = match stage {
                Stage::Develop(i) if path_name(&run.lanes()[i].project) != run.lanes()[i].name => {
                    format!("{} · {status}", path_name(&run.lanes()[i].project))
                }
                _ => status.to_owned(),
            };
            let clock = current.map(|row| stopwatch(now.saturating_sub(row.start_ms)));
            let title = stage.title(run);
            let accessible = match &clock {
                Some(clock) => format!("{title}, {subtitle}, {clock} elapsed"),
                None => format!("{title}, {subtitle}"),
            };
            let icon = if working {
                w.use_keyed_state(("working-stage-icon", i), cx, |_, cx| {
                    WorkingIcon(SteppedSpinner::new(cx))
                })
                .into_any_element()
            } else {
                Icon::new(stage.icon(run))
                    .size_4()
                    .text_color(ink)
                    .into_any_element()
            };
            let label = label_column()
                .gap_2()
                .child(
                    h_flex()
                        .size_7()
                        .flex_none()
                        .justify_center()
                        .rounded_md()
                        .bg(ink.opacity(0.12))
                        .child(icon),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(title))
                        .child(
                            h_flex()
                                .gap_1p5()
                                .text_xs()
                                .text_color(ink)
                                .when_some(clock, |el, clock| {
                                    el.child(
                                        div()
                                            .flex_none()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .child(clock),
                                    )
                                })
                                .child(div().min_w_0().truncate().child(subtitle)),
                        ),
                );
            let mut track = div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(TRACK_HEIGHT))
                .rounded_md()
                .overflow_hidden()
                // The row lifts toward `muted` under the pointer; a track of the same color would
                // dissolve into it, so the track darkens toward the background instead.
                .bg(hover.mix(
                    cx.theme().muted.opacity(TRACK_OPACITY),
                    cx.theme().background.opacity(TRACK_HOVER_OPACITY),
                ));
            if let Some(scale) = scale {
                for tick in 1..GRID_PARTS {
                    track = track.child(
                        div()
                            .absolute()
                            .h_full()
                            .w_px()
                            .left(relative(tick as f32 / GRID_PARTS as f32))
                            .bg(cx.theme().border),
                    );
                }
                for row in rows {
                    let (x, width) = span(row, scale, now);
                    let live = row.end_ms.is_none() && !row.failed;
                    let color = if row.failed {
                        cx.theme().danger
                    } else if stage.is_human() {
                        cx.theme().warning
                    } else if live {
                        ink
                    } else {
                        cx.theme().muted_foreground
                    };
                    let mut bar = div()
                        .absolute()
                        .h_full()
                        .w(relative(width))
                        .min_w(px(BAR_MIN_WIDTH))
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(color.opacity(if stage.is_human() || live {
                            BAR_PENDING_OPACITY
                        } else {
                            BAR_DONE_OPACITY
                        }));
                    bar = if x >= 1. {
                        bar.right_0()
                    } else {
                        bar.left(relative(x))
                    };
                    if stage.is_human() {
                        bar = bar.child(stripes(color.opacity(HUMAN_STRIPE_OPACITY)));
                    }
                    if live {
                        bar = bar.child(
                            div()
                                .absolute()
                                .right_0()
                                .top_0()
                                .h_full()
                                .w_0p5()
                                .bg(color),
                        );
                    }
                    if !live
                        && width >= BAR_LABEL_MIN_FRACTION
                        && (!row.failed || row.end_ms.is_some())
                    {
                        bar = bar.child(
                            h_flex()
                                .absolute()
                                .inset_0()
                                .px_2()
                                .overflow_hidden()
                                .text_xs()
                                .text_color(if stage.is_human() {
                                    cx.theme().foreground
                                } else {
                                    cx.theme().background
                                })
                                .child(duration(observed_end(row, now) - row.start_ms)),
                        );
                    }
                    track = track.child(bar);
                }
            }
            table = table.child(
                h_flex()
                    .id(("timeline-stage", i))
                    .w_full()
                    .h(px(ROW_HEIGHT))
                    .px_2()
                    .gap_3()
                    .rounded_md()
                    .cursor_pointer()
                    .hover_bg(
                        &hover,
                        if open {
                            cx.theme().muted.opacity(0.34)
                        } else {
                            cx.theme().background
                        },
                        cx.theme().muted.opacity(ROW_HOVER_OPACITY),
                    )
                    .child(label)
                    .child(track)
                    .child(
                        Icon::new(Lucide::ChevronRight)
                            .rotate(radians(QUARTER_TURN * unfolded))
                            .size_3p5()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .map(|row| {
                        actionable(row, accessible, Some(open), cx, move |v, cx| {
                            v.selected_stage = if v.selected_stage == Some(stage) {
                                None
                            } else {
                                Some(stage)
                            };
                            cx.notify();
                        })
                    }),
            );
            // A detail that is still folding away renders until its height reaches zero.
            if open || unfolded > 0. {
                let detail = self.stage_detail(stage, run, w, cx).opacity(unfolded);
                table = table.child(MotionReveal::new(
                    ("stage-detail", i),
                    unfolded,
                    detail.into_any_element(),
                ));
            }
        }
        if let Some((a, b)) = scale {
            let mut ticks = h_flex().flex_1().justify_between();
            for i in 0..=GRID_PARTS {
                ticks = ticks.child(subtle(
                    duration((b - a).saturating_mul(i as u64) / GRID_PARTS as u64),
                    cx,
                ));
            }
            table = table.child(
                h_flex()
                    .px_2()
                    .gap_3()
                    .child(label_column())
                    .child(ticks)
                    .child(div().w_3p5()),
            );
            let mut legend = h_flex().px_2().mt_4().gap_4().flex_wrap();
            for (label, color) in [
                ("Agent work", cx.theme().muted_foreground),
                ("In progress", live_color(cx)),
                ("Human decision", cx.theme().warning),
                ("Failed", cx.theme().danger),
            ] {
                legend = legend.child(
                    h_flex()
                        .gap_1p5()
                        .child(div().w_3p5().h_1p5().rounded_full().bg(color))
                        .child(subtle(label, cx)),
                );
            }
            table = table.child(legend);
        } else {
            table = table.child(
                subtle("No timing data was recorded for this run.", cx)
                    .mt_3()
                    .px_2(),
            );
        }
        scroll("run-timeline", table)
    }
}
