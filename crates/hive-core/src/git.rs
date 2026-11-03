//! Running `git` on a [`Host`] and turning its exit status into a typed error.

use crate::host::Host;
use std::path::Path;

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
    #[error("`git {command}` failed: {stderr}")]
    Failed { command: String, stderr: String },
}

impl GitError {
    /// What `git` printed on failure, empty when it never ran.
    pub fn stderr(&self) -> &str {
        match self {
            GitError::Spawn { .. } => "",
            GitError::Failed { stderr, .. } => stderr,
        }
    }
}

/// Blocking: runs `git args` in `cwd` on `host` and returns its trimmed stdout.
pub fn run(host: &Host, cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    let output = host.command("git", args, Some(cwd)).output().map_err(|source| GitError::Spawn {
        command: args.join(" "),
        host: host.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(GitError::Failed {
            command: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}
