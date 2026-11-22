use super::{
    Blocker, Entry, Facts, PushTarget, Request, RequestStatus, blocker, commit_entry, push_entry,
};
use hive_core::git::status::{Change, FileChange, Status};

fn file(path: &str, staged: Option<Change>, unstaged: Option<Change>) -> FileChange {
    FileChange {
        path: path.into(),
        old_path: None,
        staged,
        unstaged,
        submodule: false,
        staged_stats: None,
        unstaged_stats: None,
        conflict: None,
    }
}

/// A published branch with one commit and nothing changed.
fn published() -> Status {
    Status {
        oid: Some("head".into()),
        branch: Some("feature".into()),
        upstream: Some("origin/feature".into()),
        ..Status::default()
    }
}

fn facts<'a>(status: &'a Status, message: &'a str) -> Facts<'a> {
    Facts {
        status: Some(status),
        message,
        has_remote: true,
        push: Some(if status.upstream.is_some() {
            PushTarget::Upstream
        } else {
            PushTarget::NewBranch
        }),
        operation_unfinished: false,
        base: Some("refs/remotes/origin/main"),
        request: Request::Absent,
    }
}

#[test]
fn nothing_is_available_before_the_first_status() {
    let unread = Facts {
        status: None,
        message: "message",
        has_remote: true,
        push: None,
        operation_unfinished: false,
        base: None,
        request: Request::Unknown,
    };
    for entry in Entry::MENU.iter().copied().flatten().copied() {
        assert_eq!(blocker(entry, &unread), Some(Blocker::Loading), "{entry:?}");
    }
    assert_eq!(blocker(Entry::StageAll, &unread), Some(Blocker::Loading));
    assert_eq!(commit_entry(&unread), Entry::Commit);
    assert_eq!(push_entry(&unread), Entry::Publish);
}

#[test]
fn commit_requires_staged_files_message_and_no_conflicts() {
    let mut status = published();
    let commit = |status: &Status, message: &str| blocker(Entry::Commit, &facts(status, message));
    assert_eq!(commit(&status, "message"), Some(Blocker::NothingStaged));
    status
        .files
        .push(file("file.rs", None, Some(Change::Modified)));
    assert_eq!(commit(&status, "message"), Some(Blocker::NothingStaged));
    status.files[0].staged = Some(Change::Modified);
    for message in ["", " \n\t"] {
        assert_eq!(commit(&status, message), Some(Blocker::EmptyMessage));
    }
    assert_eq!(commit(&status, "message"), None);
    status
        .files
        .push(file("conflict.rs", Some(Change::Conflicted), None));
    assert_eq!(commit(&status, "message"), Some(Blocker::Conflicts));
}

#[test]
fn stage_all_needs_an_unstaged_change_and_no_conflicts() {
    let mut status = published();
    let stage = |status: &Status| blocker(Entry::StageAll, &facts(status, ""));
    assert_eq!(stage(&status), Some(Blocker::NothingToStage));
    status
        .files
        .push(file("staged.rs", Some(Change::Modified), None));
    assert_eq!(stage(&status), Some(Blocker::NothingToStage));
    status
        .files
        .push(file("new.rs", None, Some(Change::Untracked)));
    assert_eq!(stage(&status), None);
    status
        .files
        .push(file("conflict.rs", Some(Change::Conflicted), None));
    assert_eq!(stage(&status), Some(Blocker::Conflicts));
}

#[test]
fn a_push_needs_a_branch_a_destination_and_commits_to_send() {
    let mut status = published();
    let push = |status: &Status| blocker(Entry::Push, &facts(status, ""));
    assert_eq!(push(&status), Some(Blocker::UpToDate));
    status.ahead = 2;
    assert_eq!(push(&status), None);
    // Whatever the upstream has, the commits still leave or the remote refuses them.
    status.behind = 1;
    assert_eq!(push(&status), None);

    let mut local = facts(&status, "");
    local.push = None;
    assert_eq!(blocker(Entry::Push, &local), Some(Blocker::NoRemote));

    status.upstream = None;
    assert_eq!(push(&status), Some(Blocker::NotPublished));
    status.branch = None;
    assert_eq!(push(&status), Some(Blocker::Detached));
}

#[test]
fn publishing_needs_an_unpublished_branch_with_a_commit_and_a_remote() {
    let mut status = published();
    let publish = |status: &Status| blocker(Entry::Publish, &facts(status, ""));
    assert_eq!(publish(&status), Some(Blocker::Published));
    status.upstream = None;
    assert_eq!(publish(&status), None);
    status.oid = None;
    assert_eq!(publish(&status), Some(Blocker::NoCommits));
    status.oid = Some("head".into());

    let mut unreachable = facts(&status, "");
    unreachable.push = None;
    assert_eq!(
        blocker(Entry::Publish, &unreachable),
        Some(Blocker::NoRemote)
    );
    status.branch = None;
    assert_eq!(publish(&status), Some(Blocker::Detached));
}

