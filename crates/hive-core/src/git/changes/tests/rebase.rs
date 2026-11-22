use super::*;
use crate::git::conflicts::{self, Operation};

fn rebase(onto: &str) -> Action {
    Action::Rebase { onto: onto.into() }
}

fn rebased(onto: &str, changed: bool, needs_force_push: bool) -> Option<Synced> {
    Some(Synced::Rebased {
        onto: onto.into(),
        changed,
        needs_force_push,
    })
}

/// A published `main` and a `feature` branch one commit ahead of it, checked out.
fn feature() -> (Repo, Repo) {
    let (repo, remote) = published();
    repo.git(&["switch", "-c", "feature"]);
    repo.write("feature", "feature\n");
    repo.commit();
    (repo, remote)
}

/// Pushes one more commit to the remote's `main` from another checkout.
fn advance_main(remote: &Repo, path: &str, content: &str) {
    let other = colleague(remote);
    other.write(path, content);
    other.commit();
    other.git(&["push", "origin", "HEAD:main"]);
}

fn contains(repo: &Repo, ancestor: &str) -> bool {
    run(
        &Host::Local,
        &repo.0,
        &["merge-base", "--is-ancestor", ancestor, "HEAD"],
    )
    .is_ok()
}

#[test]
fn rebase_fetches_the_base_remote_then_replays_local_commits() {
    let (repo, remote) = feature();
    advance_main(&remote, "theirs", "theirs\n");
    let published = remote.git(&["rev-parse", "main"]);

    assert_eq!(
        repo.action(rebase("refs/remotes/origin/main")).unwrap(),
        rebased("origin/main", true, false)
    );
    // The base was fetched first, so the branch sits on what the remote has now.
    assert_eq!(repo.git(&["rev-parse", "origin/main"]), published);
    assert!(contains(&repo, &published));
    assert_eq!(
        repo.git(&["show", "--format=", "--name-only", "HEAD"]),
        "feature"
    );

    assert_eq!(
        repo.action(rebase("refs/remotes/origin/main")).unwrap(),
        rebased("origin/main", false, false)
    );
}

#[test]
fn rebasing_a_published_branch_says_it_needs_a_force_push() {
    let (repo, remote) = feature();
    repo.git(&["push", "--set-upstream", "origin", "feature"]);
    advance_main(&remote, "theirs", "theirs\n");

    let result = repo.action(rebase("refs/remotes/origin/main")).unwrap();
    assert_eq!(result, rebased("origin/main", true, true));
    assert!(result.unwrap().to_string().contains("Force Push"));
    // The remote branch still holds the commit as it was before the replay.
    let local = status::read(&Host::Local, &repo.0).unwrap();
    assert_eq!((local.ahead, local.behind), (2, 1));
}

#[test]
fn rebase_onto_a_local_base_does_not_fetch() {
    let (repo, remote) = feature();
    advance_main(&remote, "theirs", "theirs\n");
    let known = repo.git(&["rev-parse", "origin/main"]);

    assert_eq!(
        repo.action(rebase("refs/heads/main")).unwrap(),
        rebased("main", false, false)
    );
    assert_eq!(repo.git(&["rev-parse", "origin/main"]), known);
}

#[test]
fn rebase_pauses_on_a_conflict_and_abort_restores_the_branch() {
    let (repo, remote) = feature();
    let before = repo.git(&["rev-parse", "HEAD"]);
    advance_main(&remote, "feature", "theirs\n");

    assert_eq!(
        repo.action(rebase("refs/remotes/origin/main")).unwrap(),
        None
    );
    assert_eq!(
        conflicts::operation(&Host::Local, &repo.0).unwrap(),
        Some(Operation::Rebase)
    );
    assert!(status::read(&Host::Local, &repo.0).unwrap().has_conflicts());

    repo.action(Action::AbortOperation).unwrap();
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(repo.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "feature");
}

