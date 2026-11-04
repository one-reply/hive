//! Drag-to-resize for the commit message box. The height lives only in the panel, so every
//! session starts at the natural three rows.
use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::*;

/// Grab area straddling the footer's top border.
pub(super) const HANDLE_HEIGHT: Pixels = px(6.);

/// Drag payload of the resize handle; it renders nothing under the pointer.
pub(in crate::ui::git_panel) struct MessageResize;

impl Render for MessageResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// The message box's dragged height and what a drag needs to compute it.
#[derive(Default)]
pub(in crate::ui::git_panel) struct MessageSize {
    /// `None` keeps the natural three rows.
    height: Option<Pixels>,
    /// The natural height, the floor of a drag. Measured while `height` is `None`.
    natural: Pixels,
    /// Pointer y and box height when the current drag began.
    start: Option<(Pixels, Pixels)>,
    /// Where the box was laid out last frame.
    bounds: Rc<Cell<Bounds<Pixels>>>,
}

impl MessageSize {
    pub(super) fn height(&self) -> Option<Pixels> {
        self.height
    }

    /// An invisible overlay recording the box's bounds each frame.
    pub(super) fn measure(&self) -> impl IntoElement {
        let bounds = self.bounds.clone();
        canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
            .absolute()
            .size_full()
    }

    pub(super) fn begin(&mut self, pointer_y: Pixels) {
        let current = self.bounds.get().size.height;
        if self.height.is_none() {
            self.natural = current;
        }
        self.start = Some((pointer_y, self.height.unwrap_or(current)));
    }

    /// Follows the pointer during a drag; returns whether the height changed.
    pub(in crate::ui::git_panel) fn drag(&mut self, pointer_y: Pixels, panel: Pixels) -> bool {
        let Some((start_y, start_height)) = self.start else {
            return false;
        };
        let height = dragged_height(start_height + start_y - pointer_y, self.natural, panel);
        let changed = self.height != Some(height);
        self.height = Some(height);
        changed
    }

    pub(super) fn end(&mut self) {
        self.start = None;
    }

    /// Back to the natural three rows; returns whether anything changed.
    pub(super) fn reset(&mut self) -> bool {
        self.start = None;
        self.height.take().is_some()
    }
}

/// Never below the natural rows, never above half the Git panel (unless the natural rows
/// already are).
fn dragged_height(wanted: Pixels, natural: Pixels, panel: Pixels) -> Pixels {
    wanted.clamp(natural, (panel / 2.).max(natural))
}

#[cfg(test)]
mod tests {
    // Not a glob: `gpui_kit::*` brings GPUI's own `test` attribute.
    use super::{MessageSize, dragged_height};
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn height_stays_between_natural_rows_and_half_the_panel() {
        let (natural, panel) = (px(72.), px(800.));
        assert_eq!(dragged_height(px(200.), natural, panel), px(200.));
        assert_eq!(dragged_height(px(10.), natural, panel), natural);
        assert_eq!(dragged_height(px(700.), natural, panel), px(400.));
        assert_eq!(dragged_height(px(300.), natural, px(100.)), natural);
    }

    #[test]
    fn drag_moves_from_the_start_and_reset_restores_natural_rows() {
        let mut message = MessageSize::default();
        message.bounds.set(Bounds::new(
            point(px(0.), px(600.)),
            size(px(300.), px(72.)),
        ));
        assert!(!message.drag(px(500.), px(800.)), "no drag before begin");
        message.begin(px(590.));
        assert!(message.drag(px(490.), px(800.)));
        assert_eq!(message.height(), Some(px(172.)));
        message.end();
        assert!(!message.drag(px(100.), px(800.)), "no drag after end");
        assert!(message.reset());
        assert_eq!(message.height(), None);
    }
}