#[test]
fn commit_and_push_adds_the_push_destination_to_the_commit_rules() {
    let mut status = published();
    status
        .files
        .push(file("file.rs", Some(Change::Modified), None));
    let both = |facts: &Facts| blocker(Entry::CommitAndPush, facts);
    assert_eq!(both(&facts(&status, "")), Some(Blocker::EmptyMessage));
    assert_eq!(both(&facts(&status, "message")), None);
    assert_eq!(
        commit_entry(&facts(&status, "message")),
        Entry::CommitAndPush
    );

    // The first commit of an unborn branch can be published in the same step.
    status.oid = None;
    status.upstream = None;
    assert_eq!(both(&facts(&status, "message")), None);

    let mut local = facts(&status, "message");
    local.push = None;
    assert_eq!(both(&local), Some(Blocker::NoRemote));
    assert_eq!(commit_entry(&local), Entry::Commit);
    assert_eq!(blocker(Entry::Commit, &local), None);

    status.branch = None;
    assert_eq!(both(&facts(&status, "message")), Some(Blocker::Detached));
    assert_eq!(commit_entry(&facts(&status, "message")), Entry::Commit);
}

#[test]
fn pull_fast_forwards_a_clean_tracked_branch_only() {
    let mut status = published();
    let pull = |status: &Status| blocker(Entry::Pull, &facts(status, ""));
    // The remote may have commits the last fetch did not see.
    assert_eq!(pull(&status), None);
    status.behind = 3;
    assert_eq!(pull(&status), None);
    status.ahead = 1;
    assert_eq!(pull(&status), Some(Blocker::Diverged));
    status.ahead = 0;
    status
        .files
        .push(file("new.rs", None, Some(Change::Untracked)));
    assert_eq!(pull(&status), Some(Blocker::DirtyTree));
    status.files[0] = file("conflict.rs", Some(Change::Conflicted), None);
    assert_eq!(pull(&status), Some(Blocker::Conflicts));
    status.upstream = None;
    assert_eq!(pull(&status), Some(Blocker::NoUpstream));
}

#[test]
fn sync_needs_a_published_branch_and_a_clean_worktree_but_no_known_difference() {
    let mut status = published();
    let sync = |status: &Status| blocker(Entry::Sync, &facts(status, ""));
    // In step as far as the last fetch knows; the sync fetches again.
    assert_eq!(sync(&status), None);
    status.ahead = 1;
    status.behind = 2;
    assert_eq!(sync(&status), None);

    let mut rebasing = facts(&status, "");
    rebasing.operation_unfinished = true;
    assert_eq!(blocker(Entry::Sync, &rebasing), Some(Blocker::InProgress));
    assert_eq!(blocker(Entry::Pull, &rebasing), Some(Blocker::InProgress));
    let mut local = facts(&status, "");
    local.push = None;
    assert_eq!(blocker(Entry::Sync, &local), Some(Blocker::NoRemote));

    status
        .files
        .push(file("new.rs", None, Some(Change::Untracked)));
    assert_eq!(sync(&status), Some(Blocker::DirtyTree));
    status.files[0] = file("conflict.rs", Some(Change::Conflicted), None);
    assert_eq!(sync(&status), Some(Blocker::Conflicts));
    status.files.clear();
    status.upstream = None;
    assert_eq!(sync(&status), Some(Blocker::NotPublished));
    status.branch = None;
    assert_eq!(sync(&status), Some(Blocker::Detached));
}

#[test]
fn commit_and_sync_needs_everything_staged_and_a_published_branch() {
    let mut status = published();
    status
        .files
        .push(file("file.rs", Some(Change::Modified), None));
    let both =
        |status: &Status, message: &str| blocker(Entry::CommitAndSync, &facts(status, message));
    assert_eq!(both(&status, ""), Some(Blocker::EmptyMessage));
    assert_eq!(both(&status, "message"), None);
    // A partly staged file would be left behind by the commit.
    status.files[0].unstaged = Some(Change::Modified);
    assert_eq!(both(&status, "message"), Some(Blocker::UnstagedChanges));
    status.files[0].unstaged = None;
    status
        .files
        .push(file("new.rs", None, Some(Change::Untracked)));
    assert_eq!(both(&status, "message"), Some(Blocker::UnstagedChanges));
    status.files.pop();
    status.upstream = None;
    assert_eq!(both(&status, "message"), Some(Blocker::NotPublished));
}

