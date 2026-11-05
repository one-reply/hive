//! Accordion motion for folders opening and closing.
//!
//! The library tree is a `uniform_list`: every row has one height and a folder cannot grow.
//! Instead, when a block of rows appears or disappears, the rows after it slide from where
//! they were to where they are now. On expand they start over the new children and uncover
//! them while the children fade in; on collapse they rise into the freed space. The block is
//! found by diffing the visible row ids, so a click, a key, Reveal and a listing that arrives
//! later all animate the same way.
use gpui_kit::SharedString;
use gpui_kit::component::tree::TreeState;
use std::time::{Duration, Instant};

const DURATION: Duration = Duration::from_millis(180);

/// How one row is displaced at the frame being rendered.
pub(super) struct RowMotion {
    /// Vertical offset from the row's final position, in rows.
    pub offset: f32,
    pub opacity: f32,
}

#[derive(Default)]
pub(super) struct TreeMotion {
    rows: Vec<SharedString>,
    block: Option<ActiveBlock>,
}

/// Rows `start..start + delta` were inserted (`delta > 0`) or `-delta` rows were removed at
/// `start` (`delta < 0`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Block {
    start: usize,
    delta: isize,
}

impl TreeMotion {
    /// Snapshots the visible rows and starts a motion when exactly one block changed.
    pub(super) fn observe(&mut self, tree: &TreeState, reduce_motion: bool) {
        let rows: Vec<SharedString> = (0..)
            .map_while(|ix| tree.entry(ix).map(|entry| entry.item().id.clone()))
            .collect();
        if rows == self.rows {
            return;
        }
        // A first listing or a switched worktree is a new tree, not a folder moving.
        let block = (!reduce_motion && !self.rows.is_empty())
            .then(|| changed_block(&self.rows, &rows))
            .flatten();
        self.rows = rows;
        self.block = block.map(|block| ActiveBlock {
            block,
            started: Instant::now(),
        });
    }

    /// The displacement of row `ix`, or `None` once it rests at its final position.
    pub(super) fn sample(&self, ix: usize) -> Option<RowMotion> {
        let active = self.block.as_ref()?;
        let elapsed = active.started.elapsed();
        if elapsed >= DURATION {
            return None;
        }
        let progress = ease_out_cubic(elapsed.as_secs_f32() / DURATION.as_secs_f32());
        let Block { start, delta } = active.block;
        let rows = delta.unsigned_abs();
        let remaining = 1. - progress;
        match delta.signum() {
            1 if (start..start + rows).contains(&ix) => Some(RowMotion {
                offset: 0.,
                opacity: progress,
            }),
            1 if ix >= start + rows => Some(RowMotion {
                offset: -(rows as f32) * remaining,
                opacity: 1.,
            }),
            -1 if ix >= start => Some(RowMotion {
                offset: rows as f32 * remaining,
                opacity: 1.,
            }),
            _ => None,
        }
    }
}

struct ActiveBlock {
    block: Block,
    started: Instant,
}

/// The single contiguous insertion or removal that turns `old` into `new`, if that is all
/// that changed.
fn changed_block(old: &[SharedString], new: &[SharedString]) -> Option<Block> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let delta = new.len() as isize - old.len() as isize;
    let unchanged = prefix + suffix;
    let pure = (delta > 0 && unchanged == old.len()) || (delta < 0 && unchanged == new.len());
    pure.then_some(Block {
        start: prefix,
        delta,
    })
}

fn ease_out_cubic(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(ids: &[&'static str]) -> Vec<SharedString> {
        ids.iter().map(|id| SharedString::from(*id)).collect()
    }

    #[test]
    fn expanding_a_folder_is_an_insertion_after_it() {
        let old = rows(&["a", "b", "c"]);
        let new = rows(&["a", "a/1", "a/2", "b", "c"]);
        assert_eq!(
            changed_block(&old, &new),
            Some(Block { start: 1, delta: 2 })
        );
    }

    #[test]
    fn collapsing_the_last_folder_is_a_removal_at_the_end() {
        let old = rows(&["a", "c", "c/1", "c/2"]);
        let new = rows(&["a", "c"]);
        assert_eq!(
            changed_block(&old, &new),
            Some(Block {
                start: 2,
                delta: -2
            })
        );
    }

    #[test]
    fn repeated_ids_resolve_to_one_block() {
        let old = rows(&["a", "a"]);
        let new = rows(&["a", "a", "a"]);
        assert_eq!(
            changed_block(&old, &new),
            Some(Block { start: 2, delta: 1 })
        );
    }

    #[test]
    fn rename_or_unrelated_changes_do_not_animate() {
        assert_eq!(changed_block(&rows(&["a", "b"]), &rows(&["a", "x"])), None);
        assert_eq!(
            changed_block(&rows(&["a", "b", "c"]), &rows(&["x", "b", "y", "z"])),
            None
        );
    }
}
