//! Canvas geometry of a recipe: planner, approval, parallel lanes joining before the reviews.
use super::Item;

#[derive(Clone, Copy)]
pub(in super::super) struct BoxAt {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
impl BoxAt {
    fn centered(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            x: x - w / 2.,
            y,
            w,
            h,
        }
    }
    pub(in super::super) fn middle(self) -> f32 {
        self.x + self.w / 2.
    }
    pub(in super::super) fn floor(self) -> f32 {
        self.y + self.h
    }
    pub(in super::super) fn contains(self, (x, y): (f32, f32)) -> bool {
        (self.x..=self.x + self.w).contains(&x) && (self.y..=self.floor()).contains(&y)
    }
    fn grown(self, by: f32) -> Self {
        Self {
            x: self.x - by,
            y: self.y - by,
            w: self.w + by * 2.,
            h: self.h + by * 2.,
        }
    }
}
const NODE_W: f32 = 220.;
const LANE_W: f32 = 154.;
const SLOT: f32 = 170.;
const GUTTER: f32 = 46.;
/// Height of a lane or review card, and the distance from one review to the next.
const STAGE_H: f32 = 54.;
const REVIEW_PITCH: f32 = 74.;
/// How far beside a row of stages the pointer still counts as over it while dragging.
const DROP_REACH: f32 = 24.;
/// Thickness of the line that shows where a dragged stage would land.
const DROP_LINE: f32 = 2.;

/// The middles of the gaps around a row of spans `(start, end)`: before the first, between
/// neighbours and after the last. `gap` is the space between two neighbours.
fn gap_middles(spans: impl Iterator<Item = (f32, f32)>, gap: f32) -> Vec<f32> {
    let spans: Vec<_> = spans.collect();
    let (Some(first), Some(last)) = (spans.first(), spans.last()) else {
        return Vec::new();
    };
    let mut middles = vec![first.0 - gap / 2.];
    middles.extend(spans.windows(2).map(|pair| (pair[0].1 + pair[1].0) / 2.));
    middles.push(last.1 + gap / 2.);
    middles
}

fn nearest(middles: &[f32], at: f32) -> Option<usize> {
    (0..middles.len()).min_by(|&a, &b| (middles[a] - at).abs().total_cmp(&(middles[b] - at).abs()))
}
#[derive(Clone)]
pub(in super::super) struct Layout {
    pub width: f32,
    pub height: f32,
    pub start: BoxAt,
    pub planner: BoxAt,
    pub approve: BoxAt,
    pub build: BoxAt,
    pub lanes: Vec<BoxAt>,
    pub join: BoxAt,
    pub reviews: Vec<BoxAt>,
    pub end: BoxAt,
}
impl Layout {
    pub fn new(lanes: usize, reviews: usize) -> Self {
        let lane_span = (lanes.saturating_sub(1) as f32 * SLOT + LANE_W).max(NODE_W);
        let width = lane_span + GUTTER * 2.;
        let axis = width / 2.;
        let start = BoxAt::centered(axis, 10., 80., 22.);
        let planner = BoxAt::centered(axis, 49., NODE_W, 54.);
        let approve = BoxAt::centered(axis, 120., 126., 24.);
        let build = BoxAt::centered(axis, 165., NODE_W, 42.);
        let lanes = (0..lanes)
            .map(|i| {
                BoxAt::centered(
                    axis + (i as f32 - (lanes as f32 - 1.) / 2.) * SLOT,
                    232.,
                    LANE_W,
                    STAGE_H,
                )
            })
            .collect();
        let join = BoxAt::centered(axis, 305., 114., 22.);
        let reviews = (0..reviews)
            .map(|i| BoxAt::centered(axis, 346. + i as f32 * REVIEW_PITCH, NODE_W, STAGE_H))
            .collect::<Vec<_>>();
        let end = BoxAt::centered(
            axis,
            reviews.last().copied().map_or(join.floor(), BoxAt::floor) + 24.,
            132.,
            26.,
        );
        Self {
            width,
            height: end.floor() + 14.,
            start,
            planner,
            approve,
            build,
            lanes,
            join,
            reviews,
            end,
        }
    }
    pub fn placement(&self, item: Item) -> BoxAt {
        match item {
            Item::Planner => self.planner,
            Item::Development => self.build,
            Item::Lane(i) => self.lanes[i],
            Item::Review(i) => self.reviews[i],
        }
    }

    fn review_gaps(&self) -> Vec<f32> {
        let spans = self.reviews.iter().map(|r| (r.y, r.floor()));
        gap_middles(spans, REVIEW_PITCH - STAGE_H)
    }

    fn lane_gaps(&self) -> Vec<f32> {
        let spans = self.lanes.iter().map(|l| (l.x, l.x + l.w));
        gap_middles(spans, SLOT - LANE_W)
    }

