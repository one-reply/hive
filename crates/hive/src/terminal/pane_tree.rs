//! Layout of the panes inside one terminal tab: a tree of splits whose leaves are panes.
//! Pure data, so the split/close rules are testable without a window.

use gpui_kit::Axis;

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

    #[test]
    fn last_pane_is_not_removed() {
        let mut tree = PaneTree::Leaf(1);
        assert_eq!(tree.remove(&1), None);
        assert_eq!(shape(&tree), "1");
    }
}
