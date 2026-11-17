//! Holding ⌘ over a web link underlines it; ⌘-click opens it in the default browser.
use hive_core::terminal::{Point as GridPoint, TerminalLink};

use super::*;

/// What the pointer last showed of links in this pane.
#[derive(Default)]
pub(super) struct LinkHover {
    /// Last pointer position inside the pane, for a ⌘ press without movement.
    pointer: Option<Point<Pixels>>,
    /// The cell under the pointer while ⌘ is held.
    cell: Option<GridPoint<usize>>,
    /// The link at `cell`, looked up again on every render because output moves it.
    link: Option<TerminalLink>,
}

impl TerminalView {
    /// `pointer` is `None` when the pointer is outside the pane or over something above it.
    pub(super) fn hover_link(
        &mut self,
        pointer: Option<Point<Pixels>>,
        command: bool,
        cx: &mut Context<Self>,
    ) {
        self.link_hover.pointer = pointer;
        let cell = link_cell(self.grid, pointer, command);
        if cell != self.link_hover.cell {
            self.link_hover.cell = cell;
            cx.notify();
        }
    }

    pub(super) fn modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hover_link(self.link_hover.pointer, event.modifiers.platform, cx);
    }

    /// ⌘ is released outside the window after a click sends focus to the browser.
    pub(super) fn clear_link_hover(&mut self, cx: &mut Context<Self>) {
        self.hover_link(None, false, cx);
    }

    pub(super) fn sync_link(&mut self, cx: &App) {
        let session = self.session.read(cx);
        self.link_hover.link = self
            .link_hover
            .cell
            .and_then(|cell| session.terminal()?.link_at(cell));
    }

    pub(in crate::terminal) fn hovered_link(&self) -> Option<&TerminalLink> {
        self.link_hover.link.as_ref()
    }

    /// Opens the link under a ⌘-click; returns whether there was one.
    pub(super) fn open_link(&self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let session = self.session.read(cx);
        let link = link_cell(self.grid, Some(position), true)
            .and_then(|cell| session.terminal()?.link_at(cell));
        let Some(link) = link else { return false };
        cx.open_url(link.url());
        true
    }
}

/// The grid cell a ⌘-gesture at `pointer` points at; nothing over the padding around the grid.
fn link_cell(
    grid: Option<GridGeometry>,
    pointer: Option<Point<Pixels>>,
    command: bool,
) -> Option<GridPoint<usize>> {
    let (grid, pointer) = grid.zip(pointer).filter(|_| command)?;
    grid.bounds
        .contains(&pointer)
        .then(|| grid.cell_at(pointer).0)
}

#[cfg(test)]
mod tests {
    use super::link_cell;
    use crate::terminal::mouse::GridGeometry;
    use gpui_kit::{Bounds, point, px, size};
    use hive_core::terminal::{Column, Point};

    fn grid() -> GridGeometry {
        GridGeometry {
            bounds: Bounds::new(point(px(100.), px(100.)), size(px(100.), px(100.))),
            cell: size(px(10.), px(20.)),
            cols: 10,
            rows: 5,
        }
    }

    #[test]
    fn points_at_a_cell_only_with_command_inside_the_grid() {
        let inside = Some(point(px(125.), px(145.)));
        assert_eq!(
            link_cell(Some(grid()), inside, true),
            Some(Point::new(2, Column(2)))
        );
        assert_eq!(link_cell(Some(grid()), inside, false), None);
        assert_eq!(link_cell(None, inside, true), None);
        assert_eq!(link_cell(Some(grid()), None, true), None);
        assert_eq!(
            link_cell(Some(grid()), Some(point(px(98.), px(145.))), true),
            None
        );
        assert_eq!(
            link_cell(Some(grid()), Some(point(px(125.), px(201.))), true),
            None
        );
    }
}
