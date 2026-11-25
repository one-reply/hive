//! Bounded, deterministic context windows shared by search, references and diagnostics.
use std::{collections::BTreeMap, ops::Range, path::PathBuf};

/// Maximum input locations retained in an excerpt snapshot.
pub const MAX_HITS: usize = 5_000;
const CONTEXT: u32 = 3;

/// One source location and an optional diagnostic explanation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Absolute path on the query's host.
    pub path: PathBuf,
    /// Zero-based buffer line.
    pub line: u32,
    /// Scalar column of the match, or zero for a line-level result.
    pub column: u32,
    /// Empty for references and text matches.
    pub message: String,
}

/// A file's merged, half-open context windows in source order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// Absolute path on the query's host.
    pub path: PathBuf,
    /// Sorted matching lines, including messages.
    pub hits: Vec<Hit>,
    /// Disjoint windows; the reader clips these against the actual source length.
    pub windows: Vec<Range<u32>>,
}

/// Group locations by path, coalescing overlapping or adjacent context windows.
/// Returns whether the caller must narrow the query to see all locations.
pub fn group(mut hits: Vec<Hit>) -> (Vec<File>, bool) {
    hits.sort_by(|a, b| {
        (&a.path, a.line, a.column, &a.message).cmp(&(&b.path, b.line, b.column, &b.message))
    });
    hits.dedup();
    let truncated = hits.len() > MAX_HITS;
    hits.truncate(MAX_HITS);
    let mut files = BTreeMap::<PathBuf, File>::new();
    for hit in hits {
        let file = files.entry(hit.path.clone()).or_insert_with(|| File {
            path: hit.path.clone(),
            hits: Vec::new(),
            windows: Vec::new(),
        });
        let range = hit.line.saturating_sub(CONTEXT)..hit.line.saturating_add(CONTEXT + 1);
        if let Some(last) = file
            .windows
            .last_mut()
            .filter(|last| last.end >= range.start)
        {
            last.end = last.end.max(range.end);
        } else {
            file.windows.push(range);
        }
        file.hits.push(hit);
    }
    (files.into_values().collect(), truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hit(path: &str, line: u32) -> Hit {
        Hit {
            path: path.into(),
            line,
            column: 0,
            message: String::new(),
        }
    }
    #[test]
    fn merges_context_sorts_files_and_deduplicates_hits() {
        let (files, truncated) = group(vec![
            hit("b", 30),
            hit("a", 6),
            hit("a", 0),
            hit("a", 6),
            hit("a", 20),
        ]);
        assert!(!truncated);
        assert_eq!(files[0].path, PathBuf::from("a"));
        assert_eq!(files[0].windows, vec![0..10, 17..24]);
        assert_eq!(files[0].hits.len(), 3);
        assert_eq!(files[1].windows, vec![27..34]);
    }
    #[test]
    fn bounds_large_results_and_untrusted_line_numbers() {
        let (files, truncated) = group((0..MAX_HITS + 1).map(|i| hit("a", i as u32)).collect());
        assert!(truncated);
        assert_eq!(files[0].hits.len(), MAX_HITS);
        assert_eq!(
            group(vec![hit("a", u32::MAX)]).0[0].windows,
            vec![u32::MAX - 3..u32::MAX]
        );
        assert!(group(Vec::new()).0.is_empty());
    }
}
