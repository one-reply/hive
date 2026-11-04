use gpui_kit::{Bounds, Pixels, Point, Size};
use hive_core::terminal::{Column, Point as GridPoint, Side};

/// The same padded grid and font metrics used to paint this pane.
#[derive(Clone, Copy)]
pub(super) struct GridGeometry {
    pub bounds: Bounds<Pixels>,
    pub cell: Size<Pixels>,
    pub cols: u16,
    pub rows: u16,
}

impl GridGeometry {
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
