//! Canvas geometry of a recipe: planner, approval, parallel lanes joining before the reviews,
//! and the reviews row by row, the reviews of a row side by side.
use super::Item;
use hive_core::pipeline::Template;
use std::ops::Range;

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
/// How wide `count` stages standing side by side are.
fn side_by_side(count: usize) -> f32 {
    count.saturating_sub(1) as f32 * SLOT + LANE_W
}

/// The boxes of `count` stages side by side around `axis`; one stage alone is a full card.
fn row_of_stages(count: usize, axis: f32, y: f32) -> impl Iterator<Item = BoxAt> {
    let width = if count == 1 { NODE_W } else { LANE_W };
    let first = axis - (count as f32 - 1.) / 2. * SLOT;
    (0..count).map(move |i| BoxAt::centered(first + i as f32 * SLOT, y, width, STAGE_H))
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
    /// Every review by its position in the recipe.
    pub reviews: Vec<BoxAt>,
    /// The reviews of each row, top to bottom: the reviews of a row run together.
    pub rows: Vec<Range<usize>>,
    pub end: BoxAt,
}
impl Layout {
    /// The canvas of a recipe.
    pub fn of(template: &Template) -> Self {
        let rows: Vec<_> = template.review_groups().iter().map(Range::len).collect();
        Self::new(template.lanes.len(), &rows)
    }

