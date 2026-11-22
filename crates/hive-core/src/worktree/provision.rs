//! Making a fresh worktree usable: Git checks out tracked files only, so the ignored files a
//! project needs to build or run (`.env`, local config) are copied from the source checkout
//! as `.worktreeinclude` names them, and the project's setup command installs the rest.
use crate::command::{self, Cancellation, End};
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
///
/// Inside an ignored folder only anchored patterns reach a file, such as
/// `.vscode/settings.json` or `config/local/*.yml`; `**/settings.json` does not look inside
/// one, since that would walk every ignored folder, `node_modules` included.
#[derive(Debug)]
pub struct IncludePatterns(Vec<Pattern>);

#[derive(Debug)]
struct Pattern {
    path: Regex,
    folders_only: bool,
    /// For an anchored pattern, the part of its path before any wildcard: the whole path, or
    /// the folder its wildcard part lives in. `None` when a wildcard comes first.
    within: Option<String>,
}

impl IncludePatterns {
    /// Parses the file's text; a line this build cannot represent is skipped with a warning.
    pub fn parse(text: &str) -> Self {
        Self(text.lines().filter_map(Pattern::parse).collect())
    }

    /// Whether the repository-relative `path` is named by any pattern. A folder is also named
    /// by a pattern for everything in it, such as `cache/**` or `cache/*`.
    pub fn matches(&self, path: &str, is_folder: bool) -> bool {
        let contents = is_folder.then(|| format!("{path}/"));
        self.0.iter().any(|p| {
            (is_folder || !p.folders_only)
                && (p.path.is_match(path)
                    || contents.as_deref().is_some_and(|c| p.path.is_match(c)))
        })
    }

    /// The paths inside `folder` that anchored patterns point into, for Git to list the
    /// ignored files there that a collapsed listing of `folder` hides.
    fn inside(&self, folder: &str) -> Vec<&str> {
        let prefix = format!("{folder}/");
        self.0
            .iter()
            .filter_map(|p| p.within.as_deref())
            .filter(|within| within.starts_with(&prefix))
            .collect()
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
        let within = anchored
            .then(|| match body.find(['*', '?']) {
                None => body,
                Some(wildcard) => body[..wildcard]
                    .rfind('/')
                    .map_or("", |slash| &body[..slash]),
            })
            .filter(|within| !within.is_empty())
            .map(str::to_owned);
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
            Ok(path) => Some(Self {
                path,
                folders_only,
                within,
            }),
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
    let copy = |path: &str| {
        let from = source.join(path);
        copy_entry(&from, &checkout.join(path), stop).map_err(|error| match error {
            CopyError::Stopped => ProvisionError::Stopped,
            CopyError::Io(source) => ProvisionError::Copy { path: from, source },
        })
    };
    // `--directory` collapses an ignored folder into one entry, so a build cache with
    // millions of files is neither walked nor listed.
    let mut copied = 0;
    let mut deeper = Vec::new();
    for (path, is_folder) in ignored_entries(&list_ignored(source, true, &[])?) {
        if patterns.matches(path, is_folder) {
            copy(path)?;
            copied += 1;
        } else if is_folder {
            deeper.extend(patterns.inside(path).into_iter().map(str::to_owned));
        }
    }
    // A pattern such as `.vscode/settings.json` names a file the collapsed listing hid in its
    // ignored folder; Git lists the files under just those paths.
    if !deeper.is_empty() {
        for (path, _) in ignored_entries(&list_ignored(source, false, &deeper)?) {
            if patterns.matches(path, false) {
                copy(path)?;
                copied += 1;
            }
        }
    }
    if copied > 0 {
        tracing::info!(?checkout, copied, "copied .worktreeinclude entries");
    }
    Ok(copied)
}

/// The untracked files Git ignores in `source`, under `paths` when there are any; with
/// `collapsed`, an ignored folder is one entry ending in `/`.
fn list_ignored(source: &Path, collapsed: bool, paths: &[String]) -> Result<Vec<u8>, GitError> {
    let mut args = vec![
        "--literal-pathspecs",
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "-z",
    ];
    if collapsed {
        args.push("--directory");
    }
    if !paths.is_empty() {
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
    }
    git::bytes(&Host::Local, source, &args, None, IGNORED_LIST_LIMIT, false)
}

/// Repository-relative paths of a listing and whether each is a collapsed folder. A name that
/// is not UTF-8 cannot be matched against the patterns and is left out.
fn ignored_entries(listing: &[u8]) -> impl Iterator<Item = (&str, bool)> {
    listing
        .split(|byte| *byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .map(|entry| match entry.strip_suffix('/') {
            Some(path) => (path, true),
            None => (entry, false),
        })
        .filter(|(path, _)| !path.is_empty())
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
    setup_within(command, checkout, stop, SETUP_TIMEOUT)
}

fn setup_within(
    command: &str,
    checkout: &Path,
    stop: &Cancellation,
    timeout: Duration,
) -> Result<(), ProvisionError> {
    let failed = |outcome: String, output: String| ProvisionError::Setup {
        command: command.to_owned(),
        outcome,
        output,
    };
    let process = Host::Local.command("/bin/sh", &["-c", command], Some(checkout));
    let log =
        command::run_with_log_tail(process, stop, timeout, SETUP_LOG_TAIL).map_err(|error| {
            if stop.is_cancelled() {
                ProvisionError::Stopped
            } else {
                failed(error.to_string(), String::new())
            }
        })?;
    let outcome = match log.end {
        End::Exited(Some(0)) => return Ok(()),
        End::Exited(Some(code)) => format!("exited with {code}"),
        End::Exited(None) => "was terminated".to_owned(),
        End::TimedOut => "timed out".to_owned(),
    };
    Err(failed(outcome, log.text()))
}

#[cfg(test)]
mod tests;
