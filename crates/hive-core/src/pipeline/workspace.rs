//! Local Git checkouts, required commands and acceptance diffs for a pipeline run.
use super::{CommandResult, Lane};
use crate::{
    command::{self, Cancellation},
    git::{self, GitError, diff::DiffSource, status::Change},
    host::Host,
    projects::{self, Project},
    state::ProjectRef,
    worktree::{self, WorktreeError},
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod check_choices;

const CHECK_OUTPUT: usize = 64 * 1024;

/// Immutable start revision and whether the selected checkout also has local changes.
#[derive(Clone, Debug)]
pub struct BasePreview {
    /// Shared checkout directory, identical when two registered projects belong to one repository.
    pub worktrees_dir: PathBuf,
    /// Full Git object ID used to create the isolated worktree.
    pub commit: String,
    /// The user's checkout has changes outside that commit.
    pub dirty: bool,
    /// Local and remote refs the user may choose in the launch sheet.
    pub branches: Vec<git::branches::Branch>,
    /// Known QA commands derived from tracked manifests at this commit.
    pub checks: Vec<String>,
}

/// Resolve the ref once so moving branch tips cannot change an approved start revision.
pub fn preview(project: &Path, reference: &str) -> Result<BasePreview, GitError> {
    let commit = git::branches::resolve(&Host::Local, project, reference)?;
    let branches = git::branches::list(&Host::Local, project)?;
    let checkout = branches
        .iter()
        .find(|b| b.reference == reference)
        .and_then(|b| b.worktree.as_deref())
        .unwrap_or(project);
    let checks = check_choices::discover(project, &commit).unwrap_or_else(|error| {
        tracing::warn!(%error, "Pipeline test-command suggestions unavailable");
        Vec::new()
    });
    Ok(BasePreview {
        worktrees_dir: repository(project)?.worktrees_dir(),
        commit,
        dirty: !git::status::read(&Host::Local, checkout)?.is_clean(),
        branches,
        checks,
    })
}

fn repository(path: &Path) -> Result<Project, GitError> {
    let project = projects::scan(&ProjectRef::new(path.into(), Host::Local))?;
    if project.worktrees.is_empty() {
        return Err(GitError::Invalid(format!(
            "No Git repository at {}",
            path.display()
        )));
    }
    Ok(project)
}

/// The lane's chosen branch, or the generated one of runs saved before the user chose it.
pub fn branch(run: u64, lane: &Lane) -> String {
    match lane.branch.trim() {
        "" => format!("pipeline-{run:x}"),
        name => name.to_owned(),
    }
}

/// Branch names may nest with `/`; the checkout folder stays one level deep.
fn checkout_path(project: &Project, branch: &str) -> PathBuf {
    project.worktrees_dir().join(worktree::folder_name(branch))
}

/// Create a private worktree or recover the exact checkout registered under this run's branch.
/// An interrupted preparation can be retried without destroying files already created.
pub fn prepare(run: u64, lane: &Lane, stop: &Cancellation) -> Result<PathBuf, WorktreeError> {
    let project = repository(&lane.project)?;
    let name = branch(run, lane);
    let intended = checkout_path(&project, &name);
    if !lane.worktree.as_os_str().is_empty() && lane.worktree != intended {
        return Err(GitError::Invalid("Pipeline checkout path has changed.".into()).into());
    }
    let oid = git::branches::resolve(&Host::Local, &project.path, &lane.base)?;
    if oid != lane.base {
        return Err(GitError::Invalid("Pipeline base must be a pinned commit.".into()).into());
    }
    if let Some(tree) = project.worktrees.iter().find(|w| w.path == intended) {
        if tree.branch() != Some(&name) {
            return Err(GitError::Invalid(format!(
                "{} now has a different branch.",
                intended.display()
            ))
            .into());
        }
        if lane.worktree.as_os_str().is_empty() {
            confirm_unused_checkout(&intended, &oid)?;
        }
        return Ok(intended);
    }
    if !lane.worktree.as_os_str().is_empty() {
        return Err(GitError::Invalid(format!(
            "Restore the missing pipeline checkout at {}.",
            intended.display()
        ))
        .into());
    }
    let reference = format!("refs/heads/{name}");
    let branch_head = git::bytes(
        &Host::Local,
        &project.path,
        &["rev-parse", "--verify", "--quiet", &reference],
        None,
        1024,
        true,
    )?;
    if !branch_head.is_empty() {
        if branch_head.trim_ascii_end() != oid.as_bytes() {
            return Err(GitError::Invalid(format!(
                "Branch {name} has moved from the approved commit. Inspect it before retrying."
            ))
            .into());
        }
        // Git refuses an occupied branch or a nonempty destination without --force.
        let path = worktree::path_argument(&intended)?;
        git::run_stoppable(
            &Host::Local,
            &project.path,
            &["worktree", "add", "--", path, &name],
            stop,
        )?;
        confirm_unused_checkout(&intended, &oid)?;
        return Ok(intended);
    }
    // The branch keeps the user's exact name; only its folder is sanitized.
    let path = worktree::path_argument(&intended)?;
    git::run_stoppable(
        &Host::Local,
        &project.path,
        &[
            "worktree",
            "add",
            "--no-track",
            "-b",
            &name,
            "--",
            path,
            &oid,
        ],
        stop,
    )?;
    tracing::info!(path = ?intended, base = %oid, "created pipeline worktree");
    Ok(intended)
}

/// Provision the first pending checkout and identify every lane that shares it.
pub fn prepare_lanes(
    run: &super::Run,
    index: usize,
    stop: &Cancellation,
) -> Result<(Vec<usize>, PathBuf), WorktreeError> {
    let lane = run
        .lanes()
        .get(index)
        .ok_or_else(|| GitError::Invalid("Missing pipeline lane.".into()))?;
    let name = branch(run.id(), lane);
    let intended = checkout_path(&repository(&lane.project)?, &name);
    let mut members = Vec::new();
    let mut existing = None;
    for (i, other) in run.lanes().iter().enumerate() {
        if branch(run.id(), other) != name
            || (other.project != lane.project
                && checkout_path(&repository(&other.project)?, &name) != intended)
        {
            continue;
        }
        if other.base != lane.base {
            return Err(GitError::Invalid(format!(
                "Two lanes of {} require different base commits.",
                lane.project.display()
            ))
            .into());
        }
        if other.worktree.as_os_str().is_empty() {
            members.push(i);
        } else {
            verify(run.id(), other)?;
            existing = Some(other.worktree.clone());
        }
    }
    if let Some(path) = existing {
        return Ok((members, path));
    }
    let path = prepare(run.id(), lane, stop)?;
    provision(lane, &path, stop)?;
    Ok((members, path))
}

/// Blocking: makes a checkout no agent has used yet ready for work: the project's ignored
/// files named by `.worktreeinclude`, then the lane's setup command. Nothing is overwritten,
/// so it is safe to repeat after an interruption.
pub fn provision(lane: &Lane, checkout: &Path, stop: &Cancellation) -> Result<(), WorktreeError> {
    worktree::copy_included(&lane.project, checkout, stop)?;
    worktree::run_setup(&lane.setup, checkout, stop)?;
    Ok(())
}

/// No agent has used a checkout the run has not recorded, so it must equal the base commit.
/// A cancelled `git worktree add` leaves it registered with HEAD set but files missing.
fn confirm_unused_checkout(path: &Path, base: &str) -> Result<(), GitError> {
    if git::branches::resolve(&Host::Local, path, "HEAD")? != base {
        return Err(GitError::Invalid(format!(
            "The existing {} does not start at the approved commit.",
            path.display()
        )));
    }
    if !git::status::read(&Host::Local, path)?.is_clean() {
        return Err(GitError::Invalid(format!(
            "The checkout at {0} has local changes or missing files. Inspect and preserve your work, then retry with a clean checkout at the approved commit.",
            path.display()
        )));
    }
    Ok(())
}

/// Check checkout registration before background agents or commands operate on a recovered run.
pub fn verify(run: u64, lane: &Lane) -> Result<(), GitError> {
    let project = repository(&lane.project)?;
    let name = branch(run, lane);
    if lane.worktree != checkout_path(&project, &name)
        || !project
            .worktrees
            .iter()
            .any(|w| w.path == lane.worktree && w.branch() == Some(&name))
    {
        return Err(GitError::Invalid(format!(
            "The pipeline checkout for {} is missing or on another branch.",
            lane.name
        )));
    }
    Ok(())
}

/// Execute user-configured QA scripts in each indicated checkout. A command error is a failed required check.
pub fn checks(lanes: &[Lane], cancellation: &Cancellation) -> Vec<CommandResult> {
    let mut results = Vec::new();
    for (index, lane) in lanes
        .iter()
        .enumerate()
        .filter(|(_, lane)| lane.requires_check())
    {
        if cancellation.is_cancelled() {
            break;
        }
        let command = Host::Local.command("/bin/sh", &["-c", &lane.command], Some(&lane.worktree));
        let (exit_code, output) = match command::run_with_log_tail(
            command,
            cancellation,
            Duration::from_secs(30 * 60),
            CHECK_OUTPUT,
        ) {
            Ok(result) => {
                let mut output = String::from_utf8_lossy(&result.stdout).into_owned();
                if !result.stderr.is_empty() {
                    if !output.ends_with('\n') && !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&String::from_utf8_lossy(&result.stderr));
                }
                if result.stdout.len() == CHECK_OUTPUT || result.stderr.len() == CHECK_OUTPUT {
                    output.insert_str(0, "[Showing the last 64 KiB per output stream]\n");
                }
                (result.code, output)
            }
            Err(error) => (None, error.to_string()),
        };
        results.push(CommandResult {
            lane: index,
            command: lane.command.clone(),
            success: exit_code == Some(0),
            exit_code,
            output,
        });
    }
    results
}

mod review;
pub use review::{ReviewFile, ReviewRepo, patch, review, review_round};

mod publication;
pub use publication::{PublishedBranch, publication};
mod snapshot;
pub(super) use snapshot::is_object_name;
pub use snapshot::{release_trees, reviewed_trees};

mod revision;
pub use revision::revision;

#[cfg(test)]
mod tests;
