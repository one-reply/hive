//! The lens after each dependency of a package.json: newer releases to switch to with one
//! click, known advisories, and the toolbar switch that shows or hides them in every manifest.
//!
//! gpui-base's editor has no rows between source lines (VS Code's code lenses), so the lens sits
//! after the end of its entry's line. Like the inline blame it is painted by a canvas after the
//! editor, from the layout the editor has just painted, and its window-wide mouse handlers act
//! only where the canvas's own hitbox is hovered.
use super::fade::Fade;
use super::*;
use gpui_kit::base::TooltipRequest;
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::tooltip::{Tooltip, window_overlay};
use hive_core::package_lens::Dependency;
use segments::{Action, PADDING, Segment, Tone, layout, segments};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

mod segments;
mod toolbar;
pub(super) use toolbar::{actions, switch, toggle};

/// Space between the end of the entry's line and the lens.
const LENS_GAP: Pixels = px(20.);
/// Space between a segment's icon and its text.
const ICON_GAP: Pixels = px(4.);
/// The icon's size relative to the editor font size.
const ICON_SCALE: f32 = 0.85;
const SURFACE_RADIUS: Pixels = px(4.);
/// How much of its color a clickable segment shows at rest; under the pointer it shows all.
const ACTION_TINT: f32 = 0.6;
/// How much of its color a status shows: visible, yet quieter than code.
const STATUS_TINT: f32 = 0.35;
/// How much of its color a segment's surface takes under the pointer.
const SURFACE_TINT: f32 = 0.16;
const ADVISORY_MENU_MIN_WIDTH: Pixels = px(220.);

/// Identifies a segment across frames: its entry's first byte and its place in the lens.
type SegmentKey = (usize, usize);

/// The pointer over the lens, kept across frames by the panel.
#[derive(Default)]
pub(super) struct LensPointer {
    /// The hover surface of a clickable segment, fading in and out.
    surface: Fade<SegmentKey, Bounds<Pixels>>,
    /// The segment whose tooltip the lens asked the window for, shown or dismissed by a click.
    tooltip: Option<SegmentKey>,
}

/// Shared between the panel and the canvas that paints the lens every frame.
pub(super) type LensHover = Rc<RefCell<LensPointer>>;

/// A segment where the canvas paints it this frame.
struct Placed {
    key: SegmentKey,
    segment: Segment,
    dependency: Dependency,
    bounds: Bounds<Pixels>,
    icon: Bounds<Pixels>,
    origin: Point<Pixels>,
}

/// A segment the pointer can rest on, kept by the frame's mouse handlers.
struct Target {
    key: SegmentKey,
    bounds: Bounds<Pixels>,
    action: Option<Action>,
    tooltip: SharedString,
    dependency: Dependency,
}

fn target_at<'a>(
    targets: &'a [Target],
    hitbox: &Hitbox,
    position: Point<Pixels>,
    window: &Window,
) -> Option<&'a Target> {
    if !hitbox.is_hovered(window) {
        return None;
    }
    targets
        .iter()
        .find(|target| target.bounds.contains(&position))
}

/// The canvas that paints every visible lens; it must follow the editor so it paints after it.
pub(super) fn overlay(
    panel: WeakEntity<EditorPanel>,
    document: Entity<Document>,
    hover: LensHover,
) -> impl IntoElement {
    canvas(
        // `Normal` leaves the editor below hovered; only elements drawn later occlude it.
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            paint(&panel, &document, &hover, bounds, hitbox, window, cx)
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn paint(
    panel: &WeakEntity<EditorPanel>,
    document: &Entity<Document>,
    hover: &LensHover,
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    window: &mut Window,
    cx: &mut App,
) {
    let area = blame::label_area(bounds);
    let placed = place(document, area, window, cx);
    let targets: Rc<[Target]> = placed
        .iter()
        .map(|placed| Target {
            key: placed.key,
            bounds: placed.bounds,
            action: placed.segment.action.clone(),
            tooltip: placed.segment.tooltip.clone().into(),
            dependency: placed.dependency.clone(),
        })
        .collect();
    let under = target_at(&targets, &hitbox, window.mouse_position(), window);
    let clickable = under
        .filter(|target| target.action.is_some())
        .map(|target| (target.key, target.bounds));
    drop_stale_tooltip(hover, under.map(|target| target.key), window, cx);
    let frame = hover
        .borrow_mut()
        .surface
        .advance(clickable, Instant::now());
    if frame.is_animating() {
        window.request_animation_frame();
    }
    let progress = |placed: &Placed| match (&frame.current, &frame.leaving) {
        (Some((key, _, opacity)), _) if *key == placed.key => *opacity,
        (_, Some((bounds, opacity))) if *bounds == placed.bounds => *opacity,
        _ => 0.,
    };
    window.with_content_mask(Some(ContentMask { bounds: area }), |window| {
        for placed in &placed {
            paint_segment(placed, progress(placed), window, cx);
        }
    });
    if clickable.is_some() {
        window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
    }
    let clickable_key = clickable.map(|(key, _)| key);
    let (move_targets, move_hitbox, move_hover) = (targets.clone(), hitbox.clone(), hover.clone());
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        let under = target_at(&move_targets, &move_hitbox, event.position, window);
        let clickable = under.filter(|target| target.action.is_some());
        if clickable.map(|target| target.key) != clickable_key {
            window.refresh();
        }
        point_tooltip(&move_hover, under, window, cx);
    });
    let (panel, document, hover) = (panel.clone(), document.clone(), hover.clone());
    // Capture runs before the editor's own handler, so the click acts instead of moving the
    // caret to the end of the line.
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
            return;
        }
        let Some(target) = target_at(&targets, &hitbox, event.position, window) else {
            return;
        };
        dismiss_tooltip(&hover, target.key, window, cx);
        // A status takes no click: the editor gets it.
        let Some(action) = &target.action else {
            return;
        };
        cx.stop_propagation();
        match action {
            Action::Upgrade(spec) => document.update(cx, |d, cx| {
                d.apply_upgrade(&target.dependency, spec, window, cx)
            }),
            Action::Advisories(ids) => {
                let anchor = target.bounds.bottom_left();
                let ids = ids.clone();
                let _ = panel.update(cx, |panel, cx| {
                    panel.show_advisories(ids, anchor, window, cx)
                });
            }
        }
    });
}