    /// The gap among the reviews nearest to the pointer, as the position a stage dropped
    /// there is inserted before (`0..=reviews`). `None` away from the review column.
    pub fn review_gap_at(&self, at: (f32, f32)) -> Option<usize> {
        let (first, last) = (self.reviews.first()?, self.reviews.last()?);
        let column = BoxAt {
            h: last.floor() - first.y,
            ..*first
        };
        column
            .grown(DROP_REACH)
            .contains(at)
            .then(|| nearest(&self.review_gaps(), at.1))?
    }

    /// The gap among the lanes nearest to the pointer, as the position a lane dropped there
    /// is inserted before (`0..=lanes`). `None` away from the lane row.
    pub fn lane_gap_at(&self, at: (f32, f32)) -> Option<usize> {
        let (first, last) = (self.lanes.first()?, self.lanes.last()?);
        let row = BoxAt {
            w: last.x + last.w - first.x,
            ..*first
        };
        row.grown(DROP_REACH)
            .contains(at)
            .then(|| nearest(&self.lane_gaps(), at.0))?
    }

    /// The line across a gap among the reviews, where a stage dropped there would land.
    pub fn review_gap_line(&self, gap: usize) -> Option<BoxAt> {
        let (middle, column) = (*self.review_gaps().get(gap)?, self.reviews.first()?);
        Some(BoxAt {
            y: middle - DROP_LINE / 2.,
            h: DROP_LINE,
            ..*column
        })
    }

    /// The line down a gap among the lanes, where a lane dropped there would land.
    pub fn lane_gap_line(&self, gap: usize) -> Option<BoxAt> {
        let (middle, row) = (*self.lane_gaps().get(gap)?, self.lanes.first()?);
        Some(BoxAt {
            x: middle - DROP_LINE / 2.,
            w: DROP_LINE,
            ..*row
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Layout;
    #[test]
    fn developers_fork_in_parallel_and_join_before_every_review() {
        let map = Layout::new(3, 2);
        assert!(
            map.lanes
                .windows(2)
                .all(|p| p[0].x + p[0].w < p[1].x && p[0].y == p[1].y)
        );
        assert!(
            map.lanes
                .iter()
                .all(|l| map.build.floor() < l.y && l.floor() < map.join.y)
        );
        assert!(map.join.floor() < map.reviews[0].y && map.reviews[1].floor() < map.end.y);
    }

    #[test]
    fn the_pointer_picks_the_nearest_gap_of_the_row_it_is_over() {
        let map = Layout::new(3, 3);
        let column = map.reviews[0].middle();
        // Over the upper half of a review the gap above it is nearer, over the lower half
        // the one below; above the first and below the last there is a gap too.
        let upper = |i: usize| (column, map.reviews[i].y + 5.);
        let lower = |i: usize| (column, map.reviews[i].floor() - 5.);
        assert_eq!(map.review_gap_at(upper(0)), Some(0));
        assert_eq!(map.review_gap_at(lower(0)), Some(1));
        assert_eq!(map.review_gap_at(upper(2)), Some(2));
        assert_eq!(map.review_gap_at(lower(2)), Some(3));
        assert_eq!(map.review_gap_at((column, map.reviews[0].y - 10.)), Some(0));
        assert_eq!(map.review_gap_at((column, map.reviews[0].y - 60.)), None);
        assert_eq!(map.review_gap_at((map.reviews[0].x - 60., 400.)), None);

        let row = map.lanes[0].y + 20.;
        assert_eq!(map.lane_gap_at((map.lanes[0].x + 5., row)), Some(0));
        assert_eq!(map.lane_gap_at((map.lanes[1].middle() + 5., row)), Some(2));
        assert_eq!(
            map.lane_gap_at((map.lanes[2].x + map.lanes[2].w, row)),
            Some(3)
        );
        assert_eq!(
            map.lane_gap_at((map.lanes[1].middle(), map.join.floor() + 40.)),
            None
        );
    }

    #[test]
    fn a_gap_line_lies_between_its_neighbours_and_spans_the_stage() {
        let map = Layout::new(2, 2);
        let between = map.review_gap_line(1).unwrap();
        assert!(map.reviews[0].floor() < between.y && between.floor() < map.reviews[1].y);
        assert_eq!((between.x, between.w), (map.reviews[0].x, map.reviews[0].w));
        let last = map.review_gap_line(2).unwrap();
        assert!(map.reviews[1].floor() < last.y && last.floor() < map.end.y);
        assert!(map.review_gap_line(3).is_none());

        let beside = map.lane_gap_line(1).unwrap();
        assert!(map.lanes[0].x + map.lanes[0].w < beside.x && beside.x + beside.w < map.lanes[1].x);
        assert_eq!((beside.y, beside.h), (map.lanes[0].y, map.lanes[0].h));
        assert!(map.lane_gap_line(0).unwrap().x > 0.);
        assert!(map.lane_gap_line(3).is_none());
    }
}
