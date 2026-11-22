//! Which Git actions the worktree allows right now and, when one is unavailable, why: one
//! rule per action, shared by the header buttons, the commit footer and its menu.
use crate::state::pull_requests::Status as RequestStatus;
use hive_core::git::changes::Action;
use hive_core::git::pull_request::PullRequest;
use hive_core::git::status::{Change, Status};

/// An action the Git panel offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Entry {
    StageAll,
    Commit,
    CommitAndPush,
    CommitAndSync,
    Push,
    ForcePush,
    CreatePullRequest,
    PushBeforePullRequest,
    Pull,
    Sync,
    Rebase,
    Fetch,
    Publish,
}

impl Entry {
    /// The rows of the footer's menu, top to bottom; a separator goes between the groups.
    pub(super) const MENU: [&'static [Entry]; 2] = [
        &[Entry::Commit, Entry::CommitAndPush, Entry::CommitAndSync],
        &[
            Entry::Push,
            Entry::ForcePush,
            Entry::CreatePullRequest,
            Entry::PushBeforePullRequest,
            Entry::Pull,
            Entry::Sync,
            Entry::Rebase,
            Entry::Fetch,
            Entry::Publish,
        ],
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::StageAll => "Stage All",
            Self::Commit => "Commit",
            Self::CommitAndPush => "Commit & Push",
            Self::CommitAndSync => "Commit & Sync",
            Self::Push => "Push",
            Self::ForcePush => "Force Push",
            Self::CreatePullRequest => "Create PR",
            Self::PushBeforePullRequest => "Push before PR",
            Self::Pull => "Pull",
            Self::Sync => "Sync",
            Self::Rebase => "Rebase",
            Self::Fetch => "Fetch",
            Self::Publish => "Publish Branch",
        }
    }
}

/// Where a push from the current branch would go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PushTarget {
    /// The branch of the same name that the current one tracks on a remote.
    Upstream,
    /// A branch the push creates on the selected remote, which it then tracks.
    NewBranch,
    /// The remote branch the current one tracks has another name, as after
    /// `git switch -c topic origin/develop`. Like Git's `push.default=simple`, Hive does not
    /// push there; publishing under the branch's own name gives it a branch it can push to.
    Elsewhere,
}

impl PushTarget {
    /// Where the push `action` the worktree offers would send the branch `status` reads.
    pub(super) fn of(action: Option<&Action>, status: Option<&Status>) -> Option<Self> {
        let Action::Push { remote, publish } = action? else {
            return None;
        };
        if *publish {
            return Some(Self::NewBranch);
        }
        // Git shortens the upstream to `<remote>/<branch>`, whatever the remote's name holds.
        let namesake = status.is_some_and(|status| {
            matches!(
                (&status.branch, &status.upstream),
                (Some(branch), Some(upstream)) if *upstream == format!("{remote}/{branch}")
            )
        });
        Some(if namesake {
            Self::Upstream
        } else {
            Self::Elsewhere
        })
    }
}

/// What is known about the pull or merge request of the checked-out branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Request {
    /// Not read yet for this branch.
    Unknown,
    /// The hosting service could not be asked.
    Unavailable,
    /// The branch has no open request: none at all, or only merged and closed ones.
    Absent,
    Open,
    /// Hive is opening one right now.
    Opening,
}

impl Request {
    /// What the worktree's request card says, unless Hive is `opening` a request itself.
    pub(super) fn of(opening: bool, card: Option<&RequestStatus>) -> Self {
        match card {
            _ if opening => Self::Opening,
            None => Self::Unknown,
            Some(RequestStatus::Unavailable { .. }) => Self::Unavailable,
            Some(RequestStatus::Ready(snapshot)) => {
                if snapshot.request().is_some_and(PullRequest::is_open) {
                    Self::Open
                } else {
                    Self::Absent
                }
            }
        }
    }
}

