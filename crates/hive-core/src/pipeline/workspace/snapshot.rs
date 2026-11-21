//! Reviewed trees are kept in Git so acceptance can be compared across commits and repair rounds.
use super::*;
use std::{
    fs,
    hash::{BuildHasher, RandomState},
    os::unix::{ffi::OsStrExt, fs::DirBuilderExt},
};

/// Full Git object name, suitable for passing to a revision-taking Git command.
pub(in crate::pipeline) fn is_object_name(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

pub(super) struct Scratch(pub(super) PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0) {
            tracing::warn!(%error, path = ?self.0, "could not clean private pipeline directory");
        }
    }
}

impl Scratch {
    pub(super) fn new(run: u64) -> Result<Self, GitError> {
        let dir = std::env::temp_dir().join(format!(
            "hive-review-{:x}",
            RandomState::new().hash_one(run)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .map_err(|e| GitError::Invalid(format!("Creating private pipeline directory: {e}")))?;
        Ok(Self(dir))
    }
}

fn snapshot(run: u64, lane: &Lane, stop: &Cancellation) -> Result<String, GitError> {
    let cwd = &lane.worktree;
    let private = Scratch::new(run)?;
    let index = private.0.join("index");
    // Seed from the real index to retain force-added ignored files and staged deletions.
    let path = git::bytes(
        &Host::Local,
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        None,
        64 * 1024,
        false,
    )?;
    let path = std::ffi::OsStr::from_bytes(path.strip_suffix(b"\n").unwrap_or(&path));
    fs::copy(path, &index)
        .map_err(|e| GitError::Invalid(format!("Reading index in {}: {e}", cwd.display())))?;
    let indexed = |args: &[&str]| -> Result<String, GitError> {
        let mut cmd = Host::Local.command("git", &["-c", "core.hooksPath=/dev/null"], Some(cwd));
        cmd.args(args);
        cmd.env("GIT_INDEX_FILE", &index)
            .env("GIT_TERMINAL_PROMPT", "0");
        let out = command::run(cmd, &[], stop, git::READ_TIMEOUT, 64 * 1024)
            .map_err(|e| GitError::Invalid(format!("Snapshot {}: {e}", cwd.display())))?;
        if out.code != Some(0) {
            return Err(GitError::Invalid(format!(
                "Snapshot {}: {}",
                cwd.display(),
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    indexed(&["-c", "core.splitIndex=false", "add", "-A", "--", "."])?;
    // A Git tree can represent a submodule commit but not its uncommitted files.
    let remaining = indexed(&[
        "diff-files",
        "--name-only",
        "--ignore-submodules=none",
        "--",
    ])?;
    if !remaining.is_empty() {
        return Err(GitError::Invalid(format!(
            "Commit changes inside nested checkouts or finish editing before review: {remaining}"
        )));
    }
    let tree = indexed(&["write-tree"])?;
    if !is_object_name(&tree) {
        return Err(GitError::Invalid(
            "Git returned an invalid review tree.".into(),
        ));
    }
    git::run_stoppable(
        &Host::Local,
        cwd,
        &[
            "update-ref",
            &format!("refs/hive/pipelines/{run}/{tree}"),
            &tree,
        ],
        stop,
    )?;
    Ok(tree)
}

/// Capture one immutable tree for each physical checkout between matching content fingerprints.
pub fn reviewed_trees(
    run: u64,
    lanes: &[Lane],
    stop: &Cancellation,
) -> Result<(String, Vec<String>), GitError> {
    let before = revision(run, lanes, stop)?;
    let mut seen = std::collections::HashMap::new();
    let mut trees = Vec::with_capacity(lanes.len());
    for lane in lanes {
        let tree = match seen.get(&lane.worktree) {
            Some(tree) => String::clone(tree),
            None => {
                let tree = snapshot(run, lane, stop)?;
                seen.insert(lane.worktree.clone(), tree.clone());
                tree
            }
        };
        trees.push(tree);
    }
    if before != revision(run, lanes, stop)? {
        return Err(GitError::Invalid(
            "Files changed while capturing the review. Retry when edits have stopped.".into(),
        ));
    }
    Ok((before, trees))
}

/// Release only this run's private tree refs. Git owns reclamation of unreachable objects.
pub fn release_trees(run: u64, lanes: &[Lane]) -> Result<(), GitError> {
    let mut seen = HashSet::new();
    for lane in lanes.iter().filter(|l| seen.insert(&l.project)) {
        let prefix = format!("refs/hive/pipelines/{run}/");
        let refs = git::run(
            &Host::Local,
            &lane.project,
            &["for-each-ref", "--format=%(refname)", &prefix],
        )?;
        for name in refs.lines() {
            if let Some(tree) = name.strip_prefix(&prefix).filter(|s| is_object_name(s)) {
                git::run_mutation(
                    &Host::Local,
                    &lane.project,
                    &["update-ref", "-d", name, tree],
                )?;
            }
        }
    }
    Ok(())
}
