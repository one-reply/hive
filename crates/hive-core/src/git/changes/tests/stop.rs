//! Stopping a mutation, and telling one that never ran from one that ended midway: only the
//! second leaves an outcome nobody knows.
use super::*;
use crate::git::conflicts::{Operation, operation};
use crate::git::{bytes_mutation, run_mutation};
use std::time::Duration;

/// Runs `action` and stops it while the hook `name` is running.
fn stopped_during(repo: &Repo, name: &str, action: &Action) -> GitError {
    let started = testing::stalling_hook(&repo.0, name);
    let stop = Cancellation::default();
    std::thread::scope(|scope| {
        let running = scope.spawn(|| execute(&Host::Local, &repo.0, action, &stop));
        started
            .recv_timeout(Duration::from_secs(30))
            .expect("the hook runs");
        stop.cancel();
        running
            .join()
            .unwrap()
            .expect_err("a command stopped midway does not report success")
    })
}

fn committed() -> Repo {
    let repo = Repo::new();
    repo.write("a", "one\n");
    repo.commit();
    repo
}

#[test]
fn a_commit_stopped_while_its_hook_runs_has_an_unknown_outcome() {
    let repo = committed();
    repo.write("a", "two\n");
    repo.action(Action::Stage(vec!["a".into()])).unwrap();
    let error = stopped_during(&repo, "pre-commit", &Action::Commit("change a".into()));
    assert!(error.outcome_unknown(), "{error}");
    assert!(error.to_string().contains("Command cancelled"), "{error}");
}

#[test]
fn a_switch_and_a_new_branch_are_stopped_while_the_checkout_hook_runs() {
    let actions = [
        Action::Switch("other".into()),
        Action::CreateBranch {
            name: "feature".into(),
            base: "other".into(),
            track: false,
        },
    ];
    for action in actions {
        let repo = committed();
        repo.git(&["branch", "other"]);
        let error = stopped_during(&repo, "post-checkout", &action);
        // Git was ended after it moved HEAD and before it finished: only a look at the
        // repository says where that left it.
        assert!(error.outcome_unknown(), "{action:?}: {error}");
    }
}

#[test]
fn a_command_reached_after_the_stop_never_starts_and_leaves_nothing_to_verify() {
    let stopped = |repo: &Repo, action: Action| {
        let stop = Cancellation::default();
        stop.cancel();
        let error = execute(&Host::Local, &repo.0, &action, &stop).unwrap_err();
        assert!(!error.outcome_unknown(), "{action:?}: {error}");
        assert!(
            error.to_string().contains("changed nothing"),
            "{action:?}: {error}"
        );
    };

    let repo = committed();
    repo.git(&["branch", "other"]);
    repo.write("a", "two\n");
    // A quick local mutation ignores the stop.
    let stage = Action::Stage(vec!["a".into()]);
    let stop = Cancellation::default();
    stop.cancel();
    assert!(execute(&Host::Local, &repo.0, &stage, &stop).is_ok());
    stopped(&repo, Action::Commit("change a".into()));
    assert!(repo.git(&["status", "--porcelain"]).starts_with("M  a"));
    assert_eq!(repo.git(&["rev-list", "--count", "HEAD"]), "1");

    let repo = committed();
    repo.git(&["branch", "other"]);
    stopped(&repo, Action::Switch("other".into()));
    stopped(
        &repo,
        Action::CreateBranch {
            name: "feature".into(),
            base: "other".into(),
            track: false,
        },
    );
    assert_eq!(repo.git(&["branch", "--show-current"]), "main");
    assert!(repo.git(&["branch", "--list", "feature"]).is_empty());

    // An unfinished merge is still unfinished after an abort that never ran.
    repo.git(&["switch", "other"]);
    repo.write("a", "theirs\n");
    repo.commit();
    repo.git(&["switch", "main"]);
    repo.write("a", "ours\n");
    repo.commit();
    assert!(run(&Host::Local, &repo.0, &["merge", "other"]).is_err());
    stopped(&repo, Action::AbortOperation);
    assert_eq!(
        operation(&Host::Local, &repo.0).unwrap(),
        Some(Operation::Merge)
    );
    repo.action(Action::AbortOperation).unwrap();
    assert_eq!(operation(&Host::Local, &repo.0).unwrap(), None);
}

#[test]
fn a_mutation_that_cannot_start_changed_nothing() {
    let missing = std::env::temp_dir().join(format!("hive-git-missing-{}", std::process::id()));
    let error = run_mutation(&Host::Local, &missing, &["commit", "-m", "never"]).unwrap_err();
    assert!(matches!(error, GitError::Spawn { .. }), "{error:?}");
    assert!(!error.outcome_unknown(), "{error}");
}

#[test]
fn a_mutation_is_not_ended_for_printing_more_than_is_kept() {
    let repo = committed();
    repo.write("a", "two\n");
    repo.action(Action::Stage(vec!["a".into()])).unwrap();
    // Far more than the 1 KiB kept below, on both streams.
    testing::hook(
        &repo.0,
        "pre-commit",
        "i=0\nwhile [ $i -lt 2000 ]; do\n  echo \"hook output line $i\"\n  echo \"hook error line $i\" >&2\n  i=$((i+1))\ndone\n",
    );
    let kept = bytes_mutation(
        &Host::Local,
        &repo.0,
        &["commit", "-m", "change a"],
        None,
        1024,
        false,
        &Cancellation::default(),
    )
    .expect("the commit is allowed to finish");
    assert!(kept.len() <= 1024);
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "change a");
    // What is kept is the end: Git's own summary follows the hook's output.
    assert!(String::from_utf8_lossy(&kept).contains("change a"));
}
