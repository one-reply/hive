use super::*;
use crate::git::conflicts::{self, Operation};
use crate::git::verification;

fn sync() -> Action {
    Action::Sync {
        remote: "origin".into(),
    }
}

fn commit_and_sync() -> Action {
    Action::CommitAndSync {
        message: "feat: commit staged content".into(),
        remote: "origin".into(),
    }
}

fn synced(pulled: usize, pushed: usize) -> Option<Synced> {
    Some(Synced::PulledAndPushed {
        remote: "origin".into(),
        pulled,
        pushed,
    })
}

/// Commits `path` in `other` and pushes it to the shared remote's `main`.
fn push_from(other: &Repo, path: &str, content: &str) {
    other.write(path, content);
    other.commit();
    other.git(&["push", "origin", "HEAD:main"]);
}

fn head(repo: &Repo) -> String {
    repo.git(&["rev-parse", "HEAD"])
}

#[test]
fn sync_rebases_local_commits_onto_incoming_ones_then_pushes() {
    let (repo, remote) = published();
    push_from(&colleague(&remote), "theirs", "theirs\n");
    repo.write("ours", "ours\n");
    repo.commit();

    assert_eq!(repo.action(sync()).unwrap(), synced(1, 1));
    assert_eq!(head(&repo), remote.git(&["rev-parse", "main"]));
    assert!(repo.0.join("theirs").exists() && repo.0.join("ours").exists());
    // Replayed, not merged: the incoming commit is the parent of the local one.
    assert_eq!(repo.git(&["rev-list", "--count", "--merges", "HEAD"]), "0");
    assert_eq!(
        repo.git(&["show", "--format=", "--name-only", "HEAD"]),
        "ours"
    );
    assert_eq!(repo.action(sync()).unwrap(), synced(0, 0));
}

#[test]
fn sync_only_pushes_or_only_fast_forwards_when_one_side_has_commits() {
    let (repo, remote) = published();
    repo.write("ours", "ours\n");
    repo.commit();
    assert_eq!(repo.action(sync()).unwrap(), synced(0, 1));
    assert_eq!(head(&repo), remote.git(&["rev-parse", "main"]));

    push_from(&colleague(&remote), "theirs", "theirs\n");
    assert_eq!(repo.action(sync()).unwrap(), synced(1, 0));
    assert_eq!(head(&repo), remote.git(&["rev-parse", "main"]));
}

#[test]
fn a_branch_that_tracks_another_name_is_never_pushed_there() {
    // What `git switch -c topic origin/develop` leaves behind: topic's upstream is develop.
    let (repo, remote) = published();
    remote.git(&["branch", "develop"]);
    repo.git(&["fetch", "origin"]);
    repo.git(&["switch", "-c", "topic", "--track", "origin/develop"]);
    repo.write("ours", "ours\n");
    repo.commit();
    let develop = remote.git(&["rev-parse", "develop"]);
    let push = Action::Push {
        remote: "origin".into(),
        publish: false,
    };

    for error in [repo.action(sync()), repo.action(push)].map(Result::unwrap_err) {
        let error = error.to_string();
        assert!(error.contains("topic tracks origin/develop"), "{error}");
        assert!(
            error.contains("only to the remote branch of the same name"),
            "{error}"
        );
    }
    assert_eq!(remote.git(&["rev-parse", "develop"]), develop);

    // Publishing under its own name is the way out, and its upstream follows.
    let published = repo
        .action(Action::Push {
            remote: "origin".into(),
            publish: true,
        })
        .unwrap();
    assert_eq!(
        published,
        Some(Synced::Published {
            remote: "origin".into()
        })
    );
    assert_eq!(head(&repo), remote.git(&["rev-parse", "topic"]));
    assert_eq!(remote.git(&["rev-parse", "develop"]), develop);
    assert_eq!(repo.action(sync()).unwrap(), synced(0, 0));
}

