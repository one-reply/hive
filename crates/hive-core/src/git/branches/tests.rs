use super::{Branch, default_base, default_start, is_valid_name, list, parse_list, short_name};
use crate::git::{GitError, testing};
use crate::host::Host;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

#[test]
fn branch_names_follow_git_ref_rules() {
    for name in [
        "develop",
        "feature/login",
        "fix-1.2",
        "user/v2_beta",
        "ünïcode",
    ] {
        assert!(is_valid_name(name), "{name}");
    }
    for name in [
        "", "@", "-flag", "a..b", "a b", "end.", "a//b", "/lead", "trail/", ".hidden", "x/.dot",
        "ref.lock", "a@{1}", "a~1", "a^", "a:b", "a?", "a*", "a[", "a\\b", "tab\t",
    ] {
        assert!(!is_valid_name(name), "{name:?}");
    }
}

fn branch(reference: &str) -> Branch {
    Branch {
        reference: reference.into(),
        name: short_name(reference).into(),
        remote: reference.starts_with("refs/remotes/"),
        worktree: None,
    }
}

fn start(references: &[&str], base: Option<&str>) -> Option<String> {
    let branches: Vec<_> = references.iter().map(|r| branch(r)).collect();
    default_start(&branches, base).map(|b| b.name.clone())
}

#[test]
fn default_start_prefers_local_then_remote_develop() {
    let base = Some("refs/remotes/origin/main");
    let all = [
        "refs/remotes/origin/main",
        "refs/remotes/origin/develop",
        "refs/heads/develop",
    ];
    assert_eq!(start(&all, base).as_deref(), Some("develop"));
    assert_eq!(start(&all[..2], base).as_deref(), Some("origin/develop"));
}

#[test]
fn default_start_finds_develop_that_git_displays_as_heads_develop() {
    // What `%(refname:short)` prints for the branch when a tag is also named `develop`.
    let mut local = branch("refs/heads/develop");
    local.name = "heads/develop".into();
    let branches = [branch("refs/remotes/origin/develop"), local];
    let start = default_start(&branches, Some("refs/remotes/origin/develop"));
    assert_eq!(
        start.map(|b| b.reference.as_str()),
        Some("refs/heads/develop")
    );
}

#[test]
fn default_start_falls_back_to_review_base_then_head() {
    let branches = ["refs/heads/feature", "refs/remotes/origin/main"];
    assert_eq!(
        start(&branches, Some("refs/remotes/origin/main")).as_deref(),
        Some("origin/main")
    );
    assert_eq!(start(&branches, Some("refs/heads/gone")), None);
    assert_eq!(start(&branches, None), None);
    assert_eq!(start(&[], None), None);
}

#[test]
fn short_name_strips_branch_namespaces_only() {
    assert_eq!(short_name("refs/remotes/origin/develop"), "origin/develop");
    assert_eq!(short_name("refs/heads/feature/x"), "feature/x");
    assert_eq!(short_name("refs/tags/v1"), "refs/tags/v1");
    assert_eq!(short_name("main"), "main");
    assert_eq!(short_name(""), "");
}

/// What `parse_list` made of `raw`: each branch's full ref and where it is checked out.
fn parsed(raw: &[u8]) -> Vec<(String, Option<Vec<u8>>)> {
    parse_list(raw)
        .expect("a well-formed list")
        .into_iter()
        .map(|branch| {
            let worktree = branch
                .worktree
                .map(|path| path.as_os_str().as_bytes().to_vec());
            (branch.reference, worktree)
        })
        .collect()
}

// The samples below are `git for-each-ref` 2.54 output for the format `list` asks for.

#[test]
fn a_line_feed_in_a_worktree_path_stays_inside_the_path() {
    let raw = b"refs/heads/main\0main\0/repo\0\0\n\
                refs/heads/nl\0nl\0/tmp/wt-line\nbreak\0\0\n\
                refs/remotes/origin/main\0origin/main\0\0\0\n";
    assert_eq!(
        parsed(raw),
        [
            ("refs/heads/main".to_owned(), Some(b"/repo".to_vec())),
            (
                "refs/heads/nl".to_owned(),
                Some(b"/tmp/wt-line\nbreak".to_vec())
            ),
            ("refs/remotes/origin/main".to_owned(), None),
        ]
    );
}

