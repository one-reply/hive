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
}
const NODE_W: f32 = 220.;
const LANE_W: f32 = 154.;
const SLOT: f32 = 170.;
const GUTTER: f32 = 46.;
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
                    54.,
                )
            })
            .collect();
        let join = BoxAt::centered(axis, 305., 114., 22.);
        let reviews = (0..reviews)
            .map(|i| BoxAt::centered(axis, 346. + i as f32 * 74., NODE_W, 54.))
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
}