#[test]
fn sync_refuses_a_branch_deleted_on_the_remote_instead_of_recreating_it() {
    let (repo, remote) = published();
    repo.git(&["switch", "-c", "feature"]);
    repo.write("feature", "one\n");
    repo.commit();
    repo.git(&["push", "--set-upstream", "origin", "feature"]);
    // The request was merged and the hosting service deleted the branch; a plain fetch
    // keeps the remote-tracking ref, so the last fetch still shows it.
    remote.git(&["update-ref", "-d", "refs/heads/feature"]);
    repo.git(&["fetch", "origin"]);
    repo.write("feature", "two\n");
    repo.commit();

    let error = repo.action(sync()).unwrap_err().to_string();
    assert!(
        error.contains("origin/feature was deleted on the remote"),
        "{error}"
    );
    assert!(remote.git(&["branch", "--list", "feature"]).is_empty());
}

#[test]
fn sync_pauses_on_a_conflict_without_pushing_and_a_plain_push_finishes_it() {
    let (repo, remote) = published();
    push_from(&colleague(&remote), "base", "theirs\n");
    let published = remote.git(&["rev-parse", "main"]);
    repo.write("base", "ours\n");
    repo.commit();

    // The replay stopped for the user: that is not a failure, and nothing left the machine.
    assert_eq!(repo.action(sync()).unwrap(), None);
    assert_eq!(
        conflicts::operation(&Host::Local, &repo.0).unwrap(),
        Some(Operation::Rebase)
    );
    assert!(status::read(&Host::Local, &repo.0).unwrap().has_conflicts());
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
    // Whatever interrupted the user, the check reads the unfinished rebase, not a remote ref.
    let report = verification::inspect(&Host::Local, &repo.0, Some(&sync())).unwrap();
    assert!(report.contains("A rebase is unfinished"), "{report}");

    repo.write("base", "both\n");
    repo.git(&["add", "base"]);
    repo.action(Action::ContinueOperation).unwrap();
    assert_eq!(conflicts::operation(&Host::Local, &repo.0).unwrap(), None);
    assert_eq!(
        repo.action(Action::Push {
            remote: "origin".into(),
            publish: false,
        })
        .unwrap(),
        Some(Synced::Pushed {
            remote: "origin".into(),
            commits: 1,
        })
    );
    assert_eq!(remote.git(&["show", "main:base"]), "both");
}

#[test]
fn sync_refuses_before_fetching_what_it_cannot_reconcile() {
    let refused = |repo: &Repo, expected: &str| {
        let error = repo.action(sync()).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    };

    let (repo, remote) = published();
    push_from(&colleague(&remote), "theirs", "theirs\n");
    let known = repo.git(&["rev-parse", "origin/main"]);
    repo.write("untracked", "untracked\n");
    refused(&repo, "Commit your staged, unstaged and untracked changes");
    // Refused before the fetch: the remote-tracking branch still points where it did.
    assert_eq!(repo.git(&["rev-parse", "origin/main"]), known);
    std::fs::remove_file(repo.0.join("untracked")).unwrap();

    repo.git(&["switch", "-c", "unpublished"]);
    refused(&repo, "Publish this branch first");
    repo.git(&["switch", "--detach"]);
    refused(&repo, "Create a branch before pushing detached HEAD");
    // A branch that tracks a local branch has no remote to sync with.
    repo.git(&["switch", "-c", "local", "--track", "main"]);
    refused(&repo, "configured upstream remote");
    assert_eq!(repo.git(&["rev-parse", "origin/main"]), known);

    let unborn = Repo::new();
    remote_for(&unborn);
    refused(&unborn, "Create the first commit first");
}

#[test]
fn sync_refuses_local_merge_commits_instead_of_flattening_them() {
    let (repo, remote) = published();
    push_from(&colleague(&remote), "theirs", "theirs\n");
    repo.git(&["switch", "-c", "side"]);
    repo.write("side", "side\n");
    repo.commit();
    repo.git(&["switch", "main"]);
    repo.git(&["merge", "--no-ff", "-m", "merge side", "side"]);
    let merged = head(&repo);

    let error = repo.action(sync()).unwrap_err().to_string();
    assert!(error.contains("merge commits"), "{error}");
    assert_eq!(head(&repo), merged);
    assert_eq!(conflicts::operation(&Host::Local, &repo.0).unwrap(), None);
}