#[test]
fn a_branch_checked_out_at_a_path_that_is_not_utf8_is_still_listed() {
    let raw = b"refs/heads/latin\0latin\0/tmp/wt-caf\xe9\0\0\nrefs/heads/main\0main\0\0\0\n";
    assert_eq!(
        parsed(raw),
        [
            (
                "refs/heads/latin".to_owned(),
                Some(b"/tmp/wt-caf\xe9".to_vec())
            ),
            ("refs/heads/main".to_owned(), None),
        ]
    );
}

#[test]
fn aliases_and_names_that_are_not_text_are_left_out_of_the_list() {
    let raw = b"refs/heads/caf\xe9\0caf\xe9\0\0\0\n\
                refs/heads/main\0main\0\0\0\n\
                refs/remotes/origin/HEAD\0origin\0\0refs/remotes/origin/main\0\n\
                refs/remotes/origin/main\0origin/main\0\0\0\n";
    let references: Vec<_> = parsed(raw).into_iter().map(|(name, _)| name).collect();
    assert_eq!(references, ["refs/heads/main", "refs/remotes/origin/main"]);
    assert!(parsed(b"").is_empty());
}

#[test]
fn a_list_cut_short_or_in_another_shape_is_an_error_not_fewer_branches() {
    for raw in [
        // Ended in the middle of a record.
        &b"refs/heads/main\0main\0/repo\0\0\nrefs/heads/nl\0nl\0/tmp/wt"[..],
        // The line-based format of an older build: a line feed where a field should end.
        b"refs/heads/main\0main\0/repo\0\nrefs/heads/nl\0nl\0\0\n",
        // Something after the last record that is not Git's line feed.
        b"refs/heads/main\0main\0/repo\0\0\nfatal",
    ] {
        assert!(
            matches!(parse_list(raw), Err(GitError::Invalid(_))),
            "{:?}",
            String::from_utf8_lossy(raw)
        );
    }
}

/// A repository with one commit on `main` in its own temporary folder, removed when dropped.
struct Repo {
    root: PathBuf,
    path: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("hive-branches-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        // Git reports symlink-free paths (`/var` is `/private/var` on macOS).
        let root = root.canonicalize().expect("temp dir exists");
        let path = root.join("repo");
        testing::init(&path);
        let repo = Self { root, path };
        repo.git(&["commit", "-q", "--allow-empty", "-m", "fixture"]);
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        testing::run(&self.path, args)
    }

    fn default_base(&self) -> Option<String> {
        default_base(&Host::Local, &self.path).expect("Git answers")
    }

    fn listed(&self) -> Vec<Branch> {
        list(&Host::Local, &self.path).expect("Git lists the branches")
    }

    /// The remote-tracking branch `name` of `origin`, at the commit `main` is on.
    fn track(&self, name: &str) {
        self.git(&["update-ref", &format!("refs/remotes/origin/{name}"), "main"]);
    }