#[test]
fn force_push_is_offered_only_when_a_push_would_be_refused() {
    let mut status = published();
    let force = |status: &Status| blocker(Entry::ForcePush, &facts(status, ""));
    assert_eq!(force(&status), Some(Blocker::UpToDate));
    status.ahead = 2;
    assert_eq!(force(&status), Some(Blocker::PushIsEnough));
    status.behind = 1;
    assert_eq!(force(&status), None);
    // Dropping pushed commits locally also needs force to reach the remote.
    status.ahead = 0;
    assert_eq!(force(&status), None);
    // Uncommitted changes stay where they are; a push does not touch them.
    status
        .files
        .push(file("new.rs", None, Some(Change::Untracked)));
    assert_eq!(force(&status), None);

    let mut rebasing = facts(&status, "");
    rebasing.operation_unfinished = true;
    assert_eq!(
        blocker(Entry::ForcePush, &rebasing),
        Some(Blocker::InProgress)
    );
    status.upstream = None;
    assert_eq!(force(&status), Some(Blocker::NotPublished));
    status.branch = None;
    assert_eq!(force(&status), Some(Blocker::Detached));
}

#[test]
fn rebase_needs_a_base_other_than_the_branch_and_a_clean_worktree() {
    let mut status = published();
    let rebase = |facts: &Facts| blocker(Entry::Rebase, facts);
    assert_eq!(rebase(&facts(&status, "")), None);
    // Publishing is not required: the base is another branch.
    status.upstream = None;
    assert_eq!(rebase(&facts(&status, "")), None);

    let mut on_base = facts(&status, "");
    on_base.base = Some("refs/heads/feature");
    assert_eq!(rebase(&on_base), Some(Blocker::OnBase));
    // A remote branch of the same name is still another branch.
    on_base.base = Some("refs/remotes/origin/feature");
    assert_eq!(rebase(&on_base), None);
    on_base.base = None;
    assert_eq!(rebase(&on_base), Some(Blocker::NoBase));
    let mut merging = facts(&status, "");
    merging.operation_unfinished = true;
    assert_eq!(rebase(&merging), Some(Blocker::InProgress));

    status
        .files
        .push(file("file.rs", Some(Change::Modified), None));
    assert_eq!(rebase(&facts(&status, "")), Some(Blocker::DirtyTree));
    status.files.clear();
    status.oid = None;
    assert_eq!(rebase(&facts(&status, "")), Some(Blocker::NoCommits));
    status.branch = None;
    assert_eq!(rebase(&facts(&status, "")), Some(Blocker::Detached));
}

#[test]
fn a_request_is_created_from_a_pushed_branch_without_an_open_one() {
    let mut status = published();
    let create = |facts: &Facts| blocker(Entry::CreatePullRequest, facts);
    assert_eq!(create(&facts(&status, "")), None);
    for (request, expected) in [
        (Request::Open, Blocker::RequestOpen),
        (Request::Unknown, Blocker::RequestUnknown),
        (Request::Unavailable, Blocker::RequestUnavailable),
        (Request::Opening, Blocker::RequestOpening),
    ] {
        let mut known = facts(&status, "");
        known.request = request;
        assert_eq!(create(&known), Some(expected));
        assert_eq!(
            blocker(Entry::PushBeforePullRequest, &known),
            Some(expected)
        );
    }
    // Unpushed commits would be missing from the request.
    status.ahead = 1;
    assert_eq!(create(&facts(&status, "")), Some(Blocker::PushFirst));
    status.upstream = None;
    assert_eq!(create(&facts(&status, "")), Some(Blocker::NotPublished));
    status.branch = None;
    let mut detached = facts(&status, "");
    detached.request = Request::Unknown;
    assert_eq!(create(&detached), Some(Blocker::Detached));
}

#[test]
fn push_before_a_request_needs_something_to_push() {
    let mut status = published();
    let push = |facts: &Facts| blocker(Entry::PushBeforePullRequest, facts);
    assert_eq!(push(&facts(&status, "")), Some(Blocker::UpToDate));
    status.ahead = 2;
    assert_eq!(push(&facts(&status, "")), None);
    // An unpublished branch is published first.
    status.upstream = None;
    assert_eq!(push(&facts(&status, "")), None);
    let mut local = facts(&status, "");
    local.push = None;
    assert_eq!(push(&local), Some(Blocker::NoRemote));
    status.oid = None;
    assert_eq!(push(&facts(&status, "")), Some(Blocker::NoCommits));
}

