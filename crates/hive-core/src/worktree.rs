//! Creating and removing git worktrees the way Orca does: a generated name, a new branch of
//! the same name off the repository's default base branch, a folder in
//! [`Project::worktrees_dir`].

use crate::command::Cancellation;
use crate::git::{self, GitError};
use crate::projects::{Head, Project, Worktree};
use std::collections::HashSet;
use std::hash::{BuildHasher as _, RandomState};
use std::path::{Path, PathBuf};

mod provision;
pub use provision::{INCLUDE_FILE, IncludePatterns, ProvisionError, copy_included, run_setup};

/// Pool for generated worktree names: plants bees forage on.
const NAMES: &[&str] = &[
    "acacia",
    "alfalfa",
    "allium",
    "aster",
    "basil",
    "borage",
    "buckwheat",
    "calendula",
    "catmint",
    "chicory",
    "clover",
    "coneflower",
    "cornflower",
    "cosmos",
    "crocus",
    "dahlia",
    "dandelion",
    "fennel",
    "foxglove",
    "goldenrod",
    "hawthorn",
    "heather",
    "hyssop",
    "lavender",
    "linden",
    "lupine",
    "mallow",
    "marigold",
    "marjoram",
    "meadowsweet",
    "mint",
    "mustard",
    "oregano",
    "phacelia",
    "poppy",
    "rosemary",
    "sage",
    "sainfoin",
    "salvia",
    "snowdrop",
    "sunflower",
    "thistle",
    "thyme",
    "verbena",
    "vetch",
    "willow",
    "yarrow",
    "zinnia",
];

/// Why a worktree could not be created or removed.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("the primary worktree of {project} can't be removed; remove the project instead")]
    Primary { project: String },
    #[error("{} has modified or untracked files", path.display())]
    HasChanges { path: PathBuf },
    #[error(transparent)]
    Git(#[from] GitError),
    /// The worktree exists but its ignored files or setup command did not complete.
    #[error(transparent)]
    Provision(#[from] ProvisionError),
}

/// How hard [`remove`] tries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Refuse when the worktree has uncommitted or untracked files.
    Safe,
    /// Delete the folder whatever it contains.
    Force,
}

pub(crate) fn path_argument(path: &Path) -> Result<&str, WorktreeError> {
    // shortcut: Git mutations take UTF-8 arguments; use OsStr when adding byte-path mutations.
    path.to_str().ok_or_else(|| {
        GitError::Invalid(
            "Worktree mutations require a UTF-8 path; use Git in the terminal for this path."
                .into(),
        )
        .into()
    })
}

/// Blocking: creates a worktree on a new branch. `name` becomes both the folder and the
/// branch name; `None` or a blank name picks a free generated one. The branch starts at `base`,
/// a commit or ref, or without one at the branch [`git::branches::default_base`] names, else
/// at `HEAD`.
///
/// `stop` ends `git worktree add`, whose checkout runs `post-checkout` and content filters;
/// the error then says the outcome is unknown, because the folder, its registration or the
/// branch may already exist.
pub fn create(
    project: &Project,
    name: Option<&str>,
    base: Option<&str>,
    stop: &Cancellation,
) -> Result<Worktree, WorktreeError> {
    let base = match base.map(str::trim).filter(|base| !base.is_empty()) {
        Some(base) => git::branches::resolve(&project.host, &project.path, base)?,
        None => default_base(project)?,
    };
    let name = match name.map(folder_name).filter(|name| !name.is_empty()) {
        Some(name) => name,
        None => generate_name(
            &taken_names(project)?,
            RandomState::new().hash_one(&project.path),
        ),
    };
    let path = project.worktrees_dir().join(&name);
    let path_arg = path_argument(&path)?;
    git::run_stoppable(
        &project.host,
        &project.path,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            &name,
            path_arg,
            &base,
        ],
        stop,
    )?;
    tracing::info!(?path, %base, "created worktree");
    Ok(Worktree {
        path,
        head: Head::Branch(name),
    })
}

