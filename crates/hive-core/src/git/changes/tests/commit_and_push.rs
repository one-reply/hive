use super::*;

fn commit_and_push(publish: bool) -> Action {
    Action::CommitAndPush {
        message: "feat: commit staged content".into(),
        remote: "origin".into(),
        publish,
    }
}

#[test]
fn publishes_only_staged_content_and_refuses_an_upstream_of_another_name() {
    let repo = Repo::new();
    let remote = remote_for(&repo);
    repo.write("a", "staged\n");
    repo.git(&["add", "a"]);
    repo.write("a", "unstaged\n");
    repo.action(commit_and_push(true)).unwrap();
    assert_eq!(remote.git(&["show", "main:a"]), "staged");
    assert_eq!(
        repo.git(&["rev-parse", "HEAD"]),
        remote.git(&["rev-parse", "main"])
    );
    assert_eq!(
        repo.git(&["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/main"
    );
    assert!(!repo.git(&["diff"]).is_empty());

    // An upstream of another name is refused before anything is committed.
    let published = remote.git(&["rev-parse", "main"]);
    remote.git(&["branch", "review"]);
    let review = remote.git(&["rev-parse", "review"]);
    repo.git(&["config", "branch.main.merge", "refs/heads/review"]);
    repo.git(&["add", "a"]);
    let local = repo.git(&["rev-parse", "HEAD"]);
    let error = repo.action(commit_and_push(false)).unwrap_err().to_string();
    assert!(error.contains("main tracks origin/review"), "{error}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), local);
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
    assert_eq!(remote.git(&["rev-parse", "review"]), review);
}

#[test]
fn failed_commit_never_pushes_existing_ahead_commits() {
    let repo = Repo::new();
    let remote = remote_for(&repo);
    repo.write("a", "base");
    repo.git(&["add", "a"]);
    repo.action(commit_and_push(true)).unwrap();
    let published = remote.git(&["rev-parse", "main"]);
    repo.write("a", "ahead");
    repo.commit();
    let head = repo.git(&["rev-parse", "HEAD"]);
    repo.write("a", "staged");
    repo.git(&["add", "a"]);
    reject_hook(&repo, "pre-commit");
    let error = repo.action(commit_and_push(false)).unwrap_err();
    assert!(matches!(error, GitError::Failed { .. }));
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
    assert!(!repo.git(&["diff", "--cached"]).is_empty());
}

#[test]
fn failed_push_keeps_local_commit_and_can_retry_without_committing() {
    let repo = Repo::new();
    let remote = remote_for(&repo);
    repo.write("a", "base");
    repo.git(&["add", "a"]);
    repo.action(commit_and_push(true)).unwrap();
    let published = remote.git(&["rev-parse", "main"]);
    repo.write("a", "staged");
    repo.git(&["add", "a"]);
    reject_hook(&repo, "pre-push");
    let error = repo.action(commit_and_push(false)).unwrap_err();
    assert!(matches!(error, GitError::PushAfterCommit { .. }));
    assert!(error.to_string().contains("Commit created locally"));
    assert!(error.stderr().contains("rejected-by-fixture"));
    let committed = repo.git(&["rev-parse", "HEAD"]);
    assert_ne!(committed, published);
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
    assert!(repo.git(&["diff", "--cached"]).is_empty());

    repo.git(&["config", "core.hooksPath", "/dev/null"]);
    repo.action(Action::Push {
        remote: "origin".into(),
        publish: false,
    })
    .unwrap();
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), committed);
    assert_eq!(remote.git(&["rev-parse", "main"]), committed);
}

#[test]
fn invalid_destination_is_rejected_before_creating_a_commit() {
    let repo = Repo::new();
    repo.write("a", "base");
    repo.commit();
    let head = repo.git(&["rev-parse", "HEAD"]);
    repo.write("a", "staged");
    repo.git(&["add", "a"]);
    assert!(repo.action(commit_and_push(true)).is_err());
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    let _remote = remote_for(&repo);
    assert!(repo.action(commit_and_push(false)).is_err());
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    repo.git(&["checkout", "--detach"]);
    assert!(repo.action(commit_and_push(true)).is_err());
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert!(!repo.git(&["diff", "--cached"]).is_empty());
}

#[test]
fn verification_reads_actual_remote_without_pushing_or_losing_staged_work() {
    let repo = Repo::new();
    let remote = remote_for(&repo);
    repo.write("a", "base");
    repo.commit();
    let action = Action::Push {
        remote: "origin".into(),
        publish: true,
    };
    let inspect =
        || crate::git::verification::inspect(&Host::Local, &repo.0, Some(&action)).unwrap();
    assert!(inspect().contains("branch does not exist"));
    repo.action(action.clone()).unwrap();
    let published = remote.git(&["rev-parse", "main"]);
    assert!(inspect().contains("matches current HEAD"));
    repo.write("a", "ahead");
    repo.commit();
    repo.write("a", "staged");
    repo.git(&["add", "a"]);
    let report = inspect();
    assert!(report.contains("different from HEAD"));
    assert!(report.contains("1 staged files"));
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
    assert!(report.contains("has not retried"));
}

#[test]
fn verification_of_an_interrupted_push_still_reports_when_the_remote_cannot_be_read() {
    let (repo, _remote) = published();
    repo.write("a", "ahead");
    repo.commit();
    // The network that interrupted the push is still down.
    repo.git(&[
        "remote",
        "set-url",
        "origin",
        "/nonexistent/hive-remote.git",
    ]);
    let action = Action::Push {
        remote: "origin".into(),
        publish: false,
    };

    let report = crate::git::verification::inspect(&Host::Local, &repo.0, Some(&action)).unwrap();
    assert!(report.contains("Current HEAD:"), "{report}");
    assert!(
        report.contains("Remote origin: could not be read"),
        "{report}"
    );
    assert!(report.contains("has not retried"), "{report}");
}
