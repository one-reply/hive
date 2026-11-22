//! Read the actual repository and remote after an interrupted write; never retry it.
use super::{
    GitError,
    changes::{self, Action},
    conflicts, run, status,
};
use crate::host::Host;
use std::path::Path;

/// Blocking: inspect HEAD, index, an unfinished operation and (for a push) the actual remote
/// ref, without fetching.
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
    let pushed = match action {
        Some(
            Action::Push { remote, publish }
            | Action::CommitAndPush {
                remote, publish, ..
            },
        ) => Some((remote, *publish)),
        Some(
            Action::Sync { remote }
            | Action::CommitAndSync { remote, .. }
            | Action::ForcePush { remote, .. },
        ) => Some((remote, false)),
        _ => None,
    };
    if let Some(operation) = conflicts::operation(host, cwd)? {
        // HEAD is detached mid-replay, so the branch's remote ref says nothing yet.
        report.push_str(&format!(
            "\nA {} is unfinished: continue or abort it from the Git panel.",
            operation.command()
        ));
    } else if let Some((remote, publish)) = pushed {
        let reference = changes::push_destination(host, cwd, remote, publish)?;
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
