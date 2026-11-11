//! Running `git` on a [`Host`] and turning its exit status into a typed error.

use crate::command::{self, Cancellation};
use crate::host::Host;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub mod branches;
pub mod changes;
#[cfg(feature = "desktop")]
pub mod commit_context;
pub mod conflicts;
pub mod diff;
pub mod history;
pub mod hosting;
mod numstat;
pub mod pull_request;
pub mod review_notes;
pub mod status;
pub mod verification;

/// A `git` invocation that could not run or exited non-zero.
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The process could not be started (no `git`, no `ssh`, bad cwd).
    #[error("failed to run `git {command}` on {host:?}: {source}")]
    Spawn {
        command: String,
        host: Host,
        #[source]
        source: std::io::Error,
    },
    /// `git` ran and exited non-zero; `stderr` is what it printed.
    #[error("`git {command}` failed on {host:?} in {cwd}: {stderr}")]
    Failed {
        command: String,
        host: Host,
        cwd: PathBuf,
        code: Option<i32>,
        stderr: String,
    },
    /// A deadline or pipe failure interrupted Git; writes may already have taken effect.
    #[error("`git {command}` interrupted on {host:?} in {cwd}: {source}. {guidance}")]
    Interrupted {
        /// Arguments identifying the interrupted operation.
        command: String,
        /// Host on which Git was started.
        host: Host,
        /// Worktree containing the operation.
        cwd: PathBuf,
        /// Why the process was interrupted.
        #[source]
        source: command::Error,
        /// Whether the operation could have changed files, refs or a remote.
        mutation: bool,
        /// Recovery instructions appropriate to the operation.
        guidance: &'static str,
    },
    /// A malformed response or an operation that cannot safely proceed.
    #[error("{0}")]
    Invalid(String),
    /// A bounded read exceeded the preview budget.
    #[error("Git output exceeds the {0} byte preview limit. Open this change in the terminal.")]
    TooLarge(usize),
    /// The commit succeeded locally, but its subsequent push failed.
    #[error(
        "Commit created locally, but push did not finish normally. Inspect the remote before retrying Push.\n{source}"
    )]
    PushAfterCommit {
        #[source]
        source: Box<GitError>,
    },
}

impl GitError {
    /// Whether Git reported that the working directory is outside a repository.
    pub fn is_not_repository(&self) -> bool {
        matches!(self, Self::Failed { code: Some(128), stderr, .. }
            if stderr.contains("not a git repository"))
    }

    /// Whether a command was interrupted after it could have changed the repository.
    pub fn outcome_unknown(&self) -> bool {
        match self {
            Self::Interrupted { mutation, .. } => *mutation,
            Self::PushAfterCommit { source } => source.outcome_unknown(),
            _ => false,
        }
    }

    /// What `git` printed on failure, empty when it never ran.
    pub fn stderr(&self) -> &str {
        match self {
            GitError::Spawn { .. }
            | GitError::Invalid(_)
            | GitError::TooLarge(_)
            | GitError::Interrupted { .. } => "",
            GitError::Failed { stderr, .. } => stderr,
            GitError::PushAfterCommit { source } => source.stderr(),
        }
    }
}

/// Blocking: runs `git args` in `cwd` on `host` and returns its trimmed stdout.
pub fn run(host: &Host, cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    Ok(
        String::from_utf8_lossy(&bytes(host, cwd, args, None, READ_LIMIT, false)?)
            .trim_end()
            .to_string(),
    )
}

const READ_LIMIT: usize = 16 * 1024 * 1024;
/// Deadline for queries, including anything run over ssh to inspect a worktree.
pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(15);
/// Deadline for commands that may change files, refs or a remote.
const MUTATION_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) fn run_mutation(host: &Host, cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    Ok(
        String::from_utf8_lossy(&bytes_mutation(host, cwd, args, None, READ_LIMIT, false)?)
            .trim_end()
            .to_owned(),
    )
}

pub(crate) fn bytes(
    host: &Host,
    cwd: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    limit: usize,
    allow_exit_one: bool,
) -> Result<Vec<u8>, GitError> {
    invoke(host, cwd, args, input, limit, allow_exit_one, false)
}

fn bytes_mutation(
    host: &Host,
    cwd: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    limit: usize,
    allow_exit_one: bool,
) -> Result<Vec<u8>, GitError> {
    invoke(host, cwd, args, input, limit, allow_exit_one, true)
}

