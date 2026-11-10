use super::*;
use crate::git::conflicts::{self, Conflict, Operation, Side};

/// `main` and `feature` each rewrite `a`, and `feature` is being merged into `main`.
fn conflicted_merge() -> Repo {
    let repo = diverged(
        &[("a", "base\n")],
        &[("a", "feature\n")],
        &[("a", "main\n")],
    );
    assert!(run(&Host::Local, &repo.0, &["merge", "feature"]).is_err());
    repo
}

/// A base commit, then one commit on `feature` and one on `main`; an empty list adds none,
/// for a side the test commits itself.
fn diverged(base: &[(&str, &str)], feature: &[(&str, &str)], main: &[(&str, &str)]) -> Repo {
    let repo = Repo::new();
    let apply = |files: &[(&str, &str)]| {
        if files.is_empty() {
            return;
        }
        for (path, content) in files {
            repo.write(path, content);
        }
        repo.commit();
    };
    apply(base);
    repo.git(&["checkout", "-b", "feature"]);
    apply(feature);
    repo.git(&["checkout", "main"]);
    apply(main);
    repo
}

fn conflict(repo: &Repo, path: &str) -> Option<Conflict> {
    status::read(&Host::Local, &repo.0)
        .unwrap()
        .files
        .iter()
        .find(|file| file.path == path)
        .and_then(|file| file.conflict)
}

fn operation(repo: &Repo) -> Option<Operation> {
    conflicts::operation(&Host::Local, &repo.0).unwrap()
}

fn resolve(repo: &Repo, path: &str, side: Side) -> Result<Option<Synced>, GitError> {
    repo.action(Action::ResolveWith {
        path: path.into(),
        side,
    })
}

fn read(repo: &Repo, path: &str) -> String {
    std::fs::read_to_string(repo.0.join(path)).unwrap()
}

#[test]
fn unfinished_operation_is_named_and_blocks_checkout() {
    let repo = Repo::new();
    repo.write("a", "base\n");
    repo.commit();
    assert_eq!(operation(&repo), None);
    branches::require_clean(&Host::Local, &repo.0).unwrap();

    let merge = conflicted_merge();
    assert_eq!(operation(&merge), Some(Operation::Merge));

    let pick = diverged(
        &[("a", "base\n")],
        &[("a", "feature\n")],
        &[("a", "main\n")],
    );
    assert!(run(&Host::Local, &pick.0, &["cherry-pick", "feature"]).is_err());
    assert_eq!(operation(&pick), Some(Operation::CherryPick));

    let revert = Repo::new();
    for content in ["1\n", "2\n", "3\n"] {
        revert.write("a", content);
        revert.commit();
    }
    assert!(run(&Host::Local, &revert.0, &["revert", "HEAD~1"]).is_err());
    assert_eq!(operation(&revert), Some(Operation::Revert));

    // A rebase replays commits as cherry-picks; it must still read as a rebase.
    let rebase = diverged(
        &[("a", "base\n")],
        &[("a", "feature\n")],
        &[("a", "main\n")],
    );
    rebase.git(&["checkout", "feature"]);
    assert!(run(&Host::Local, &rebase.0, &["rebase", "main"]).is_err());
    assert_eq!(operation(&rebase), Some(Operation::Rebase));

    // A sequence paused between commits has no head marker but is still unfinished.
    std::fs::create_dir(repo.0.join(".git/sequencer")).unwrap();
    assert_eq!(operation(&repo), None);
    assert!(branches::require_clean(&Host::Local, &repo.0).is_err());
}

#[test]
fn whole_file_resolution_takes_one_side_of_a_content_conflict() {
    for (side, expected) in [(Side::Ours, "main\n"), (Side::Theirs, "feature\n")] {
        let repo = conflicted_merge();
        assert_eq!(conflict(&repo, "a"), Some(Conflict::BothModified));
        assert!(read(&repo, "a").contains("<<<<<<<"));
        resolve(&repo, "a", side).unwrap();
        assert_eq!(read(&repo, "a"), expected);
        assert_eq!(repo.git(&["show", ":a"]), expected.trim_end());
        assert!(!status::read(&Host::Local, &repo.0).unwrap().has_conflicts());
        // The merge itself is still open until it is committed.
        assert_eq!(operation(&repo), Some(Operation::Merge));
    }
}

