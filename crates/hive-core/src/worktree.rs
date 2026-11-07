//! Creating and removing git worktrees the way Orca does: a generated name, a new branch of
//! the same name off the remote's default branch, a folder in [`Project::worktrees_dir`].

use crate::git::{self, GitError};
use crate::projects::{Head, Project, Worktree};
use std::collections::HashSet;
use std::hash::{BuildHasher as _, RandomState};
use std::path::{Path, PathBuf};

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

/// Base refs tried, in order, when the remote has no `HEAD` symref.
const BASE_REF_FALLBACKS: [&str; 4] = ["origin/main", "origin/master", "main", "master"];

/// Why a worktree could not be created or removed.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("the primary worktree of {project} can't be removed; remove the project instead")]
    Primary { project: String },
    #[error("{} has modified or untracked files", path.display())]
    HasChanges { path: PathBuf },
    #[error(transparent)]
    Git(#[from] GitError),
}

/// How hard [`remove`] tries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// Refuse when the worktree has uncommitted or untracked files.
    Safe,
    /// Delete the folder whatever it contains.
    Force,
}

/// Blocking: creates a worktree on a new branch off the default base branch. `name` becomes
/// both the folder and the branch name; `None` or a blank name picks a free generated one.
pub fn create(project: &Project, name: Option<&str>) -> Result<Worktree, WorktreeError> {
    create_from(project, name, None)
}

fn path_argument(path: &Path) -> Result<&str, WorktreeError> {
    // shortcut: Git mutations take UTF-8 arguments; use OsStr when adding byte-path mutations.
    path.to_str().ok_or_else(|| {
        GitError::Invalid(
            "Worktree mutations require a UTF-8 path; use Git in the terminal for this path."
                .into(),
        )
        .into()
    })
}

/// Blocking: creates a worktree from an explicit commit/ref, or the default base.
pub fn create_from(
    project: &Project,
    name: Option<&str>,
    base: Option<&str>,
) -> Result<Worktree, WorktreeError> {
    let base = match base.map(str::trim).filter(|base| !base.is_empty()) {
        Some(base) => git::branches::resolve(&project.host, &project.path, base)?,
        None => default_base_ref(project)?,
    };
    let name = match name.map(sanitize).filter(|name| !name.is_empty()) {
        Some(name) => name,
        None => generate_name(
            &taken_names(project)?,
            RandomState::new().hash_one(&project.path),
        ),
    };
    let path = project.worktrees_dir().join(&name);
    let path_arg = path_argument(&path)?;
    git::run_mutation(
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

/// The remote's default branch (`origin/HEAD`), else the first fallback that exists, else `HEAD`.
/// Only a missing ref moves on to the next candidate: a Git that could not answer (ssh down,
/// timeout) is an error, so a worktree is never silently based on the wrong commit.
fn default_base_ref(project: &Project) -> Result<String, GitError> {
    let lookup = |args: &[&str]| existing(git::run(&project.host, &project.path, args));
    if let Some(head) = lookup(&[
        "symbolic-ref",
        "--quiet",
        "--short",
        "refs/remotes/origin/HEAD",
    ])? {
        return Ok(head);
    }
    for candidate in BASE_REF_FALLBACKS {
        if lookup(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{candidate}^{{commit}}"),
        ])?
        .is_some()
        {
            return Ok(candidate.to_string());
        }
    }
    Ok("HEAD".to_string())
}

/// `--quiet` ref lookups exit 1 when the ref does not exist; anything else is a real failure.
fn existing(result: Result<String, GitError>) -> Result<Option<String>, GitError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Folder names of existing worktrees and local branch names: a new name must avoid both.
fn taken_names(project: &Project) -> Result<HashSet<String>, GitError> {
    let branches = git::run(
        &project.host,
        &project.path,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
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
fn sanitize(name: &str) -> String {
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
    use crate::host::Host;
    use crate::projects::scan;
    use crate::state::ProjectRef;
    use std::path::Path;

    fn failed(code: Option<i32>) -> GitError {
        GitError::Failed {
            command: "rev-parse".into(),
            host: Host::Local,
            cwd: PathBuf::from("/repo"),
            code,
            stderr: String::new(),
        }
    }

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
    fn only_a_missing_ref_falls_through_to_the_next_base() {
        assert_eq!(existing(Ok("main".into())).unwrap(), Some("main".into()));
        assert_eq!(existing(Err(failed(Some(1)))).unwrap(), None);
        assert!(matches!(
            existing(Err(failed(Some(128)))),
            Err(GitError::Failed {
                code: Some(128),
                ..
            })
        ));
        let interrupted = GitError::Interrupted {
            command: "rev-parse".into(),
            host: Host::Local,
            cwd: PathBuf::from("/repo"),
            source: crate::command::Error::TimedOut,
            mutation: false,
            guidance: "",
        };
        assert!(matches!(
            existing(Err(interrupted)),
            Err(GitError::Interrupted { .. })
        ));
    }

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize("  Fix login bug!  "), "Fix-login-bug");
        assert_eq!(sanitize("feat/ünïcode..ok"), "feat-ünïcode..ok");
        assert_eq!(sanitize("../.."), "");
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

    fn git_in(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} failed");
    }

    /// End to end against a real repository in a temp folder.
    #[test]
    fn creates_and_removes_a_worktree() {
        let root = std::env::temp_dir().join(format!("hive-worktree-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("repo")).expect("temp dir");
        // git reports symlink-free paths (`/var` is `/private/var` on macOS).
        let root = root.canonicalize().expect("temp dir exists");
        let repo = root.join("repo");
        git_in(&repo, &["init", "-q", "-b", "main"]);
        git_in(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        let project = || scan(&ProjectRef::new(repo.clone(), Host::Local)).unwrap();

        let created = create(&project(), Some("my task")).expect("created");
        assert_eq!(created.branch(), Some("my-task"));
        assert_eq!(created.path, root.join("repo-worktrees/my-task"));
        let generated = create(&project(), None).expect("created with a generated name");
        assert!(NAMES.contains(&generated.name().as_str()));
        assert_eq!(project().visible_worktrees().count(), 3);

        let initial = git::run(&Host::Local, &repo, &["rev-parse", "HEAD"]).unwrap();
        git_in(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "--allow-empty",
                "-m",
                "later",
            ],
        );
        let explicit = create_from(&project(), Some("from-initial"), Some(&initial)).unwrap();
        assert_eq!(
            git::run(&Host::Local, &explicit.path, &["rev-parse", "HEAD"]).unwrap(),
            initial
        );
        assert!(create_from(&project(), Some("bad-base"), Some("--help")).is_err());
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

        let _ = std::fs::remove_dir_all(&root);
    }
}
