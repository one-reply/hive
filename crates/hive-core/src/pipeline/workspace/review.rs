//! Pin the before-side to the task's start commit and load only the selected patch.
use super::*;
use std::collections::BTreeMap;

/// File changes for one participating repository.
#[derive(Clone, Debug)]
pub struct ReviewRepo {
    /// Repository folder name for display.
    pub name: String,
    /// Index of the first role bound to this repository.
    pub lane: usize,
    /// Changed files sorted by their repository-relative path.
    pub files: Vec<ReviewFile>,
}

/// One selectable file in the acceptance review.
#[derive(Clone, Debug)]
pub struct ReviewFile {
    /// Exact repository-relative path.
    pub path: String,
    /// Git change against the immutable task base.
    pub change: Change,
    /// Added lines, unavailable for binary or oversized files.
    pub added: Option<usize>,
    /// Removed lines, unavailable for binary or oversized files.
    pub removed: Option<usize>,
    old_path: Option<String>,
    untracked: bool,
    comparison: Option<std::sync::Arc<(String, String)>>,
}

/// Read statuses and line counts without rendering or loading whole-repository patches.
pub fn review(id: u64, lanes: &[Lane]) -> Result<Vec<ReviewRepo>, GitError> {
    read_files(id, lanes, &[], &[])
}

/// Read immutable reviewed files from the original base or the previous feedback round.
pub fn review_round(
    run: &crate::pipeline::Run,
    since_feedback: bool,
) -> Result<Vec<ReviewRepo>, GitError> {
    if !matches!(
        run.phase(),
        crate::pipeline::Phase::Acceptance | crate::pipeline::Phase::Accepted
    ) || run.review_trees().is_empty()
    {
        return review(run.id(), run.lanes());
    }
    read_files(
        run.id(),
        run.lanes(),
        if since_feedback {
            run.feedback_trees()
        } else {
            &[]
        },
        run.review_trees(),
    )
}

fn read_files(
    id: u64,
    lanes: &[Lane],
    before: &[String],
    after: &[String],
) -> Result<Vec<ReviewRepo>, GitError> {
    if [before, after]
        .iter()
        .any(|trees| !trees.is_empty() && trees.len() != lanes.len())
    {
        return Err(GitError::Invalid(
            "Incomplete stored review snapshot.".into(),
        ));
    }
    let mut seen = HashSet::new();
    let mut repos = Vec::new();
    for (index, lane) in lanes
        .iter()
        .enumerate()
        .filter(|(_, l)| seen.insert(&l.worktree))
    {
        verify(id, lane)?;
        let base = pinned_base(lane)?;
        let base = before.get(index).cloned().unwrap_or(base);
        let comparison = after
            .get(index)
            .map(|tree| std::sync::Arc::new((base.clone(), tree.clone())));
        if !is_object_name(&base)
            || comparison
                .as_deref()
                .is_some_and(|(_, tree)| !is_object_name(tree))
        {
            return Err(GitError::Invalid("Invalid stored review snapshot.".into()));
        }
        let mut args = vec!["diff".into(), "--ignore-submodules=none".into(), base];
        args.extend(after.get(index).cloned());
        let tracked = git::history::files(&Host::Local, &lane.worktree, args)?;
        let mut files = BTreeMap::new();
        for change in tracked {
            files.insert(
                change.path.clone(),
                ReviewFile {
                    path: change.path,
                    change: change.staged.unwrap_or(Change::Modified),
                    added: change.staged_stats.map(|s| s.added),
                    removed: change.staged_stats.map(|s| s.removed),
                    old_path: change.old_path,
                    untracked: false,
                    comparison: comparison.clone(),
                },
            );
        }
        if comparison.is_none() {
            for change in git::status::read_with_stats(&Host::Local, &lane.worktree)?
                .files
                .into_iter()
                .filter(|f| f.unstaged == Some(Change::Untracked))
            {
                let added = change.unstaged_stats.map(|s| s.added);
                let removed = change.unstaged_stats.map(|s| s.removed);
                if let Some(file) = files.get_mut(&change.path) {
                    // git rm --cached leaves both a tracked deletion and an untracked replacement.
                    file.untracked = true;
                    file.change = Change::Modified;
                    file.added = file.added.zip(added).map(|(a, b)| a + b);
                    file.removed = file.removed.zip(removed).map(|(a, b)| a + b);
                } else {
                    files.insert(
                        change.path.clone(),
                        ReviewFile {
                            path: change.path,
                            change: Change::Untracked,
                            added,
                            removed,
                            old_path: None,
                            untracked: true,
                            comparison: None,
                        },
                    );
                }
            }
        }
        repos.push(ReviewRepo {
            name: lane
                .project
                .file_name()
                .map_or_else(|| lane.name.clone(), |s| s.to_string_lossy().into_owned()),
            lane: index,
            files: files.into_values().collect(),
        });
    }
    Ok(repos)
}

pub(super) fn pinned_base(lane: &Lane) -> Result<String, GitError> {
    let base = git::branches::resolve(&Host::Local, &lane.worktree, &lane.base)?;
    if base != lane.base {
        return Err(GitError::Invalid(
            "Pipeline base must be a pinned commit.".into(),
        ));
    }
    Ok(base)
}

/// Load the selected file's full-index patch, including added files and both paths of a rename.
pub fn patch(id: u64, lane: &Lane, file: &ReviewFile) -> Result<String, GitError> {
    verify(id, lane)?;
    let mut result = String::new();
    if file.change != Change::Untracked {
        let mut args = vec!["diff".into(), "--ignore-submodules=none".into()];
        if let Some((before, after)) = file.comparison.as_deref() {
            args.extend([before.clone(), after.clone()]);
        } else {
            args.push(pinned_base(lane)?);
        }
        let diff = git::diff::read_patch(
            &Host::Local,
            &lane.worktree,
            &file.path,
            file.old_path.as_deref(),
            args,
            false,
        )?;
        result.push_str(diff.patch());
    }
    if file.untracked {
        let added = git::diff::read(
            &Host::Local,
            &lane.worktree,
            &file.path,
            &DiffSource::Untracked,
            None,
        )?;
        if result.len().saturating_add(added.patch().len()) > git::diff::PREVIEW_LIMIT {
            return Err(GitError::TooLarge(git::diff::PREVIEW_LIMIT));
        }
        result.push_str(added.patch());
    }
    Ok(result)
}
