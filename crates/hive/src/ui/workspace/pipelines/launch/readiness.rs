//! What keeps a run from starting, named so the user knows what to do next.
use crate::state::pipelines::{Failure, Preview};
use hive_core::git::branches::Branch;
use hive_core::pipeline::draft::WorkBranch;

/// Where one repository of the run stands before planning can start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LaneState {
    Ready,
    NoBranch,
    BranchUnavailable,
    Reading,
    Failed,
    EditingRef,
    MissingCommand,
}

/// Why Git can no longer give a lane the branch chosen for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BranchProblem {
    Missing,
    OpenElsewhere,
    AlreadyExists,
}

impl BranchProblem {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Missing => "This branch no longer exists. Choose another one.",
            Self::OpenElsewhere => "This branch is now open in another folder. Choose another one.",
            Self::AlreadyExists => {
                "A branch with this name now exists. Choose it from the list or name another."
            }
        }
    }
}

/// Checks the chosen branch against the branches just read, by the name the branch menu
/// lists them under: a chosen branch must still exist and be open nowhere, a new one must
/// still be unused.
pub(super) fn branch_problem(branch: &WorkBranch, branches: &[Branch]) -> Option<BranchProblem> {
    let local = |name: &str| branches.iter().find(|b| !b.remote && b.name == name);
    match branch {
        WorkBranch::Unchosen => None,
        WorkBranch::Existing(name) => match local(name) {
            None => Some(BranchProblem::Missing),
            Some(found) if found.worktree.is_some() => Some(BranchProblem::OpenElsewhere),
            Some(_) => None,
        },
        WorkBranch::New(name) => local(name).map(|_| BranchProblem::AlreadyExists),
    }
}

/// Where a bound repository stands. A failed read comes before a missing branch: branches
/// arrive with the read, so until it succeeds there is nothing to choose from.
pub(super) fn lane_state(
    editing_ref: bool,
    branch: &WorkBranch,
    base: Preview<'_>,
    missing_command: bool,
) -> LaneState {
    if editing_ref {
        return LaneState::EditingRef;
    }
    match base {
        Preview::Failed(_) => LaneState::Failed,
        _ if *branch == WorkBranch::Unchosen => LaneState::NoBranch,
        Preview::Reading => LaneState::Reading,
        Preview::Ready(preview) if branch_problem(branch, &preview.branches).is_some() => {
            LaneState::BranchUnavailable
        }
        Preview::Ready(_) if missing_command => LaneState::MissingCommand,
        Preview::Ready(_) => LaneState::Ready,
    }
}

/// The blocking reason for a repository whose read failed: its name and what Git answered
/// in one line. The repository's card shows the whole error.
pub(super) fn read_failure(repository: &str, failure: &Failure) -> String {
    let cause = failure
        .summary
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("the read failed without a message");
    format!("{repository} can’t be read: {cause}")
}

/// The first thing that keeps **Start planning** disabled, named so the user knows what to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Blocker {
    Task,
    Criteria,
    Repository,
    Branch,
    BranchUnavailable,
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
            Self::BranchUnavailable => {
                "A chosen branch can no longer be used. Choose or create another one."
            }
            Self::Failed => "A repository can’t be read. Try again or pick another one.",
            Self::EditingRef => "Press Use ref to apply the branch, tag or commit.",
            Self::MissingCommand => "Enter the custom check command.",
            Self::Reading => "Reading repositories…",
        }
    }
}

/// Things the user must fix come before a read that only needs time. `missing_criteria` is
/// set when quick start is on and its acceptance criteria are still empty.
pub(super) fn blocker(
    has_task: bool,
    lanes: &[LaneState],
    missing_criteria: bool,
) -> Option<Blocker> {
    if !has_task {
        return Some(Blocker::Task);
    }
    if lanes.is_empty() {
        return Some(Blocker::Repository);
    }
    [
        (LaneState::NoBranch, Blocker::Branch),
        (LaneState::BranchUnavailable, Blocker::BranchUnavailable),
        (LaneState::Failed, Blocker::Failed),
        (LaneState::EditingRef, Blocker::EditingRef),
        (LaneState::MissingCommand, Blocker::MissingCommand),
    ]
    .into_iter()
    .find(|(state, _)| lanes.contains(state))
    .map(|(_, blocker)| blocker)
    .or(missing_criteria.then_some(Blocker::Criteria))
    .or(lanes
        .contains(&LaneState::Reading)
        .then_some(Blocker::Reading))
}

#[cfg(test)]
mod tests {
    use super::{
        Blocker, Branch, BranchProblem, Failure, LaneState, Preview, WorkBranch, blocker,
        branch_problem, lane_state, read_failure,
    };
    use hive_core::pipeline::workspace::BasePreview;

    fn local(name: &str, worktree: Option<&str>) -> Branch {
        Branch {
            reference: format!("refs/heads/{name}"),
            name: name.into(),
            remote: false,
            worktree: worktree.map(Into::into),
        }
    }

    fn read(branches: Vec<Branch>) -> BasePreview {
        BasePreview {
            worktrees_dir: "/repo-worktrees".into(),
            commit: "4f2c9d1".into(),
            dirty: false,
            branches,
            checks: Vec::new(),
        }
    }

