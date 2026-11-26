//! Virtualized 3-pane row projection for `MergeDocument` and conflicted-file navigation.
use hive_core::git::conflict_merge::{
    ConflictResolution, MergeDocument, MergeSegment, collapsed_common_range,
};
use std::collections::HashSet;
use std::ops::Range;

/// Unchanged lines shown above and below each conflict when collapsing unchanged blocks.
const COLLAPSED_CONTEXT_LINES: usize = 3;
/// Minimum run length before collapsing an unchanged middle span.
const MIN_COLLAPSED_LINES: usize = 4;
/// Side lines alongside the inline editor. Remaining source lines stay in the virtual list.
pub(super) const EDITOR_SIDE_LINES: usize = 8;

/// One row of the virtualized 3-column merge table.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum DisplayRow {
    /// One shared line shown identically in Left, Center, and Right.
    Common {
        segment: usize,
        line: usize,
        /// One-based line number in the Result column (updates as conflicts above resolve).
        result_line: usize,
    },
    /// Collapsed run of shared lines inside `segment`.
    Fold { segment: usize, range: Range<usize> },
    /// Header bar at the top of conflict `conflict`.
    ConflictHeader { segment: usize, conflict: usize },
    /// Optional `diff3` common-ancestor row inside conflict `conflict`.
    ConflictBase {
        segment: usize,
        conflict: usize,
        line: usize,
    },
    /// Aligned row `row` of conflict `conflict` across Left (`current`), Center (`result`),
    /// and Right (`incoming`).
    ConflictLine {
        segment: usize,
        conflict: usize,
        row: usize,
        /// Starting 1-based line number in Result for resolved lines of this conflict.
        result_start: usize,
    },
    /// Inline editor replacing `ConflictLine` rows when editing `conflict` directly in Center.
    ConflictEditor { segment: usize, conflict: usize },
}

/// Stable scroll-preservation identity for a row (ignoring shifting Result line numbers).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum RowKey {
    Common { segment: usize, line: usize },
    Fold { segment: usize, start: usize },
    ConflictHeader(usize),
    ConflictBase { conflict: usize, line: usize },
    ConflictLine { conflict: usize, row: usize },
    ConflictEditor(usize),
}

impl DisplayRow {
    pub(super) fn key(&self) -> RowKey {
        match self {
            Self::Common { segment, line, .. } => RowKey::Common {
                segment: *segment,
                line: *line,
            },
            Self::Fold { segment, range } => RowKey::Fold {
                segment: *segment,
                start: range.start,
            },
            Self::ConflictHeader { conflict, .. } => RowKey::ConflictHeader(*conflict),
            Self::ConflictBase { conflict, line, .. } => RowKey::ConflictBase {
                conflict: *conflict,
                line: *line,
            },
            Self::ConflictLine { conflict, row, .. } => RowKey::ConflictLine {
                conflict: *conflict,
                row: *row,
            },
            Self::ConflictEditor { conflict, .. } => RowKey::ConflictEditor(*conflict),
        }
    }
}

pub(super) struct ProjectedRows {
    pub rows: Vec<DisplayRow>,
    /// Row index of each conflict's `ConflictHeader`, in conflict order.
    pub conflict_rows: Vec<usize>,
    /// Precomputed Result display lines for every resolved conflict.
    pub resolved_lines: Vec<Vec<String>>,
}

