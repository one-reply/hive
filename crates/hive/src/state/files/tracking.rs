//! What the navigator takes from Git's answer about its worktree: the badges of its files
//! and where a listing learns which paths are ignored.
use crate::state::git::Tracking;
use hive_core::files::decorations::{self, Decoration};
use std::{collections::BTreeMap, path::PathBuf};

/// The badges `tracking` gives, or `None` while it cannot tell.
pub(super) fn badges(tracking: &Tracking<'_>) -> Option<BTreeMap<PathBuf, Decoration>> {
    match tracking {
        Tracking::Unknown => None,
        Tracking::NoRepository | Tracking::Unreadable => Some(BTreeMap::new()),
        Tracking::Status(status) => Some(decorations::collect(&status.files)),
    }
}

/// Where a listing learns which of its paths Git ignores.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum IgnoredPaths {
    /// Git has not answered for this worktree yet: the paths ignored so far stay ignored.
    Unchanged,
    /// Git has nothing to say about these files, so none of them is ignored.
    Nothing,
    /// The worktree is a repository: Git is asked about the listed paths.
    FromGit,
}

impl IgnoredPaths {
    pub(super) fn of(tracking: &Tracking<'_>) -> Self {
        match tracking {
            Tracking::Unknown => Self::Unchanged,
            Tracking::NoRepository | Tracking::Unreadable => Self::Nothing,
            Tracking::Status(_) => Self::FromGit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::git::status::{Change, FileChange, Status};

    fn status_with_a_modified_file() -> Status {
        Status {
            files: vec![FileChange {
                path: "src/lib.rs".into(),
                old_path: None,
                staged: None,
                unstaged: Some(Change::Modified),
                submodule: false,
                conflict: None,
                staged_stats: None,
                unstaged_stats: None,
            }],
            ..Status::default()
        }
    }

    #[test]
    fn badges_wait_for_the_worktrees_own_status() {
        assert_eq!(badges(&Tracking::Unknown), None);
        assert_eq!(
            IgnoredPaths::of(&Tracking::Unknown),
            IgnoredPaths::Unchanged
        );
    }

    #[test]
    fn a_status_badges_the_changed_file_and_its_folders() {
        let status = status_with_a_modified_file();
        let tracking = Tracking::Status(&status);
        let badges = badges(&tracking).expect("a status always answers");
        assert!(badges.contains_key(&PathBuf::from("src/lib.rs")));
        assert!(badges.contains_key(&PathBuf::from("src")));
        assert_eq!(IgnoredPaths::of(&tracking), IgnoredPaths::FromGit);
    }

    #[test]
    fn a_clean_status_clears_the_badges() {
        let status = Status::default();
        assert_eq!(badges(&Tracking::Status(&status)), Some(BTreeMap::new()));
    }

    #[test]
    fn a_folder_git_cannot_describe_has_no_badges_and_no_ignored_paths() {
        for tracking in [Tracking::NoRepository, Tracking::Unreadable] {
            assert_eq!(badges(&tracking), Some(BTreeMap::new()));
            assert_eq!(IgnoredPaths::of(&tracking), IgnoredPaths::Nothing);
        }
    }
}
