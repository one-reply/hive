//! Shared stage vocabulary and geometry for the recipe canvas and its library previews.
use super::editor::Item;
use super::*;
mod layout;
pub(super) use crate::ui::stage_role::RoleKind;
pub(super) use layout::{BoxAt, Layout};

/// Node geometry is authored at the default UI size; text, icons and their wires scale together.
pub(super) fn units(value: f32) -> Rems {
    rems(value / hive_core::settings::UI_FONT_SIZE_DEFAULT)
}
fn path(
    window: &mut Window,
    points: &[(f32, f32)],
    origin: Point<Pixels>,
    color: Hsla,
    dashed: bool,
    scale: f32,
) {
    let Some((first, rest)) = points.split_first() else {
        return;
    };
    let mut builder = PathBuilder::stroke(px(1.3));
    if dashed {
        builder = builder.dash_array(&[px(4.), px(3.)]);
    }
    let p = |(x, y): (f32, f32)| origin + point(px(x * scale), px(y * scale));
    builder.move_to(p(*first));
    for &point in rest {
        builder.line_to(p(point));
    }
    if let Ok(shape) = builder.build() {
        window.paint_path(shape, color);
    }
}
pub(super) fn wires(
    layout: Layout,
    selected: Item,
    template: &Template,
    cx: &App,
) -> impl IntoElement {
    let wire = cx.theme().muted_foreground.alpha(0.6);
    let retry = cx.theme().warning.alpha(0.8);
    let routed = match selected {
        Item::Review(i) => Some(i),
        _ => None,
    };
    let fixes: Vec<_> = template
        .reviews
        .iter()
        .map(|s| {
            s.return_lane
                .as_ref()
                .and_then(|n| template.lanes.iter().position(|l| &l.name == n))
        })
        .collect();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let at = bounds.origin;
            let scale = f32::from(bounds.size.width) / layout.width;
            let from_to = |a: BoxAt, b: BoxAt, window: &mut Window| {
                let mid = (a.floor() + b.y) / 2.;
                path(
                    window,
                    &[
                        (a.middle(), a.floor()),
                        (a.middle(), mid),
                        (b.middle(), mid),
                        (b.middle(), b.y),
                    ],
                    at,
                    wire,
                    false,
                    scale,
                )
            };
            from_to(layout.start, layout.planner, window);
            from_to(layout.planner, layout.approve, window);
            from_to(layout.approve, layout.build, window);
            for &lane in &layout.lanes {
                from_to(layout.build, lane, window);
                from_to(lane, layout.join, window);
            }
            let mut prev = layout.join;
            for (i, &review) in layout.reviews.iter().enumerate() {
                from_to(prev, review, window);
                let x = review.x + review.w;
                let y = review.y + review.h / 2.;
                let rail = layout.width - 13.;
                let color = if routed.is_none() || routed == Some(i) {
                    retry
                } else {
                    retry.alpha(0.26)
                };
                let target = fixes[i]
                    .and_then(|lane| layout.lanes.get(lane).copied())
                    .unwrap_or(layout.build);
                let tx = target.x + target.w;
                let ty = target.y + target.h / 2.;
                path(
                    window,
                    &[(x, y), (rail, y), (rail, ty), (tx + 1., ty)],
                    at,
                    color,
                    true,
                    scale,
                );
                path(
                    window,
                    &[(tx + 5., ty - 3.), (tx + 1., ty), (tx + 5., ty + 3.)],
                    at,
                    color,
                    false,
                    scale,
                );
                prev = review;
            }
            from_to(prev, layout.end, window);
        },
    )
    .absolute()
    .inset_0()
    .size_full()
}

/// A blueprint grid is clipped and batched in one GPUI paint layer.
pub(super) fn grid(cx: &App) -> impl IntoElement {
    const PITCH: f32 = 18.;
    let ink = cx.theme().muted_foreground.alpha(0.18);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let visible = bounds.intersect(&window.content_mask().bounds);
            window.paint_layer(visible, |window| {
                let mut y = visible.top();
                while y < visible.bottom() {
                    let mut x = visible.left();
                    while x < visible.right() {
                        window
                            .paint_quad(fill(Bounds::new(point(x, y), size(px(1.), px(1.))), ink));
                        x += px(PITCH);
                    }
                    y += px(PITCH);
                }
            });
        },
    )
    .absolute()
    .size_full()
    .inset_0()
}

pub(super) fn miniature(t: &Template, cx: &App) -> impl IntoElement {
    let builds = t.lanes.len().clamp(1, 3);
    let mut roles = vec![RoleKind::Plan, RoleKind::Human, RoleKind::Build];
    roles.extend((0..t.reviews.len().min(3)).map(|i| RoleKind::review(t, i)));
    roles.push(RoleKind::Human);
    let count = roles.len();
    let wire = cx.theme().muted_foreground.alpha(0.55);
    let surface = cx.theme().sidebar;
    let mut nodes = Vec::new();
    for (col, role) in roles.into_iter().enumerate() {
        let x = (col as f32 + 0.5) / count as f32;
        let rows = if col == 2 { builds } else { 1 };
        for row in 0..rows {
            let y = 0.5 + (row as f32 - (rows as f32 - 1.) / 2.) * 0.31;
            nodes.push((col, x, y, role));
        }
    }
    let geometry = nodes.clone();
    let mut preview = div()
        .w_full()
        .h(rems(6.))
        .relative()
        .overflow_hidden()
        .bg(cx.theme().background)
        .child(grid(cx))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let w: f32 = bounds.size.width.into();
                    let h: f32 = bounds.size.height.into();
                    for &(col, x, y, _) in &geometry {
                        for &(_, nx, ny, _) in
                            geometry.iter().filter(|(next, _, _, _)| *next == col + 1)
                        {
                            let mid = (x + nx) * w / 2.;
                            path(
                                window,
                                &[
                                    (x * w, y * h),
                                    (mid, y * h),
                                    (mid, ny * h),
                                    (nx * w, ny * h),
                                ],
                                bounds.origin,
                                wire,
                                false,
                                1.,
                            );
                        }
                    }
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        );
    for (_, x, y, role) in nodes {
        let color = role.color(cx);
        preview = preview.child(
            div()
                .absolute()
                .left(relative(x))
                .top(relative(y))
                .ml(-px(10.))
                .mt(-px(10.))
                .size(px(20.))
                .flex()
                .justify_center()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(color.alpha(0.68))
                .bg(surface)
                .child(Icon::new(role.icon()).size_3().text_color(color)),
        );
    }
    preview
}
