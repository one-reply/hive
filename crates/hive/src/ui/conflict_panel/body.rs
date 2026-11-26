//! What a conflict tab shows, decided from Git's status and the working file.
use crate::state::{git::ConflictState, git_conflict::Content};
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
            Content::Text { document, .. } if document.conflict_count == 0 => Self::Merged,
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

#[cfg(test)]
mod tests {
    use super::{Body, File, Progress, body, next_conflict};
    use crate::state::git::ConflictState;
    use hive_core::git::conflicts::Conflict;

    #[test]
    fn body_follows_the_status_before_the_file() {
        let unmerged = ConflictState::Unmerged(Conflict::BothModified);
        assert_eq!(body(unmerged, None), Body::Loading);
        assert_eq!(body(unmerged, Some(File::Conflicts)), Body::Conflicts);
        assert_eq!(body(unmerged, Some(File::Merged)), Body::Merged);
        assert_eq!(
            body(ConflictState::Unknown, Some(File::Conflicts)),
            Body::Conflicts
        );
        for file in [None, Some(File::Conflicts), Some(File::Merged)] {
            assert_eq!(body(ConflictState::Resolved, file), Body::Resolved);
        }
    }

    #[test]
    fn files_without_two_text_versions_offer_whole_sides_only() {
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
        assert_eq!(next_conflict(Some(0), &[true, true], true), Some(1));
        assert_eq!(next_conflict(Some(0), &[true, true], false), Some(1));
        assert_eq!(next_conflict(Some(0), &[false], true), Some(0));
        assert_eq!(next_conflict(Some(9), &[true, false], true), Some(1));
        assert_eq!(next_conflict(None, &[], true), None);
    }
}