/// Ask the window's tooltip overlay, which every component tooltip shares, for the tooltip of
/// the segment under the pointer, or to hide the lens's tooltip once the pointer left it.
fn point_tooltip(hover: &LensHover, under: Option<&Target>, window: &mut Window, cx: &mut App) {
    let key = under.map(|target| target.key);
    let previous = std::mem::replace(&mut hover.borrow_mut().tooltip, key);
    if previous == key {
        return;
    }
    let Some(overlay) = window_overlay(window, cx) else {
        return;
    };
    match under {
        Some(target) => {
            let text = target.tooltip.clone();
            let request = TooltipRequest::new(target.bounds, move |window, cx| {
                Tooltip::new(text.clone()).build(window, cx)
            });
            overlay.update(cx, |overlay, cx| overlay.request_show(request, window, cx));
        }
        None => overlay.update(cx, |overlay, cx| overlay.request_hide(window, cx)),
    }
}

/// A press hides the tooltip until the pointer leaves the segment, as a component's press does.
fn dismiss_tooltip(hover: &LensHover, key: SegmentKey, window: &Window, cx: &mut App) {
    hover.borrow_mut().tooltip = Some(key);
    if let Some(overlay) = window_overlay(window, cx) {
        overlay.update(cx, |overlay, cx| overlay.hide(cx));
    }
}

/// The pointer left the lens's tooltip without a move the canvas saw: its segment went away
/// (versions hidden, the row answered again), or typing took the hover from the pointer.
fn drop_stale_tooltip(
    hover: &LensHover,
    under: Option<SegmentKey>,
    window: &mut Window,
    cx: &mut App,
) {
    {
        let mut pointer = hover.borrow_mut();
        if pointer.tooltip.is_none() || pointer.tooltip == under {
            return;
        }
        pointer.tooltip = None;
    }
    let Some(overlay) = window_overlay(window, cx) else {
        return;
    };
    // Not while painting: the overlay re-renders the window it is part of.
    window.defer(cx, move |window, cx| {
        overlay.update(cx, |overlay, cx| overlay.request_hide(window, cx))
    });
}

