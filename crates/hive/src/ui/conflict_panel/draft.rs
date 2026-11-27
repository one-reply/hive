//! Conflict anchors and reversible edits in the marker-free result buffer.
use hive_core::git::conflict_merge::ConflictResolution;
use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Status {
    #[default]
    Pending,
    Accepted(ConflictResolution),
    Edited,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Block {
    pub range: Range<usize>,
    pub status: Status,
}

/// Multiple empty blocks can share an offset. An explicit navigation target wins that tie.
pub(super) fn block_at(
    blocks: &[Block],
    offset: usize,
    eof: usize,
    active: Option<usize>,
) -> Option<usize> {
    let owns =
        |b: &Block| b.range.contains(&offset) || (b.range.is_empty() && b.range.start == offset);
    if active.is_some_and(|ix| blocks.get(ix).is_some_and(owns)) {
        return active;
    }
    let first = blocks.partition_point(|b| b.range.end < offset);
    blocks[first..]
        .iter()
        .take_while(|b| b.range.start <= offset)
        .position(owns)
        .map(|ix| first + ix)
        .or_else(|| {
            blocks
                .last()
                .filter(|b| offset == eof && b.range.end == eof)
                .map(|_| blocks.len() - 1)
        })
}

pub(super) struct Fragment {
    pub text: String,
    blocks: Vec<Block>,
}

/// Only the intersected anchors need history. Later anchors shift by the byte delta.
pub(super) struct Change {
    pub offset: usize,
    first: usize,
    pub before: Fragment,
    pub after: Fragment,
}

impl Change {
    pub fn manual(
        blocks: &[Block],
        range: Range<usize>,
        before: String,
        after: String,
        eof: usize,
        active: Option<usize>,
    ) -> Self {
        let insertion_owner = range
            .is_empty()
            .then(|| block_at(blocks, range.start, eof, active))
            .flatten();
        let first = insertion_owner
            .unwrap_or_else(|| blocks.partition_point(|b| b.range.end <= range.start));
        let count = if insertion_owner.is_some() {
            1
        } else {
            blocks[first..].partition_point(|b| b.range.start < range.end)
        };
        let touched = &blocks[first..first + count];
        let mut updated = Vec::with_capacity(count);
        for (i, block) in touched.iter().enumerate() {
            // Replacement text belongs to the segment containing the beginning of the edit.
            // Other swallowed blocks keep distinct, ordered insertion points after it.
            let cut = range.start + after.len();
            let start = if block.range.start < range.start
                || (i == 0 && block.range.start == range.start)
            {
                block.range.start
            } else {
                cut
            };
            let end = cut + block.range.end.saturating_sub(range.end);
            updated.push(Block {
                range: start..end,
                status: Status::Edited,
            });
        }
        Self {
            offset: range.start,
            first,
            before: Fragment {
                text: before,
                blocks: touched.to_vec(),
            },
            after: Fragment {
                text: after,
                blocks: updated,
            },
        }
    }

    /// A bulk side choice replaces the span once, including the untouched text between blocks.
    pub fn span(
        first: usize,
        offset: usize,
        before: String,
        old: &[Block],
        after: String,
        new: Vec<Block>,
    ) -> Self {
        let (before, after) = if before == after {
            (String::new(), String::new())
        } else {
            (before, after)
        };
        Self {
            first,
            offset,
            before: Fragment {
                text: before,
                blocks: old.to_vec(),
            },
            after: Fragment {
                text: after,
                blocks: new,
            },
        }
    }

    pub fn bytes(&self) -> usize {
        self.before.text.len()
            + self.after.text.len()
            + (self.before.blocks.len() + self.after.blocks.len()) * size_of::<Block>()
    }

    pub fn fragments(&self, forward: bool) -> (&Fragment, &Fragment) {
        if forward {
            (&self.before, &self.after)
        } else {
            (&self.after, &self.before)
        }
    }

    pub fn apply(&self, blocks: &mut [Block], forward: bool) {
        let (from, to) = self.fragments(forward);
        let end = self.first + to.blocks.len();
        blocks[self.first..end].clone_from_slice(&to.blocks);
        if from.text.len() == to.text.len() {
            return;
        }
        for block in &mut blocks[end..] {
            block.range = (block.range.start - from.text.len() + to.text.len())
                ..(block.range.end - from.text.len() + to.text.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(range: Range<usize>) -> Block {
        Block {
            range,
            status: Status::Pending,
        }
    }

    #[test]
    fn crossing_two_conflicts_assigns_the_replacement_once_and_undo_restores_both() {
        let before = vec![pending(1..4), pending(6..9), pending(10..13)];
        let mut blocks = before.clone();
        let change = Change::manual(&blocks, 2..8, "abcdef".into(), "🙂".into(), 14, None);
        change.apply(&mut blocks, true);
        assert_eq!(
            blocks,
            vec![
                Block {
                    range: 1..6,
                    status: Status::Edited
                },
                Block {
                    range: 6..7,
                    status: Status::Edited
                },
                pending(8..11),
            ]
        );
        change.apply(&mut blocks, false);
        assert_eq!(blocks, before);
    }

    #[test]
    fn common_edits_only_shift_anchors_and_coincident_empty_blocks_are_independent() {
        let mut blocks = vec![pending(2..4), pending(6..6), pending(6..6), pending(8..10)];
        let common = Change::manual(&blocks, 4..5, "a".into(), "bbb".into(), 12, Some(0));
        common.apply(&mut blocks, true);
        assert_eq!(
            blocks,
            [pending(2..4), pending(8..8), pending(8..8), pending(10..12)]
        );
        let empty = Change::manual(&blocks, 8..8, "".into(), "é".into(), 14, Some(2));
        empty.apply(&mut blocks, true);
        assert_eq!(blocks[1], pending(8..8));
        assert_eq!(
            blocks[2],
            Block {
                range: 8..10,
                status: Status::Edited
            }
        );
        assert_eq!(blocks[3], pending(12..14));
        empty.apply(&mut blocks, false);
        common.apply(&mut blocks, false);
        assert_eq!(
            blocks,
            [pending(2..4), pending(6..6), pending(6..6), pending(8..10)]
        );
    }

    #[test]
    fn deletion_through_the_entire_file_keeps_ordered_empty_anchors_for_side_choices() {
        let mut blocks = vec![pending(2..4), pending(5..5), pending(5..7)];
        let deletion = Change::manual(&blocks, 0..8, "12345678".into(), String::new(), 8, None);
        deletion.apply(&mut blocks, true);
        assert!(
            blocks
                .iter()
                .all(|b| b.range == (0..0) && b.status == Status::Edited)
        );
        let choice = Change::span(
            1,
            0,
            String::new(),
            &blocks[1..2],
            "new".into(),
            vec![Block {
                range: 0..3,
                status: Status::Accepted(ConflictResolution::Incoming),
            }],
        );
        choice.apply(&mut blocks, true);
        assert_eq!(
            blocks.iter().map(|b| b.range.clone()).collect::<Vec<_>>(),
            [0..0, 0..3, 3..3]
        );
        choice.apply(&mut blocks, false);
        deletion.apply(&mut blocks, false);
        assert_eq!(blocks, [pending(2..4), pending(5..5), pending(5..7)]);
    }
}
