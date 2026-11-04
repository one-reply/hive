//! Commit history and changed-file lists for immutable review targets.
use super::status::{Change, FileChange};
use super::{GitError, READ_LIMIT, branches, bytes, run, text};
use crate::host::Host;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One commit in the current branch's history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    /// Full object ID.
    pub oid: String,
    /// Subject line.
    pub subject: String,
    /// Author name.
    pub author: String,
    /// ISO-8601 author date.
    pub date: String,
}

/// Read a page from a fixed HEAD so pagination cannot skip commits after a refresh.
pub fn read(host: &Host, cwd: &Path, head: &str, skip: usize) -> Result<Vec<Commit>, GitError> {
    let oid = branches::resolve(host, cwd, head)?;
    let raw = bytes(
        host,
        cwd,
        &[
            "log",
            "-z",
            "--max-count=50",
            &format!("--skip={skip}"),
            "--format=%H%x00%an%x00%aI%x00%s",
            &oid,
            "--",
        ],
        None,
        READ_LIMIT,
        false,
    )?;
    let mut records: Vec<_> = raw.split(|b| *b == 0).collect();
    if records.last() == Some(&&[][..]) {
        records.pop();
    }
    if records.len() % 4 != 0 {
        return Err(GitError::Invalid("Malformed Git history".into()));
    }
    records
        .as_chunks::<4>()
        .0
        .iter()
        .map(|f| {
            Ok(Commit {
                oid: text(f[0])?.into(),
                author: text(f[1])?.into(),
                date: text(f[2])?.into(),
                subject: text(f[3])?.into(),
            })
        })
        .collect()
}

pub(super) fn commit_args(host: &Host, cwd: &Path, commit: &str) -> Result<Vec<String>, GitError> {
    let oid = branches::resolve(host, cwd, commit)?;
    Ok(vec![
        "show".into(),
        "--format=".into(),
        "--root".into(),
        "--diff-merges=first-parent".into(),
        oid,
    ])
}

pub(super) fn compare_args(host: &Host, cwd: &Path, base: &str) -> Result<Vec<String>, GitError> {
    let base = branches::resolve(host, cwd, base)?;
    let head = branches::resolve(host, cwd, "HEAD")?;
    let ancestor = run(host, cwd, &["merge-base", &base, &head]).map_err(|error| {
        if matches!(error, GitError::Failed { code: Some(1), .. }) {
            GitError::Invalid("These branches have no available common ancestor. Fetch history or choose another base.".into())
        } else {
            error
        }
    })?;
    Ok(vec!["diff".into(), ancestor, head])
}

/// Files introduced by this branch since its common ancestor with `base`.
pub fn compare_files(host: &Host, cwd: &Path, base: &str) -> Result<Vec<FileChange>, GitError> {
    files(host, cwd, compare_args(host, cwd, base)?)
}

/// Files changed by a commit relative to its first parent (empty tree for the root).
pub fn commit_files(host: &Host, cwd: &Path, commit: &str) -> Result<Vec<FileChange>, GitError> {
    files(host, cwd, commit_args(host, cwd, commit)?)
}

fn files(host: &Host, cwd: &Path, mut args: Vec<String>) -> Result<Vec<FileChange>, GitError> {
    args.extend(
        [
            "--name-status",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames",
            "--",
        ]
        .map(String::from),
    );
    let raw = bytes(
        host,
        cwd,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
        READ_LIMIT,
        false,
    )?;
    let mut parts = raw.split(|b| *b == 0).filter(|p| !p.is_empty());
    let mut result = Vec::new();
    while let Some(code) = parts.next() {
        let change = match code.first() {
            Some(b'A') => Change::Added,
            Some(b'D') => Change::Deleted,
            Some(b'R') => Change::Renamed,
            Some(b'C') => Change::Copied,
            Some(b'T') => Change::TypeChanged,
            _ => Change::Modified,
        };
        let mut path = text(
            parts
                .next()
                .ok_or_else(|| GitError::Invalid("Missing Git path".into()))?,
        )?
        .to_string();
        let old_path = if matches!(change, Change::Renamed | Change::Copied) {
            let old = path;
            path = text(
                parts
                    .next()
                    .ok_or_else(|| GitError::Invalid("Missing rename destination".into()))?,
            )?
            .into();
            Some(old)
        } else {
            None
        };
        result.push(FileChange {
            path,
            old_path,
            staged: Some(change),
            unstaged: None,
            submodule: false,
        });
    }
    Ok(result)
}
