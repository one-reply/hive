use super::*;
use crate::git::{
    diff::{self, DiffSource},
    history,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
mod line_stats;

struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hive-git-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        let repo = Self(path.canonicalize().unwrap());
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.name", "Hive Test"]);
        repo.git(&["config", "user.email", "hive@example.invalid"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "core.hooksPath", "/dev/null"]);
        repo
    }
    fn git(&self, args: &[&str]) -> String {
        run(&Host::Local, &self.0, args).unwrap()
    }
    fn write(&self, path: &str, value: &str) {
        std::fs::write(self.0.join(path), value).unwrap();
    }
    fn action(&self, action: Action) -> Result<(), GitError> {
        execute(&Host::Local, &self.0, &action)
    }
    fn commit(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", "fixture"]);
    }
    /// Commits with a fixed committer date, so branch ordering is deterministic.
    fn commit_at(&self, date: &str) {
        self.git(&["add", "-A"]);
        let status = std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "fixture"])
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_AUTHOR_DATE", date)
            .current_dir(&self.0)
            .status()
            .unwrap();
        assert!(status.success());
    }
}

#[test]
fn delete_branch_refuses_checked_out_and_unmerged_unless_forced() {
    let repo = Repo::new();
    repo.write("a", "1");
    repo.commit();
    repo.git(&["branch", "merged"]);
    repo.git(&["checkout", "-b", "unmerged"]);
    repo.write("a", "2");
    repo.commit();
    repo.git(&["checkout", "main"]);
    assert!(branches::is_merged(&Host::Local, &repo.0, "merged").unwrap());
    assert!(!branches::is_merged(&Host::Local, &repo.0, "unmerged").unwrap());
    let delete = |name: &str, force| {
        repo.action(Action::DeleteBranch {
            name: name.into(),
            force,
        })
    };
    assert!(delete("main", true).is_err());
    assert!(delete("unmerged", false).is_err());
    assert!(delete("missing", false).is_err());
    delete("merged", false).unwrap();
    delete("unmerged", true).unwrap();
    let names: Vec<_> = branches::list(&Host::Local, &repo.0)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert_eq!(names, ["main"]);
}

