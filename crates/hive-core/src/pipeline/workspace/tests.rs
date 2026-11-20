use super::*;
use crate::pipeline::{Assignment, Phase, Report, Run, Template};
use std::fs;

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("hive-pipeline-{name}-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn lane(&self, name: &str) -> Lane {
        let path = self.0.join(name);
        fs::create_dir(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        git(&path, &["config", "core.hooksPath", "/dev/null"]);
        fs::write(path.join("contract"), "original\n").unwrap();
        git(&path, &["add", "contract"]);
        git(&path, &["commit", "-qm", "base"]);
        let preview = preview(&path, "main").unwrap();
        assert!(!preview.dirty);
        Lane {
            name: name.into(),
            project: path,
            base: preview.commit,
            ..Lane::default()
        }
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn git(path: &Path, args: &[&str]) {
    let output = Host::Local
        .command("git", args, Some(path))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn multi_repo_checks_and_acceptance_use_task_worktrees_and_report_new_files() {
    let dir = Scratch::new("end-to-end");
    let mut api = dir.lane("api");
    let mut web = dir.lane("web");
    fs::write(api.project.join("local-change"), "keep").unwrap();
    assert!(preview(&api.project, "main").unwrap().dirty);
    api.worktree = prepare(1, &api, &Cancellation::default()).unwrap();
    web.worktree = prepare(1, &web, &Cancellation::default()).unwrap();
    assert_eq!(
        prepare(1, &api, &Cancellation::default()).unwrap(),
        api.worktree
    );
    assert!(!api.worktree.join("local-change").exists());
    api.command = "test -f contract".into();
    web.command = "test -f new-file".into();
    let template = Template {
        reviews: vec![crate::pipeline::Step::new("QA", "Check contract")],
        ..Template::default()
    };
    let mut run = Run::new(
        1,
        api.project.clone(),
        "Change contract".into(),
        template,
        vec![api.clone(), web.clone()],
    )
    .unwrap();
    let plan = run.pending()[0];
    run.report(
        &plan,
        Report::Plan {
            contract: String::new(),
            assignments: (0..2)
                .map(|lane| Assignment {
                    lane,
                    task: "Change".into(),
                    criteria: "Tests pass".into(),
                })
                .collect(),
        },
    )
    .unwrap();
    run.approve().unwrap();
    let dev = run.pending();
    assert_eq!(dev.len(), 2);
    fs::write(api.worktree.join("contract"), "changed\n").unwrap();
    git(&api.worktree, &["commit", "-qam", "updated contract"]);
    for ticket in dev {
        run.report(
            &ticket,
            Report::Developed {
                summary: "Done".into(),
            },
        )
        .unwrap();
    }
    // The checks run before any reviewer: the red one returns only its own lane.
    assert_eq!(run.phase, Phase::Commands(0));
    let first = checks(&run.lanes, &Cancellation::default());
    assert!(first[0].success);
    assert_eq!(first[1].exit_code, Some(1));
    run.command_results(first).unwrap();
    assert_eq!(run.pending().len(), 1);
    assert_eq!(run.pending()[0].lane, Some(1));
    fs::write(web.worktree.join("new-file"), "new contract\n").unwrap();
    run.report(
        &run.pending()[0],
        Report::Developed {
            summary: "Updated".into(),
        },
    )
    .unwrap();
    run.command_results(checks(&run.lanes, &Cancellation::default()))
        .unwrap();
    assert_eq!(run.phase, Phase::Review(0));
    run.report(
        &run.pending()[0],
        Report::Review {
            evidence: vec![],
            summary: "Contracts agree".into(),
            findings: vec![],
        },
    )
    .unwrap();
    assert_eq!(run.phase, Phase::Acceptance);
    let repos = review(1, &run.lanes).unwrap();
    assert_eq!(repos.len(), 2);
    let change = &repos[0].files[0];
    assert_eq!(change.path, "contract");
    assert_eq!(change.added, Some(1));
    assert_eq!(change.removed, Some(1));
    assert!(
        patch(1, &run.lanes[repos[0].lane], change)
            .unwrap()
            .contains("+changed")
    );
    let added = &repos[1].files[0];
    assert_eq!(added.change, Change::Untracked);
    assert_eq!(added.added, Some(1));
    assert!(
        patch(1, &run.lanes[repos[1].lane], added)
            .unwrap()
            .contains("+new contract")
    );
    assert_eq!(
        fs::read_to_string(api.project.join("contract")).unwrap(),
        "original\n"
    );
    assert!(!web.project.join("new-file").exists());
}

#[test]
fn published_branch_must_match_the_review_and_round_diff_only_shows_the_repair() {
    let dir = Scratch::new("published-round");
    let stop = Cancellation::default();
    let mut lane = dir.lane("web");
    lane.branch = "fix/login".into();
    lane.worktree = prepare(52, &lane, &stop).unwrap();
    let cwd = lane.worktree.clone();
    let mut run = Run::new(
        52,
        lane.project.clone(),
        "Login".into(),
        Template::default(),
        vec![lane],
    )
    .unwrap();
    run.start_direct("Returns 401").unwrap();
    for i in 0..30 {
        fs::write(cwd.join(format!("file{i}")), "first\n").unwrap();
    }
    let staged = git::run(&Host::Local, &cwd, &["write-tree"]).unwrap();
    let (_, old) = reviewed_trees(52, run.lanes(), &stop).unwrap();
    assert_eq!(
        git::run(&Host::Local, &cwd, &["write-tree"]).unwrap(),
        staged
    );
    fs::write(cwd.join("file13"), "corrected\n").unwrap();
    let (revision, new) = reviewed_trees(52, run.lanes(), &stop).unwrap();
    run.review_revision = Some(revision);
    run.review_trees = new;
    run.feedback_trees = old;
    run.phase = Phase::Accepted;
    let all = review_round(&run, false).unwrap();
    let round = review_round(&run, true).unwrap();
    assert_eq!(all[0].files.len(), 30);
    assert_eq!(round[0].files.len(), 1);
    assert_eq!(round[0].files[0].path, "file13");
    let diff = patch(52, &run.lanes[0], &round[0].files[0]).unwrap();
    assert!(diff.contains("-first") && diff.contains("+corrected"));
    assert!(
        publication::published_remote(&run, 0)
            .unwrap_err()
            .to_string()
            .contains("Commit")
    );
    git(&cwd, &["add", "-A"]);
    git(&cwd, &["commit", "-qm", "accepted work"]);
    let remote = dir.0.join("origin.git");
    git(&dir.0, &["init", "--bare", "-q", remote.to_str().unwrap()]);
    git(&cwd, &["remote", "add", "origin", remote.to_str().unwrap()]);
    assert!(
        publication::published_remote(&run, 0)
            .unwrap_err()
            .to_string()
            .contains("Push")
    );
    git(&cwd, &["push", "-qu", "origin", "fix/login"]);
    assert_eq!(
        publication::published_remote(&run, 0).unwrap().1,
        "fix/login"
    );
    git(&cwd, &["switch", "-qc", "unrelated"]);
    assert!(
        publication::published_remote(&run, 0)
            .unwrap_err()
            .to_string()
            .contains("another branch")
    );
    git(&cwd, &["switch", "-q", "fix/login"]);
    fs::write(cwd.join("contract"), "different").unwrap();
    git(&cwd, &["commit", "-qam", "later"]);
    assert!(
        publication::published_remote(&run, 0)
            .unwrap_err()
            .to_string()
            .contains("differ")
    );
    release_trees(52, run.lanes()).unwrap();
    assert!(
        git::run(
            &Host::Local,
            &cwd,
            &["for-each-ref", "refs/hive/pipelines/52"]
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn shared_lanes_provision_once_even_when_setup_creates_local_files() {
    let dir = Scratch::new("shared-setup");
    let mut one = dir.lane("repo");
    one.setup = "echo one >> marker".into();
    let two = Lane {
        name: "Web".into(),
        setup: "echo two >> marker".into(),
        ..one.clone()
    };
    let mut run = Run::new(
        44,
        one.project.clone(),
        "Both".into(),
        Template::default(),
        vec![one, two],
    )
    .unwrap();
    let (members, path) = prepare_lanes(&run, 0, &Cancellation::default()).unwrap();
    assert_eq!(members, [0, 1]);
    for i in members {
        run.record_worktree(i, path.clone());
    }
    assert_eq!(fs::read_to_string(path.join("marker")).unwrap(), "one\n");
    for lane in &mut run.lanes {
        lane.assignment = "Assigned".into();
    }
    assert_eq!(run.delivery_lanes(), [0]);
}

#[test]
fn preparation_recovers_only_the_registered_checkout_and_never_recreates_missing_work() {
    let dir = Scratch::new("recovery");
    let mut lane = dir.lane("repo");
    let path = prepare(13, &lane, &Cancellation::default()).unwrap();
    // A crash after Git succeeded but before the checkpoint saved.
    assert_eq!(prepare(13, &lane, &Cancellation::default()).unwrap(), path);
    lane.worktree = path;
    git(&lane.worktree, &["switch", "-qc", "unrelated"]);
    assert!(verify(13, &lane).is_err());
    assert!(prepare(13, &lane, &Cancellation::default()).is_err());
    lane.worktree = dir.0.join("unrelated");
    assert!(verify(13, &lane).is_err());
    assert!(prepare(13, &lane, &Cancellation::default()).is_err());
    assert!(!lane.worktree.exists());
}

#[test]
fn preparation_rejects_a_checkout_that_an_interrupted_add_left_incomplete() {
    let dir = Scratch::new("partial-checkout");
    let lane = dir.lane("repo");
    let path = prepare(14, &lane, &Cancellation::default()).unwrap();
    // A killed `git worktree add` keeps the registration and HEAD but not every file.
    fs::remove_file(path.join("contract")).unwrap();
    let error = prepare(14, &lane, &Cancellation::default()).unwrap_err();
    assert!(error.to_string().contains("missing files"), "{error}");
    let user_file = path.join("manual-edit");
    fs::write(&user_file, "keep me").unwrap();
    assert!(prepare(14, &lane, &Cancellation::default()).is_err());
    assert_eq!(fs::read_to_string(&user_file).unwrap(), "keep me");
    fs::remove_file(user_file).unwrap();
    git(&path, &["checkout", "--", "contract"]);
    assert_eq!(prepare(14, &lane, &Cancellation::default()).unwrap(), path);
}

#[test]
fn unassigned_scripts_and_cancelled_commands_do_not_run() {
    let dir = Scratch::new("commands");
    let lane = Lane {
        name: "unused".into(),
        worktree: dir.0.clone(),
        command: "touch marker".into(),
        ..Lane::default()
    };
    assert!(checks(std::slice::from_ref(&lane), &Cancellation::default()).is_empty());
    let cancel = Cancellation::default();
    cancel.cancel();
    assert!(
        checks(
            &[Lane {
                assignment: "run".into(),
                ..lane
            }],
            &cancel
        )
        .is_empty()
    );
    assert!(!dir.0.join("marker").exists());
}

#[test]
fn revision_changes_after_manual_edits_new_files_binary_changes_and_permission_changes() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Scratch::new("revision");
    let mut lane = dir.lane("binary");
    fs::write(lane.project.join("image"), b"\x00zero").unwrap();
    git(&lane.project, &["add", "image"]);
    git(&lane.project, &["commit", "-qm", "binary"]);
    lane.base = preview(&lane.project, "HEAD").unwrap().commit;
    lane.worktree = prepare(1, &lane, &Cancellation::default()).unwrap();
    let key = || revision(1, std::slice::from_ref(&lane), &Cancellation::default()).unwrap();
    let original = key();
    let file = lane.worktree.join("contract");
    fs::write(&file, b"Edited during QA").unwrap();
    let modified = key();
    assert_ne!(original, modified);
    fs::write(&file, b"original\n").unwrap();
    assert_eq!(original, key());
    let binary = lane.worktree.join("image");
    fs::write(&binary, b"\x00one").unwrap();
    let binary1 = key();
    fs::write(&binary, b"\x00two").unwrap();
    let binary2 = key();
    assert_ne!(binary1, binary2);
    let added = lane.worktree.join("new");
    fs::write(&added, b"\x00A").unwrap();
    let added1 = key();
    fs::write(&added, b"\x00B").unwrap();
    let added2 = key();
    assert_ne!(added1, added2);
    assert_ne!(binary2, added2);
    fs::set_permissions(&added, fs::Permissions::from_mode(0o755)).unwrap();
    let executable = key();
    assert_ne!(added2, executable);
    let link = lane.worktree.join("link");
    std::os::unix::fs::symlink("new", &link).unwrap();
    let linked = key();
    assert_ne!(executable, linked);
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("contract", &link).unwrap();
    assert_ne!(linked, key());
}

#[test]
fn large_text_changes_are_fingerprinted_beyond_the_display_budget() {
    let dir = Scratch::new("large");
    let mut lane = dir.lane("repo");
    lane.worktree = prepare(1, &lane, &Cancellation::default()).unwrap();
    let path = lane.worktree.join("contract");
    let mut text = vec![b'x'; git::diff::PREVIEW_LIMIT + 1];
    fs::write(&path, &text).unwrap();
    let lanes = std::slice::from_ref(&lane);
    let reviews = review(1, lanes).unwrap();
    assert!(patch(1, &lane, &reviews[0].files[0]).is_err());
    let a = revision(1, lanes, &Cancellation::default()).unwrap();
    *text.last_mut().unwrap() = b'y';
    fs::write(&path, &text).unwrap();
    assert_ne!(revision(1, lanes, &Cancellation::default()).unwrap(), a);
}

#[test]
fn file_review_uses_both_rename_paths_and_preserves_untracked_replacements() {
    let dir = Scratch::new("file-review");
    let mut lane = dir.lane("repo");
    lane.worktree = prepare(1, &lane, &Cancellation::default()).unwrap();
    git(&lane.worktree, &["mv", "contract", "new-name"]);
    let listings = review(1, std::slice::from_ref(&lane)).unwrap();
    let renamed = &listings[0].files[0];
    assert_eq!(renamed.change, Change::Renamed);
    assert_eq!(renamed.path, "new-name");
    let renamed_patch = patch(1, &lane, renamed).unwrap();
    assert!(renamed_patch.contains("rename from contract"));
    assert!(renamed_patch.contains("rename to new-name"));

    git(&lane.worktree, &["mv", "new-name", "contract"]);
    git(&lane.worktree, &["rm", "--cached", "contract"]);
    fs::write(lane.worktree.join("contract"), "replacement\n").unwrap();
    let listings = review(1, std::slice::from_ref(&lane)).unwrap();
    let files = &listings[0].files;
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].added, Some(1));
    assert_eq!(files[0].removed, Some(1));
    let actual = patch(1, &lane, &files[0]).unwrap();
    assert!(actual.contains("-original"));
    assert!(actual.contains("+replacement"));
}

#[test]
fn cancelled_preparation_does_not_create_a_branch_or_checkout() {
    let dir = Scratch::new("cancel-prepare");
    let lane = dir.lane("repo");
    let stop = Cancellation::default();
    stop.cancel();
    assert!(prepare(23, &lane, &stop).is_err());
    let project = repository(&lane.project).unwrap();
    assert_eq!(project.worktrees.len(), 1);
    assert!(git::branches::resolve(&Host::Local, &lane.project, &branch(23, &lane)).is_err());
}

#[test]
fn distinct_dirty_edits_inside_the_same_gitlink_change_acceptance_identity() {
    let dir = Scratch::new("submodule");
    let mut lane = dir.lane("parent");
    let nested = dir.lane("library");
    git(
        &lane.project,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{},library", nested.base),
        ],
    );
    git(&lane.project, &["commit", "-qm", "pin library"]);
    lane.base = preview(&lane.project, "HEAD").unwrap().commit;
    lane.worktree = prepare(1, &lane, &Cancellation::default()).unwrap();
    git(
        &lane.worktree,
        &[
            "clone",
            "--quiet",
            "--local",
            nested.project.to_str().unwrap(),
            "library",
        ],
    );
    let key = || revision(1, std::slice::from_ref(&lane), &Cancellation::default()).unwrap();
    let clean = key();
    let file = lane.worktree.join("library/contract");
    fs::write(&file, "first").unwrap();
    let first = key();
    fs::write(&file, "second").unwrap();
    let second = key();
    assert_ne!(clean, first);
    assert_ne!(first, second);
    fs::write(lane.worktree.join("library/new-file"), "untracked").unwrap();
    assert_ne!(second, key());
    assert!(
        reviewed_trees(1, std::slice::from_ref(&lane), &Cancellation::default())
            .unwrap_err()
            .to_string()
            .contains("nested checkouts")
    );
}

#[test]
fn launch_check_menu_reads_manifests_from_the_selected_commit() {
    let scratch = Scratch::new("launch-choices");
    let lane = scratch.lane("project");
    fs::write(
        lane.project.join("package.json"),
        r#"{"scripts":{"test":"vitest"}}"#,
    )
    .unwrap();
    fs::write(lane.project.join("pnpm-lock.yaml"), "").unwrap();
    git(&lane.project, &["add", "."]);
    git(&lane.project, &["commit", "-qm", "Add package tests"]);
    fs::write(lane.project.join("Cargo.toml"), "").unwrap();
    fs::write(
        lane.project.join("package.json"),
        r#"{"scripts":{"lint":"eslint"}}"#,
    )
    .unwrap();
    let preview = preview(&lane.project, "refs/heads/main").unwrap();
    assert_eq!(preview.checks, ["pnpm run test"]);
    assert!(preview.dirty);
    assert!(
        preview
            .branches
            .iter()
            .any(|b| b.reference == "refs/heads/main")
    );
    assert!(
        super::preview(&lane.project, &lane.base)
            .unwrap()
            .checks
            .is_empty()
    );
}

#[test]
fn interrupted_preparation_reattaches_only_an_unoccupied_branch_at_the_approved_commit() {
    let dir = Scratch::new("reattach");
    let lane = dir.lane("repo");
    let stop = Cancellation::default();
    let checkout = prepare(42, &lane, &stop).unwrap();
    let path = checkout.to_str().unwrap();
    let name = branch(42, &lane);
    git(&lane.project, &["worktree", "remove", path]);
    git(&lane.project, &["switch", &name]);
    assert!(prepare(42, &lane, &stop).is_err());
    assert!(!checkout.exists());
    git(&lane.project, &["switch", "main"]);
    assert_eq!(prepare(42, &lane, &stop).unwrap(), checkout);
    git(&checkout, &["commit", "--allow-empty", "-m", "preserve"]);
    let moved = preview(&checkout, "HEAD").unwrap().commit;
    git(&lane.project, &["worktree", "remove", path]);
    assert!(
        prepare(42, &lane, &stop)
            .unwrap_err()
            .to_string()
            .contains("moved")
    );
    assert_eq!(preview(&lane.project, &name).unwrap().commit, moved);
    assert!(!checkout.exists());
}

#[test]
fn detached_preview_uses_the_selected_checkout_for_head_and_local_changes() {
    let dir = Scratch::new("detached-preview");
    let lane = dir.lane("repo");
    let detached = dir.0.join("detached");
    git(
        &lane.project,
        &[
            "worktree",
            "add",
            "--detach",
            detached.to_str().unwrap(),
            "HEAD",
        ],
    );
    git(
        &lane.project,
        &["commit", "--allow-empty", "-m", "move main"],
    );
    fs::write(detached.join("local"), "keep").unwrap();
    let selected = preview(&detached, "HEAD").unwrap();
    let primary = preview(&lane.project, "HEAD").unwrap();
    assert_eq!(selected.commit, lane.base);
    assert_ne!(primary.commit, selected.commit);
    assert!(selected.dirty && !primary.dirty);
    assert_eq!(selected.worktrees_dir, primary.worktrees_dir);
}

#[test]
fn preparation_commits_to_a_chosen_existing_branch() {
    let dir = Scratch::new("existing-branch");
    let mut lane = dir.lane("repo");
    git(&lane.project, &["branch", "feature"]);
    lane.branch = "feature".into();
    lane.worktree = prepare(5, &lane, &Cancellation::default()).unwrap();
    let project = repository(&lane.project).unwrap();
    let checkout = project
        .worktrees
        .iter()
        .find(|w| w.path == lane.worktree)
        .unwrap();
    assert_eq!(checkout.branch(), Some("feature"));
    verify(5, &lane).unwrap();
}

#[test]
fn a_new_nested_branch_keeps_its_name_and_gets_a_flat_folder() {
    let dir = Scratch::new("nested-branch");
    let mut lane = dir.lane("repo");
    lane.branch = "feature/login".into();
    lane.worktree = prepare(6, &lane, &Cancellation::default()).unwrap();
    assert_eq!(lane.worktree.file_name().unwrap(), "feature-login");
    assert_eq!(
        preview(&lane.project, "refs/heads/feature/login")
            .unwrap()
            .commit,
        lane.base
    );
    verify(6, &lane).unwrap();
}

#[test]
fn a_chosen_branch_that_moved_after_launch_is_not_taken_over() {
    let dir = Scratch::new("moved-branch");
    let mut lane = dir.lane("repo");
    git(&lane.project, &["branch", "feature"]);
    git(&lane.project, &["switch", "-q", "feature"]);
    git(
        &lane.project,
        &["commit", "-q", "--allow-empty", "-m", "later"],
    );
    git(&lane.project, &["switch", "-q", "main"]);
    lane.branch = "feature".into();
    let error = prepare(8, &lane, &Cancellation::default()).unwrap_err();
    assert!(error.to_string().contains("moved"), "{error}");
}

#[test]
fn lanes_saved_before_branches_were_chosen_keep_the_generated_branch() {
    let lane: Lane = serde_json::from_str(r#"{"name":"api","project":"/api","base":"abc"}"#)
        .expect("a lane without a branch still loads");
    assert!(lane.branch.is_empty());
    assert_eq!(branch(0x2a, &lane), "pipeline-2a");
}

#[test]
fn a_new_checkout_gets_the_projects_ignored_files_and_setup_and_a_retry_repeats_them() {
    let dir = Scratch::new("provision");
    let mut lane = dir.lane("api");
    fs::write(lane.project.join(".gitignore"), ".env\n.installed\n").unwrap();
    git(&lane.project, &["add", ".gitignore"]);
    git(&lane.project, &["commit", "-qm", "ignore local files"]);
    lane.base = preview(&lane.project, "main").unwrap().commit;
    fs::write(lane.project.join(".env"), "TOKEN=1").unwrap();
    fs::write(lane.project.join(".worktreeinclude"), ".env\n").unwrap();
    let stop = Cancellation::default();

    lane.setup = "echo installing >&2; exit 7".into();
    let checkout = prepare(1, &lane, &stop).unwrap();
    let error = provision(&lane, &checkout, &stop).unwrap_err().to_string();
    assert!(error.contains("exited with 7"), "{error}");
    assert!(error.contains("installing"), "{error}");
    assert_eq!(
        fs::read_to_string(checkout.join(".env")).unwrap(),
        "TOKEN=1"
    );

    // The run has not recorded the checkout yet, so a retry prepares the same one again.
    lane.setup = "test -f .env && echo done > .installed".into();
    assert_eq!(prepare(1, &lane, &stop).unwrap(), checkout);
    provision(&lane, &checkout, &stop).unwrap();
    assert!(checkout.join(".installed").exists());
    // Ignored files are not changes: the reviewers and the fingerprint never see them.
    assert!(
        review(
            1,
            &[Lane {
                worktree: checkout,
                ..lane
            }]
        )
        .unwrap()[0]
            .files
            .is_empty()
    );
}

#[test]
fn independent_cargo_test_checks_the_base_and_pinned_candidate_without_touching_the_main_checkout()
{
    use crate::pipeline::{RegressionStep, RegressionTest, TestRunner};
    let temp = Scratch::new("independent-regression");
    let mut lane = temp.lane("api");
    let project = &lane.project;
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname='counter'\nversion='0.1.0'\nedition='2024'\n",
    )
    .unwrap();
    fs::create_dir(project.join("src")).unwrap();
    fs::write(project.join("src/lib.rs"), "pub fn count() -> u8 { 1 }\n").unwrap();
    fs::write(project.join(".gitignore"), "/target\n/Cargo.lock\n").unwrap();
    fs::create_dir(project.join(".cargo")).unwrap();
    fs::write(
        project.join(".cargo/config.toml"),
        "[net]\noffline = true\n",
    )
    .unwrap();
    git(project, &["add", "."]);
    git(project, &["commit", "-qm", "counter fixture"]);
    lane.base = preview(project, "main").unwrap().commit;
    let stop = Cancellation::default();
    lane.worktree = prepare(89, &lane, &stop).unwrap();
    let mut run = Run::new(
        89,
        project.clone(),
        "Fix count".into(),
        Template::default(),
        vec![lane.clone()],
    )
    .unwrap();
    run.enable_regression().unwrap();
    run.start_direct("Returns two").unwrap();
    let path = "tests/counter.rs";
    fs::create_dir(lane.worktree.join("tests")).unwrap();
    let test = "#[test] fn returns_two() {assert_eq!(counter::count(), 2, \"expected two\");}";
    fs::write(lane.worktree.join(path), test).unwrap();
    run.report(
        &run.pending()[0],
        Report::Regression {
            test: RegressionTest {
                runner: TestRunner::Cargo,
                path: path.into(),
                name: "returns_two".into(),
                expected: "expected two".into(),
            },
        },
    )
    .unwrap();
    let execute = |r: &mut Run| {
        let result = run_regression(r, &stop).unwrap();
        r.record_regression(result).unwrap();
    };
    execute(&mut run);
    assert_eq!(
        run.phase(),
        Phase::Development,
        "{}",
        run.regression().unwrap().description()
    );
    fs::write(
        lane.worktree.join("src/lib.rs"),
        "pub fn count() -> u8 { 2 }\n",
    )
    .unwrap();
    fs::write(lane.worktree.join(path), format!("{test}\n")).unwrap();
    run.report(
        &run.pending()[0],
        Report::Developed {
            summary: "Fixed count".into(),
        },
    )
    .unwrap();
    execute(&mut run);
    assert_eq!(run.phase(), Phase::Regression(RegressionStep::Decision));
    let replacement = run.regression().unwrap().changed_blob().unwrap().to_owned();
    run.retry_regression(Some(&replacement)).unwrap();
    execute(&mut run);
    assert_eq!(run.phase(), Phase::Regression(RegressionStep::Candidate));
    execute(&mut run);
    assert_eq!(
        run.phase(),
        Phase::Review(0),
        "{}",
        run.regression().unwrap().description()
    );
    assert_eq!(
        fs::read_to_string(project.join("src/lib.rs")).unwrap(),
        "pub fn count() -> u8 { 1 }\n"
    );
    assert!(!project.join(path).exists());
}
