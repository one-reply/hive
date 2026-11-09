use gpui_kit::base::AutoScroll;
use gpui_kit::{Bounds, Pixels, Point, Size, px};
use hive_core::terminal::{Column, Point as GridPoint, Side};

// Terminal rows need finer control than the editor's pixel-based selection scroll.
const SELECTION_SCROLL_SPEED_SCALE: f32 = 0.25;

/// The same padded grid and font metrics used to paint this pane.
#[derive(Clone, Copy)]
pub(super) struct GridGeometry {
    pub bounds: Bounds<Pixels>,
    pub cell: Size<Pixels>,
    pub cols: u16,
    pub rows: u16,
}

impl GridGeometry {
    pub fn selection_scroll_delta(self, position: Point<Pixels>) -> Option<Pixels> {
        if self.cell.height <= px(0.)
            || (position.y >= self.bounds.top() && position.y <= self.bounds.bottom())
        {
            return None;
        }
        AutoScroll::compute_delta(position.y, self.bounds)
            .map(|delta| -delta * SELECTION_SCROLL_SPEED_SCALE)
    }

    pub fn selection_scroll_lines(self, position: Point<Pixels>, remainder: &mut f32) -> i32 {
        let Some(delta) = self.selection_scroll_delta(position) else {
            *remainder = 0.;
            return 0;
        };
        let lines = delta / self.cell.height;
        if remainder.signum() != lines.signum() {
            *remainder = 0.;
        }
        *remainder += lines;
        let whole = remainder.trunc() as i32;
        *remainder -= whole as f32;
        whole
    }

    pub fn cell_at(self, position: Point<Pixels>) -> (GridPoint<usize>, Side) {
        let column = ((position.x - self.bounds.origin.x) / self.cell.width).max(0.);
        let row = ((position.y - self.bounds.origin.y) / self.cell.height).max(0.);
        let side = if column >= self.cols as f32 || column.fract() >= 0.5 {
            Side::Right
        } else {
            Side::Left
        };
        (
            GridPoint::new(
                (row as usize).min(self.rows as usize - 1),
                Column((column as usize).min(self.cols as usize - 1)),
            ),
            side,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::GridGeometry;
    use gpui_kit::{Bounds, point, px, size};
    use hive_core::terminal::{Column, Point, Side};

    fn grid() -> GridGeometry {
        GridGeometry {
            bounds: Bounds::new(point(px(100.), px(100.)), size(px(100.), px(100.))),
            cell: size(px(10.), px(20.)),
            cols: 10,
            rows: 5,
        }
    }

    #[test]
    fn selection_scroll_starts_only_beyond_vertical_edges_and_accelerates_with_distance() {
        let grid = grid();
        for position in [
            point(px(150.), px(100.)),
            point(px(150.), px(200.)),
            point(px(0.), px(150.)),
        ] {
            assert_eq!(grid.selection_scroll_delta(position), None);
        }
        let near = grid
            .selection_scroll_delta(point(px(150.), px(99.)))
            .unwrap();
        let far = grid
            .selection_scroll_delta(point(px(150.), px(50.)))
            .unwrap();
        let bottom = grid
            .selection_scroll_delta(point(px(150.), px(250.)))
            .unwrap();
        assert!(near > px(0.) && far > near);
        assert_eq!(bottom, -far);
        assert_eq!(
            grid.selection_scroll_delta(point(px(150.), px(-10000.))),
            grid.selection_scroll_delta(point(px(150.), px(-20000.)))
        );
    }

    #[test]
    fn selection_scroll_accumulates_fractional_rows_and_resets_on_return_or_reversal() {
        let grid = grid();
        let above = point(px(150.), px(99.));
        let below = point(px(150.), px(201.));
        let mut remainder = 0.;
        assert_eq!(grid.selection_scroll_lines(above, &mut remainder), 0);
        assert!(remainder > 0.);
        let mut total = 0;
        for _ in 0..10 {
            total += grid.selection_scroll_lines(above, &mut remainder);
        }
        assert!(total > 0);
        assert_eq!(grid.selection_scroll_lines(below, &mut remainder), 0);
        assert!(remainder < 0.);
        assert_eq!(
            grid.selection_scroll_lines(point(px(150.), px(150.)), &mut remainder),
            0
        );
        assert_eq!(remainder, 0.);
    }

    #[test]
    fn uses_pane_origin_cell_halves_and_clamps_outside_drag() {
        let grid = GridGeometry {
            bounds: Bounds::new(point(px(104.), px(54.)), size(px(100.), px(60.))),
            cell: size(px(10.), px(20.)),
            cols: 10,
            rows: 3,
        };
        assert_eq!(
            grid.cell_at(point(px(116.), px(75.))),
            (Point::new(1, Column(1)), Side::Left)
        );
        assert_eq!(
            grid.cell_at(point(px(119.), px(75.))),
            (Point::new(1, Column(1)), Side::Right)
        );
        assert_eq!(
            grid.cell_at(point(px(0.), px(0.))),
            (Point::new(0, Column(0)), Side::Left)
        );
        assert_eq!(
            grid.cell_at(point(px(500.), px(500.))),
            (Point::new(2, Column(9)), Side::Right)
        );
    }
}