    #[test]
    fn a_failed_read_is_named_even_before_a_branch_is_chosen() {
        let failure = Failure::plain("fatal: bad revision");
        let failed = Preview::Failed(&failure);
        assert_eq!(
            lane_state(false, &WorkBranch::Unchosen, failed, false),
            LaneState::Failed
        );
        let chosen = WorkBranch::Existing("feature".into());
        assert_eq!(lane_state(false, &chosen, failed, false), LaneState::Failed);
        // The user is already replacing the ref that could not be read.
        assert_eq!(
            lane_state(true, &chosen, failed, false),
            LaneState::EditingRef
        );
    }

    #[test]
    fn a_lane_is_ready_once_its_read_branch_and_command_are_in_place() {
        let preview = read(vec![local("feature", None)]);
        let ready = Preview::Ready(&preview);
        let chosen = WorkBranch::Existing("feature".into());
        assert_eq!(
            lane_state(false, &WorkBranch::Unchosen, Preview::Reading, false),
            LaneState::NoBranch
        );
        assert_eq!(
            lane_state(false, &chosen, Preview::Reading, false),
            LaneState::Reading
        );
        assert_eq!(
            lane_state(false, &chosen, ready, true),
            LaneState::MissingCommand
        );
        assert_eq!(lane_state(false, &chosen, ready, false), LaneState::Ready);
        assert_eq!(
            lane_state(false, &WorkBranch::New("fix".into()), ready, false),
            LaneState::Ready
        );
    }

    #[test]
    fn a_branch_that_git_can_no_longer_give_the_lane_blocks_the_run() {
        let branches = [
            local("develop", Some("/repo")),
            local("feature", None),
            Branch {
                reference: "refs/remotes/origin/release".into(),
                name: "release".into(),
                remote: true,
                worktree: None,
            },
        ];
        let problem = |branch| branch_problem(&branch, &branches);
        assert_eq!(problem(WorkBranch::Existing("feature".into())), None);
        assert_eq!(
            problem(WorkBranch::Existing("develop".into())),
            Some(BranchProblem::OpenElsewhere)
        );
        assert_eq!(
            problem(WorkBranch::Existing("removed".into())),
            Some(BranchProblem::Missing)
        );
        assert_eq!(
            problem(WorkBranch::New("feature".into())),
            Some(BranchProblem::AlreadyExists)
        );
        // A remote-tracking ref of the same name does not take the local name.
        assert_eq!(problem(WorkBranch::New("release".into())), None);
        assert_eq!(problem(WorkBranch::Unchosen), None);

        let preview = read(branches.to_vec());
        let taken = WorkBranch::Existing("develop".into());
        assert_eq!(
            lane_state(false, &taken, Preview::Ready(&preview), false),
            LaneState::BranchUnavailable
        );
        assert_eq!(
            blocker(true, &[LaneState::BranchUnavailable], false),
            Some(Blocker::BranchUnavailable)
        );
    }

    #[test]
    fn a_read_failure_names_the_repository_and_what_git_answered_in_one_line() {
        let failure = Failure {
            summary: "fatal: bad revision 'nope'".into(),
            detail: "`git rev-parse nope` failed on Local in /api: fatal: bad revision 'nope'"
                .into(),
        };
        assert_eq!(
            read_failure("api", &failure),
            "api can’t be read: fatal: bad revision 'nope'"
        );
        assert_eq!(
            read_failure("api", &Failure::plain(" \n")),
            "api can’t be read: the read failed without a message"
        );
    }

    #[test]
    fn a_missing_task_is_named_before_anything_else() {
        assert_eq!(blocker(false, &[], false), Some(Blocker::Task));
        assert_eq!(
            blocker(false, &[LaneState::Failed], false),
            Some(Blocker::Task)
        );
    }

    #[test]
    fn a_run_needs_at_least_one_repository() {
        assert_eq!(blocker(true, &[], false), Some(Blocker::Repository));
    }

    #[test]
    fn a_fix_the_user_must_make_outranks_a_read_in_progress() {
        let lanes = [LaneState::Reading, LaneState::MissingCommand];
        assert_eq!(blocker(true, &lanes, false), Some(Blocker::MissingCommand));
        let lanes = [LaneState::EditingRef, LaneState::Failed];
        assert_eq!(blocker(true, &lanes, false), Some(Blocker::Failed));
        assert_eq!(
            blocker(true, &[LaneState::Ready, LaneState::Reading], false),
            Some(Blocker::Reading)
        );
    }

    #[test]
    fn a_lane_without_a_branch_blocks_planning_before_slower_reads() {
        let lanes = [LaneState::Reading, LaneState::NoBranch];
        assert_eq!(blocker(true, &lanes, false), Some(Blocker::Branch));
        assert_eq!(blocker(false, &lanes, false), Some(Blocker::Task));
    }

    #[test]
    fn ready_lanes_and_a_task_unblock_planning() {
        let ready = [LaneState::Ready, LaneState::Ready];
        assert_eq!(blocker(true, &ready, false), None);
    }

    #[test]
    fn quick_start_waits_for_its_criteria_after_the_repository_is_set_up() {
        assert_eq!(
            blocker(true, &[LaneState::Ready], true),
            Some(Blocker::Criteria)
        );
        assert_eq!(
            blocker(true, &[LaneState::NoBranch], true),
            Some(Blocker::Branch)
        );
        // Something to type outranks a read that finishes on its own.
        assert_eq!(
            blocker(true, &[LaneState::Reading], true),
            Some(Blocker::Criteria)
        );
        assert_eq!(
            blocker(false, &[LaneState::Ready], true),
            Some(Blocker::Task)
        );
    }
}