#[test]
fn sync_refuses_a_branch_rewritten_after_it_was_pushed() {
    let (repo, remote) = published();
    repo.write("ours", "ours\n");
    repo.commit();
    repo.git(&["push", "origin", "main"]);
    let published = remote.git(&["rev-parse", "main"]);
    repo.write("ours", "amended\n");
    repo.git(&["add", "ours"]);
    repo.git(&["commit", "--amend", "-m", "rewritten"]);
    let rewritten = head(&repo);

    let error = repo.action(sync()).unwrap_err().to_string();
    assert!(error.contains("rewritten after it was pushed"), "{error}");
    assert!(error.contains("Force Push"), "{error}");
    assert_eq!(head(&repo), rewritten);
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
}

#[test]
fn an_upstream_somebody_else_rewound_never_drops_local_commits() {
    let (repo, remote) = published();
    let other = colleague(&remote);
    push_from(&other, "theirs", "theirs\n");
    repo.git(&["pull", "--ff-only"]);
    repo.write("ours", "ours\n");
    repo.commit();
    let before = head(&repo);
    // The colleague replaces their commit; this branch never rewrote anything itself.
    other.write("theirs", "replaced\n");
    other.git(&["add", "theirs"]);
    other.git(&["commit", "--amend", "-m", "replaced"]);
    other.git(&["push", "--force", "origin", "HEAD:main"]);
    let replaced = remote.git(&["rev-parse", "main"]);

    // Their old commit is replayed like any local one and meets its replacement, so the
    // user decides; a fork-point rebase would have dropped it and whatever it carried.
    assert_eq!(repo.action(sync()).unwrap(), None);
    assert_eq!(
        conflicts::operation(&Host::Local, &repo.0).unwrap(),
        Some(Operation::Rebase)
    );
    assert_eq!(remote.git(&["rev-parse", "main"]), replaced);
    repo.action(Action::AbortOperation).unwrap();
    assert_eq!(head(&repo), before);
    assert!(repo.0.join("ours").exists());
}

