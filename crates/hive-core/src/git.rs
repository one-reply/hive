//! Running `git` on a [`Host`] and turning its exit status into a typed error.

use crate::host::Host;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub mod branches;
pub mod changes;
pub mod commit_context;
pub mod diff;
pub mod history;
mod numstat;
pub mod status;

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
    /// A malformed response or an operation that cannot safely proceed.
    #[error("{0}")]
    Invalid(String),
    /// A bounded read exceeded the preview budget.
    #[error("Git output exceeds the {0} byte preview limit. Open this change in the terminal.")]
    TooLarge(usize),
}

impl GitError {
    /// What `git` printed on failure, empty when it never ran.
    pub fn stderr(&self) -> &str {
        match self {
            GitError::Spawn { .. } | GitError::Invalid(_) | GitError::TooLarge(_) => "",
            GitError::Failed { stderr, .. } => stderr,
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

fn read_bounded(mut reader: impl Read, limit: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    let mut oversized = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let keep = count.min(limit.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..keep]);
        oversized |= keep < count;
    }
    Ok((output, oversized))
}

// Drain both pipes while writing stdin: hooks and large patches can fill either pipe.
fn bytes(
    host: &Host,
    cwd: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    limit: usize,
    diff_exit: bool,
) -> Result<Vec<u8>, GitError> {
    let failure = |source| GitError::Spawn {
        command: args.join(" "),
        host: host.clone(),
        source,
    };
    let mut command = match host {
        Host::Local => host.command("git", args, Some(cwd)),
        Host::Ssh { .. } => {
            let mut remote = vec!["GIT_TERMINAL_PROMPT=0", "LC_ALL=C", "git"];
            remote.extend_from_slice(args);
            host.command("env", &remote, Some(cwd))
        }
    };
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(failure)?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let stdin = child.stdin.take();
    let (output, errors, written) = std::thread::scope(|scope| {
        let out = scope.spawn(|| read_bounded(stdout, limit));
        let err = scope.spawn(|| read_bounded(stderr, READ_LIMIT));
        let written = match (stdin, input) {
            (Some(mut stdin), Some(input)) => stdin.write_all(input),
            _ => Ok(()),
        };
        (out.join(), err.join(), written)
    });
    let status = child.wait().map_err(failure)?;
    let join_error = || GitError::Invalid("Git output reader stopped unexpectedly".into());
    let (output, oversized) = output.map_err(|_| join_error())?.map_err(failure)?;
    let (errors, _) = errors.map_err(|_| join_error())?.map_err(failure)?;
    if !status.success() && !(diff_exit && status.code() == Some(1)) {
        return Err(GitError::Failed {
            command: args.join(" "),
            host: host.clone(),
            cwd: cwd.to_path_buf(),
            code: status.code(),
            stderr: format!(
                "{}\n{}",
                String::from_utf8_lossy(&errors),
                String::from_utf8_lossy(&output)
            )
            .trim()
            .into(),
        });
    }
    written.map_err(failure)?;
    if oversized {
        return Err(GitError::TooLarge(limit));
    }
    Ok(output)
}

fn text(bytes: &[u8]) -> Result<&str, GitError> {
    std::str::from_utf8(bytes).map_err(|_| {
        GitError::Invalid("Git returned non-UTF-8 content. Use the terminal for this file.".into())
    })
}
