//! Commit history and changed-file lists for immutable review targets.
use super::status::{Change, FileChange};
use super::{GitError, READ_LIMIT, branches, bytes, numstat, run, text};
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
    let counts = numstat::read(
        host,
        cwd,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        &[],
    )
    .unwrap_or_else(|error| {
        tracing::warn!(%error, path = %cwd.display(), "Review line counts unavailable");
        Default::default()
    });
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
            staged_stats: counts.get(&path).copied(),
            unstaged_stats: None,
            conflict: None,
            path,
            old_path,
            staged: Some(change),
            unstaged: None,
            submodule: false,
        });
    }
    Ok(result)
}

/// Human age of an ISO-8601 author date: "just now", "5m ago", "3h ago", "2d ago", then the
/// plain calendar date. Unparseable input is returned as is.
pub fn relative_date(iso: &str) -> String {
    relative_date_at(iso, chrono::Utc::now())
}

/// Spelled-out age of an ISO-8601 date for prose such as inline blame: "just now",
/// "5 minutes ago", "3 days ago", "2 weeks ago", "4 months ago", "1 year ago". Unparseable input
/// is returned as is.
pub fn age(iso: &str) -> String {
    age_at(iso, chrono::Utc::now())
}

fn age_at(iso: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(date) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let seconds = (now - date.with_timezone(&chrono::Utc)).num_seconds();
    let (count, unit) = match seconds {
        ..60 => return "just now".into(),
        60..3_600 => (seconds / 60, "minute"),
        3_600..86_400 => (seconds / 3_600, "hour"),
        86_400..604_800 => (seconds / 86_400, "day"),
        604_800..2_592_000 => (seconds / 604_800, "week"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month"),
        _ => (seconds / 31_536_000, "year"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// Calendar day of an ISO-8601 date in its own zone, as `11 Oct 2026`. Unparseable input is
/// returned as is.
pub fn calendar_date(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|date| date.format("%-d %b %Y").to_string())
        .unwrap_or_else(|_| iso.to_string())
}

fn relative_date_at(iso: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(date) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let seconds = (now - date.with_timezone(&chrono::Utc)).num_seconds();
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3600)
    } else if seconds < 2_592_000 {
        format!("{}d ago", seconds / 86_400)
    } else {
        date.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::relative_date_at as relative_date;
    use super::{age_at, calendar_date};
    use chrono::{DateTime, Utc};

    #[test]
    fn relative_dates() {
        let now: DateTime<Utc> = "2026-11-04T12:00:00Z".parse().unwrap();
        assert_eq!(relative_date("2026-11-04T11:59:30Z", now), "just now");
        assert_eq!(relative_date("2026-11-04T11:15:00+00:00", now), "45m ago");
        assert_eq!(relative_date("2026-11-04T05:00:00+02:00", now), "9h ago");
        assert_eq!(relative_date("2026-11-01T12:00:00Z", now), "3d ago");
        assert_eq!(relative_date("2026-01-15T12:00:00Z", now), "2026-01-15");
        assert_eq!(relative_date("yesterday", now), "yesterday");
    }

    #[test]
    fn ages_are_spelled_out() {
        let now: DateTime<Utc> = "2026-11-04T12:00:00Z".parse().unwrap();
        for (date, age) in [
            ("2026-11-04T11:59:30Z", "just now"),
            ("2026-11-04T11:59:00Z", "1 minute ago"),
            ("2026-11-04T11:15:00+00:00", "45 minutes ago"),
            ("2026-11-04T05:00:00+02:00", "9 hours ago"),
            ("2026-11-03T12:00:00Z", "1 day ago"),
            ("2026-10-21T12:00:00Z", "2 weeks ago"),
            ("2026-07-04T12:00:00Z", "4 months ago"),
            ("2025-11-04T12:00:00Z", "1 year ago"),
            ("2020-01-15T12:00:00Z", "6 years ago"),
            ("2026-11-05T12:00:00Z", "just now"),
            ("yesterday", "yesterday"),
        ] {
            assert_eq!(age_at(date, now), age, "{date}");
        }
    }

    #[test]
    fn calendar_dates_use_the_author_zone() {
        assert_eq!(calendar_date("2026-10-11T23:30:00-02:00"), "11 Oct 2026");
        assert_eq!(calendar_date(""), "");
    }
}