    /// `rows` holds how many reviews run together in each row of reviews, top to bottom.
    pub fn new(lanes: usize, rows: &[usize]) -> Self {
        let widest = rows.iter().copied().chain([lanes]).max().unwrap_or(1);
        let width = side_by_side(widest).max(NODE_W) + GUTTER * 2.;
        let axis = width / 2.;
        let start = BoxAt::centered(axis, 10., 80., 22.);
        let planner = BoxAt::centered(axis, 49., NODE_W, 54.);
        let approve = BoxAt::centered(axis, 120., 126., 24.);
        let build = BoxAt::centered(axis, 165., NODE_W, 42.);
        // Lanes keep the narrow card even when there is one: a lane is always a member.
        let first_lane = axis - (lanes as f32 - 1.) / 2. * SLOT;
        let lanes = (0..lanes)
            .map(|i| BoxAt::centered(first_lane + i as f32 * SLOT, 232., LANE_W, STAGE_H))
            .collect();
        let join = BoxAt::centered(axis, 305., 114., 22.);
        let mut reviews = Vec::new();
        let mut spans = Vec::new();
        for (row, &count) in rows.iter().enumerate() {
            let y = 346. + row as f32 * REVIEW_PITCH;
            spans.push(reviews.len()..reviews.len() + count);
            reviews.extend(row_of_stages(count, axis, y));
        }
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
            rows: spans,
            end,
        }
    }

    /// Which row of reviews the review at `review` stands in.
    pub fn row_of(&self, review: usize) -> Option<usize> {
        self.rows.iter().position(|row| row.contains(&review))
    }

    /// The reviews as one block: the space the rows of reviews take together.
    fn review_block(&self) -> Option<BoxAt> {
        let (first, last) = (self.reviews.first()?, self.reviews.last()?);
        let left = self.reviews.iter().map(|r| r.x).fold(first.x, f32::min);
        let right = self.reviews.iter().map(|r| r.x + r.w).fold(left, f32::max);
        Some(BoxAt {
            x: left,
            y: first.y,
            w: right - left,
            h: last.floor() - first.y,
        })
    }
    pub fn placement(&self, item: Item) -> BoxAt {
        match item {
            Item::Planner => self.planner,
            Item::Development => self.build,
            Item::Lane(i) => self.lanes[i],
            Item::Review(i) => self.reviews[i],
        }
    }

    /// Middles of the gaps around the rows of reviews, top to bottom.
    fn review_gaps(&self) -> Vec<f32> {
        let rows = self
            .rows
            .iter()
            .filter_map(|row| self.reviews.get(row.start));
        gap_middles(rows.map(|r| (r.y, r.floor())), REVIEW_PITCH - STAGE_H)
    }

    fn lane_gaps(&self) -> Vec<f32> {
        let spans = self.lanes.iter().map(|l| (l.x, l.x + l.w));
        gap_middles(spans, SLOT - LANE_W)
    }

    /// The gap among the rows of reviews nearest to the pointer, as the row a stage dropped
    /// there stands before (`0..=rows`). `None` away from the reviews.
    pub fn review_gap_at(&self, at: (f32, f32)) -> Option<usize> {
        self.review_block()?
            .grown(DROP_REACH)
            .contains(at)
            .then(|| nearest(&self.review_gaps(), at.1))?
    }

    /// The review whose side the pointer is over, and whether it is the far side: the left
    /// and right thirds of a review's card are where another review is put beside it.
    pub fn review_side_at(&self, at: (f32, f32)) -> Option<(usize, bool)> {
        let review = self.reviews.iter().position(|r| r.contains(at))?;
        let card = self.reviews[review];
        let third = card.w / 3.;
        if at.0 < card.x + third {
            Some((review, false))
        } else if at.0 > card.x + card.w - third {
            Some((review, true))
        } else {
            None
        }
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

    /// The line across a gap among the rows of reviews, where a stage dropped there would
    /// land on a row of its own.
    pub fn review_gap_line(&self, gap: usize) -> Option<BoxAt> {
        let (middle, block) = (*self.review_gaps().get(gap)?, self.review_block()?);
        Some(BoxAt {
            y: middle - DROP_LINE / 2.,
            h: DROP_LINE,
            ..block
        })
    }

    /// The line down the side of a review, where a review dropped there would stand to run
    /// together with it: `after` it, or before it.
    pub fn review_side_line(&self, review: usize, after: bool) -> Option<BoxAt> {
        let card = *self.reviews.get(review)?;
        let beside = (SLOT - LANE_W) / 2.;
        let x = if after {
            card.x + card.w + beside
        } else {
            card.x - beside
        };
        Some(BoxAt {
            x: x - DROP_LINE / 2.,
            w: DROP_LINE,
            ..card
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
        let map = Layout::new(3, &[1, 1]);
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
        let map = Layout::new(3, &[1, 1, 1]);
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
    fn reviews_of_a_row_stand_side_by_side_and_the_rows_follow_one_another() {
        let map = Layout::new(1, &[1, 3, 1]);
        assert_eq!(map.rows, [0..1, 1..4, 4..5]);
        assert_eq!(
            (map.row_of(3), map.row_of(4), map.row_of(9)),
            (Some(1), Some(2), None)
        );
        let row = &map.reviews[1..4];
        assert!(
            row.windows(2)
                .all(|p| p[0].x + p[0].w < p[1].x && p[0].y == p[1].y)
        );
        assert!(map.reviews[0].floor() < row[0].y && row[0].floor() < map.reviews[4].y);
        // The canvas is as wide as its widest row needs, and a lone review keeps its width.
        assert!(row[0].x > 0. && row[2].x + row[2].w < map.width);
        assert!(map.reviews[0].w > row[0].w);
        assert!(map.reviews[4].floor() < map.end.y);
        // Gaps are counted between rows, not between reviews.
        let below_the_row = (map.width / 2., row[0].floor() + 8.);
        assert_eq!(map.review_gap_at(below_the_row), Some(2));
        let across = map.review_gap_line(2).unwrap();
        assert_eq!(
            (across.x, across.w),
            (row[0].x, row[2].x + row[2].w - row[0].x)
        );
    }

    #[test]
    fn the_sides_of_a_review_are_where_another_is_put_beside_it() {
        let map = Layout::new(1, &[2, 1]);
        let (first, second) = (map.reviews[0], map.reviews[1]);
        let y = first.y + 10.;
        assert_eq!(map.review_side_at((first.x + 4., y)), Some((0, false)));
        assert_eq!(
            map.review_side_at((first.x + first.w - 4., y)),
            Some((0, true))
        );
        assert_eq!(map.review_side_at((first.middle(), y)), None);
        assert_eq!(map.review_side_at((first.x - 4., y)), None);
        let after = map.review_side_line(0, true).unwrap();
        assert!(first.x + first.w < after.x && after.x + after.w < second.x);
        assert_eq!((after.y, after.h), (first.y, first.h));
        let before = map.review_side_line(0, false).unwrap();
        assert!(before.x + before.w < first.x && before.x > 0.);
        assert!(map.review_side_line(3, true).is_none());
    }

    #[test]
    fn a_gap_line_lies_between_its_neighbours_and_spans_the_stage() {
        let map = Layout::new(2, &[1, 1]);
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
