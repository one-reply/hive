//! Layout of the panes inside one terminal tab: a tree of splits whose leaves are panes.
//! Pure data, so the split/close rules are testable without a window.

use gpui_kit::Axis;

/// The side of a pane that keyboard focus moves toward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// A pane's place inside the tab, in fractions of the tab's width and height.
#[derive(Clone, Copy)]
struct Frame {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

impl Frame {
    const TAB: Self = Self {
        left: 0.,
        top: 0.,
        width: 1.,
        height: 1.,
    };

    /// The span along the axis of movement, then the span across it.
    fn spans(self, direction: Direction) -> ((f32, f32), (f32, f32)) {
        let horizontal = (self.left, self.left + self.width);
        let vertical = (self.top, self.top + self.height);
        match direction {
            Direction::Left | Direction::Right => (horizontal, vertical),
            Direction::Up | Direction::Down => (vertical, horizontal),
        }
    }
}

/// Frames come from sums of fractions; panes that touch must still count as touching.
const FRAME_TOLERANCE: f32 = 1e-4;

/// A `Split` always has at least two children, and never a child split along its own axis
/// (same-axis splits are flattened into one row/column, like Orca and Zed do).
pub(crate) enum PaneTree<T> {
    Leaf(T),
    Split {
        axis: Axis,
        children: Vec<PaneTree<T>>,
    },
}

impl<T: Clone + PartialEq> PaneTree<T> {
    pub(crate) fn for_each(&self, visit: &mut impl FnMut(&T)) {
        match self {
            Self::Leaf(leaf) => visit(leaf),
            Self::Split { children, .. } => {
                for child in children {
                    child.for_each(visit);
                }
            }
        }
    }
    /// Put `new` right after `target` along `axis`. `false` when `target` is not in the tree.
    pub(crate) fn split(&mut self, target: &T, new: T, axis: Axis) -> bool {
        match self {
            Self::Leaf(leaf) => {
                if leaf != target {
                    return false;
                }
                let old = Self::Leaf(leaf.clone());
                *self = Self::Split {
                    axis,
                    children: vec![old, Self::Leaf(new)],
                };
                true
            }
            Self::Split {
                axis: own_axis,
                children,
            } => {
                if *own_axis == axis
                    && let Some(ix) = children.iter().position(|child| child.is_leaf(target))
                {
                    children.insert(ix + 1, Self::Leaf(new));
                    return true;
                }
                match children.iter_mut().find(|child| child.contains(target)) {
                    Some(child) => child.split(target, new, axis),
                    None => false,
                }
            }
        }
    }

    /// Remove the `target` pane and return the pane that should take focus: the nearest one
    /// before it, else the one after it. `None` when `target` is missing or is the only pane
    /// (closing the last pane means closing the tab, which is the caller's call).
    pub(crate) fn remove(&mut self, target: &T) -> Option<T> {
        let Self::Split { axis, children } = self else {
            return None;
        };
        if let Some(ix) = children.iter().position(|child| child.is_leaf(target)) {
            children.remove(ix);
            let neighbor = match ix.checked_sub(1) {
                Some(before) => children[before].last_leaf(),
                None => children[0].first_leaf(),
            }
            .clone();
            if children.len() == 1 {
                *self = children.remove(0);
            }
            return Some(neighbor);
        }
        let ix = children.iter().position(|child| child.contains(target))?;
        let neighbor = children[ix].remove(target);
        // The child may have collapsed into a split along our axis: merge it into our row.
        if matches!(&children[ix], Self::Split { axis: child_axis, .. } if child_axis == axis)
            && let Self::Split {
                children: grandchildren,
                ..
            } = children.remove(ix)
        {
            children.splice(ix..ix, grandchildren);
        }
        neighbor
    }

    pub(crate) fn first_leaf(&self) -> &T {
        match self {
            Self::Leaf(leaf) => leaf,
            Self::Split { children, .. } => children[0].first_leaf(),
        }
    }

