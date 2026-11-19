//! Making a fresh worktree usable: Git checks out tracked files only, so the ignored files a
//! project needs to build or run (`.env`, local config) are copied from the source checkout
//! as `.worktreeinclude` names them, and the project's setup command installs the rest.
use crate::command::{self, Cancellation};
use crate::git::{self, GitError};
use crate::host::Host;
use regex::Regex;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Lists, in `.gitignore` syntax, the ignored files and folders a new worktree should get.
pub const INCLUDE_FILE: &str = ".worktreeinclude";

/// Installing dependencies takes minutes; this only bounds a command nobody is watching.
const SETUP_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// A failed setup is diagnosed from the end of its log.
const SETUP_LOG_TAIL: usize = 8 * 1024;
/// The collapsed list of ignored entries is short even in a repository with a large build cache.
const IGNORED_LIST_LIMIT: usize = 16 * 1024 * 1024;

/// Why a new worktree could not be made ready for work.
#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    /// A file `.worktreeinclude` names could not be read or written.
    #[error("could not copy {} into the worktree: {source}", path.display())]
    Copy {
        /// The source file or folder.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// The setup command did not finish successfully.
    #[error("setup command `{command}` {outcome}\n{output}")]
    Setup {
        /// The command as configured.
        command: String,
        /// How it ended: an exit code, a timeout or a spawn error.
        outcome: String,
        /// End of its combined output.
        output: String,
    },
    /// The user stopped the preparation.
    #[error("worktree preparation was stopped")]
    Stopped,
    /// Git could not list the ignored files.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// The patterns of a `.worktreeinclude` file.
///
/// Supported `.gitignore` syntax: `#` comments, `*` and `?` within a path segment, `**`, a
/// leading or inner `/` anchoring the pattern to the repository root, and a trailing `/` for
/// folders only. A pattern without `/` matches the name at any depth. Negation (`!`) is not
/// supported: such a line is skipped, since guessing would copy the wrong files.
#[derive(Debug)]
pub struct IncludePatterns(Vec<Pattern>);

#[derive(Debug)]
struct Pattern {
    path: Regex,
    folders_only: bool,
}

impl IncludePatterns {
    /// Parses the file's text; a line this build cannot represent is skipped with a warning.
    pub fn parse(text: &str) -> Self {
        Self(text.lines().filter_map(Pattern::parse).collect())
    }

    /// Whether the repository-relative `path` is named by any pattern.
    pub fn matches(&self, path: &str, is_folder: bool) -> bool {
        self.0
            .iter()
            .any(|p| (is_folder || !p.folders_only) && p.path.is_match(path))
    }

    /// Whether the file names nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Pattern {
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        if line.starts_with('!') {
            tracing::warn!(line, "negated .worktreeinclude pattern skipped");
            return None;
        }
        let (body, folders_only) = match line.strip_suffix('/') {
            Some(body) => (body, true),
            None => (line, false),
        };
        let anchored = body.contains('/');
        let body = body.strip_prefix('/').unwrap_or(body);
        if body.is_empty() {
            return None;
        }
        let mut expression = String::from(if anchored { "^" } else { "^(?:.*/)?" });
        let mut rest = body;
        while let Some(c) = rest.chars().next() {
            if let Some(after) = rest.strip_prefix("**/") {
                expression.push_str("(?:.*/)?");
                rest = after;
            } else if let Some(after) = rest.strip_prefix("**") {
                expression.push_str(".*");
                rest = after;
            } else {
                match c {
                    '*' => expression.push_str("[^/]*"),
                    '?' => expression.push_str("[^/]"),
                    c => expression.push_str(&regex::escape(c.encode_utf8(&mut [0; 4]))),
                }
                rest = &rest[c.len_utf8()..];
            }
        }
        expression.push('$');
        match Regex::new(&expression) {
            Ok(path) => Some(Self { path, folders_only }),
            Err(error) => {
                tracing::warn!(line, %error, "unreadable .worktreeinclude pattern skipped");
                None
            }
        }
    }
}

