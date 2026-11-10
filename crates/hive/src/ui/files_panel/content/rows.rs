//! The content results list as plain data: its rows, keyboard moves and status line.
use crate::state::files::ContentStatus;
use hive_core::files::content_search::{FileMatches, MAX_MATCHED_LINES};
use std::{collections::HashSet, path::PathBuf};

/// One list row, pointing into the results it was built from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    File { file: usize },
    Line { file: usize, line: usize },
}

impl Row {
    pub(super) fn file(self) -> usize {
        match self {
            Self::File { file } | Self::Line { file, .. } => file,
        }
    }
}

/// A file header per result, followed by its lines unless the file is collapsed.
pub(super) fn rows(results: &[FileMatches], collapsed: &HashSet<PathBuf>) -> Vec<Row> {
    let mut rows = Vec::new();
    for (file, matches) in results.iter().enumerate() {
        rows.push(Row::File { file });
        if !collapsed.contains(&matches.path) {
            rows.extend((0..matches.lines.len()).map(|line| Row::Line { file, line }));
        }
    }
    rows
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Effect {
    Select(usize),
    Collapse { file: usize },
    Expand { file: usize },
    FocusInput,
    None,
}

/// What an arrow key does: Up from the top returns to the field; Left on a line climbs to
/// its file, Left on a file collapses it; Right expands a file or enters its first line.
pub(super) fn navigate(
    rows: &[Row],
    is_collapsed: impl Fn(usize) -> bool,
    selected: Option<usize>,
    key: Key,
) -> Effect {
    let selected = selected.filter(|&ix| ix < rows.len());
    let Some(ix) = selected else {
        return match key {
            Key::Down if !rows.is_empty() => Effect::Select(0),
            Key::Up => Effect::FocusInput,
            _ => Effect::None,
        };
    };
    match (key, rows[ix]) {
        (Key::Up, _) if ix == 0 => Effect::FocusInput,
        (Key::Up, _) => Effect::Select(ix - 1),
        (Key::Down, _) => Effect::Select((ix + 1).min(rows.len() - 1)),
        (Key::Left, Row::Line { file, .. }) => rows
            .iter()
            .position(|row| *row == Row::File { file })
            .map_or(Effect::None, Effect::Select),
        (Key::Left, Row::File { file }) if !is_collapsed(file) => Effect::Collapse { file },
        (Key::Right, Row::File { file }) if is_collapsed(file) => Effect::Expand { file },
        (Key::Right, Row::File { file }) => match rows.get(ix + 1) {
            Some(Row::Line { file: next, .. }) if *next == file => Effect::Select(ix + 1),
            _ => Effect::None,
        },
        _ => Effect::None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Muted,
    Danger,
}

/// The line under the field describing the search; `lines` and `files` count the results.
pub(super) fn status_text(
    status: &ContentStatus,
    query: &str,
    lines: usize,
    files: usize,
) -> (String, Tone) {
    let found = || {
        format!(
            "{} {} in {} {}",
            grouped(lines),
            plural(lines, "result", "results"),
            grouped(files),
            plural(files, "file", "files"),
        )
    };
    match status {
        ContentStatus::Idle => ("Search the contents of every file".into(), Tone::Muted),
        ContentStatus::Searching if lines == 0 => ("Searching…".into(), Tone::Muted),
        ContentStatus::Searching => (found(), Tone::Muted),
        ContentStatus::Done { truncated, .. } if *truncated => (
            format!(
                "Showing the first {} results. Narrow the search to see more.",
                grouped(MAX_MATCHED_LINES)
            ),
            Tone::Muted,
        ),
        ContentStatus::Done {
            index_truncated, ..
        } if lines == 0 => {
            let mut text = format!("No results for “{query}”");
            if *index_truncated {
                text.push_str(" in the first 100,000 files");
            }
            (text, Tone::Muted)
        }
        ContentStatus::Done {
            index_truncated, ..
        } => {
            let mut text = found();
            if *index_truncated {
                text.push_str(" · first 100,000 files searched");
            }
            (text, Tone::Muted)
        }
        ContentStatus::Failed(error) => (error.clone(), Tone::Danger),
        ContentStatus::Remote => (
            "Content search is not available for SSH projects yet.".into(),
            Tone::Muted,
        ),
    }
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

/// `5000` as `5,000`.
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::files::content_search::LineMatch;

    fn file(path: &str, lines: usize) -> FileMatches {
        FileMatches {
            path: path.into(),
            lines: (0..lines)
                .map(|line| LineMatch {
                    line: line as u32,
                    column: 0,
                    preview: String::new(),
                    ranges: Vec::new(),
                })
                .collect(),
        }
    }

    fn sample() -> Vec<Row> {
        rows(&[file("a", 2), file("b", 1)], &HashSet::new())
    }

    #[test]
    fn collapsed_files_keep_only_their_header() {
        let results = [file("a", 2), file("b", 1)];
        let collapsed = HashSet::from([PathBuf::from("a")]);
        assert_eq!(
            rows(&results, &collapsed),
            [
                Row::File { file: 0 },
                Row::File { file: 1 },
                Row::Line { file: 1, line: 0 }
            ]
        );
        assert_eq!(sample().len(), 5);
    }

    #[test]
    fn vertical_moves_enter_and_leave_the_list() {
        let rows = sample();
        let open = |_| false;
        assert_eq!(navigate(&rows, open, None, Key::Down), Effect::Select(0));
        assert_eq!(navigate(&[], open, None, Key::Down), Effect::None);
        assert_eq!(navigate(&rows, open, Some(0), Key::Up), Effect::FocusInput);
        assert_eq!(navigate(&rows, open, Some(2), Key::Up), Effect::Select(1));
        assert_eq!(navigate(&rows, open, Some(4), Key::Down), Effect::Select(4));
        assert_eq!(navigate(&rows, open, Some(9), Key::Down), Effect::Select(0));
    }

    #[test]
    fn horizontal_moves_fold_and_climb() {
        let rows = sample();
        let open = |_| false;
        assert_eq!(navigate(&rows, open, Some(2), Key::Left), Effect::Select(0));
        assert_eq!(
            navigate(&rows, open, Some(0), Key::Left),
            Effect::Collapse { file: 0 }
        );
        assert_eq!(navigate(&rows, |_| true, Some(0), Key::Left), Effect::None);
        assert_eq!(
            navigate(&rows, |_| true, Some(0), Key::Right),
            Effect::Expand { file: 0 }
        );
        assert_eq!(
            navigate(&rows, open, Some(0), Key::Right),
            Effect::Select(1)
        );
        assert_eq!(navigate(&rows, open, Some(1), Key::Right), Effect::None);
    }

    #[test]
    fn status_describes_each_state() {
        let status =
            |status: ContentStatus, lines, files| status_text(&status, "foo", lines, files);
        let done = |truncated, index_truncated| ContentStatus::Done {
            truncated,
            index_truncated,
        };
        assert_eq!(status(done(false, false), 1, 1).0, "1 result in 1 file");
        assert_eq!(
            status(done(false, false), 1234, 3).0,
            "1,234 results in 3 files"
        );
        assert_eq!(status(done(false, false), 0, 0).0, "No results for “foo”");
        assert_eq!(
            status(done(true, false), 5000, 9).0,
            "Showing the first 5,000 results. Narrow the search to see more."
        );
        assert!(
            status(done(false, true), 2, 1)
                .0
                .ends_with("first 100,000 files searched")
        );
        assert_eq!(status(ContentStatus::Searching, 0, 0).0, "Searching…");
        assert_eq!(
            status(ContentStatus::Searching, 2, 1).0,
            "2 results in 1 file"
        );
        assert_eq!(
            status(ContentStatus::Failed("bad".into()), 0, 0),
            ("bad".into(), Tone::Danger)
        );
        assert_eq!(status(ContentStatus::Remote, 0, 0).1, Tone::Muted);
        assert_eq!(grouped(1_000_000), "1,000,000");
        assert_eq!(grouped(999), "999");
    }
}