/// Blocking: removes `worktree`'s folder and registration, then deletes its branch if that
/// branch is fully merged (`git branch -d`), so unmerged work is never lost.
pub fn remove(
    project: &Project,
    worktree: &Worktree,
    removal: Removal,
) -> Result<(), WorktreeError> {
    if project.is_primary(worktree) {
        return Err(WorktreeError::Primary {
            project: project.name.clone(),
        });
    }
    let path_arg = path_argument(&worktree.path)?;
    let mut args = vec!["worktree", "remove"];
    if removal == Removal::Force {
        args.push("--force");
    }
    args.push(path_arg);
    git::run_mutation(&project.host, &project.path, &args).map_err(|err| {
        if needs_force(err.stderr()) {
            WorktreeError::HasChanges {
                path: worktree.path.clone(),
            }
        } else {
            err.into()
        }
    })?;
    tracing::info!(path = ?worktree.path, "removed worktree");
    if let Some(branch) = worktree.branch()
        && let Err(err) = git::run_mutation(
            &project.host,
            &project.path,
            &["branch", "-d", "--", branch],
        )
    {
        tracing::info!(%branch, %err, "kept the worktree's branch");
    }
    Ok(())
}

/// `git worktree remove` refuses a dirty worktree with "... use --force to delete it".
fn needs_force(stderr: &str) -> bool {
    stderr.contains("use --force")
}

/// The commit a worktree starts from when no base was picked: the branch that
/// [`git::branches::default_base`] names, the same rule the review base follows, else `HEAD`.
/// Resolved here to a commit, so the name is read once and a tag of the same name cannot
/// stand in for the branch when Git creates the worktree.
fn default_base(project: &Project) -> Result<String, GitError> {
    let (host, path) = (&project.host, project.path.as_path());
    let reference = git::branches::default_base(host, path)?;
    git::branches::resolve(host, path, reference.as_deref().unwrap_or("HEAD"))
}

/// Folder names of existing worktrees and local branch names: a new name must avoid both.
fn taken_names(project: &Project) -> Result<HashSet<String>, GitError> {
    let branches = git::run(
        &project.host,
        &project.path,
        // `refname:short` would print `heads/x` for a branch that shares its name with a tag.
        &["for-each-ref", "--format=%(refname:lstrip=2)", "refs/heads"],
    )?;
    Ok(branches
        .lines()
        .map(str::to_string)
        .chain(project.worktrees.iter().map(Worktree::name))
        .collect())
}

/// The first free name walking the pool from `seed`; once the pool is used up, the same
/// names with `-2`, `-3`, ... appended.
fn generate_name(taken: &HashSet<String>, seed: u64) -> String {
    let start = (seed % NAMES.len() as u64) as usize;
    (1..)
        .flat_map(|tier| {
            (0..NAMES.len()).map(move |offset| (NAMES[(start + offset) % NAMES.len()], tier))
        })
        .map(|(name, tier)| {
            if tier == 1 {
                name.to_string()
            } else {
                format!("{name}-{tier}")
            }
        })
        .find(|name| !taken.contains(name))
        .expect("an endless sequence of distinct names has a free one")
}

