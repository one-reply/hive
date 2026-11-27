//! Read-only source rows; the result has its own continuous editor.
use hive_core::git::conflict_merge::{MergeDocument, MergeSegment, collapsed_common_range};
use std::collections::HashSet;
use std::ops::Range;

const CONTEXT_LINES: usize = 3;
const MIN_FOLD_LINES: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum DisplayRow {
    Common { segment: usize, line: usize },
    Fold { segment: usize, range: Range<usize> },
    ConflictBase { segment: usize, line: usize },
    ConflictLine { segment: usize, line: usize },
}

impl DisplayRow {
    pub fn location(&self) -> (usize, usize) {
        match self {
            Self::Fold { segment, range } => (*segment, range.start),
            Self::ConflictBase { segment, .. } => (*segment, 0),
            Self::ConflictLine { segment, line } | Self::Common { segment, line } => {
                (*segment, *line)
            }
        }
    }
}

pub(super) fn build_rows(
    doc: &MergeDocument,
    expanded: &HashSet<usize>,
    collapse: bool,
    bases: &HashSet<usize>,
) -> Vec<DisplayRow> {
    let mut rows = Vec::new();
    for (segment, part) in doc.segments.iter().enumerate() {
        match part {
            MergeSegment::Common(lines) => {
                let fold = (collapse && doc.conflict_count > 0 && !expanded.contains(&segment))
                    .then(|| {
                        collapsed_common_range(
                            lines.len(),
                            segment > 0,
                            segment + 1 < doc.segments.len(),
                            CONTEXT_LINES,
                            MIN_FOLD_LINES,
                        )
                    })
                    .flatten();
                let mut line = 0;
                while line < lines.len() {
                    if let Some(range) = fold.as_ref().filter(|r| r.start == line) {
                        rows.push(DisplayRow::Fold {
                            segment,
                            range: range.clone(),
                        });
                        line = range.end;
                    } else {
                        rows.push(DisplayRow::Common { segment, line });
                        line += 1;
                    }
                }
            }
            MergeSegment::Conflict(region) => {
                if let Some(base) = region
                    .base
                    .as_ref()
                    .filter(|_| bases.contains(&region.index))
                {
                    rows.extend(
                        (0..base.len()).map(|line| DisplayRow::ConflictBase { segment, line }),
                    );
                }
                let count = region.current.len().max(region.incoming.len()).max(1);
                rows.extend((0..count).map(|line| DisplayRow::ConflictLine { segment, line }));
            }
        }
    }
    rows
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
    fn source_folds_and_base_rows_preserve_access_to_every_side_line() {
        let common = "common\n".repeat(12);
        let incoming = "incoming\n".repeat(50);
        let text = format!(
            "{common}<<<<<<< HEAD\n||||||| base\nancestor\n=======\n{incoming}>>>>>>> topic\nend"
        );
        let doc = MergeDocument::parse(&text).unwrap();
        let folded = build_rows(&doc, &HashSet::new(), true, &HashSet::from([0]));
        assert!(matches!(&folded[0], DisplayRow::Fold { range, .. } if *range == (0..9)));
        assert_eq!(
            folded[4],
            DisplayRow::ConflictBase {
                segment: 1,
                line: 0
            }
        );
        assert_eq!(
            folded[54],
            DisplayRow::ConflictLine {
                segment: 1,
                line: 49
            }
        );
        assert_eq!(
            folded[55],
            DisplayRow::Common {
                segment: 2,
                line: 0
            }
        );
        let expanded = build_rows(&doc, &HashSet::from([0]), true, &HashSet::new());
        assert_eq!(
            expanded[11],
            DisplayRow::Common {
                segment: 0,
                line: 11
            }
        );
        assert_eq!(
            expanded[12],
            DisplayRow::ConflictLine {
                segment: 1,
                line: 0
            }
        );
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
