//! A common elapsed-time scale keeps parallel attempts and repair rounds comparable.
use super::elapsed::{duration, now_ms, observed_end, range, span};
use super::*;
use crate::ui::hover::SmoothHoverBg as _;
use gpui_kit::base::animation::Lerp as _;

const LABEL_FRACTION: f32 = 0.30;
const LABEL_MIN: f32 = 142.;
const LABEL_MAX: f32 = 260.;
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
const GRID_PARTS: usize = 4;

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
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = now_ms();
        let scale = range(&run.timings, now);
        if self.timer.is_none()
            && run
                .timings
                .iter()
                .any(|row| row.end_ms.is_none() && !row.failed)
        {
            self.timer = Some(cx.spawn(async |view, cx| {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = view.update(cx, |this, cx| {
                    this.timer.take();
                    cx.notify();
                });
            }));
        }
        let mut table = v_flex().min_w_0().w_full();
        for (i, stage) in Stage::recipe(run).into_iter().enumerate() {
            let rows = stage.observations(run);
            let active = stage.active(run);
            let status = stage.state(run, &rows);
            let open = self.selected_stage == Some(stage);
            let hover = Hover::new(("stage-hover", i), w, cx);
            let ink = if status == "Interrupted" || status == "Failed" {
                cx.theme().danger
            } else if active && stage.human() {
                cx.theme().warning
            } else if active {
                live_color(cx)
            } else {
                cx.theme().muted_foreground
            };
            let subtitle = match stage {
                Stage::Develop(i) if path_name(&run.lanes[i].project) != run.lanes[i].name => {
                    format!("{} · {status}", path_name(&run.lanes[i].project))
                }
                _ => status.to_owned(),
            };
            let accessible = format!("{}, {subtitle}", stage.title(run));
            let label = label_column()
                .gap_2()
                .child(
                    h_flex()
                        .size_7()
                        .flex_none()
                        .justify_center()
                        .rounded_md()
                        .bg(ink.opacity(0.12))
                        .child(Icon::new(stage.icon(run)).size_4().text_color(ink)),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(stage.title(run)))
                        .child(div().text_xs().truncate().text_color(ink).child(subtitle)),
                );
            let mut track = div()
                .relative()
                .flex_1()
                .min_w_0()
                .h(px(TRACK_HEIGHT))
                .rounded_md()
                .overflow_hidden()
                .bg(cx.theme().muted.opacity(0.65));
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
                    } else if stage.human() {
                        cx.theme().warning
                    } else if live {
                        live_color(cx)
                    } else {
                        cx.theme().muted_foreground
                    };
                    let mut bar = div()
                        .absolute()
                        .h_full()
                        .w(relative(width))
                        .min_w(px(3.))
                        .rounded_sm()
                        .overflow_hidden()
                        .bg(color.opacity(if stage.human() { 0.21 } else { 0.84 }));
                    bar = if x >= 1. {
                        bar.right_0()
                    } else {
                        bar.left(relative(x))
                    };
                    if stage.human() {
                        bar = bar.child(stripes(color.opacity(0.61)));
                    }
                    if width >= 0.24 && (!row.failed || row.end_ms.is_some()) {
                        bar = bar.child(
                            h_flex()
                                .absolute()
                                .inset_0()
                                .px_2()
                                .overflow_hidden()
                                .text_xs()
                                .text_color(if stage.human() {
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
                        cx.theme().muted,
                    )
                    .child(label)
                    .child(track)
                    .child(
                        Icon::new(if open {
                            Lucide::ChevronDown
                        } else {
                            Lucide::ChevronRight
                        })
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
            if open {
                table = table.child(self.stage_detail(stage, run, w, cx));
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
