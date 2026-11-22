//! Keeps the Changes list where the user scrolled it while its rows come and go.
use gpui_kit::{ListOffset, ListState};
use std::collections::HashMap;
use std::hash::Hash;

/// Shows the rows `new` in `list` in place of `old`, keeping in view what was in view.
///
/// `ListState::reset` alone forgets the scroll position and shows the first row, so staging
/// one file far down the list would throw the user back to its start.
pub(super) fn replace_rows<K: Hash + Eq>(list: &ListState, old: &[K], new: &[K]) {
    let top = after_change(old, new, list.logical_scroll_top());
    list.reset(new.len());
    list.scroll_to(top);
}

/// Where the list is scrolled once `new` replaces `old`, having been at `top`.
///
/// - A list at its start stays there, so a group that appears above the first row (Staged,
///   after the first file is staged) is seen rather than scrolled past.
/// - The row at the top stays at the top when it is still listed, whatever was added or
///   removed above it.
/// - A top row that is gone gives its place to the next row that is still listed.
/// - With none of them left the position is kept by index, which the list clamps to its end.
fn after_change<K: Hash + Eq>(old: &[K], new: &[K], top: ListOffset) -> ListOffset {
    if top == ListOffset::default() || top.item_ix >= old.len() {
        return ListOffset::default();
    }
    let position: HashMap<&K, usize> = new.iter().enumerate().map(|(ix, key)| (key, ix)).collect();
    if let Some(&item_ix) = position.get(&old[top.item_ix]) {
        return ListOffset { item_ix, ..top };
    }
    old[top.item_ix + 1..]
        .iter()
        .find_map(|key| position.get(key))
        .map_or(top, |&item_ix| ListOffset {
            item_ix,
            ..ListOffset::default()
        })
}

#[cfg(test)]
mod tests {
    use super::{after_change, replace_rows};
    use gpui_kit::{ListAlignment, ListOffset, ListState, px};

    /// A list row: a group header, or a file of a group.
    type Row = (&'static str, Option<&'static str>);

    const STAGED: Row = ("Staged", None);
    const CHANGES: Row = ("Changes", None);

    fn file(group: &'static str, path: &'static str) -> Row {
        (group, Some(path))
    }

    fn at(item_ix: usize, offset: f32) -> ListOffset {
        ListOffset {
            item_ix,
            offset_in_item: px(offset),
        }
    }

    fn changes() -> Vec<Row> {
        vec![
            CHANGES,
            file("Changes", "a"),
            file("Changes", "b"),
            file("Changes", "c"),
            file("Changes", "d"),
        ]
    }

    #[test]
    fn the_row_at_the_top_stays_there_when_rows_above_it_come_and_go() {
        let old = changes();
        // `a` was staged: a header and a row appeared above, `a` left the group.
        let staged = vec![
            STAGED,
            file("Staged", "a"),
            CHANGES,
            file("Changes", "b"),
            file("Changes", "c"),
            file("Changes", "d"),
        ];
        assert_eq!(after_change(&old, &staged, at(3, 7.)), at(4, 7.));
        // And back again once it is unstaged.
        assert_eq!(after_change(&staged, &old, at(4, 7.)), at(3, 7.));
        // An agent touched a file that sorts above the viewport.
        let mut grown = changes();
        grown.insert(1, file("Changes", "0"));
        assert_eq!(after_change(&old, &grown, at(2, 0.)), at(3, 0.));
    }

    #[test]
    fn a_top_row_that_is_gone_gives_its_place_to_the_next_one_still_listed() {
        let old = changes();
        let mut discarded = changes();
        discarded.remove(2);
        // `b` was at the top, half scrolled away; `c` takes its place from its first pixel.
        assert_eq!(after_change(&old, &discarded, at(2, 9.)), at(2, 0.));
        // `b` and `c` both went: `d` is the next one left.
        discarded.remove(2);
        assert_eq!(after_change(&old, &discarded, at(2, 9.)), at(2, 0.));
    }

    #[test]
    fn a_list_at_its_start_stays_at_its_start() {
        let old = changes();
        let staged = vec![STAGED, file("Staged", "a"), CHANGES, file("Changes", "b")];
        // The new Staged group must be seen, not scrolled past to keep `Changes` on top.
        assert_eq!(after_change(&old, &staged, at(0, 0.)), at(0, 0.));
        // Nothing to compare with: the first rows of a list, or another worktree's.
        assert_eq!(after_change(&[], &staged, at(3, 4.)), at(0, 0.));
        assert_eq!(after_change(&old, &[], at(3, 4.)), at(3, 4.));
    }

    #[test]
    fn with_nothing_left_below_the_position_is_kept_by_index() {
        // Stage All from the middle of Changes: every row of the group moves into Staged.
        let old = vec![
            STAGED,
            file("Staged", "s"),
            CHANGES,
            file("Changes", "a"),
            file("Changes", "b"),
        ];
        let all_staged = vec![
            STAGED,
            file("Staged", "s"),
            file("Staged", "a"),
            file("Staged", "b"),
        ];
        assert_eq!(after_change(&old, &all_staged, at(3, 5.)), at(3, 5.));
    }

    #[test]
    fn replacing_the_rows_of_a_list_does_not_send_it_back_to_its_start() {
        let old = changes();
        let list = ListState::new(old.len(), ListAlignment::Top, px(26.));
        list.scroll_to(at(3, 0.));
        let mut staged = changes();
        staged.remove(1);
        staged.splice(0..0, [STAGED, file("Staged", "a")]);

        replace_rows(&list, &old, &staged);
        assert_eq!(list.item_count(), staged.len());
        // `c` was on top and still is, one row further down the list.
        assert_eq!(list.logical_scroll_top(), at(4, 0.));
    }
}