    fn remote_head(&self, target: &str) {
        self.git(&["symbolic-ref", "refs/remotes/origin/HEAD", target]);
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn well_known_branches_are_found_beside_tags_of_the_same_name() {
    let repo = Repo::new("tags");
    for args in [
        &["branch", "-m", "develop"][..],
        &["branch", "main"],
        &["tag", "develop"],
        &["tag", "main"],
    ] {
        repo.git(args);
    }
    let branches = repo.listed();
    let names: Vec<_> = branches.iter().map(|b| b.name.as_str()).collect();
    assert!(names.contains(&"heads/develop"), "{names:?}");
    assert_eq!(
        default_start(&branches, None).map(|b| b.reference.as_str()),
        Some("refs/heads/develop")
    );
    assert_eq!(repo.default_base().as_deref(), Some("refs/heads/main"));
}

#[test]
fn branches_are_listed_wherever_git_says_they_are_checked_out() {
    let repo = Repo::new("paths");
    let line_feed = repo.root.join("line\nfeed");
    let elsewhere = repo.root.join("elsewhere");
    for (branch, path) in [("fed", &line_feed), ("bytes", &elsewhere)] {
        let path = path.to_str().expect("the fixture path is text");
        repo.git(&["worktree", "add", "-q", "-b", branch, path]);
    }
    // APFS refuses a folder whose name is not UTF-8. Where Git says a worktree is comes
    // from its registration, which is a file of bytes on every file system.
    let mut byte_path = repo.root.as_os_str().as_bytes().to_vec();
    byte_path.extend_from_slice(b"/caf\xe9");
    let registration = [&byte_path[..], b"/.git\n"].concat();
    std::fs::write(
        repo.path.join(".git/worktrees/elsewhere/gitdir"),
        registration,
    )
    .unwrap();

    let branches = repo.listed();
    let worktree_of = |name: &str| {
        let branch = branches.iter().find(|b| b.name == name);
        branch
            .unwrap_or_else(|| panic!("{name} is listed: {branches:?}"))
            .worktree
            .clone()
    };
    assert_eq!(worktree_of("main").as_deref(), Some(repo.path.as_path()));
    assert_eq!(worktree_of("fed"), Some(line_feed));
    assert_eq!(
        worktree_of("bytes").as_deref(),
        Some(Path::new(OsStr::from_bytes(&byte_path)))
    );
}

#[test]
fn the_default_base_follows_the_remote_head_then_the_well_known_branches() {
    let repo = Repo::new("order");
    repo.git(&["branch", "master"]);
    assert_eq!(repo.default_base().as_deref(), Some("refs/heads/main"));
    repo.track("master");
    assert_eq!(
        repo.default_base().as_deref(),
        Some("refs/remotes/origin/master")
    );
    repo.track("main");
    assert_eq!(
        repo.default_base().as_deref(),
        Some("refs/remotes/origin/main")
    );
    repo.track("develop");
    repo.remote_head("refs/remotes/origin/develop");
    assert_eq!(
        repo.default_base().as_deref(),
        Some("refs/remotes/origin/develop")
    );
    // The link without its target, as a pruned default branch leaves it.
    repo.remote_head("refs/remotes/origin/gone");
    assert_eq!(
        repo.default_base().as_deref(),
        Some("refs/remotes/origin/main")
    );
    // A link out of the branches is not a branch to compare with.
    repo.git(&["tag", "v1"]);
    repo.remote_head("refs/tags/v1");
    assert_eq!(
        repo.default_base().as_deref(),
        Some("refs/remotes/origin/main")
    );
}

#[test]
fn a_candidate_that_is_not_a_commit_is_passed_over() {
    let repo = Repo::new("blob");
    std::fs::write(repo.path.join("note"), "not a commit\n").unwrap();
    let blob = repo.git(&["hash-object", "-w", "note"]);
    // Nothing stops a remote-tracking ref from naming an object that is not a commit.
    repo.git(&["update-ref", "refs/remotes/origin/main", &blob]);
    assert_eq!(repo.default_base().as_deref(), Some("refs/heads/main"));
    repo.git(&["update-ref", "refs/remotes/origin/develop", &blob]);
    repo.remote_head("refs/remotes/origin/develop");
    assert_eq!(repo.default_base().as_deref(), Some("refs/heads/main"));
    repo.track("master");
    assert_eq!(
        repo.default_base().as_deref(),
        Some("refs/remotes/origin/master")
    );
}

#[test]
fn the_default_base_is_a_branch_the_list_shows_or_none() {
    let repo = Repo::new("alias");
    repo.git(&["branch", "-m", "trunk"]);
    assert_eq!(repo.default_base(), None);
    // The name kept for old scripts after a rename: an alias, which the list leaves out.
    repo.git(&["symbolic-ref", "refs/heads/master", "refs/heads/trunk"]);
    assert_eq!(repo.default_base(), None);
    // A branch below a well-known name is not that branch.
    repo.git(&["update-ref", "refs/remotes/origin/main/next", "trunk"]);
    assert_eq!(repo.default_base(), None);
    let listed: Vec<_> = repo.listed().into_iter().map(|b| b.reference).collect();
    assert_eq!(
        listed,
        ["refs/heads/trunk", "refs/remotes/origin/main/next"]
    );
}

#[test]
fn a_git_that_cannot_answer_is_an_error_not_a_repository_without_a_base() {
    let repo = Repo::new("unreadable");
    // A folder beside the repository, in none itself.
    let outside = repo.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    for answer in [
        default_base(&Host::Local, &outside).map(drop),
        list(&Host::Local, &outside).map(drop),
    ] {
        assert!(
            matches!(
                answer,
                Err(GitError::Failed {
                    code: Some(128),
                    ..
                })
            ),
            "{answer:?}"
        );
    }
}