fn invoke(
    host: &Host,
    cwd: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    limit: usize,
    allow_exit_one: bool,
    mutation: bool,
) -> Result<Vec<u8>, GitError> {
    let mut command = match host {
        Host::Local => host.command("git", args, Some(cwd)),
        Host::Ssh { .. } => {
            let mut remote = vec![
                "GIT_TERMINAL_PROMPT=0",
                "GIT_EDITOR=true",
                "LC_ALL=C",
                "git",
            ];
            remote.extend_from_slice(args);
            host.command("env", &remote, Some(cwd))
        }
    };
    // Nothing here has a terminal: an editor Git opened would only hold the command until
    // its deadline. The variable outranks `core.editor`, so configuration cannot bring it back.
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("LC_ALL", "C");
    let timeout = if mutation {
        MUTATION_TIMEOUT
    } else {
        READ_TIMEOUT
    };
    let output = command::run(command, input.unwrap_or_default(), &Cancellation::default(), timeout, limit)
        .map_err(|source| match source {
            command::Error::TooLarge(limit) if !mutation => GitError::TooLarge(limit),
            source => GitError::Interrupted {
                command: args.join(" "), host: host.clone(), cwd: cwd.to_owned(), source, mutation,
                guidance: if mutation { "Outcome unknown. Refresh and inspect the index, History and remote before retrying; no automatic retry was made." } else { "Refresh to retry." },
            },
        })?;
    finish(host, cwd, args, output, allow_exit_one, mutation)
}

fn finish(
    host: &Host,
    cwd: &Path,
    args: &[&str],
    output: command::Output,
    allow_exit_one: bool,
    mutation: bool,
) -> Result<Vec<u8>, GitError> {
    if mutation && !host.is_local() && output.code == Some(255) {
        return Err(GitError::Interrupted {
            command: args.join(" "),
            host: host.clone(),
            cwd: cwd.to_owned(),
            source: command::Error::Io(std::io::Error::other(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            )),
            mutation: true,
            guidance: "SSH disconnected. Outcome unknown. Refresh and inspect the repository before retrying; nothing was retried.",
        });
    }
    if output.code != Some(0) && !(allow_exit_one && output.code == Some(1)) {
        return Err(GitError::Failed {
            command: args.join(" "),
            host: host.clone(),
            cwd: cwd.to_owned(),
            code: output.code,
            stderr: format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            )
            .trim()
            .into(),
        });
    }
    Ok(output.stdout)
}

fn text(bytes: &[u8]) -> Result<&str, GitError> {
    std::str::from_utf8(bytes).map_err(|_| {
        GitError::Invalid("Git returned non-UTF-8 content. Use the terminal for this file.".into())
    })
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    #[test]
    fn only_missing_repository_diagnostics_are_classified_as_absent() {
        for host in [
            Host::Local,
            Host::Ssh {
                target: "test".into(),
            },
        ] {
            for (code, stderr, absent) in [
                (
                    128,
                    "fatal: not a git repository (or any of the parent directories): .git",
                    true,
                ),
                (
                    128,
                    "fatal: detected dubious ownership in repository",
                    false,
                ),
                (255, "not a git repository", false),
                (128, "Permission denied", false),
            ] {
                let error = GitError::Failed {
                    command: "status".into(),
                    host: host.clone(),
                    cwd: "/repo".into(),
                    code: Some(code),
                    stderr: stderr.into(),
                };
                assert_eq!(error.is_not_repository(), absent);
            }
        }
        assert!(!GitError::Invalid("not a git repository".into()).is_not_repository());
    }

    #[test]
    fn ssh_disconnect_marks_writes_unknown_but_keeps_read_and_git_failures_distinct() {
        let remote = Host::Ssh {
            target: "home".into(),
        };
        let outcome = |host: &Host, mutation, code| {
            finish(
                host,
                Path::new("/repo"),
                &["operation"],
                command::Output {
                    code: Some(code),
                    stdout: vec![],
                    stderr: b"connection lost".to_vec(),
                },
                false,
                mutation,
            )
            .unwrap_err()
        };
        assert!(outcome(&remote, true, 255).outcome_unknown());
        for error in [
            outcome(&remote, false, 255),
            outcome(&remote, true, 1),
            outcome(&Host::Local, true, 255),
        ] {
            assert!(!error.outcome_unknown());
            assert!(matches!(error, GitError::Failed { .. }));
        }
    }
}