#[test]
fn whole_file_resolution_keeps_or_deletes_a_file_one_side_removed() {
    let deleted_by_them = || {
        let repo = diverged(&[("a", "base\n"), ("keep", "k\n")], &[], &[("a", "main\n")]);
        repo.git(&["checkout", "feature"]);
        repo.git(&["rm", "-q", "a"]);
        repo.git(&["commit", "-m", "delete"]);
        repo.git(&["checkout", "main"]);
        assert!(run(&Host::Local, &repo.0, &["merge", "feature"]).is_err());
        assert_eq!(conflict(&repo, "a"), Some(Conflict::DeletedByThem));
        repo
    };
    let repo = deleted_by_them();
    resolve(&repo, "a", Side::Ours).unwrap();
    assert_eq!(read(&repo, "a"), "main\n");
    assert!(!status::read(&Host::Local, &repo.0).unwrap().has_conflicts());

    let repo = deleted_by_them();
    resolve(&repo, "a", Side::Theirs).unwrap();
    assert!(!repo.0.join("a").exists());
    assert!(!status::read(&Host::Local, &repo.0).unwrap().has_conflicts());
    assert!(repo.git(&["ls-files", "a"]).is_empty());

    // The mirror case: we deleted what they changed, so their version sits in the worktree.
    let repo = diverged(
        &[("a", "base\n"), ("keep", "k\n")],
        &[("a", "feature\n")],
        &[],
    );
    repo.git(&["rm", "-q", "a"]);
    repo.git(&["commit", "-m", "delete"]);
    assert!(run(&Host::Local, &repo.0, &["merge", "feature"]).is_err());
    assert_eq!(conflict(&repo, "a"), Some(Conflict::DeletedByUs));
    resolve(&repo, "a", Side::Ours).unwrap();
    assert!(!repo.0.join("a").exists());
    assert!(!status::read(&Host::Local, &repo.0).unwrap().has_conflicts());
}

#[test]
fn whole_file_resolution_handles_both_added_and_literal_paths() {
    let path = "new [1] *.txt";
    let repo = diverged(
        &[("a", "base\n")],
        &[(path, "feature\n")],
        &[(path, "main\n")],
    );
    repo.write("new 2.txt", "bystander\n");
    assert!(run(&Host::Local, &repo.0, &["merge", "feature"]).is_err());
    assert_eq!(conflict(&repo, path), Some(Conflict::BothAdded));
    resolve(&repo, path, Side::Theirs).unwrap();
    assert_eq!(read(&repo, path), "feature\n");
    // The glob characters are literal: a matching neighbour is left alone.
    assert_eq!(read(&repo, "new 2.txt"), "bystander\n");
    assert!(repo.git(&["ls-files", "new 2.txt"]).is_empty());
}

#[test]
fn whole_file_resolution_refuses_paths_that_are_not_in_conflict() {
    let repo = conflicted_merge();
    repo.write("other", "untracked\n");
    assert!(matches!(
        resolve(&repo, "other", Side::Ours),
        Err(GitError::Invalid(_))
    ));
    assert!(resolve(&repo, "missing", Side::Theirs).is_err());
    assert!(resolve(&repo, "../outside", Side::Theirs).is_err());
    assert_eq!(read(&repo, "other"), "untracked\n");
    // Once resolved, a second request must not check out a side again over new edits.
    resolve(&repo, "a", Side::Ours).unwrap();
    repo.write("a", "edited after resolving\n");
    assert!(resolve(&repo, "a", Side::Theirs).is_err());
    assert_eq!(read(&repo, "a"), "edited after resolving\n");
}

