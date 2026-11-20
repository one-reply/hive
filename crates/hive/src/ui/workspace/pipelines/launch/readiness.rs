//! What keeps a run from starting, named so the user knows what to do next.

/// Where one repository of the run stands before planning can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LaneState {
    Ready,
    NoBranch,
    Reading,
    Failed,
    EditingRef,
    MissingCommand,
}

/// The first thing that keeps **Start planning** disabled, named so the user knows what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Blocker {
    Task,
    Criteria,
    Repository,
    Branch,
    Failed,
    EditingRef,
    MissingCommand,
    Reading,
}

impl Blocker {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Criteria => "Enter acceptance criteria for quick start.",
            Self::Task => "Describe the task to start the run.",
            Self::Repository => "Add at least one repository.",
            Self::Branch => "Choose or create a branch for each repository.",
            Self::Failed => "A repository can’t be read. Pick another branch or repository.",
            Self::EditingRef => "Press Use ref to apply the branch, tag or commit.",
            Self::MissingCommand => "Enter the custom check command.",
            Self::Reading => "Reading repositories…",
        }
    }
}

/// Things the user must fix come before a read that only needs time.
pub(super) fn blocker(has_task: bool, lanes: &[LaneState]) -> Option<Blocker> {
    if !has_task {
        return Some(Blocker::Task);
    }
    if lanes.is_empty() {
        return Some(Blocker::Repository);
    }
    [
        (LaneState::NoBranch, Blocker::Branch),
        (LaneState::Failed, Blocker::Failed),
        (LaneState::EditingRef, Blocker::EditingRef),
        (LaneState::MissingCommand, Blocker::MissingCommand),
        (LaneState::Reading, Blocker::Reading),
    ]
    .into_iter()
    .find(|(state, _)| lanes.contains(state))
    .map(|(_, blocker)| blocker)
}

#[cfg(test)]
mod tests {
    use super::{Blocker, LaneState, blocker};

    #[test]
    fn a_missing_task_is_named_before_anything_else() {
        assert_eq!(blocker(false, &[]), Some(Blocker::Task));
        assert_eq!(blocker(false, &[LaneState::Failed]), Some(Blocker::Task));
    }

    #[test]
    fn a_run_needs_at_least_one_repository() {
        assert_eq!(blocker(true, &[]), Some(Blocker::Repository));
    }

    #[test]
    fn a_fix_the_user_must_make_outranks_a_read_in_progress() {
        let lanes = [LaneState::Reading, LaneState::MissingCommand];
        assert_eq!(blocker(true, &lanes), Some(Blocker::MissingCommand));
        let lanes = [LaneState::EditingRef, LaneState::Failed];
        assert_eq!(blocker(true, &lanes), Some(Blocker::Failed));
        assert_eq!(
            blocker(true, &[LaneState::Ready, LaneState::Reading]),
            Some(Blocker::Reading)
        );
    }

    #[test]
    fn a_lane_without_a_branch_blocks_planning_before_slower_reads() {
        let lanes = [LaneState::Reading, LaneState::NoBranch];
        assert_eq!(blocker(true, &lanes), Some(Blocker::Branch));
        assert_eq!(blocker(false, &lanes), Some(Blocker::Task));
    }

    #[test]
    fn ready_lanes_and_a_task_unblock_planning() {
        assert_eq!(blocker(true, &[LaneState::Ready, LaneState::Ready]), None);
    }
}
