//! Whether this session may write `state.json`, decided once at startup, and what the user is
//! told when it may not or when the file could not be used as written.

use hive_core::state::{Access, LoadedState, Recovery, RecoveryCause, State, StateError};

/// The state to show and whether this session saves it.
pub(super) struct Opened {
    pub(super) state: State,
    pub(super) notice: Option<String>,
    /// Kept while this session saves; `None` leaves `state.json` untouched for the session.
    pub(super) access: Option<Access>,
    /// Whether this session keeps editor drafts in the data folder; see [`keeps_drafts`].
    pub(super) keeps_drafts: bool,
}

/// Whether a session with this access keeps editor drafts in the data folder. In a folder
/// another Hive holds the drafts are that Hive's: a second copy that adopted them would show
/// someone else's unsaved edits, and one that wrote there would replace or remove them.
pub(super) fn keeps_drafts(access: &Access) -> bool {
    access.can_write()
}

/// A failed load, a file from a newer Hive and a folder another Hive holds all leave the file
/// as it is: saving would replace data this session cannot see or understand.
pub(super) fn open(loaded: Result<LoadedState, StateError>, access: Access) -> Opened {
    let keeps_drafts = keeps_drafts(&access);
    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(error) => {
            tracing::error!(%error, "state loading failed; persistence disabled");
            return Opened {
                state: State::default(),
                notice: Some(format!(
                    "{error}. Saving is disabled to protect the original state. Restart Hive after fixing the file."
                )),
                access: None,
                keeps_drafts,
            };
        }
    };
    let recovery = loaded.recovery.as_ref().map(recovery_notice);
    if matches!(
        loaded.recovery,
        Some(Recovery {
            cause: RecoveryCause::Newer { .. },
            ..
        })
    ) {
        tracing::warn!("state written by a newer Hive; persistence disabled");
        return Opened {
            state: loaded.state,
            notice: recovery,
            access: None,
            keeps_drafts,
        };
    }
    if !access.can_write() {
        return Opened {
            state: loaded.state,
            notice: Some(join(recovery, HELD_NOTICE)),
            access: None,
            keeps_drafts,
        };
    }
    Opened {
        state: loaded.state,
        notice: recovery,
        access: Some(access),
        keeps_drafts,
    }
}

const HELD_NOTICE: &str = "Another Hive is already running with this data folder. Changes made in this window are not saved, so they cannot overwrite its state, and unsaved edits are not kept for recovery. Quit this copy and keep using the other one.";

fn join(first: Option<String>, second: &str) -> String {
    match first {
        Some(first) => format!("{first}\n{second}"),
        None => second.to_owned(),
    }
}

/// What the user is told when `state.json` could not be used as written.
fn recovery_notice(recovery: &Recovery) -> String {
    let backup = recovery.backup.display();
    match &recovery.cause {
        RecoveryCause::Corrupt => format!(
            "State was damaged. The original was preserved at {backup}. Default settings are in use."
        ),
        RecoveryCause::Partial(lost) => format!(
            "Part of the saved state could not be read and was reset: {}. The original was preserved at {backup}.",
            lost.join(", ")
        ),
        RecoveryCause::Newer { version } => format!(
            "The saved state was written by a newer Hive (format {version}). This version does not save changes, so the newer Hive's data stays intact. A copy is at {backup}. Update Hive, or move state.json aside and restart to start over with this version."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(cause: Option<RecoveryCause>) -> Result<LoadedState, StateError> {
        let mut state = State::default();
        state.last_seen_release = Some("1.0.31".into());
        Ok(LoadedState {
            state,
            recovery: cause.map(|cause| Recovery {
                backup: "/backup/state.original.json".into(),
                cause,
            }),
        })
    }

    #[test]
    fn a_file_from_a_newer_hive_is_shown_but_never_saved_over() {
        let opened = open(
            loaded(Some(RecoveryCause::Newer { version: 7 })),
            Access::Unguarded,
        );
        assert!(opened.access.is_none(), "saving would downgrade the file");
        assert_eq!(opened.state.last_seen_release.as_deref(), Some("1.0.31"));
        let notice = opened.notice.unwrap();
        assert!(notice.contains("format 7"), "{notice}");
        assert!(notice.contains("does not save changes"), "{notice}");
        assert!(notice.contains("/backup/state.original.json"), "{notice}");
    }

    #[test]
    fn a_folder_another_hive_holds_is_shown_read_only_with_the_reason() {
        let opened = open(loaded(None), Access::Held);
        assert!(opened.access.is_none());
        assert_eq!(opened.state.last_seen_release.as_deref(), Some("1.0.31"));
        assert_eq!(opened.notice.as_deref(), Some(HELD_NOTICE));

        let partial = open(
            loaded(Some(RecoveryCause::Partial(vec!["settings".into()]))),
            Access::Held,
        );
        let notice = partial.notice.unwrap();
        assert!(notice.starts_with("Part of the saved state"), "{notice}");
        assert!(notice.ends_with(HELD_NOTICE), "{notice}");
    }

    #[test]
    fn editor_drafts_belong_to_the_hive_that_holds_the_folder() {
        let unreadable = || {
            Err(StateError::Io {
                operation: "read",
                path: "/state.json".into(),
                source: std::io::Error::other("denied"),
            })
        };
        let newer = || loaded(Some(RecoveryCause::Newer { version: 7 }));
        // Another Hive's drafts are neither adopted nor replaced, whatever else is wrong.
        assert!(!open(loaded(None), Access::Held).keeps_drafts);
        assert!(!open(unreadable(), Access::Held).keeps_drafts);
        assert!(!open(newer(), Access::Held).keeps_drafts);
        // A state file this session will not save still leaves the drafts its own.
        assert!(open(loaded(None), Access::Unguarded).keeps_drafts);
        assert!(open(unreadable(), Access::Unguarded).keeps_drafts);
        assert!(open(newer(), Access::Unguarded).keeps_drafts);
    }

    #[test]
    fn a_readable_or_partly_reset_file_is_saved_by_the_lock_holder() {
        assert!(open(loaded(None), Access::Unguarded).access.is_some());
        let partial = open(
            loaded(Some(RecoveryCause::Partial(vec!["settings".into()]))),
            Access::Unguarded,
        );
        assert!(partial.access.is_some());
        assert!(partial.notice.unwrap().contains("settings"));
    }

    #[test]
    fn a_failed_load_is_never_saved() {
        let opened = open(
            Err(StateError::Io {
                operation: "read",
                path: "/state.json".into(),
                source: std::io::Error::other("denied"),
            }),
            Access::Unguarded,
        );
        assert!(opened.access.is_none());
        assert!(opened.notice.unwrap().contains("Saving is disabled"));
    }
}
