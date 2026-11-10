//! What a conflict tab shows, decided from Git's status and the working file.
use crate::state::{git::ConflictState, git_conflict::Content};
use gpui_kit::component::diff::DiffConflict;
use hive_core::git::conflicts::Conflict;

/// The working file, reduced to what the layout depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum File<'a> {
    Conflicts,
    /// Text with no conflict markers left.
    Merged,
    Malformed(&'a str),
    Unavailable(&'a str),
}

impl<'a> From<&'a Content> for File<'a> {
    fn from(content: &'a Content) -> Self {
        match content {
            Content::Text { file, .. } if file.conflicts().is_empty() => Self::Merged,
            Content::Text { .. } => Self::Conflicts,
            Content::Malformed(reason) => Self::Malformed(reason),
            Content::Unavailable(reason) => Self::Unavailable(reason),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Body {
    Loading,
    /// Conflict blocks to choose between.
    Conflicts,
    /// The marker-free file, ready to be staged.
    Merged,
    /// Nothing to resolve in place: an explanation above the two whole-file choices.
    Sides(String),
    /// Git no longer lists the file as unmerged.
    Resolved,
}

pub(super) fn body(state: ConflictState, file: Option<File>) -> Body {
    let kind = match state {
        ConflictState::Resolved => return Body::Resolved,
        ConflictState::Unmerged(kind) => Some(kind),
        ConflictState::Unknown => None,
    };
    // A file only one side still has holds no markers: staging what is on disk would pick
    // that side without saying so.
    if let Some(explanation) = kind.and_then(one_sided) {
        return Body::Sides(explanation.into());
    }
    match file {
        None => Body::Loading,
        Some(File::Conflicts) => Body::Conflicts,
        Some(File::Merged) => Body::Merged,
        Some(File::Malformed(reason)) => Body::Sides(format!(
            "The conflict markers in this file do not pair up ({reason}). Open the file to repair them, or take one side of the whole file."
        )),
        Some(File::Unavailable(reason)) => Body::Sides(format!(
            "{reason}. Take one side of the whole file to resolve it."
        )),
    }
}

/// Why a conflict has no two versions to merge line by line, if it has none.
fn one_sided(kind: Conflict) -> Option<&'static str> {
    Some(match kind {
        Conflict::BothModified | Conflict::BothAdded => return None,
        Conflict::BothDeleted => "This file was deleted on both sides.",
        Conflict::AddedByUs => "This file exists only on the current side.",
        Conflict::AddedByThem => "This file exists only on the incoming side.",
        Conflict::DeletedByUs => {
            "This file was deleted on the current side and changed on the incoming side."
        }
        Conflict::DeletedByThem => {
            "This file was changed on the current side and deleted on the incoming side."
        }
    })
}

/// How many of a file's conflicts have a chosen resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Progress {
    pub resolved: usize,
    pub total: usize,
}

impl Progress {
    pub(super) fn of(resolved: &[bool]) -> Self {
        Self {
            resolved: resolved.iter().filter(|resolved| **resolved).count(),
            total: resolved.len(),
        }
    }

    pub(super) fn is_complete(self) -> bool {
        self.resolved == self.total
    }

    pub(super) fn label(self) -> String {
        match self.total {
            0 => "No conflict markers left".into(),
            1 => format!("{} of 1 conflict resolved", self.resolved),
            total => format!("{} of {total} conflicts resolved", self.resolved),
        }
    }
}

/// The conflict an arrow moves to from `current`, wrapping around. Resolved conflicts are
/// skipped while any is left to decide.
pub(super) fn next_conflict(
    current: Option<usize>,
    resolved: &[bool],
    forward: bool,
) -> Option<usize> {
    let count = resolved.len();
    if count == 0 {
        return None;
    }
    let step = |ix: usize| {
        if forward {
            (ix + 1) % count
        } else {
            (ix + count - 1) % count
        }
    };
    let start = match current {
        Some(ix) if ix < count => step(ix),
        _ if forward => 0,
        _ => count - 1,
    };
    std::iter::successors(Some(start), |&ix| Some(step(ix)))
        .take(count)
        .find(|&ix| !resolved[ix])
        .or(Some(start))
}

/// The working-file line to scroll to for conflict `ix`.
///
/// The widget scrolls a source line to the top, and a conflict's markers are headings rather
/// than lines. The line above the opening marker keeps the heading and its buttons in view;
/// when that is the previous conflict's closing marker, the first line inside is used.
pub(super) fn anchor_line(conflicts: &[DiffConflict], ix: usize) -> Option<usize> {
    let conflict = conflicts.get(ix)?;
    let start = conflict.lines().start;
    let follows_marker = ix
        .checked_sub(1)
        .and_then(|previous| conflicts.get(previous))
        .is_some_and(|previous| previous.lines().end == start);
    if start > 1 && !follows_marker {
        return Some(start - 1);
    }
    [
        Some(conflict.current_lines()),
        conflict.base_lines(),
        Some(conflict.incoming_lines()),
    ]
    .into_iter()
    .flatten()
    .find(|lines| !lines.is_empty())
    .map(|lines| lines.start)
}

#[cfg(test)]
mod tests {
    use super::{Body, File, Progress, anchor_line, body, next_conflict};
    use crate::state::git::ConflictState;
    use gpui_kit::component::diff::DiffFile;
    use hive_core::git::conflicts::Conflict;

    #[test]
    fn body_follows_the_status_before_the_file() {
        let unmerged = ConflictState::Unmerged(Conflict::BothModified);
        assert_eq!(body(unmerged, None), Body::Loading);
        assert_eq!(body(unmerged, Some(File::Conflicts)), Body::Conflicts);
        assert_eq!(body(unmerged, Some(File::Merged)), Body::Merged);
        // Another worktree's status is no reason to hide the file.
        assert_eq!(
            body(ConflictState::Unknown, Some(File::Conflicts)),
            Body::Conflicts
        );
        // Once staged elsewhere, whatever is on disk is no longer a conflict to resolve.
        for file in [None, Some(File::Conflicts), Some(File::Merged)] {
            assert_eq!(body(ConflictState::Resolved, file), Body::Resolved);
        }
    }

    #[test]
    fn files_without_two_text_versions_offer_whole_sides_only() {
        // The surviving version has no markers; it must not look ready to stage.
        let deleted = ConflictState::Unmerged(Conflict::DeletedByThem);
        for file in [None, Some(File::Merged), Some(File::Unavailable("gone"))] {
            let Body::Sides(explanation) = body(deleted, file) else {
                panic!("expected whole-file choices for {file:?}");
            };
            assert!(explanation.contains("deleted on the incoming side"));
        }
        let unmerged = ConflictState::Unmerged(Conflict::BothAdded);
        let Body::Sides(binary) = body(unmerged, Some(File::Unavailable("Cannot edit a.png")))
        else {
            panic!("expected whole-file choices for a binary file");
        };
        assert!(binary.starts_with("Cannot edit a.png. Take one side"));
        let Body::Sides(malformed) = body(unmerged, Some(File::Malformed("patch line 3"))) else {
            panic!("expected whole-file choices for malformed markers");
        };
        assert!(malformed.contains("patch line 3") && malformed.contains("Open the file"));
    }

    #[test]
    fn progress_counts_and_names_resolved_conflicts() {
        let progress = Progress::of(&[true, false, true]);
        assert_eq!((progress.resolved, progress.total), (2, 3));
        assert!(!progress.is_complete());
        assert_eq!(progress.label(), "2 of 3 conflicts resolved");
        assert_eq!(Progress::of(&[false]).label(), "0 of 1 conflict resolved");
        assert!(Progress::of(&[true, true]).is_complete());
        // No markers at all is complete too: the file is ready to stage.
        assert!(Progress::of(&[]).is_complete());
        assert_eq!(Progress::of(&[]).label(), "No conflict markers left");
    }

    #[test]
    fn arrows_visit_unresolved_conflicts_first_and_wrap() {
        let resolved = [true, false, true, false];
        assert_eq!(next_conflict(None, &resolved, true), Some(1));
        assert_eq!(next_conflict(Some(1), &resolved, true), Some(3));
        assert_eq!(next_conflict(Some(3), &resolved, true), Some(1));
        assert_eq!(next_conflict(None, &resolved, false), Some(3));
        assert_eq!(next_conflict(Some(3), &resolved, false), Some(1));
        assert_eq!(next_conflict(Some(1), &resolved, false), Some(3));
        // With nothing left to decide the arrows still walk every conflict.
        assert_eq!(next_conflict(Some(0), &[true, true], true), Some(1));
        assert_eq!(next_conflict(Some(0), &[true, true], false), Some(1));
        assert_eq!(next_conflict(Some(0), &[false], true), Some(0));
        // A position left over from a longer list restarts from the edge.
        assert_eq!(next_conflict(Some(9), &[true, false], true), Some(1));
        assert_eq!(next_conflict(None, &[], true), None);
    }

    #[test]
    fn anchor_keeps_the_conflict_heading_in_view_when_a_line_precedes_it() {
        let text = "top\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\n<<<<<<< HEAD\n=======\nadded\n>>>>>>> feature\n<<<<<<< HEAD\n=======\n>>>>>>> feature\nshared\n<<<<<<< HEAD\nlast\n=======\n>>>>>>> feature\n";
        let file = DiffFile::parse_conflicts("a.txt", text).unwrap();
        let conflicts = file.conflicts();
        let anchors: Vec<_> = (0..conflicts.len())
            .map(|ix| anchor_line(conflicts, ix))
            .collect();
        // "top" precedes the first conflict. The second and third start right after a closing
        // marker: one has only an incoming line (9), the other no lines at all. "shared" (14)
        // precedes the last.
        assert_eq!(anchors, [Some(1), Some(9), None, Some(14)]);
        assert_eq!(anchor_line(conflicts, 9), None);
        // A conflict on the first line has nothing above it.
        let first =
            DiffFile::parse_conflicts("a.txt", "<<<<<<< HEAD\na\n=======\nb\n>>>>>>> f\n").unwrap();
        assert_eq!(anchor_line(first.conflicts(), 0), Some(2));
    }
}
