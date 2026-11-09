//! Drag selection and history autoscroll, including pointer movement outside the pane.
use super::*;

#[cfg(test)]
mod tests;

impl TerminalView {
    /// Element-local move handlers stop at its hitbox, so a drag listens on the window.
    pub(in crate::terminal) fn track_selection(view: &Entity<Self>, window: &mut Window) {
        let this = view.downgrade();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                let _ = this.update(cx, |this, cx| this.mouse_move(event, window, cx));
            }
        });
        let this = view.downgrade();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                let _ = this.update(cx, |this, cx| {
                    this.extend_selection(event.position, cx);
                    this.stop_selection();
                });
            }
        });
    }

    pub(super) fn stop_selection(&mut self) {
        self.selecting = false;
        self.selection_scroll.stop();
        self.selection_scroll_remainder = 0.;
    }

    pub(super) fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        let Some(grid) = self.grid else { return };
        let (point, side) = grid.cell_at(event.position);
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.stop_selection();
        self.selecting = true;
        self.session.update(cx, |session, cx| {
            session.start_selection(point, side, kind, cx)
        });
        cx.stop_propagation();
    }

    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        let Some(grid) = self.grid else { return };
        let (point, side) = grid.cell_at(position);
        self.session
            .update(cx, |session, cx| session.update_selection(point, side, cx));
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) || !window.is_window_active() {
            self.stop_selection();
            return;
        }
        self.extend_selection(event.position, cx);
        self.selection_scroll.last_drag_position = Some(event.position);
        let delta = self
            .grid
            .and_then(|grid| grid.selection_scroll_delta(event.position));
        if delta.is_none() {
            self.selection_scroll_remainder = 0.;
        }
        self.selection_scroll
            .set(delta, cx, |_, this, cx| this.autoscroll_selection(cx));
    }

    fn autoscroll_selection(&mut self, cx: &mut Context<Self>) {
        let Some((grid, position)) = self.grid.zip(self.selection_scroll.last_drag_position) else {
            self.selection_scroll.stop();
            return;
        };
        if grid.selection_scroll_delta(position).is_none() {
            self.selection_scroll.stop();
            self.selection_scroll_remainder = 0.;
            return;
        }
        let lines = grid.selection_scroll_lines(position, &mut self.selection_scroll_remainder);
        if lines == 0 {
            return;
        }
        let (point, side) = grid.cell_at(position);
        if !self.session.update(cx, |session, cx| {
            session.scroll_selection(lines, point, side, cx)
        }) {
            self.selection_scroll.stop();
            self.selection_scroll_remainder = 0.;
        }
    }
}