#[test]
fn abort_restores_the_state_before_a_merge_or_rebase() {
    let repo = conflicted_merge();
    let head = repo.git(&["rev-parse", "HEAD"]);
    repo.action(Action::AbortOperation).unwrap();
    assert_eq!(operation(&repo), None);
    assert_eq!(read(&repo, "a"), "main\n");
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert!(status::read(&Host::Local, &repo.0).unwrap().is_clean());
    // Nothing is left to abort or continue.
    assert!(repo.action(Action::AbortOperation).is_err());
    assert!(repo.action(Action::ContinueOperation).is_err());

    let repo = diverged(
        &[("a", "base\n")],
        &[("a", "feature\n")],
        &[("a", "main\n")],
    );
    repo.git(&["checkout", "feature"]);
    let head = repo.git(&["rev-parse", "HEAD"]);
    assert!(run(&Host::Local, &repo.0, &["rebase", "main"]).is_err());
    repo.action(Action::AbortOperation).unwrap();
    assert_eq!(operation(&repo), None);
    assert_eq!(repo.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(repo.git(&["branch", "--show-current"]), "feature");
}

#[test]
fn continue_concludes_a_merge_without_opening_an_editor() {
    let repo = conflicted_merge();
    // An editor that fails would abort the commit if Git were allowed to start it.
    repo.git(&["config", "core.editor", "false"]);
    assert!(matches!(
        repo.action(Action::ContinueOperation),
        Err(GitError::Invalid(_))
    ));
    assert_eq!(operation(&repo), Some(Operation::Merge));
    repo.write("a", "resolved\n");
    repo.action(Action::Stage(vec!["a".into()])).unwrap();
    repo.action(Action::ContinueOperation).unwrap();
    assert_eq!(operation(&repo), None);
    let parents = repo.git(&["log", "-1", "--format=%P"]);
    assert_eq!(parents.split_whitespace().count(), 2);
    assert!(repo.git(&["log", "-1", "--format=%s"]).contains("feature"));
    assert!(status::read(&Host::Local, &repo.0).unwrap().is_clean());
}

#[test]
fn continue_stops_at_the_next_conflicting_commit_without_failing() {
    let repo = Repo::new();
    repo.write("a", "base\n");
    repo.commit();
    repo.git(&["checkout", "-b", "feature"]);
    for content in ["feature 1\n", "feature 2\n"] {
        repo.write("a", content);
        repo.commit();
    }
    repo.git(&["checkout", "main"]);
    repo.write("a", "main\n");
    repo.commit();
    repo.git(&["checkout", "feature"]);
    repo.git(&["config", "core.editor", "false"]);
    assert!(run(&Host::Local, &repo.0, &["rebase", "main"]).is_err());

    repo.write("a", "resolved 1\n");
    repo.action(Action::Stage(vec!["a".into()])).unwrap();
    repo.action(Action::ContinueOperation).unwrap();
    // The second commit conflicts with the first resolution: still rebasing, new conflict.
    assert_eq!(operation(&repo), Some(Operation::Rebase));
    assert_eq!(conflict(&repo, "a"), Some(Conflict::BothModified));

    repo.write("a", "resolved 2\n");
    repo.action(Action::Stage(vec!["a".into()])).unwrap();
    repo.action(Action::ContinueOperation).unwrap();
    assert_eq!(operation(&repo), None);
    assert_eq!(read(&repo, "a"), "resolved 2\n");
    assert_eq!(repo.git(&["branch", "--show-current"]), "feature");
    assert_eq!(repo.git(&["rev-list", "--count", "main..HEAD"]), "2");
}

#[test]
fn continue_reports_a_failure_that_is_not_a_new_conflict() {
    let repo = diverged(
        &[("a", "base\n")],
        &[("a", "feature\n")],
        &[("a", "main\n")],
    );
    assert!(run(&Host::Local, &repo.0, &["cherry-pick", "feature"]).is_err());
    // Taking our side leaves nothing to commit, which Git refuses to continue with.
    resolve(&repo, "a", Side::Ours).unwrap();
    let error = repo.action(Action::ContinueOperation).unwrap_err();
    assert!(matches!(error, GitError::Failed { .. }), "{error}");
    assert_eq!(operation(&repo), Some(Operation::CherryPick));
}