#[test]
fn sync_ignores_pull_and_rebase_configuration() {
    let (repo, remote) = published();
    push_from(&colleague(&remote), "theirs", "theirs\n");
    for (key, value) in [
        ("pull.ff", "only"),
        ("pull.rebase", "false"),
        ("rebase.autoStash", "true"),
        ("rebase.autoSquash", "true"),
        ("rebase.updateRefs", "true"),
    ] {
        repo.git(&["config", key, value]);
    }
    repo.write("ours", "ours\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "feature"]);
    // Another branch inside the replayed range, and a commit a squashing rebase would fold.
    repo.git(&["branch", "marker"]);
    let marker = repo.git(&["rev-parse", "marker"]);
    repo.write("ours", "fixed\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "fixup! feature"]);

    assert_eq!(repo.action(sync()).unwrap(), synced(1, 2));
    assert_eq!(repo.git(&["rev-parse", "marker"]), marker);
    assert_eq!(
        repo.git(&["log", "--format=%s", "-2"]),
        "fixup! feature\nfeature"
    );
    assert_eq!(head(&repo), remote.git(&["rev-parse", "main"]));
}

#[test]
fn a_replay_that_stops_without_a_conflict_is_an_error_with_gits_output() {
    let (repo, remote) = published();
    push_from(&colleague(&remote), "theirs", "theirs\n");
    let published = remote.git(&["rev-parse", "main"]);
    repo.write("ours", "ours\n");
    repo.commit();
    // Replaying has to sign the commit again, and the signing program fails.
    repo.git(&["config", "commit.gpgsign", "true"]);
    repo.git(&["config", "gpg.program", "false"]);

    let error = repo.action(sync()).unwrap_err();
    assert!(matches!(error, GitError::Failed { .. }), "{error}");
    assert!(!status::read(&Host::Local, &repo.0).unwrap().has_conflicts());
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
    // The banner offers Abort for what the failed replay left behind.
    assert_eq!(
        conflicts::operation(&Host::Local, &repo.0).unwrap(),
        Some(Operation::Rebase)
    );
    repo.action(Action::AbortOperation).unwrap();
    assert!(repo.0.join("ours").exists());
}

#[test]
fn a_rejected_push_after_the_replay_keeps_the_rebased_commits_for_a_retry() {
    let (repo, remote) = published();
    push_from(&colleague(&remote), "theirs", "theirs\n");
    let published = remote.git(&["rev-parse", "main"]);
    repo.write("ours", "ours\n");
    repo.commit();
    reject_hook(&repo, "pre-push");

    let error = repo.action(sync()).unwrap_err();
    assert!(error.stderr().contains("rejected-by-fixture"), "{error}");
    assert!(!error.outcome_unknown());
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
    let local = status::read(&Host::Local, &repo.0).unwrap();
    assert_eq!((local.ahead, local.behind), (1, 0));

    repo.git(&["config", "core.hooksPath", "/dev/null"]);
    assert_eq!(repo.action(sync()).unwrap(), synced(0, 1));
}

#[test]
fn a_stopped_sync_is_an_interrupted_mutation() {
    let (repo, _remote) = published();
    let stop = Cancellation::default();
    stop.cancel();
    let error = execute(&Host::Local, &repo.0, &sync(), &stop).unwrap_err();
    assert!(error.outcome_unknown(), "{error}");
}

#[test]
fn commit_and_sync_commits_then_reconciles_and_pushes() {
    let (repo, remote) = published();
    push_from(&colleague(&remote), "theirs", "theirs\n");
    repo.write("ours", "ours\n");
    repo.git(&["add", "ours"]);

    assert_eq!(repo.action(commit_and_sync()).unwrap(), synced(1, 1));
    assert_eq!(head(&repo), remote.git(&["rev-parse", "main"]));
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "feat: commit staged content"
    );
}

#[test]
fn commit_and_sync_refuses_before_committing_what_the_sync_would_refuse() {
    let (repo, _remote) = published();
    let before = head(&repo);
    repo.write("staged", "staged\n");
    repo.git(&["add", "staged"]);
    repo.write("left", "left\n");

    let error = repo.action(commit_and_sync()).unwrap_err().to_string();
    assert!(
        error.contains("Stage or discard the remaining changes"),
        "{error}"
    );
    assert_eq!(head(&repo), before);
    assert!(repo.git(&["status", "--porcelain"]).contains("A  staged"));

    std::fs::remove_file(repo.0.join("left")).unwrap();
    repo.git(&["switch", "-c", "unpublished"]);
    let error = repo.action(commit_and_sync()).unwrap_err().to_string();
    assert!(error.contains("Publish this branch first"), "{error}");
    assert_eq!(head(&repo), before);
}

#[test]
fn a_failed_commit_hook_never_fetches_or_pushes() {
    let (repo, remote) = published();
    push_from(&colleague(&remote), "theirs", "theirs\n");
    let known = repo.git(&["rev-parse", "origin/main"]);
    let before = head(&repo);
    repo.write("ours", "ours\n");
    repo.git(&["add", "ours"]);
    reject_hook(&repo, "pre-commit");

    let error = repo.action(commit_and_sync()).unwrap_err();
    assert!(
        !matches!(error, GitError::SyncAfterCommit { .. }),
        "{error}"
    );
    assert_eq!(head(&repo), before);
    assert_eq!(repo.git(&["rev-parse", "origin/main"]), known);
}

#[test]
fn a_sync_failure_after_the_commit_keeps_it_and_says_so() {
    let (repo, remote) = published();
    let published = remote.git(&["rev-parse", "main"]);
    repo.write("ours", "ours\n");
    repo.git(&["add", "ours"]);
    reject_hook(&repo, "pre-push");

    let error = repo.action(commit_and_sync()).unwrap_err();
    assert!(matches!(error, GitError::SyncAfterCommit { .. }), "{error}");
    assert!(
        error.to_string().starts_with("Commit created locally"),
        "{error}"
    );
    // The cause is the wrapped error's: its output, and whether the outcome is known.
    assert!(error.stderr().contains("rejected-by-fixture"), "{error}");
    assert!(!error.outcome_unknown());
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]),
        "feat: commit staged content"
    );
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
}
