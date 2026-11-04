use super::*;

fn remote_for(repo: &Repo) -> Repo {
    let remote = Repo::new();
    remote.git(&["config", "core.bare", "true"]);
    repo.git(&["remote", "add", "origin", remote.0.to_str().unwrap()]);
    remote
}

fn commit_and_push(publish: bool) -> Action {
    Action::CommitAndPush {
        message: "feat: commit staged content".into(),
        remote: "origin".into(),
        publish,
    }
}

fn reject_hook(repo: &Repo, name: &str) {
    let hooks = repo.0.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join(name);
    std::fs::write(&hook, "#!/bin/sh\necho rejected-by-fixture >&2\nexit 1\n").unwrap();
    assert!(
        Host::Local
            .command("chmod", &["+x", hook.to_str().unwrap()], None)
            .status()
            .unwrap()
            .success()
    );
    repo.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
}

#[test]
fn publishes_only_staged_content_then_uses_configured_destination() {
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

    let published = remote.git(&["rev-parse", "main"]);
    remote.git(&["branch", "review"]);
    repo.git(&["config", "branch.main.merge", "refs/heads/review"]);
    repo.git(&["add", "a"]);
    repo.action(commit_and_push(false)).unwrap();
    assert_eq!(remote.git(&["show", "review:a"]), "unstaged");
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
    assert_eq!(
        repo.git(&["rev-parse", "HEAD"]),
        remote.git(&["rev-parse", "review"])
    );
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