/// Blocking: copies the ignored files and folders that `source`'s `.worktreeinclude` names
/// into `checkout`, keeping their relative paths. Returns how many entries were copied.
///
/// Only entries Git ignores are candidates, so a tracked or merely untracked file is never
/// duplicated into the new worktree's changes. Nothing already present in `checkout` is
/// overwritten, which makes a retry after an interruption safe.
pub fn copy_included(
    source: &Path,
    checkout: &Path,
    stop: &Cancellation,
) -> Result<usize, ProvisionError> {
    let text = match fs::read(source.join(INCLUDE_FILE)) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(ProvisionError::Copy {
                path: source.join(INCLUDE_FILE),
                source: error,
            });
        }
    };
    let patterns = IncludePatterns::parse(&text);
    if patterns.is_empty() {
        return Ok(0);
    }
    // `--directory` collapses an ignored folder into one entry, so a build cache with
    // millions of files is neither walked nor listed.
    let listing = git::bytes(
        &Host::Local,
        source,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
        None,
        IGNORED_LIST_LIMIT,
        false,
    )?;
    let mut copied = 0;
    for entry in listing.split(|byte| *byte == 0) {
        let Ok(entry) = std::str::from_utf8(entry) else {
            continue;
        };
        let (path, is_folder) = match entry.strip_suffix('/') {
            Some(path) => (path, true),
            None => (entry, false),
        };
        if path.is_empty() || !patterns.matches(path, is_folder) {
            continue;
        }
        let from = source.join(path);
        copy_entry(&from, &checkout.join(path), stop).map_err(|error| match error {
            CopyError::Stopped => ProvisionError::Stopped,
            CopyError::Io(source) => ProvisionError::Copy { path: from, source },
        })?;
        copied += 1;
    }
    if copied > 0 {
        tracing::info!(?checkout, copied, "copied .worktreeinclude entries");
    }
    Ok(copied)
}

enum CopyError {
    Stopped,
    Io(io::Error),
}

impl From<io::Error> for CopyError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Copies a file, a symbolic link (as a link) or a folder's contents; existing destinations
/// are left as they are.
fn copy_entry(from: &Path, to: &Path, stop: &Cancellation) -> Result<(), CopyError> {
    if stop.is_cancelled() {
        return Err(CopyError::Stopped);
    }
    let kind = fs::symlink_metadata(from)?.file_type();
    if kind.is_dir() {
        fs::create_dir_all(to)?;
        for child in fs::read_dir(from)? {
            let child = child?;
            copy_entry(&child.path(), &to.join(child.file_name()), stop)?;
        }
        return Ok(());
    }
    if fs::symlink_metadata(to).is_ok() {
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if kind.is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
    } else {
        fs::copy(from, to)?;
    }
    Ok(())
}

/// Blocking: runs the project's setup command with `sh -c` in `checkout`. A blank command
/// is no setup.
pub fn run_setup(
    command: &str,
    checkout: &Path,
    stop: &Cancellation,
) -> Result<(), ProvisionError> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(());
    }
    let failed = |outcome: String, output: String| ProvisionError::Setup {
        command: command.to_owned(),
        outcome,
        output,
    };
    let process = Host::Local.command("/bin/sh", &["-c", command], Some(checkout));
    let result = command::run_with_log_tail(process, stop, SETUP_TIMEOUT, SETUP_LOG_TAIL).map_err(
        |error| {
            if stop.is_cancelled() {
                ProvisionError::Stopped
            } else {
                failed(error.to_string(), String::new())
            }
        },
    )?;
    if result.code == Some(0) {
        return Ok(());
    }
    let mut output = String::from_utf8_lossy(&result.stdout).into_owned();
    if !result.stderr.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&String::from_utf8_lossy(&result.stderr));
    }
    let outcome = match result.code {
        Some(code) => format!("exited with {code}"),
        None => "was terminated".to_owned(),
    };
    Err(failed(outcome, output))
}

#[cfg(test)]
mod tests;
