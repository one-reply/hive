//! Project-wide text search over a worktree's file index. Blocking; run it off the UI thread.
//!
//! One compiled `regex::Regex` serves literal and pattern queries alike: the crate extracts
//! literals and uses SIMD substring prefilters, which is what makes ripgrep fast. Files follow
//! the editor's document rule, so every result opens.
use super::{FileError, MAX_FILE_BYTES, document_text, io_error, read_bytes, resolve_within};
use crate::command::Cancellation;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    num::NonZero,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

mod lines;

/// Matched lines kept per search; a broader query must be narrowed to see more.
pub const MAX_MATCHED_LINES: usize = 5_000;
/// Past this the scan is bound by disk and memory bandwidth; leave cores for running agents.
const MAX_WORKERS: usize = 8;

/// What to look for and how to compare it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    /// Literal text, or a pattern when `regex` is set.
    pub text: String,
    /// Compare letter case exactly.
    pub case_sensitive: bool,
    /// Match only at word boundaries.
    pub whole_word: bool,
    /// Treat `text` as a regular expression.
    pub regex: bool,
}

/// Matching lines of one file, in line order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMatches {
    /// Worktree-relative path.
    #[serde(with = "crate::path_encoding")]
    pub path: PathBuf,
    /// Lines with at least one match.
    pub lines: Vec<LineMatch>,
}

/// One line with every match it contains.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineMatch {
    /// Zero-based line number.
    pub line: u32,
    /// Zero-based position of the first match in chars, as the editor's `Position` counts.
    pub column: u32,
    /// Display text of the line, trimmed and windowed around the first match.
    pub preview: String,
    /// Sorted, non-overlapping byte ranges of the matches inside `preview`.
    pub ranges: Vec<Range<usize>>,
}

/// How a completed search ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Results stopped at `MAX_MATCHED_LINES`.
    pub truncated: bool,
}

/// Why a search produced no complete result.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The regular expression does not compile.
    #[error("Invalid regular expression: {0}")]
    InvalidPattern(String),
    /// The owner cancelled the search.
    #[error("Search cancelled")]
    Cancelled,
    /// The worktree itself cannot be searched.
    #[error(transparent)]
    Files(#[from] FileError),
}

/// Search `paths` under `root`, calling `emit` for each matching file in `paths` order.
/// Unreadable, binary, non-UTF-8 and oversized files are skipped like the editor rejects them.
pub fn search(
    root: &Path,
    paths: &[PathBuf],
    query: &Query,
    cancellation: &Cancellation,
    mut emit: impl FnMut(FileMatches),
) -> Result<Summary, Error> {
    if query.text.is_empty() || paths.is_empty() {
        return Ok(Summary::default());
    }
    let regex = compile(query)?;
    let root = fs::canonicalize(root).map_err(|e| io_error("resolve worktree", root, e))?;
    let scan = Scan {
        root: &root,
        paths,
        cancellation,
        next: AtomicUsize::new(0),
        found: AtomicUsize::new(0),
    };
    let summary = thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let workers = thread::available_parallelism()
            .map_or(1, NonZero::get)
            .min(MAX_WORKERS)
            .min(paths.len());
        let mut started = 0;
        for _ in 0..workers {
            let (scan, sender, regex) = (&scan, sender.clone(), regex.clone());
            match thread::Builder::new()
                .name("hive-content-search".into())
                .spawn_scoped(scope, move || scan.work(&regex, &sender))
            {
                Ok(_) => started += 1,
                Err(error) => tracing::warn!(%error, "could not start a content search worker"),
            }
        }
        drop(sender);
        if started == 0 {
            return Err(Error::Files(io_error(
                "start search workers",
                &root,
                std::io::Error::other("no worker thread could be started"),
            )));
        }
        Ok(collect(receiver, &mut emit))
    })?;
    if cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let unclaimed = scan.next.load(Ordering::Acquire) < paths.len();
    Ok(Summary {
        truncated: summary.truncated || unclaimed,
    })
}