#[test]
fn the_request_card_says_what_is_known_about_the_branch() {
    let unavailable = RequestStatus::Unavailable {
        message: "signed out".into(),
        authentication: None,
    };
    assert_eq!(Request::of(false, None), Request::Unknown);
    assert_eq!(Request::of(false, Some(&unavailable)), Request::Unavailable);
    // Hive's own request in flight outranks whatever the last read said.
    assert_eq!(Request::of(true, None), Request::Opening);
    assert_eq!(Request::of(true, Some(&unavailable)), Request::Opening);
}

#[test]
fn fetch_needs_only_a_remote() {
    let mut status = published();
    status.branch = None;
    status.upstream = None;
    let mut detached = facts(&status, "");
    assert_eq!(blocker(Entry::Fetch, &detached), None);
    detached.has_remote = false;
    assert_eq!(blocker(Entry::Fetch, &detached), Some(Blocker::NoRemote));
}

#[test]
fn the_send_button_publishes_a_branch_without_an_upstream() {
    let mut status = published();
    assert_eq!(push_entry(&facts(&status, "")), Entry::Push);
    status.upstream = None;
    assert_eq!(push_entry(&facts(&status, "")), Entry::Publish);
}

#[test]
fn every_blocker_has_a_short_note_and_a_longer_explanation() {
    let blockers = [
        Blocker::Loading,
        Blocker::Conflicts,
        Blocker::NothingToStage,
        Blocker::NothingStaged,
        Blocker::EmptyMessage,
        Blocker::Detached,
        Blocker::NoRemote,
        Blocker::NoUpstream,
        Blocker::NotPublished,
        Blocker::Published,
        Blocker::NoCommits,
        Blocker::UpToDate,
        Blocker::PushIsEnough,
        Blocker::DirtyTree,
        Blocker::UnstagedChanges,
        Blocker::Diverged,
        Blocker::InProgress,
        Blocker::NoBase,
        Blocker::OnBase,
        Blocker::RequestOpen,
        Blocker::RequestUnknown,
        Blocker::RequestUnavailable,
        Blocker::RequestOpening,
        Blocker::PushFirst,
    ];
    for blocker in blockers {
        // The note has to fit beside the label in a menu as narrow as the panel.
        assert!(blocker.note().len() <= 20, "{blocker:?}");
        assert!(!blocker.explanation().is_empty(), "{blocker:?}");
    }
}

#[test]
fn the_push_target_names_an_upstream_of_another_name_apart() {
    use hive_core::git::changes::Action;
    let push = |remote: &str, publish| Action::Push {
        remote: remote.into(),
        publish,
    };
    let tracking = |upstream: &str| Status {
        upstream: Some(upstream.into()),
        ..published()
    };
    let of = |action: &Action, status: &Status| PushTarget::of(Some(action), Some(status));

    assert_eq!(PushTarget::of(None, Some(&published())), None);
    assert_eq!(
        of(&push("origin", true), &Status::default()),
        Some(PushTarget::NewBranch)
    );
    assert_eq!(
        of(&push("origin", false), &tracking("origin/feature")),
        Some(PushTarget::Upstream)
    );
    // A remote's name may itself hold a slash.
    assert_eq!(
        of(&push("team/fork", false), &tracking("team/fork/feature")),
        Some(PushTarget::Upstream)
    );
    // `git switch -c feature origin/develop` leaves develop as the upstream.
    assert_eq!(
        of(&push("origin", false), &tracking("origin/develop")),
        Some(PushTarget::Elsewhere)
    );
}

#[test]
fn a_branch_tracking_another_name_can_only_be_published_under_its_own() {
    let mut status = Status {
        upstream: Some("origin/develop".into()),
        ahead: 1,
        behind: 1,
        files: vec![file("file.rs", Some(Change::Modified), None)],
        ..published()
    };
    fn elsewhere(status: &Status) -> Facts<'_> {
        Facts {
            push: Some(PushTarget::Elsewhere),
            ..facts(status, "message")
        }
    }
    for entry in [
        Entry::Push,
        Entry::CommitAndPush,
        Entry::CommitAndSync,
        Entry::ForcePush,
        Entry::CreatePullRequest,
        Entry::PushBeforePullRequest,
    ] {
        assert_eq!(
            blocker(entry, &elsewhere(&status)),
            Some(Blocker::OtherName),
            "{entry:?}"
        );
    }
    assert_eq!(blocker(Entry::Publish, &elsewhere(&status)), None);
    assert_eq!(commit_entry(&elsewhere(&status)), Entry::Commit);
    assert_eq!(push_entry(&elsewhere(&status)), Entry::Publish);
    status.files.clear();
    assert_eq!(
        blocker(Entry::Sync, &elsewhere(&status)),
        Some(Blocker::OtherName)
    );
}
