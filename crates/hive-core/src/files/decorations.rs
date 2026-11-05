//! Git decorations shared by file trees; no UI types or per-file processes.
use super::FileError;
use crate::{
    git::status::{Change, FileChange},
    host::Host,
};
use std::{
    collections::{BTreeMap, HashSet},
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    time::Duration,
};

/// Independent index/worktree changes plus an aggregate for ancestor directories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Decoration {
    /// Change staged for commit.
    pub staged: Option<Change>,
    /// Change outside the index.
    pub unstaged: Option<Change>,
    /// A descendant has changes.
    pub descendants: bool,
    /// This path or a descendant has a merge conflict.
    pub conflicted: bool,
}

/// Build path and ancestor decorations without conflating `src` and `src-other`.
pub fn collect(files: &[FileChange]) -> BTreeMap<PathBuf, Decoration> {
    let mut result = BTreeMap::<PathBuf, Decoration>::new();
    for file in files {
        let path = Path::new(&file.path);
        let conflict =
            file.staged == Some(Change::Conflicted) || file.unstaged == Some(Change::Conflicted);
        let value = result.entry(path.into()).or_default();
        value.staged = file.staged.or(value.staged);
        value.unstaged = file.unstaged.or(value.unstaged);
        value.conflicted |= conflict;
        for parent in path
            .ancestors()
            .skip(1)
            .filter(|p| !p.as_os_str().is_empty())
        {
            let value = result.entry(parent.into()).or_default();
            value.descendants = true;
            value.conflicted |= conflict;
        }
    }
    result
}

/// Read ignore rules for a batch of already discovered paths. Git errors never hide files.
pub fn ignored(host: &Host, root: &Path, paths: &[PathBuf]) -> Result<HashSet<PathBuf>, FileError> {
    if paths.is_empty() {
        return Ok(HashSet::new());
    }
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(path.as_os_str().as_bytes());
        input.push(0);
    }
    let result = crate::command::run(
        host.command("git", &["check-ignore", "-z", "--stdin"], Some(root)),
        &input,
        &crate::command::Cancellation::default(),
        Duration::from_secs(10),
        32 * 1024 * 1024,
    )
    .map_err(|e| FileError::Listing {
        path: root.into(),
        message: e.to_string(),
    })?;
    if !matches!(result.code, Some(0 | 1)) {
        return Err(FileError::Listing {
            path: root.into(),
            message: String::from_utf8_lossy(&result.stderr).into_owned(),
        });
    }
    Ok(result
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(std::ffi::OsString::from_vec(p.to_vec())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_sides_and_conflicts_reach_only_real_ancestors() {
        let file = |path: &str, staged, unstaged| FileChange {
            path: path.into(),
            old_path: None,
            staged,
            unstaged,
            submodule: false,
            staged_stats: None,
            unstaged_stats: None,
        };
        let values = collect(&[
            file("src/a.rs", Some(Change::Added), Some(Change::Modified)),
            file(
                "src/deep/b.rs",
                Some(Change::Conflicted),
                Some(Change::Conflicted),
            ),
            file("src-other/c.rs", None, Some(Change::Untracked)),
        ]);
        assert_eq!(values[Path::new("src/a.rs")].staged, Some(Change::Added));
        assert_eq!(
            values[Path::new("src/a.rs")].unstaged,
            Some(Change::Modified)
        );
        assert!(values[Path::new("src")].conflicted);
        assert!(values[Path::new("src/deep")].descendants);
        assert!(!values[Path::new("src-other")].conflicted);
    }
}