/// Everything the availability of an action depends on.
pub(super) struct Facts<'a> {
    /// `None` until the first status read of the worktree finishes.
    pub status: Option<&'a Status>,
    /// The commit message as typed.
    pub message: &'a str,
    /// Whether a fetch has a remote to ask.
    pub has_remote: bool,
    /// Where a push would go, if it has a destination at all.
    pub push: Option<PushTarget>,
    /// Whether a merge, rebase, cherry-pick or revert is unfinished.
    pub operation_unfinished: bool,
    /// The review base by its full ref, which Rebase replays the branch onto.
    pub base: Option<&'a str>,
    pub request: Request,
}

/// Why an action is unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Blocker {
    Loading,
    Conflicts,
    NothingToStage,
    NothingStaged,
    EmptyMessage,
    Detached,
    NoRemote,
    NoUpstream,
    NotPublished,
    Published,
    OtherName,
    NoCommits,
    UpToDate,
    PushIsEnough,
    DirtyTree,
    UnstagedChanges,
    Diverged,
    InProgress,
    NoBase,
    OnBase,
    RequestOpen,
    RequestUnknown,
    RequestUnavailable,
    RequestOpening,
    PushFirst,
}

impl Blocker {
    /// A few words for the right edge of a menu row.
    pub(super) fn note(self) -> &'static str {
        match self {
            Self::Loading => "Loading",
            Self::Conflicts => "Resolve conflicts",
            Self::NothingToStage => "No changes",
            Self::NothingStaged => "Stage changes",
            Self::EmptyMessage => "Write a message",
            Self::Detached => "Detached HEAD",
            Self::NoRemote => "No remote",
            Self::NoUpstream => "No upstream",
            Self::NotPublished => "Not published",
            Self::Published => "Published",
            Self::OtherName => "Tracks another branch",
            Self::NoCommits => "No commits",
            Self::UpToDate => "Up to date",
            Self::PushIsEnough => "Push is enough",
            Self::DirtyTree => "Commit changes first",
            Self::UnstagedChanges => "Stage everything",
            Self::Diverged => "Diverged",
            Self::InProgress => "Finish operation",
            Self::NoBase => "No base branch",
            Self::OnBase => "On the base branch",
            Self::RequestOpen => "PR open",
            Self::RequestUnknown => "Checking…",
            Self::RequestUnavailable => "Unavailable",
            Self::RequestOpening => "Opening…",
            Self::PushFirst => "Push first",
        }
    }

    /// A sentence for the tooltip of a disabled button.
    pub(super) fn explanation(self) -> &'static str {
        match self {
            Self::Loading => "Waiting for changes",
            Self::Conflicts => "Resolve conflicts first",
            Self::NothingToStage => "No changes to stage",
            Self::NothingStaged => "Stage changes first",
            Self::EmptyMessage => "Write a commit message",
            Self::Detached => "Create a branch before pushing detached HEAD",
            Self::NoRemote => "No remote configured or selected",
            Self::NoUpstream => "No upstream to pull from",
            Self::NotPublished => "Publish this branch first",
            Self::Published => "This branch is already published",
            Self::OtherName => {
                "This branch tracks a remote branch of another name; publish it under its own name first"
            }
            Self::NoCommits => "Create the first commit first",
            Self::UpToDate => "Nothing to push",
            Self::PushIsEnough => "The remote has nothing to replace; Push is enough",
            Self::DirtyTree => "Commit or discard your changes first",
            Self::UnstagedChanges => {
                "Stage or discard the remaining changes: Sync needs a clean worktree"
            }
            Self::Diverged => "Local and remote history have diverged; Pull only fast-forwards",
            Self::InProgress => "Finish or abort the current merge, rebase or cherry-pick first",
            Self::NoBase => "Pick a base branch in Compare first",
            Self::OnBase => "This is the base branch itself",
            Self::RequestOpen => "This branch already has an open request",
            Self::RequestUnknown => "Still reading this branch's request",
            Self::RequestUnavailable => {
                "The hosting service could not be asked; the worktree's card says why"
            }
            Self::RequestOpening => "The request is being opened",
            Self::PushFirst => "Push the latest commits before opening the request",
        }
    }
}