/// Every segment of the visible rows that fits between its line's end and the area's edge.
fn place(
    document: &Entity<Document>,
    area: Bounds<Pixels>,
    window: &Window,
    cx: &App,
) -> Vec<Placed> {
    let document = document.read(cx);
    if !document.is_package_manifest() || !document.shows_package_versions(cx) {
        return Vec::new();
    }
    let editor = document.editor.read(cx);
    let Some(visible) = editor.visible_row_range() else {
        return Vec::new();
    };
    let rows = document.packages.rows();
    let first = rows.partition_point(|row| row.dependency().line < visible.start);
    let theme = cx.theme();
    let font_size = theme.mono_font_size;
    let icon_size = font_size * ICON_SCALE;
    let mut placed = Vec::new();
    let mut previous_line = None;
    for row in rows[first..]
        .iter()
        .take_while(|row| row.dependency().line < visible.end)
    {
        let dependency = row.dependency();
        // Two entries written on one line share it; the first one's lens shows.
        if previous_line.replace(dependency.line) == Some(dependency.line) {
            continue;
        }
        let Some(end) = blame::visible_line_end(editor, dependency.line) else {
            continue;
        };
        if end.top() < area.top() || end.bottom() > area.bottom() {
            continue;
        }
        let segments = segments(row);
        let widths: Vec<Pixels> = segments
            .iter()
            .map(|segment| {
                let text = shape(&segment.text, theme.muted_foreground, window, cx);
                icon_size + ICON_GAP + text.width
            })
            .collect();
        let extents = layout(end.right() + LENS_GAP - PADDING, area.right(), &widths);
        for (index, (segment, (left, right))) in segments.into_iter().zip(extents).enumerate() {
            let icon = Bounds::new(
                point(
                    left + PADDING,
                    end.top() + (end.size.height - icon_size) / 2.,
                ),
                size(icon_size, icon_size),
            );
            placed.push(Placed {
                key: (dependency.pair.start, index),
                segment,
                dependency: dependency.clone(),
                bounds: Bounds::from_corners(point(left, end.top()), point(right, end.bottom())),
                icon,
                origin: point(icon.right() + ICON_GAP, end.top()),
            });
        }
    }
    placed
}

fn shape(text: &str, color: Hsla, window: &Window, cx: &App) -> ShapedLine {
    let theme = cx.theme();
    let text = SharedString::from(text.to_owned());
    let run = TextRun {
        len: text.len(),
        font: font(theme.mono_font_family.clone()),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text, theme.mono_font_size, &[run], None)
}

fn tone_color(tone: Tone, cx: &App) -> Hsla {
    let theme = cx.theme();
    match tone {
        Tone::Quiet => theme.muted_foreground,
        Tone::Current | Tone::Patch => theme.success,
        Tone::Minor => theme.info,
        Tone::Major => theme.warning,
        Tone::Danger => theme.danger,
    }
}

/// Paint one segment; `hovered` eases its surface in and its color up to full.
fn paint_segment(placed: &Placed, hovered: f32, window: &mut Window, cx: &mut App) {
    let full = tone_color(placed.segment.tone, cx);
    let muted = cx.theme().muted_foreground;
    let color = match (placed.segment.tone, &placed.segment.action) {
        (Tone::Quiet, _) => muted,
        (_, None) => muted.blend(full.opacity(STATUS_TINT)),
        (_, Some(_)) => muted
            .blend(full.opacity(ACTION_TINT))
            .blend(full.opacity(hovered)),
    };
    if hovered > 0. && placed.segment.action.is_some() {
        window.paint_quad(
            fill(placed.bounds, full.opacity(SURFACE_TINT * hovered)).corner_radii(SURFACE_RADIUS),
        );
    }
    let line_height = placed.bounds.size.height;
    let painted = window
        .paint_svg(
            placed.icon,
            placed.segment.icon.path(),
            None,
            TransformationMatrix::unit(),
            color,
            cx,
        )
        .and_then(|_| {
            shape(&placed.segment.text, color, window, cx).paint(
                placed.origin,
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            )
        });
    if let Err(error) = painted {
        tracing::warn!(%error, "package version paint failed");
    }
}

fn advisory_url(id: &str) -> String {
    format!("https://osv.dev/vulnerability/{id}")
}

/// The advisories of one dependency, listed under its segment.
pub(super) struct AdvisoryMenu {
    anchor: Point<Pixels>,
    menu: Entity<PopupMenu>,
    _dismissed: Subscription,
}

impl EditorPanel {
    /// One advisory opens at once; several are listed under the segment to choose from.
    fn show_advisories(
        &mut self,
        ids: Vec<String>,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let [id] = ids.as_slice() {
            cx.open_url(&advisory_url(id));
            return;
        }
        let menu = PopupMenu::build(window, cx, |menu, _, _| {
            ids.iter()
                .fold(menu.min_w(ADVISORY_MENU_MIN_WIDTH), |menu, id| {
                    menu.link(id.clone(), advisory_url(id))
                })
        });
        let dismissed = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.advisories = None;
            cx.notify();
        });
        // After the press that opened the menu, so nothing else takes the focus back from it.
        let focus = menu.focus_handle(cx);
        window.defer(cx, move |window, cx| focus.focus(window, cx));
        self.advisories = Some(AdvisoryMenu {
            anchor,
            menu,
            _dismissed: dismissed,
        });
        cx.notify();
    }

    pub(super) fn render_advisories(&self) -> Option<AnyElement> {
        let advisories = self.advisories.as_ref()?;
        Some(
            deferred(
                anchored()
                    .position(advisories.anchor)
                    .snap_to_window_with_margin(px(8.))
                    .child(advisories.menu.clone()),
            )
            .into_any_element(),
        )
    }
}