    fn last_leaf(&self) -> &T {
        match self {
            Self::Leaf(leaf) => leaf,
            Self::Split { children, .. } => children[children.len() - 1].last_leaf(),
        }
    }

    fn is_leaf(&self, target: &T) -> bool {
        matches!(self, Self::Leaf(leaf) if leaf == target)
    }

    fn contains(&self, target: &T) -> bool {
        match self {
            Self::Leaf(leaf) => leaf == target,
            Self::Split { children, .. } => children.iter().any(|child| child.contains(target)),
        }
    }

    /// The pane focus lands on when it moves from `target` toward `direction`: the nearest one
    /// on that side that shares an edge span with `target`, and among several the one whose
    /// middle is closest to `target`'s (the earlier one on a tie). `None` at the tab's edge.
    ///
    /// `shares` gives the sizes of a split's children along its axis, in any unit; a split
    /// without them divides its space equally.
    pub(crate) fn neighbor(
        &self,
        target: &T,
        direction: Direction,
        shares: &impl Fn(&Self) -> Option<Vec<f32>>,
    ) -> Option<&T> {
        let mut frames = Vec::new();
        self.frames(Frame::TAB, shares, &mut frames);
        let (_, from) = frames.iter().find(|(pane, _)| *pane == target)?;
        let ((start, end), (across_start, across_end)) = from.spans(direction);
        let middle = (across_start + across_end) / 2.;
        frames
            .iter()
            .filter(|(pane, _)| *pane != target)
            .filter_map(|(pane, frame)| {
                let ((near, far), (side_start, side_end)) = frame.spans(direction);
                let gap = match direction {
                    Direction::Right | Direction::Down => near - end,
                    Direction::Left | Direction::Up => start - far,
                };
                let beside = side_start < across_end - FRAME_TOLERANCE
                    && side_end > across_start + FRAME_TOLERANCE;
                let offset = ((side_start + side_end) / 2. - middle).abs();
                (gap > -FRAME_TOLERANCE && beside).then_some((*pane, gap, offset))
            })
            .min_by(|(_, gap, offset), (_, other_gap, other_offset)| {
                gap.total_cmp(other_gap)
                    .then(offset.total_cmp(other_offset))
            })
            .map(|(pane, ..)| pane)
    }

    fn frames<'a>(
        &'a self,
        frame: Frame,
        lookup: &impl Fn(&Self) -> Option<Vec<f32>>,
        frames: &mut Vec<(&'a T, Frame)>,
    ) {
        let (axis, children) = match self {
            Self::Leaf(leaf) => return frames.push((leaf, frame)),
            Self::Split { axis, children } => (*axis, children),
        };
        let shares = lookup(self)
            .filter(|shares| shares.len() == children.len() && shares.iter().all(|s| *s > 0.))
            .unwrap_or_else(|| vec![1.; children.len()]);
        let total: f32 = shares.iter().sum();
        let mut offset = 0.;
        for (child, share) in children.iter().zip(shares) {
            let part = share / total;
            let child_frame = match axis {
                Axis::Horizontal => Frame {
                    left: frame.left + frame.width * offset,
                    width: frame.width * part,
                    ..frame
                },
                Axis::Vertical => Frame {
                    top: frame.top + frame.height * offset,
                    height: frame.height * part,
                    ..frame
                },
            };
            child.frames(child_frame, lookup, frames);
            offset += part;
        }
    }

