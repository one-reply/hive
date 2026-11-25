//! Hovering a web link underlines it; a click that does not drag opens it in the default
//! browser. A drag that starts on a link still selects text.
use hive_core::terminal::{Point as GridPoint, TerminalLink};

use super::*;

/// What the pointer last showed of links in this pane.
#[derive(Default)]
pub(super) struct LinkHover {
    /// The cell under the pointer.
    cell: Option<GridPoint<usize>>,
    /// The link at `cell`, looked up again on every render because output moves it.
    link: Option<TerminalLink>,
    /// Dispatch only if the same target is still under the pointer on release.
    pub(super) press: Option<(GridPoint<usize>, ClickTarget)>,
}

#[derive(PartialEq)]
pub(super) enum ClickTarget {
    Web(TerminalLink),
    Application(Vec<u8>),
}

impl TerminalView {
    /// `pointer` is `None` when the pointer is outside the pane or over something above it.
    pub(super) fn hover_link(&mut self, pointer: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let cell = link_cell(self.grid, pointer);
        if cell == self.link_hover.cell {
            return;
        }
        self.link_hover.cell = cell;
        // Repaint only when the underline moves, not on every cell the pointer crosses.
        let link = cell.and_then(|cell| self.session.read(cx).terminal()?.link_at(cell));
        if link != self.link_hover.link {
            self.link_hover.link = link;
            cx.notify();
        }
    }

    /// The pointer can leave through another window, which this pane never hears about.
    pub(super) fn clear_link_hover(&mut self, cx: &mut Context<Self>) {
        self.link_hover.press = None;
        self.hover_link(None, cx);
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

    /// Remembers a single press on a link; a double or triple click selects instead.
    pub(super) fn press_link(&mut self, event: &MouseDownEvent, cx: &App) {
        self.link_hover.press = (event.click_count == 1)
            .then(|| self.click_target(event.position, event.modifiers, cx))
            .flatten();
    }

    /// Opens the pressed link when the button comes up in the cell it went down in.
    pub(super) fn release_link(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(pressed) = self.link_hover.press.take() else {
            return;
        };
        if self
            .click_target(event.position, event.modifiers, cx)
            .as_ref()
            != Some(&pressed)
        {
            return;
        }
        match pressed.1 {
            ClickTarget::Web(link) => cx.open_url(link.url()),
            ClickTarget::Application(bytes) => self
                .session
                .update(cx, |session, cx| session.write_input(bytes, cx)),
        }
    }

    fn click_target(
        &self,
        position: Point<Pixels>,
        modifiers: Modifiers,
        cx: &App,
    ) -> Option<(GridPoint<usize>, ClickTarget)> {
        let cell = link_cell(self.grid, Some(position))?;
        let terminal = self.session.read(cx).terminal()?;
        let target = if let Some(link) = terminal.link_at(cell) {
            ClickTarget::Web(link)
        } else if !modifiers.modified() {
            ClickTarget::Application(terminal.click_input(cell)?)
        } else {
            return None;
        };
        Some((cell, target))
    }
}

/// The grid cell under `pointer`; nothing over the padding around the grid.
fn link_cell(
    grid: Option<GridGeometry>,
    pointer: Option<Point<Pixels>>,
) -> Option<GridPoint<usize>> {
    let (grid, pointer) = grid.zip(pointer)?;
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
    fn points_at_a_cell_only_inside_the_grid() {
        let inside = Some(point(px(125.), px(145.)));
        assert_eq!(
            link_cell(Some(grid()), inside),
            Some(Point::new(2, Column(2)))
        );
        assert_eq!(link_cell(None, inside), None);
        assert_eq!(link_cell(Some(grid()), None), None);
        assert_eq!(
            link_cell(Some(grid()), Some(point(px(98.), px(145.)))),
            None
        );
        assert_eq!(
            link_cell(Some(grid()), Some(point(px(125.), px(201.)))),
            None
        );
    }
}