/// Why `entry` cannot run now, or `None` when it can.
///
/// "Up to date" is only said where the local repository settles it: a push has nothing to
/// send. Pull, Sync and Fetch ask the remote, which may know more than the last fetch did.
pub(super) fn blocker(entry: Entry, facts: &Facts) -> Option<Blocker> {
    let Some(status) = facts.status else {
        return Some(Blocker::Loading);
    };
    match entry {
        Entry::StageAll => stage_all(status),
        Entry::Commit => commit(status, facts.message),
        Entry::CommitAndPush => {
            commit(status, facts.message).or_else(|| match push_target(status, facts) {
                Err(blocker) => Some(blocker),
                Ok(PushTarget::Elsewhere) => Some(Blocker::OtherName),
                Ok(PushTarget::Upstream | PushTarget::NewBranch) => None,
            })
        }
        Entry::CommitAndSync => commit(status, facts.message)
            .or_else(|| upstream(status, facts).err())
            .or_else(|| {
                // The sync rebases, so the commit has to leave nothing behind.
                status
                    .files
                    .iter()
                    .any(|file| file.unstaged.is_some())
                    .then_some(Blocker::UnstagedChanges)
            }),
        Entry::Push => match push_target(status, facts) {
            Err(blocker) => Some(blocker),
            Ok(PushTarget::NewBranch) => Some(Blocker::NotPublished),
            Ok(PushTarget::Elsewhere) => Some(Blocker::OtherName),
            Ok(PushTarget::Upstream) => (status.ahead == 0).then_some(Blocker::UpToDate),
        },
        Entry::Publish => match push_target(status, facts) {
            Err(blocker) => Some(blocker),
            Ok(PushTarget::Upstream) => Some(Blocker::Published),
            Ok(PushTarget::NewBranch | PushTarget::Elsewhere) => {
                status.oid.is_none().then_some(Blocker::NoCommits)
            }
        },
        Entry::ForcePush => upstream(status, facts).err().or({
            // Only a push the remote would refuse needs force.
            if facts.operation_unfinished {
                Some(Blocker::InProgress)
            } else if status.behind > 0 {
                None
            } else if status.ahead > 0 {
                Some(Blocker::PushIsEnough)
            } else {
                Some(Blocker::UpToDate)
            }
        }),
        // The request is opened from what the remote holds.
        Entry::CreatePullRequest => request(status, facts, |target| match target {
            PushTarget::NewBranch => Some(Blocker::NotPublished),
            PushTarget::Elsewhere => Some(Blocker::OtherName),
            PushTarget::Upstream => (status.ahead > 0).then_some(Blocker::PushFirst),
        }),
        Entry::PushBeforePullRequest => request(status, facts, |target| match target {
            PushTarget::NewBranch => status.oid.is_none().then_some(Blocker::NoCommits),
            PushTarget::Elsewhere => Some(Blocker::OtherName),
            PushTarget::Upstream => (status.ahead == 0).then_some(Blocker::UpToDate),
        }),
        Entry::Pull => pull(status, facts),
        Entry::Sync => upstream(status, facts)
            .err()
            .or_else(|| replay(status, facts)),
        Entry::Rebase => rebase(status, facts),
        Entry::Fetch => (!facts.has_remote).then_some(Blocker::NoRemote),
    }
}

/// What the footer's main button does once changes are staged: commit and push where a push
/// has somewhere to go, otherwise only commit, so a commit is always one click away.
pub(super) fn commit_entry(facts: &Facts) -> Entry {
    match facts.status.map(|status| push_target(status, facts)) {
        Some(Ok(PushTarget::Upstream | PushTarget::NewBranch)) => Entry::CommitAndPush,
        _ => Entry::Commit,
    }
}

