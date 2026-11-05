//! Read the actual repository and remote after an interrupted write; never retry it.
use super::{
    GitError,
    changes::{self, Action},
    run, status,
};
use crate::host::Host;
use std::path::Path;

/// Blocking: inspect HEAD, index and (for push) the actual remote ref, without fetching.
pub fn inspect(host: &Host, cwd: &Path, action: Option<&Action>) -> Result<String, GitError> {
    let status = status::read(host, cwd)?;
    let head = match &status.oid {
        Some(_) => run(host, cwd, &["log", "-1", "--format=%h %s"])?,
        None => "No commits yet".into(),
    };
    let staged = status.files.iter().filter(|f| f.staged.is_some()).count();
    let unstaged = status.files.iter().filter(|f| f.unstaged.is_some()).count();
    let mut report = format!(
        "Current HEAD: {head}\nIndex: {staged} staged files; working tree: {unstaged} changed files."
    );
    if let Some(
        Action::Push { remote, publish }
        | Action::CommitAndPush {
            remote, publish, ..
        },
    ) = action
    {
        let reference = changes::push_destination(host, cwd, remote, *publish)?;
        let output = run(
            host,
            cwd,
            &[
                "-c",
                "credential.interactive=false",
                "ls-remote",
                "--heads",
                remote,
                &reference,
            ],
        )?;
        let remote_oid = output
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .find_map(|(oid, name)| (name == reference).then_some(oid));
        report.push_str(&format!(
            "\nRemote {remote}/{reference}: {}.",
            match remote_oid {
                Some(oid) if Some(oid) == status.oid.as_deref() =>
                    "matches current HEAD".to_owned(),
                Some(oid) => format!("{oid} (different from HEAD)"),
                None => "branch does not exist".to_owned(),
            }
        ));
    }
    report.push_str(
        "\nReview this result before enabling another write. Hive has not retried the operation.",
    );
    Ok(report)
}