/// Shared work queue of one search.
struct Scan<'a> {
    root: &'a Path,
    paths: &'a [PathBuf],
    cancellation: &'a Cancellation,
    next: AtomicUsize,
    found: AtomicUsize,
}

impl Scan<'_> {
    /// Claim files until the queue, the cap or cancellation ends the search. Every claimed
    /// index is reported, matched or not, so the collector's ordered prefix never stalls.
    fn work(&self, regex: &Regex, sender: &mpsc::Sender<(usize, Option<FileMatches>)>) {
        loop {
            if self.cancellation.is_cancelled()
                || self.found.load(Ordering::Acquire) >= MAX_MATCHED_LINES
            {
                return;
            }
            let ix = self.next.fetch_add(1, Ordering::AcqRel);
            let Some(path) = self.paths.get(ix) else {
                return;
            };
            let matches = search_file(self.root, path, regex);
            if let Some(file) = &matches {
                self.found.fetch_add(file.lines.len(), Ordering::AcqRel);
            }
            if sender.send((ix, matches)).is_err() {
                return;
            }
        }
    }
}

/// Re-order worker reports by file index and emit at most `MAX_MATCHED_LINES` lines.
fn collect(
    receiver: mpsc::Receiver<(usize, Option<FileMatches>)>,
    emit: &mut impl FnMut(FileMatches),
) -> Summary {
    let mut pending = BTreeMap::new();
    let (mut next, mut emitted, mut truncated) = (0, 0, false);
    for (ix, matches) in receiver {
        pending.insert(ix, matches);
        while let Some(matches) = pending.remove(&next) {
            next += 1;
            let Some(mut file) = matches else {
                continue;
            };
            let room = MAX_MATCHED_LINES - emitted;
            if file.lines.len() > room {
                file.lines.truncate(room);
                truncated = true;
            }
            if file.lines.is_empty() {
                continue;
            }
            emitted += file.lines.len();
            emit(file);
        }
    }
    Summary { truncated }
}

fn search_file(root: &Path, relative: &Path, regex: &Regex) -> Option<FileMatches> {
    let lines = match file_lines(root, relative, regex) {
        Ok(lines) => lines,
        // Binary, oversized, special and symlinked files are filtered out by design.
        Err(FileError::Unsupported { .. }) => return None,
        Err(error) => {
            tracing::debug!(%error, "content search skipped a file");
            return None;
        }
    };
    (!lines.is_empty()).then(|| FileMatches {
        path: relative.to_path_buf(),
        lines,
    })
}

fn file_lines(root: &Path, relative: &Path, regex: &Regex) -> Result<Vec<LineMatch>, FileError> {
    // ponytail: one lstat per path component per file; cache verified directories if a
    // profile of a deep 100k-file tree shows it.
    let path = resolve_within(root, relative)?;
    let bytes = read_bytes(&path, MAX_FILE_BYTES, "file is too large to search")?;
    let text = document_text(&path, &bytes)?;
    // Columns must match the editor, which drops the BOM.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // Read one extra match so the collector can report a truncated single file.
    Ok(lines::matches(text, regex, MAX_MATCHED_LINES + 1))
}

/// Compile a query; literal text is escaped, whole words get Unicode word boundaries.
fn compile(query: &Query) -> Result<Regex, Error> {
    let pattern = if query.regex {
        query.text.clone()
    } else {
        regex::escape(&query.text)
    };
    let pattern = if query.whole_word {
        format!(r"\b(?:{pattern})\b")
    } else {
        pattern
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!query.case_sensitive)
        .multi_line(true)
        .crlf(true)
        .build()
        .map_err(|error| Error::InvalidPattern(pattern_error(&error.to_string())))
}

/// `regex` reports a caret diagram over several lines; the last line names the problem.
fn pattern_error(message: &str) -> String {
    let last = message.lines().rfind(|line| !line.trim().is_empty());
    let last = last.unwrap_or(message).trim();
    last.strip_prefix("error: ").unwrap_or(last).to_owned()
}

#[cfg(test)]
mod tests;