#[test]
fn rebase_refuses_what_it_cannot_replay_before_fetching() {
    let (repo, remote) = feature();
    advance_main(&remote, "theirs", "theirs\n");
    let known = repo.git(&["rev-parse", "origin/main"]);
    let before = repo.git(&["rev-parse", "HEAD"]);
    let refused = |onto: &str, expected: &str| {
        let error = repo.action(rebase(onto)).unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    };

    // Only a branch Git lists is a base: never an option, a path or an arbitrary revision.
    for unknown in ["refs/heads/missing", "--onto", "HEAD~1", "origin/main"] {
        refused(unknown, "Select an existing branch to rebase onto");
    }
    refused("refs/heads/feature", "feature is the current branch");
    repo.write("untracked", "untracked\n");
    refused(
        "refs/remotes/origin/main",
        "Commit your staged, unstaged and untracked changes",
    );
    std::fs::remove_file(repo.0.join("untracked")).unwrap();
    repo.git(&["switch", "--detach"]);
    refused(
        "refs/remotes/origin/main",
        "Check out a branch before rebasing",
    );
    repo.git(&["switch", "feature"]);

    repo.git(&["switch", "--orphan", "unrelated"]);
    repo.write("other", "other\n");
    repo.commit();
    refused("refs/heads/main", "shares no history");

    repo.git(&["switch", "feature"]);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(conflicts::operation(&Host::Local, &repo.0).unwrap(), None);
    // Nothing was fetched for a rebase that was refused.
    assert_eq!(repo.git(&["rev-parse", "origin/main"]), known);
}

#[test]
fn rebase_refuses_merge_commits_instead_of_flattening_them() {
    let (repo, remote) = feature();
    advance_main(&remote, "theirs", "theirs\n");
    repo.git(&["switch", "-c", "side"]);
    repo.write("side", "side\n");
    repo.commit();
    repo.git(&["switch", "feature"]);
    repo.git(&["merge", "--no-ff", "-m", "merge side", "side"]);
    let merged = repo.git(&["rev-parse", "HEAD"]);

    let error = repo
        .action(rebase("refs/remotes/origin/main"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("merge commits"), "{error}");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), merged);
}

#[test]
fn rebase_leaves_other_branches_alone_when_update_refs_is_configured() {
    let (repo, remote) = feature();
    advance_main(&remote, "theirs", "theirs\n");
    repo.git(&["config", "rebase.updateRefs", "true"]);
    repo.git(&["branch", "marker"]);
    let marker = repo.git(&["rev-parse", "marker"]);
    repo.write("feature", "more\n");
    repo.commit();

    assert_eq!(
        repo.action(rebase("refs/remotes/origin/main")).unwrap(),
        rebased("origin/main", true, false)
    );
    assert_eq!(repo.git(&["rev-parse", "marker"]), marker);
}

#[test]
fn the_base_remote_is_found_when_its_name_contains_a_slash() {
    let repo = Repo::new();
    let remote = Repo::new();
    remote.git(&["config", "core.bare", "true"]);
    repo.git(&["remote", "add", "team/origin", remote.0.to_str().unwrap()]);
    repo.write("base", "base\n");
    repo.commit();
    repo.git(&["push", "team/origin", "main"]);
    repo.git(&["fetch", "team/origin"]);
    repo.git(&["switch", "-c", "feature"]);
    repo.write("feature", "feature\n");
    repo.commit();
    advance_main(&remote, "theirs", "theirs\n");

    assert_eq!(
        repo.action(rebase("refs/remotes/team/origin/main"))
            .unwrap(),
        rebased("team/origin/main", true, false)
    );
    assert!(contains(&repo, &remote.git(&["rev-parse", "main"])));
}

#[test]
fn a_failed_fetch_before_the_rebase_changes_nothing() {
    let (repo, remote) = feature();
    let before = repo.git(&["rev-parse", "HEAD"]);
    // The remote is gone: its directory no longer holds a repository.
    std::fs::remove_dir_all(&remote.0).unwrap();

    assert!(repo.action(rebase("refs/remotes/origin/main")).is_err());
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), before);
    assert_eq!(conflicts::operation(&Host::Local, &repo.0).unwrap(), None);
}