/// Turns user input into a folder and branch name: runs of anything but letters, digits,
/// `.`, `_` and `-` become one `-`; leading/trailing `-` and `.` are dropped.
pub(crate) fn folder_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        if c.is_alphanumeric() || "._-".contains(c) {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches(['-', '.']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testing;
    use crate::host::Host;
    use crate::projects::scan;
    use crate::state::ProjectRef;
    use std::path::Path;

    #[test]
    fn worktree_mutations_reject_lossy_path_arguments() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let path = PathBuf::from(OsString::from_vec(b"/repo/\xff".to_vec()));
        assert!(super::path_argument(&path).is_err());
        assert_eq!(
            super::path_argument(Path::new("/repo/世界")).unwrap(),
            "/repo/世界"
        );
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(folder_name("  Fix login bug!  "), "Fix-login-bug");
        assert_eq!(folder_name("feat/ünïcode..ok"), "feat-ünïcode..ok");
        assert_eq!(folder_name("../.."), "");
    }

    #[test]
    fn generated_names_skip_taken_ones_then_add_a_tier() {
        let taken: HashSet<String> = NAMES.iter().map(|n| n.to_string()).collect();
        assert_eq!(generate_name(&HashSet::new(), 0), NAMES[0]);
        assert_eq!(
            generate_name(&HashSet::from([NAMES[0].to_string()]), 0),
            NAMES[1]
        );
        assert_eq!(generate_name(&taken, 0), format!("{}-2", NAMES[0]));
    }

    #[test]
    fn detects_dirty_worktree_refusal() {
        // Captured from git 2.50.
        assert!(needs_force(
            "fatal: '/tmp/r-worktrees/x' contains modified or untracked files, use --force to delete it"
        ));
        assert!(!needs_force("fatal: '/tmp/x' is not a working tree"));
    }

    /// A repository with one commit in its own temporary folder, removed when dropped.
    struct Fixture {
        root: PathBuf,
        repo: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("hive-worktree-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("temp dir");
            // git reports symlink-free paths (`/var` is `/private/var` on macOS).
            let root = root.canonicalize().expect("temp dir exists");
            let repo = root.join("repo");
            testing::init(&repo);
            let fixture = Self { root, repo };
            fixture.commit("init");
            fixture
        }

        /// Adds an empty commit and returns its id.
        fn commit(&self, message: &str) -> String {
            testing::run(
                &self.repo,
                &["commit", "-q", "--allow-empty", "-m", message],
            );
            testing::run(&self.repo, &["rev-parse", "HEAD"])
        }

        fn project(&self) -> Project {
            scan(&ProjectRef::new(self.repo.clone(), Host::Local)).unwrap()
        }

        fn create(
            &self,
            name: Option<&str>,
            base: Option<&str>,
        ) -> Result<Worktree, WorktreeError> {
            create(&self.project(), name, base, &Cancellation::default())
        }

        fn head_of(&self, worktree: &Worktree) -> String {
            testing::run(&worktree.path, &["rev-parse", "HEAD"])
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// End to end against a real repository in a temp folder.
    #[test]
    fn creates_and_removes_a_worktree() {
        let fixture = Fixture::new("lifecycle");
        let project = || fixture.project();

        let created = fixture.create(Some("my task"), None).expect("created");
        assert_eq!(created.branch(), Some("my-task"));
        assert_eq!(created.path, fixture.root.join("repo-worktrees/my-task"));
        let generated = fixture
            .create(None, None)
            .expect("created with a generated name");
        assert!(NAMES.contains(&generated.name().as_str()));
        assert_eq!(project().visible_worktrees().count(), 3);

        let initial = testing::run(&fixture.repo, &["rev-parse", "HEAD"]);
        fixture.commit("later");
        let explicit = fixture
            .create(Some("from-initial"), Some(&initial))
            .unwrap();
        assert_eq!(fixture.head_of(&explicit), initial);
        assert!(fixture.create(Some("bad-base"), Some("--help")).is_err());
        assert!(!project().worktrees_dir().join("bad-base").exists());

        std::fs::write(created.path.join("scratch.txt"), "wip").expect("write");
        assert!(matches!(
            remove(&project(), &created, Removal::Safe),
            Err(WorktreeError::HasChanges { .. })
        ));
        remove(&project(), &created, Removal::Force).expect("force removed");
        assert!(!created.path.exists());
        assert!(matches!(
            remove(&project(), &project().worktrees[0], Removal::Force),
            Err(WorktreeError::Primary { .. })
        ));
    }

    #[test]
    fn a_dangling_remote_head_does_not_fail_the_new_worktree() {
        let fixture = Fixture::new("dangling-head");
        let head = fixture.commit("on main");
        // What a pruned or renamed default branch leaves behind: the link without its target.
        testing::run(
            &fixture.repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/gone",
            ],
        );
        let created = fixture
            .create(Some("task"), None)
            .expect("falls back to main");
        assert_eq!(fixture.head_of(&created), head);
    }

    #[test]
    fn the_default_base_is_the_branch_even_beside_a_tag_of_its_name() {
        let fixture = Fixture::new("tag-collision");
        // `main` alone resolves to the tag, which stays on the first commit.
        testing::run(&fixture.repo, &["tag", "main"]);
        let head = fixture.commit("on main");
        let created = fixture.create(Some("task"), None).unwrap();
        assert_eq!(fixture.head_of(&created), head);
    }

    #[test]
    fn the_default_base_is_the_remote_head_when_its_branch_exists() {
        let fixture = Fixture::new("remote-head");
        let published = testing::run(&fixture.repo, &["rev-parse", "HEAD"]);
        for args in [
            &["update-ref", "refs/remotes/origin/develop", &published][..],
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/develop",
            ],
        ] {
            testing::run(&fixture.repo, args);
        }
        fixture.commit("local only");
        let created = fixture.create(Some("task"), None).unwrap();
        assert_eq!(fixture.head_of(&created), published);
    }

    #[test]
    fn a_line_feed_in_another_worktrees_path_does_not_fail_the_new_worktree() {
        let fixture = Fixture::new("line-feed");
        let head = fixture.commit("on main");
        let elsewhere = fixture.root.join("line\nfeed");
        testing::run(
            &fixture.repo,
            &["worktree", "add", "-q", "-b", "other", path_arg(&elsewhere)],
        );
        let created = fixture.create(Some("task"), None).expect("created");
        assert_eq!(fixture.head_of(&created), head);
    }

    #[test]
    fn a_worktree_path_that_is_not_utf8_does_not_move_the_default_base() {
        let fixture = Fixture::new("byte-path");
        let main = testing::run(&fixture.repo, &["rev-parse", "HEAD"]);
        testing::run(&fixture.repo, &["switch", "-q", "-c", "work"]);
        fixture.commit("on work");
        let elsewhere = fixture.root.join("elsewhere");
        testing::run(
            &fixture.repo,
            &["worktree", "add", "-q", path_arg(&elsewhere), "main"],
        );
        register_at_byte_path(&fixture, "elsewhere");
        // `main` is checked out where no text can name: it is still the default base.
        let created = fixture.create(Some("task"), None).expect("created");
        assert_eq!(fixture.head_of(&created), main);
    }

    #[test]
    fn a_remote_main_that_is_not_a_commit_is_passed_over_for_the_next_candidate() {
        let fixture = Fixture::new("blob-base");
        let head = fixture.commit("on main");
        // Nothing stops a remote-tracking ref from naming an object that is not a commit.
        std::fs::write(fixture.repo.join("note"), "not a commit\n").unwrap();
        let blob = testing::run(&fixture.repo, &["hash-object", "-w", "note"]);
        testing::run(
            &fixture.repo,
            &["update-ref", "refs/remotes/origin/main", &blob],
        );
        let created = fixture.create(Some("task"), None).expect("starts at main");
        assert_eq!(fixture.head_of(&created), head);
    }

    fn path_arg(path: &Path) -> &str {
        path.to_str().expect("the fixture path is text")
    }

    /// Makes Git report the linked worktree `name` at a path that is not UTF-8. The folder
    /// itself keeps its name, because APFS refuses such a one; where Git says a worktree is
    /// comes from its registration, which is a file of bytes.
    fn register_at_byte_path(fixture: &Fixture, name: &str) {
        use std::os::unix::ffi::OsStrExt;
        let mut registration = fixture.root.as_os_str().as_bytes().to_vec();
        registration.extend_from_slice(b"/caf\xe9/.git\n");
        let gitdir = fixture
            .repo
            .join(".git/worktrees")
            .join(name)
            .join("gitdir");
        std::fs::write(gitdir, registration).expect("registration rewritten");
    }

    #[test]
    fn a_repository_git_cannot_read_is_an_error_not_a_start_from_head() {
        let missing =
            std::env::temp_dir().join(format!("hive-worktree-none-{}", std::process::id()));
        std::fs::create_dir_all(&missing).unwrap();
        let project = Project {
            path: missing.clone(),
            ..Fixture::new("unreadable").project()
        };
        assert!(matches!(
            default_base(&project),
            Err(GitError::Failed {
                code: Some(128),
                ..
            })
        ));
        let _ = std::fs::remove_dir_all(&missing);
    }

    #[test]
    fn a_worktree_being_added_is_stopped_while_its_checkout_hook_runs() {
        let fixture = Fixture::new("stopped");
        let started = testing::stalling_hook(&fixture.repo, "post-checkout");
        let stop = Cancellation::default();
        let project = fixture.project();
        let error = std::thread::scope(|scope| {
            let running = scope.spawn(|| create(&project, Some("task"), None, &stop));
            started
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the hook runs");
            stop.cancel();
            running
                .join()
                .unwrap()
                .expect_err("a creation stopped midway does not report success")
        });
        // Git had already made the folder and the branch: only a look says what is left.
        assert!(
            matches!(&error, WorktreeError::Git(git) if git.outcome_unknown()),
            "{error}"
        );
    }
}
