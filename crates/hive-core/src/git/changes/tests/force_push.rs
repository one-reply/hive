use super::*;

fn force_push(preview: &ForcePushPreview) -> Action {
    confirmed(&preview.expected, &preview.head)
}

fn confirmed(expected: &str, head: &str) -> Action {
    Action::ForcePush {
        remote: "origin".into(),
        expected: expected.into(),
        head: head.into(),
    }
}

fn preview(repo: &Repo) -> Result<ForcePushPreview, GitError> {
    force_push_preview(&Host::Local, &repo.0, "origin")
}

/// A published `main` whose second commit was amended after it was pushed.
fn rewritten() -> (Repo, Repo) {
    let (repo, remote) = published();
    repo.write("ours", "ours\n");
    repo.commit();
    repo.git(&["push", "origin", "main"]);
    repo.write("ours", "amended\n");
    repo.git(&["add", "ours"]);
    repo.git(&["commit", "--amend", "-m", "rewritten"]);
    (repo, remote)
}

#[test]
fn force_push_replaces_the_remote_commits_the_preview_listed() {
    let (repo, remote) = rewritten();
    let published = remote.git(&["rev-parse", "main"]);

    let preview = preview(&repo).unwrap();
    assert_eq!(preview.upstream, "origin/main");
    assert_eq!(preview.expected, published);
    assert_eq!(preview.head, repo.git(&["rev-parse", "HEAD"]));
    assert_eq!(preview.total, 1);
    assert_eq!(preview.discarded.len(), 1);
    assert!(preview.discarded[0].ends_with(" fixture"), "{preview:?}");

    assert_eq!(
        repo.action(force_push(&preview)).unwrap(),
        Some(Synced::ForcePushed {
            remote: "origin".into()
        })
    );
    assert_eq!(
        repo.git(&["rev-parse", "HEAD"]),
        remote.git(&["rev-parse", "main"])
    );
    assert_eq!(remote.git(&["show", "main:ours"]), "amended");
}

#[test]
fn force_push_is_refused_once_the_remote_moved_past_the_confirmed_commit() {
    let (repo, remote) = rewritten();
    let shown = preview(&repo).unwrap();
    let other = colleague(&remote);
    other.write("theirs", "theirs\n");
    other.commit();
    other.git(&["push", "origin", "HEAD:main"]);
    let moved = remote.git(&["rev-parse", "main"]);
    // A fetch from anywhere, such as another worktree of this repository, updates the
    // remote-tracking branch; the confirmation was about the commit shown then.
    repo.git(&["fetch", "origin"]);

    let error = repo.action(force_push(&shown)).unwrap_err().to_string();
    assert!(
        error.contains("origin/main changed since you confirmed"),
        "{error}"
    );
    assert_eq!(remote.git(&["rev-parse", "main"]), moved);
    // A fresh look names both commits it would now discard.
    assert_eq!(preview(&repo).unwrap().total, 2);
}

#[test]
fn force_push_is_refused_once_the_local_branch_moved_after_the_preview() {
    let (repo, remote) = rewritten();
    let published = remote.git(&["rev-parse", "main"]);
    let preview = preview(&repo).unwrap();
    // An agent in the same worktree commits while the confirmation is open.
    repo.write("later", "unreviewed\n");
    repo.commit();

    let error = repo.action(force_push(&preview)).unwrap_err().to_string();
    assert!(error.contains("main moved since you confirmed"), "{error}");
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
}

#[test]
fn the_preview_asks_the_remote_rather_than_trusting_the_last_fetch() {
    let (repo, remote) = rewritten();
    let other = colleague(&remote);
    other.write("theirs", "theirs\n");
    other.commit();
    other.git(&["push", "origin", "HEAD:main"]);

    let error = preview(&repo).unwrap_err().to_string();
    assert!(
        error.contains("origin/main moved since the last fetch"),
        "{error}"
    );

    // Merged and deleted on the hosting service; a plain fetch keeps the tracking ref.
    remote.git(&["update-ref", "-d", "refs/heads/main"]);
    repo.git(&["fetch", "origin"]);
    let error = preview(&repo).unwrap_err().to_string();
    assert!(
        error.contains("origin/main was deleted on the remote"),
        "{error}"
    );
}

#[test]
fn force_push_never_replaces_a_remote_branch_of_another_name() {
    let (repo, remote) = published();
    repo.git(&["switch", "-c", "topic", "--track", "origin/main"]);
    repo.write("ours", "ours\n");
    repo.commit();
    let published = remote.git(&["rev-parse", "main"]);
    let head = repo.git(&["rev-parse", "HEAD"]);

    for error in [
        preview(&repo).unwrap_err().to_string(),
        repo.action(confirmed(&published, &head))
            .unwrap_err()
            .to_string(),
    ] {
        assert!(
            error.contains("only to the remote branch of the same name"),
            "{error}"
        );
        assert!(error.contains("topic tracks origin/main"), "{error}");
    }
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
}

#[test]
fn force_push_refuses_unborn_detached_unpublished_and_unconfirmed_branches() {
    let refused = |repo: &Repo, expected: &str, head: &str, message: &str| {
        let error = repo
            .action(confirmed(expected, head))
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{error}");
    };
    let (repo, remote) = rewritten();
    let published = remote.git(&["rev-parse", "main"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    // The lease is the confirmation: without both commit ids there is nothing to bind it to.
    for (expected, local) in [
        ("", head.as_str()),
        ("--delete", head.as_str()),
        ("main", head.as_str()),
        (published.as_str(), ""),
        (published.as_str(), "HEAD"),
    ] {
        refused(
            &repo,
            expected,
            local,
            "Review the remote branch again before force pushing",
        );
    }
    repo.git(&["switch", "-c", "unpublished"]);
    refused(&repo, &published, &head, "Publish this branch first");
    repo.git(&["switch", "--detach"]);
    refused(
        &repo,
        &published,
        &head,
        "Create a branch before pushing detached HEAD",
    );
    assert_eq!(remote.git(&["rev-parse", "main"]), published);

    let unborn = Repo::new();
    remote_for(&unborn);
    refused(&unborn, &published, &head, "Create the first commit first");
}

#[test]
fn a_rejecting_pre_push_hook_leaves_the_remote_unchanged() {
    let (repo, remote) = rewritten();
    let published = remote.git(&["rev-parse", "main"]);
    let preview = preview(&repo).unwrap();
    reject_hook(&repo, "pre-push");

    let error = repo.action(force_push(&preview)).unwrap_err();
    assert!(error.stderr().contains("rejected-by-fixture"), "{error}");
    assert_eq!(remote.git(&["rev-parse", "main"]), published);
}

#[test]
fn the_preview_lists_only_the_newest_discarded_commits_and_counts_them_all() {
    let (repo, remote) = published();
    let base = repo.git(&["rev-parse", "HEAD"]);
    for index in 0..10 {
        repo.write("ours", &format!("{index}\n"));
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-m", &format!("change {index}")]);
    }
    repo.git(&["push", "origin", "main"]);
    repo.git(&["reset", "--hard", &base]);
    repo.write("ours", "replacement\n");
    repo.commit();

    let preview = preview(&repo).unwrap();
    assert_eq!(preview.total, 10);
    assert_eq!(preview.discarded.len(), 8);
    assert!(preview.discarded[0].ends_with(" change 9"), "{preview:?}");
    assert_eq!(preview.expected, remote.git(&["rev-parse", "main"]));
}