    /// The first pane, in reading order, that `matches`.
    pub(crate) fn find(&self, matches: &impl Fn(&T) -> bool) -> Option<&T> {
        match self {
            Self::Leaf(leaf) => matches(leaf).then_some(leaf),
            Self::Split { children, .. } => children.iter().find_map(|child| child.find(matches)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compact shape: `1`, `H[1 2]`, `V[1 H[2 3]]`.
    fn shape(tree: &PaneTree<u32>) -> String {
        match tree {
            PaneTree::Leaf(leaf) => leaf.to_string(),
            PaneTree::Split { axis, children } => {
                let tag = match axis {
                    Axis::Horizontal => "H",
                    Axis::Vertical => "V",
                };
                let inner: Vec<String> = children.iter().map(shape).collect();
                format!("{tag}[{}]", inner.join(" "))
            }
        }
    }

    #[test]
    fn same_axis_splits_stay_in_one_row() {
        let mut tree = PaneTree::Leaf(1);
        assert!(tree.split(&1, 2, Axis::Horizontal));
        assert!(tree.split(&1, 3, Axis::Horizontal));
        assert_eq!(shape(&tree), "H[1 3 2]");
    }

    #[test]
    fn find_walks_nested_splits_in_order() {
        let mut tree = PaneTree::Leaf(1);
        tree.split(&1, 2, Axis::Horizontal);
        tree.split(&2, 3, Axis::Vertical);
        assert_eq!(tree.find(&|pane| *pane > 1), Some(&2));
        assert_eq!(tree.find(&|pane| *pane == 3), Some(&3));
        assert_eq!(tree.find(&|pane| *pane == 9), None);
    }

    #[test]
    fn cross_axis_split_nests() {
        let mut tree = PaneTree::Leaf(1);
        tree.split(&1, 2, Axis::Horizontal);
        tree.split(&2, 3, Axis::Vertical);
        assert_eq!(shape(&tree), "H[1 V[2 3]]");
    }

    #[test]
    fn split_of_missing_pane_is_refused() {
        let mut tree = PaneTree::Leaf(1);
        assert!(!tree.split(&9, 2, Axis::Horizontal));
        assert_eq!(shape(&tree), "1");
    }

    #[test]
    fn remove_focuses_previous_else_next() {
        let mut tree = PaneTree::Leaf(1);
        tree.split(&1, 2, Axis::Horizontal);
        tree.split(&2, 3, Axis::Horizontal);
        assert_eq!(tree.remove(&2), Some(1));
        assert_eq!(tree.remove(&1), Some(3));
        assert_eq!(shape(&tree), "3");
    }

    #[test]
    fn remove_collapses_and_flattens() {
        // H[1 V[2 H[3 4]]] minus 2 → the V collapses to H[3 4], merged into the outer row.
        let mut tree = PaneTree::Leaf(1);
        tree.split(&1, 2, Axis::Horizontal);
        tree.split(&2, 3, Axis::Vertical);
        tree.split(&3, 4, Axis::Horizontal);
        assert_eq!(shape(&tree), "H[1 V[2 H[3 4]]]");
        assert_eq!(tree.remove(&2), Some(3));
        assert_eq!(shape(&tree), "H[1 3 4]");
    }

    fn equal(_: &PaneTree<u32>) -> Option<Vec<f32>> {
        None
    }

    /// `V[H[1 2] H[3 4]]`: two rows of two.
    fn grid() -> PaneTree<u32> {
        let mut tree = PaneTree::Leaf(1);
        tree.split(&1, 3, Axis::Vertical);
        tree.split(&1, 2, Axis::Horizontal);
        tree.split(&3, 4, Axis::Horizontal);
        tree
    }

    #[test]
    fn neighbor_in_a_row_is_the_adjacent_pane_and_the_edge_has_none() {
        let mut tree = PaneTree::Leaf(1);
        tree.split(&1, 2, Axis::Horizontal);
        tree.split(&2, 3, Axis::Horizontal);
        assert_eq!(tree.neighbor(&1, Direction::Right, &equal), Some(&2));
        assert_eq!(tree.neighbor(&2, Direction::Right, &equal), Some(&3));
        assert_eq!(tree.neighbor(&3, Direction::Left, &equal), Some(&2));
        assert_eq!(tree.neighbor(&1, Direction::Left, &equal), None);
        assert_eq!(tree.neighbor(&3, Direction::Right, &equal), None);
        assert_eq!(tree.neighbor(&2, Direction::Up, &equal), None);
        assert_eq!(tree.neighbor(&2, Direction::Down, &equal), None);
    }

    #[test]
    fn neighbor_in_a_grid_is_the_pane_straight_across_not_the_first_of_the_row() {
        let tree = grid();
        assert_eq!(shape(&tree), "V[H[1 2] H[3 4]]");
        assert_eq!(tree.neighbor(&2, Direction::Down, &equal), Some(&4));
        assert_eq!(tree.neighbor(&1, Direction::Down, &equal), Some(&3));
        assert_eq!(tree.neighbor(&4, Direction::Up, &equal), Some(&2));
        assert_eq!(tree.neighbor(&3, Direction::Right, &equal), Some(&4));
        assert_eq!(tree.neighbor(&4, Direction::Left, &equal), Some(&3));
        assert_eq!(tree.neighbor(&4, Direction::Down, &equal), None);
    }

    #[test]
    fn neighbor_of_a_tall_pane_is_the_earlier_of_two_equally_close_ones() {
        // H[1 V[2 3]]: pane 1 is as tall as 2 and 3 together.
        let mut tree = PaneTree::Leaf(1);
        tree.split(&1, 2, Axis::Horizontal);
        tree.split(&2, 3, Axis::Vertical);
        assert_eq!(tree.neighbor(&1, Direction::Right, &equal), Some(&2));
        assert_eq!(tree.neighbor(&2, Direction::Left, &equal), Some(&1));
        assert_eq!(tree.neighbor(&3, Direction::Left, &equal), Some(&1));
        assert_eq!(tree.neighbor(&2, Direction::Down, &equal), Some(&3));
        assert_eq!(tree.neighbor(&1, Direction::Down, &equal), None);
    }

    #[test]
    fn neighbor_follows_resized_panes() {
        // V[H[1 2] H[3 4 5]]: with equal columns pane 1 sits over 3; widened to four fifths
        // of the row, its middle is over 4.
        let mut tree = PaneTree::Leaf(1);
        tree.split(&1, 3, Axis::Vertical);
        tree.split(&1, 2, Axis::Horizontal);
        tree.split(&3, 4, Axis::Horizontal);
        tree.split(&4, 5, Axis::Horizontal);
        assert_eq!(shape(&tree), "V[H[1 2] H[3 4 5]]");
        assert_eq!(tree.neighbor(&1, Direction::Down, &equal), Some(&3));
        let top_row = |sizes: Vec<f32>| {
            move |split: &PaneTree<u32>| {
                let is_top_row = matches!(
                    split,
                    PaneTree::Split {
                        axis: Axis::Horizontal,
                        ..
                    }
                ) && *split.first_leaf() == 1;
                is_top_row.then(|| sizes.clone())
            }
        };
        let widened = top_row(vec![800., 200.]);
        assert_eq!(tree.neighbor(&1, Direction::Down, &widened), Some(&4));
        assert_eq!(tree.neighbor(&5, Direction::Up, &widened), Some(&2));
        // Sizes that do not describe the split (stale count, an unknown zero) are ignored.
        for stale in [vec![800.], vec![800., 0.]] {
            let stale = top_row(stale);
            assert_eq!(tree.neighbor(&1, Direction::Down, &stale), Some(&3));
        }
    }

    #[test]
    fn neighbor_of_a_lone_or_missing_pane_is_none() {
        let tree = PaneTree::Leaf(1);
        assert_eq!(tree.neighbor(&1, Direction::Right, &equal), None);
        assert_eq!(grid().neighbor(&9, Direction::Right, &equal), None);
    }

    #[test]
    fn last_pane_is_not_removed() {
        let mut tree = PaneTree::Leaf(1);
        assert_eq!(tree.remove(&1), None);
        assert_eq!(shape(&tree), "1");
    }
}