#[test]
fn branches_list_newest_commit_first() {
    let repo = Repo::new();
    repo.write("a", "1");
    repo.commit_at("2020-01-01T00:00:00Z");
    repo.git(&["branch", "old"]);
    repo.git(&["checkout", "-b", "fresh"]);
    repo.write("a", "2");
    repo.commit_at("2024-06-01T00:00:00Z");
    let names: Vec<_> = branches::list(&Host::Local, &repo.0)
        .unwrap()
        .into_iter()
        .map(|b| b.name)
        .collect();
    assert_eq!(names, ["fresh", "main", "old"]);
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn hunk_staging_preserves_worktree_and_rejects_stale_review() {
    let repo = Repo::new();
    let original = (1..=30)
        .map(|n| format!("line {n}\r\n"))
        .collect::<String>();
    repo.write("файл [1].txt", &original);
    repo.commit();
    let changed = original
        .replace("line 2\r", "second\r")
        .replace("line 28\r", "last\r");
    repo.write("файл [1].txt", &changed);
    let reviewed = diff::read(
        &Host::Local,
        &repo.0,
        "файл [1].txt",
        &DiffSource::WorkingTree,
        None,
    )
    .unwrap();
    assert_eq!(reviewed.hunks().len(), 2);
    diff::apply_hunk(
        &Host::Local,
        &repo.0,
        "файл [1].txt",
        &DiffSource::WorkingTree,
        &reviewed,
        0,
    )
    .unwrap();
    let indexed = repo.git(&["show", ":файл [1].txt"]);
    assert!(indexed.contains("second"));
    assert!(!indexed.contains("last"));
    assert_eq!(
        std::fs::read_to_string(repo.0.join("файл [1].txt")).unwrap(),
        changed
    );
    assert!(
        diff::apply_hunk(
            &Host::Local,
            &repo.0,
            "файл [1].txt",
            &DiffSource::WorkingTree,
            &reviewed,
            1
        )
        .is_err()
    );
    let staged = diff::read(
        &Host::Local,
        &repo.0,
        "файл [1].txt",
        &DiffSource::Staged,
        None,
    )
    .unwrap();
    diff::apply_hunk(
        &Host::Local,
        &repo.0,
        "файл [1].txt",
        &DiffSource::Staged,
        &staged,
        0,
    )
    .unwrap();
    assert!(repo.git(&["diff", "--cached"]).is_empty());
}

#[test]
fn literal_paths_unborn_unstage_discard_and_index_only_commit() {
    let repo = Repo::new();
    let path = "-[x] проба\n.txt";
    repo.write(path, "one");
    repo.action(Action::Stage(vec![path.into()])).unwrap();
    repo.action(Action::Unstage(vec![path.into()])).unwrap();
    assert!(repo.0.join(path).exists());
    repo.action(Action::Stage(vec![path.into()])).unwrap();
    repo.action(Action::Commit("initial".into())).unwrap();
    repo.write(path, "two");
    repo.action(Action::Stage(vec![path.into()])).unwrap();
    repo.write(path, "three");
    repo.action(Action::Discard(path.into())).unwrap();
    assert_eq!(std::fs::read_to_string(repo.0.join(path)).unwrap(), "two");
    repo.write(path, "three");
    repo.action(Action::Commit("index only".into())).unwrap();
    assert_eq!(repo.git(&["show", &format!("HEAD:{path}")]), "two");
    assert_eq!(std::fs::read_to_string(repo.0.join(path)).unwrap(), "three");
    repo.write("new", "keep");
    assert!(repo.action(Action::Discard("new".into())).is_err());
}

#[test]
fn history_compare_and_branch_guards() {
    let repo = Repo::new();
    repo.write("a", "base\n");
    repo.commit();
    let root = repo.git(&["rev-parse", "HEAD"]);
    assert_eq!(
        history::commit_files(&Host::Local, &repo.0, &root)
            .unwrap()
            .len(),
        1
    );
    repo.action(Action::CreateBranch {
        name: "feature".into(),
        base: "HEAD".into(),
        track: false,
    })
    .unwrap();
    repo.write("a", "feature\n");
    repo.commit();
    assert_eq!(
        history::compare_files(&Host::Local, &repo.0, "main")
            .unwrap()
            .len(),
        1
    );
    let log = history::read(&Host::Local, &repo.0, "HEAD", 0).unwrap();
    assert_eq!(log.len(), 2);
    repo.write("dirty", "x");
    assert!(repo.action(Action::Switch("main".into())).is_err());
    std::fs::remove_file(repo.0.join("dirty")).unwrap();
    let other = repo.0.join("other");
    repo.git(&["worktree", "add", other.to_str().unwrap(), "main"]);
    let branches = branches::list(&Host::Local, &repo.0).unwrap();
    assert_eq!(
        branches.iter().find(|b| b.name == "main").unwrap().worktree,
        Some(other)
    );
    assert!(repo.action(Action::Switch("main".into())).is_err());
    repo.git(&["checkout", "--detach"]);
    assert!(
        status::read(&Host::Local, &repo.0)
            .unwrap()
            .branch
            .is_none()
    );
}

#[test]
fn binary_rename_deletion_and_missing_newline() {
    let repo = Repo::new();
    repo.write("a", "no newline");
    repo.write("b", "binary\0one");
    repo.commit();
    repo.write("a", "changed");
    repo.write("b", "binary\0two");
    let patch = diff::read(&Host::Local, &repo.0, "a", &DiffSource::WorkingTree, None).unwrap();
    assert!(patch.patch().contains("No newline at end of file"));
    diff::apply_hunk(
        &Host::Local,
        &repo.0,
        "a",
        &DiffSource::WorkingTree,
        &patch,
        0,
    )
    .unwrap();
    assert_eq!(repo.git(&["show", ":a"]), "changed");
    assert!(
        diff::read(&Host::Local, &repo.0, "b", &DiffSource::WorkingTree, None)
            .unwrap()
            .hunks()
            .is_empty()
    );
    repo.git(&["commit", "-m", "text change"]);
    repo.git(&["mv", "a", "renamed"]);
    let rename = diff::read(
        &Host::Local,
        &repo.0,
        "renamed",
        &DiffSource::Staged,
        Some("a"),
    )
    .unwrap();
    assert!(rename.patch().contains("rename from a"));
    assert!(rename.patch().contains("rename to renamed"));
    assert!(!rename.can_stage_hunks());
    let status = status::read(&Host::Local, &repo.0).unwrap();
    assert!(
        status
            .files
            .iter()
            .any(|f| f.old_path.as_deref() == Some("a"))
    );
    std::fs::remove_file(repo.0.join("b")).unwrap();
    assert!(
        status::read(&Host::Local, &repo.0)
            .unwrap()
            .files
            .iter()
            .any(|f| f.unstaged == Some(status::Change::Deleted))
    );
}

#[test]
fn added_and_deleted_text_hunks_preserve_working_files() {
    let repo = Repo::new();
    repo.write("new", "new text\n");
    repo.git(&["add", "new"]);
    let patch = diff::read(&Host::Local, &repo.0, "new", &DiffSource::Staged, None).unwrap();
    diff::apply_hunk(&Host::Local, &repo.0, "new", &DiffSource::Staged, &patch, 0).unwrap();
    assert_eq!(repo.git(&["ls-files"]), "");
    assert!(repo.0.join("new").exists());
    repo.commit();
    std::fs::remove_file(repo.0.join("new")).unwrap();
    let patch = diff::read(&Host::Local, &repo.0, "new", &DiffSource::WorkingTree, None).unwrap();
    diff::apply_hunk(
        &Host::Local,
        &repo.0,
        "new",
        &DiffSource::WorkingTree,
        &patch,
        0,
    )
    .unwrap();
    assert_eq!(repo.git(&["ls-files"]), "");
    assert!(!repo.0.join("new").exists());
}

#[test]
fn unrelated_history_reports_no_common_ancestor() {
    let repo = Repo::new();
    repo.write("first", "first\n");
    repo.commit();
    repo.git(&["switch", "--orphan", "unrelated"]);
    repo.write("other", "other\n");
    repo.commit();
    let error = history::compare_files(&Host::Local, &repo.0, "main").unwrap_err();
    assert!(error.to_string().contains("no available common ancestor"));
}

#[test]
fn remote_publish_fetch_pull_and_divergence() {
    let repo = Repo::new();
    repo.write("a", "base");
    repo.commit();
    let remote = Repo::new();
    remote.git(&["config", "core.bare", "true"]);
    repo.git(&["remote", "add", "origin", remote.0.to_str().unwrap()]);
    repo.action(Action::Push {
        remote: "origin".into(),
        publish: true,
    })
    .unwrap();
    assert_eq!(
        status::read(&Host::Local, &repo.0)
            .unwrap()
            .upstream
            .as_deref(),
        Some("origin/main")
    );
    let second = Repo::new();
    second.git(&["remote", "add", "origin", remote.0.to_str().unwrap()]);
    second.action(Action::Fetch("origin".into())).unwrap();
    second.git(&["reset", "--hard", "origin/main"]);
    second.git(&["branch", "--set-upstream-to=origin/main"]);
    second.write("remote", "new");
    second.commit();
    second
        .action(Action::Push {
            remote: "origin".into(),
            publish: false,
        })
        .unwrap();
    repo.action(Action::Pull).unwrap();
    assert!(repo.0.join("remote").exists());
    repo.write("local", "local");
    repo.commit();
    second.write("remote", "again");
    second.commit();
    second
        .action(Action::Push {
            remote: "origin".into(),
            publish: false,
        })
        .unwrap();
    assert!(repo.action(Action::Pull).is_err());
    assert!(
        repo.action(Action::Push {
            remote: "origin".into(),
            publish: false
        })
        .is_err()
    );
}

#[test]
fn failed_hook_keeps_index_and_reports_both_output_streams() {
    let repo = Repo::new();
    repo.write("a", "staged");
    repo.action(Action::Stage(vec!["a".into()])).unwrap();
    let hooks = repo.0.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("pre-commit");
    std::fs::write(
        &hook,
        "#!/bin/sh\necho hook-stdout\necho hook-stderr >&2\nexit 1\n",
    )
    .unwrap();
    assert!(
        Host::Local
            .command("chmod", &["+x", hook.to_str().unwrap()], None)
            .status()
            .unwrap()
            .success()
    );
    repo.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let error = repo
        .action(Action::Commit("keep this draft".into()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("hook-stdout") && error.contains("hook-stderr"));
    let status = status::read(&Host::Local, &repo.0).unwrap();
    assert!(status.oid.is_none());
    assert_eq!(status.files[0].staged, Some(status::Change::Added));
}

#[test]
fn merge_conflicts_and_first_parent_review() {
    let repo = Repo::new();
    repo.write("a", "base\n");
    repo.commit();
    repo.git(&["checkout", "-b", "feature"]);
    repo.write("a", "feature\n");
    repo.commit();
    repo.git(&["checkout", "main"]);
    repo.write("a", "main\n");
    repo.commit();
    assert!(run(&Host::Local, &repo.0, &["merge", "feature"]).is_err());
    assert!(status::read(&Host::Local, &repo.0).unwrap().has_conflicts());
    assert!(repo.action(Action::Switch("feature".into())).is_err());
    assert!(repo.action(Action::Commit("blocked".into())).is_err());
    repo.write("a", "resolved\n");
    repo.action(Action::Stage(vec!["a".into()])).unwrap();
    // Even a clean index after resolving cannot switch away from an unfinished merge.
    repo.action(Action::Commit("merge".into())).unwrap();
    let files = history::commit_files(&Host::Local, &repo.0, "HEAD").unwrap();
    assert_eq!(files.len(), 1);
    let patch = diff::read(
        &Host::Local,
        &repo.0,
        "a",
        &DiffSource::Commit("HEAD".into()),
        None,
    )
    .unwrap();
    assert!(patch.patch().contains("-main") && patch.patch().contains("+resolved"));
}

#[test]
fn clean_checkout_still_rejects_unfinished_rebase_and_large_preview() {
    let repo = Repo::new();
    repo.write("a", "base\n");
    repo.commit();
    let marker = repo.0.join(".git/rebase-merge");
    std::fs::create_dir(&marker).unwrap();
    assert!(branches::require_clean(&Host::Local, &repo.0).is_err());
    std::fs::remove_dir(&marker).unwrap();
    repo.write("large", &"x".repeat(diff::PREVIEW_LIMIT + 1));
    assert!(matches!(
        diff::read(&Host::Local, &repo.0, "large", &DiffSource::Untracked, None),
        Err(GitError::TooLarge(_))
    ));
    assert!(
        diff::read(
            &Host::Local,
            &repo.0,
            "../outside",
            &DiffSource::WorkingTree,
            None
        )
        .is_err()
    );
}
