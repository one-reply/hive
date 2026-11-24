//! Dragging the inspector's left edge trades canvas for a wider stage form.
use super::*;
use crate::ui::workspace::RESIZE_HANDLE_WIDTH;
use hive_core::settings::clamp_pipeline_inspector_width;

/// The most of the editor the inspector may take; the canvas keeps the rest. Above the
/// narrowest inspector's share of the narrowest split editor, so the default never shrinks.
pub(super) const INSPECTOR_MAX_SHARE: f32 = 0.6;

/// The drag that resizes the inspector; it draws nothing of its own.
pub(super) struct InspectorResize;
impl Render for InspectorResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// Inspector width in rems for a pointer at `pointer_x`: the inspector ends at the right
/// edge of `body`, the canvas and the inspector together.
pub(super) fn dragged_inspector_width(
    body: Bounds<Pixels>,
    pointer_x: Pixels,
    rem_size: Pixels,
) -> f32 {
    let width = (body.right() - pointer_x).min(body.size.width * INSPECTOR_MAX_SHARE);
    clamp_pipeline_inspector_width(width / rem_size)
}

impl TemplateEditor {
    fn finish_inspector_resize(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.resizing_inspector) {
            let width = self.inspector_width;
            AppState::get(cx)
                .store
                .clone()
                .update(cx, |s, cx| s.set_pipeline_inspector_width(width, cx));
        }
    }

    /// Centered on the inspector's left border, like the handles of the sidebars.
    pub(super) fn inspector_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("pipeline-inspector-resize")
            .absolute()
            .top_0()
            .bottom_0()
            .left(-RESIZE_HANDLE_WIDTH / 2.)
            .w(RESIZE_HANDLE_WIDTH)
            .occlude()
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|s, _, _, _| s.resizing_inspector = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|s, _, _, cx| s.finish_inspector_resize(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|s, _, _, cx| s.finish_inspector_resize(cx)),
            )
            .on_drag(InspectorResize, |_, _, _, cx| cx.new(|_| InspectorResize))
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the prelude's `gpui::test` would shadow `#[test]`.
    use super::dragged_inspector_width;
    use gpui_kit::gpui::{Bounds, Pixels, point, px, size};
    use hive_core::settings::{PIPELINE_INSPECTOR_WIDTH_MAX, PIPELINE_INSPECTOR_WIDTH_MIN};

    const REM: Pixels = px(16.);

    fn body(width: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(300.), px(0.)), size(px(width), px(800.)))
    }

    #[test]
    fn inspector_follows_the_pointer_from_the_editors_right_edge_in_rems() {
        // 1500 px wide from x = 300: the right edge is at 1800, the pointer 400 px left of it.
        assert_eq!(dragged_inspector_width(body(1500.), px(1400.), REM), 25.);
    }

    #[test]
    fn inspector_stops_at_its_narrowest_and_widest() {
        assert_eq!(
            dragged_inspector_width(body(1500.), px(1790.), REM),
            PIPELINE_INSPECTOR_WIDTH_MIN
        );
        assert_eq!(
            dragged_inspector_width(body(1500.), px(1900.), REM),
            PIPELINE_INSPECTOR_WIDTH_MIN
        );
        assert_eq!(
            dragged_inspector_width(body(1500.), px(0.), REM),
            PIPELINE_INSPECTOR_WIDTH_MAX
        );
    }

    #[test]
    fn inspector_leaves_the_canvas_its_share_of_a_narrow_editor() {
        // 60% of 640 px is 384 px = 24 rem, below the widest inspector.
        assert_eq!(dragged_inspector_width(body(640.), px(0.), REM), 24.);
    }

    #[test]
    fn a_zero_rem_falls_back_to_the_narrowest_inspector() {
        assert_eq!(
            dragged_inspector_width(body(0.), px(0.), px(0.)),
            PIPELINE_INSPECTOR_WIDTH_MIN
        );
    }
}