/// What a button that sends commits does for this branch: push, or publish it first, also
/// when it tracks a remote branch of another name.
pub(super) fn push_entry(facts: &Facts) -> Entry {
    match facts.push {
        Some(PushTarget::Upstream) => Entry::Push,
        Some(PushTarget::NewBranch | PushTarget::Elsewhere) => Entry::Publish,
        None => match facts.status.and_then(|status| status.upstream.as_ref()) {
            Some(_) => Entry::Push,
            None => Entry::Publish,
        },
    }
}

/// Files with a staged change that a commit would include.
pub(super) fn staged_count(status: &Status) -> usize {
    status
        .files
        .iter()
        .filter(|file| {
            file.staged
                .is_some_and(|change| change != Change::Conflicted)
        })
        .count()
}

fn stage_all(status: &Status) -> Option<Blocker> {
    if status.has_conflicts() {
        Some(Blocker::Conflicts)
    } else if status.files.iter().all(|file| file.unstaged.is_none()) {
        Some(Blocker::NothingToStage)
    } else {
        None
    }
}

fn commit(status: &Status, message: &str) -> Option<Blocker> {
    if status.has_conflicts() {
        Some(Blocker::Conflicts)
    } else if staged_count(status) == 0 {
        Some(Blocker::NothingStaged)
    } else if message.trim().is_empty() {
        Some(Blocker::EmptyMessage)
    } else {
        None
    }
}

fn push_target(status: &Status, facts: &Facts) -> Result<PushTarget, Blocker> {
    if status.branch.is_none() {
        return Err(Blocker::Detached);
    }
    facts.push.ok_or(Blocker::NoRemote)
}

/// The upstream on a remote that a sync and a force push bring the branch in line with.
fn upstream(status: &Status, facts: &Facts) -> Result<(), Blocker> {
    match push_target(status, facts)? {
        PushTarget::Upstream => Ok(()),
        PushTarget::NewBranch => Err(Blocker::NotPublished),
        PushTarget::Elsewhere => Err(Blocker::OtherName),
    }
}

/// What moving the branch needs of the worktree, as `hive_core::git::branches::require_clean`
/// enforces for a pull and a rebase.
fn replay(status: &Status, facts: &Facts) -> Option<Blocker> {
    if facts.operation_unfinished {
        Some(Blocker::InProgress)
    } else if status.has_conflicts() {
        Some(Blocker::Conflicts)
    } else if !status.is_clean() {
        Some(Blocker::DirtyTree)
    } else {
        None
    }
}

/// Pull only fast-forwards a clean worktree.
fn pull(status: &Status, facts: &Facts) -> Option<Blocker> {
    if status.upstream.is_none() {
        return Some(Blocker::NoUpstream);
    }
    replay(status, facts)
        .or_else(|| (status.ahead > 0 && status.behind > 0).then_some(Blocker::Diverged))
}

/// A branch may get a request while it has no open one and somewhere to be pushed; `pushed`
/// then says what the push it relies on still lacks.
fn request(
    status: &Status,
    facts: &Facts,
    pushed: impl FnOnce(PushTarget) -> Option<Blocker>,
) -> Option<Blocker> {
    if status.branch.is_none() {
        return Some(Blocker::Detached);
    }
    let known = match facts.request {
        Request::Opening => Some(Blocker::RequestOpening),
        Request::Unknown => Some(Blocker::RequestUnknown),
        Request::Unavailable => Some(Blocker::RequestUnavailable),
        Request::Open => Some(Blocker::RequestOpen),
        Request::Absent => None,
    };
    known.or_else(|| match push_target(status, facts) {
        Err(blocker) => Some(blocker),
        Ok(target) => pushed(target),
    })
}

fn rebase(status: &Status, facts: &Facts) -> Option<Blocker> {
    let Some(branch) = &status.branch else {
        return Some(Blocker::Detached);
    };
    let Some(base) = facts.base else {
        return Some(Blocker::NoBase);
    };
    if base.strip_prefix("refs/heads/") == Some(branch.as_str()) {
        return Some(Blocker::OnBase);
    }
    if status.oid.is_none() {
        return Some(Blocker::NoCommits);
    }
    replay(status, facts)
}

#[cfg(test)]
mod tests;