/// Projects `doc` into aligned 3-column rows given current resolutions, fold state, and any
/// conflict currently open for inline editing.
pub(super) fn build_rows(
    doc: &MergeDocument,
    resolutions: &[Option<ConflictResolution>],
    expanded_folds: &HashSet<usize>,
    collapse_unchanged: bool,
    editing: Option<usize>,
    shown_bases: &HashSet<usize>,
) -> ProjectedRows {
    let mut rows = Vec::new();
    let mut conflict_rows = Vec::with_capacity(doc.conflict_count);
    let mut resolved_lines = vec![Vec::new(); doc.conflict_count];
    let mut result_line = 1usize;

    for (seg_ix, segment) in doc.segments.iter().enumerate() {
        match segment {
            MergeSegment::Common(lines) => {
                let fold = if collapse_unchanged
                    && doc.conflict_count > 0
                    && !expanded_folds.contains(&seg_ix)
                {
                    let has_above = seg_ix > 0;
                    let has_below = seg_ix + 1 < doc.segments.len();
                    collapsed_common_range(
                        lines.len(),
                        has_above,
                        has_below,
                        COLLAPSED_CONTEXT_LINES,
                        MIN_COLLAPSED_LINES,
                    )
                } else {
                    None
                };

                match fold {
                    Some(range) => {
                        for line_ix in 0..range.start {
                            rows.push(DisplayRow::Common {
                                segment: seg_ix,
                                line: line_ix,
                                result_line: result_line + line_ix,
                            });
                        }
                        rows.push(DisplayRow::Fold {
                            segment: seg_ix,
                            range: range.clone(),
                        });
                        for line_ix in range.end..lines.len() {
                            rows.push(DisplayRow::Common {
                                segment: seg_ix,
                                line: line_ix,
                                result_line: result_line + line_ix,
                            });
                        }
                    }
                    None => {
                        for line_ix in 0..lines.len() {
                            rows.push(DisplayRow::Common {
                                segment: seg_ix,
                                line: line_ix,
                                result_line: result_line + line_ix,
                            });
                        }
                    }
                }
                result_line += lines.len();
            }
            MergeSegment::Conflict(region) => {
                let ix = region.index;
                conflict_rows.push(rows.len());
                rows.push(DisplayRow::ConflictHeader {
                    segment: seg_ix,
                    conflict: ix,
                });
                if shown_bases.contains(&ix)
                    && let Some(base) = &region.base
                {
                    for line_ix in 0..base.len() {
                        rows.push(DisplayRow::ConflictBase {
                            segment: seg_ix,
                            conflict: ix,
                            line: line_ix,
                        });
                    }
                }
                let res = resolutions.get(ix).and_then(Option::as_ref);
                let center_lines = res
                    .map(|resolution| {
                        region
                            .resolution_text(resolution)
                            .lines()
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let result_start = result_line;
                result_line += center_lines.len();
                resolved_lines[ix] = center_lines;

                let side_lines = region.current.len().max(region.incoming.len());
                let visible = if editing == Some(ix) {
                    rows.push(DisplayRow::ConflictEditor {
                        segment: seg_ix,
                        conflict: ix,
                    });
                    EDITOR_SIDE_LINES..side_lines
                } else {
                    0..side_lines.max(resolved_lines[ix].len()).max(1)
                };
                for row in visible {
                    rows.push(DisplayRow::ConflictLine {
                        segment: seg_ix,
                        conflict: ix,
                        row,
                        result_start,
                    });
                }
            }
        }
    }

    ProjectedRows {
        rows,
        conflict_rows,
        resolved_lines,
    }
}

/// Returns the previous or next conflicted file path relative to `current`, wrapping around.
pub(super) fn adjacent_conflicted_file(
    current: &str,
    all: &[String],
    forward: bool,
) -> Option<String> {
    if all.len() <= 1 {
        return None;
    }
    let pos = all.iter().position(|path| path == current);
    let next = match pos {
        Some(ix) if forward => (ix + 1) % all.len(),
        Some(ix) => (ix + all.len() - 1) % all.len(),
        None => 0,
    };
    let candidate = &all[next];
    (candidate != current).then(|| candidate.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_aligned_rows_with_folds_and_inline_editor() {
        let mut lines = String::new();
        for i in 1..=12 {
            lines.push_str(&format!("common {i}\n"));
        }
        lines.push_str("<<<<<<< HEAD\ncur 1\n=======\ninc 1\ninc 2\n>>>>>>> dev\nafter\n");
        let doc = MergeDocument::parse(&lines).unwrap();
        let projected = build_rows(&doc, &[None], &HashSet::new(), true, None, &HashSet::new());

        // Top 12 lines fold 0..9, keep 9..12 (3 context lines above conflict).
        assert!(matches!(
            projected.rows[0],
            DisplayRow::Fold {
                segment: 0,
                range: Range { start: 0, end: 9 }
            }
        ));
        assert_eq!(projected.conflict_rows, vec![4]);
        // Left has 1 line, Right has 2 lines -> 2 aligned ConflictLine rows.
        assert!(matches!(
            projected.rows[4],
            DisplayRow::ConflictHeader { conflict: 0, .. }
        ));
        assert!(matches!(
            projected.rows[5],
            DisplayRow::ConflictLine {
                conflict: 0,
                row: 0,
                result_start: 13,
                ..
            }
        ));
        assert!(matches!(
            projected.rows[6],
            DisplayRow::ConflictLine {
                conflict: 0,
                row: 1,
                ..
            }
        ));
        assert!(matches!(
            projected.rows[7],
            DisplayRow::Common {
                segment: 2,
                line: 0,
                result_line: 13
            }
        ));

        // When resolved with BothCurrentFirst (3 lines), Common below shifts to line 16.
        let resolved = build_rows(
            &doc,
            &[Some(ConflictResolution::BothCurrentFirst)],
            &HashSet::new(),
            true,
            None,
            &HashSet::new(),
        );
        assert_eq!(resolved.resolved_lines[0], vec!["cur 1", "inc 1", "inc 2"]);
        assert!(matches!(
            resolved.rows.last(),
            Some(DisplayRow::Common {
                result_line: 16,
                ..
            })
        ));

        // When editing conflict 0, ConflictLine rows collapse into one ConflictEditor row.
        let editing = build_rows(
            &doc,
            &[None],
            &HashSet::new(),
            true,
            Some(0),
            &HashSet::new(),
        );
        assert!(matches!(
            editing.rows[5],
            DisplayRow::ConflictEditor { conflict: 0, .. }
        ));
    }

    #[test]
    fn editing_a_large_conflict_keeps_remaining_side_lines_virtualized() {
        let text = format!(
            "<<<<<<< HEAD\n{}=======\nb\n>>>>>>> develop\n",
            "a\n".repeat(EDITOR_SIDE_LINES + 2)
        );
        let doc = MergeDocument::parse(&text).unwrap();
        let projected = build_rows(
            &doc,
            &[None],
            &HashSet::new(),
            true,
            Some(0),
            &HashSet::new(),
        );
        assert!(matches!(
            projected.rows.as_slice(),
            [
                DisplayRow::ConflictHeader { .. },
                DisplayRow::ConflictEditor { .. },
                DisplayRow::ConflictLine {
                    row: EDITOR_SIDE_LINES,
                    ..
                },
                DisplayRow::ConflictLine { .. },
            ]
        ));
    }

    #[test]
    fn adjacent_conflicted_file_cycles_across_other_files() {
        let files = vec!["a.rs".into(), "b.rs".into(), "c.rs".into()];
        assert_eq!(
            adjacent_conflicted_file("a.rs", &files, true),
            Some("b.rs".into())
        );
        assert_eq!(
            adjacent_conflicted_file("a.rs", &files, false),
            Some("c.rs".into())
        );
        assert_eq!(
            adjacent_conflicted_file("a.rs", &["a.rs".into()], true),
            None
        );
    }
}
