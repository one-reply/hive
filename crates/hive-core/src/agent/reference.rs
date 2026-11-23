//! A reference to lines of code for an agent's prompt, copied from the editor or a diff.
//!
//! Lines of the worktree's file become Claude Code's file mention, `@path#L10-12`: that CLI
//! attaches the lines itself, and other agents read a path and lines they can open. No CLI
//! can attach another version of a file, so its lines are named instead: `path#L10-12 (HEAD)`.
use crate::git::diff::{BodyLine, DiffSource, FileDiff};
use crate::git::review_notes::Line;
use crate::host::Host;
use crate::terminal::paste::{self, PasteError};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Range, RangeInclusive};
use std::path::{Component, Path};

/// How many characters of a commit ID a reference keeps: unique in any real repository.
const SHORT_ID: usize = 12;

/// The version of a file whose lines a reference names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Version {
    /// The file in the worktree, which an agent reads itself.
    Worktree,
    /// The staged version, Git's index.
    Index,
    /// The commit checked out in the worktree.
    Head,
    /// A commit, by ID.
    Commit(String),
    /// The first parent of a commit, by the commit's ID.
    Parent(String),
    /// The common ancestor of a branch and HEAD, by the branch's name.
    MergeBase(String),
}

impl Version {
    /// The version on the original side of a diff of `source`; an untracked file has none.
    pub fn original(source: &DiffSource) -> Option<Self> {
        match source {
            DiffSource::WorkingTree => Some(Self::Index),
            DiffSource::Staged => Some(Self::Head),
            DiffSource::Commit(oid) => Some(Self::Parent(oid.clone())),
            DiffSource::Compare(base) => Some(Self::MergeBase(base.clone())),
            DiffSource::Untracked => None,
        }
    }

    /// The version on the modified side of a diff of `source`.
    pub fn modified(source: &DiffSource) -> Self {
        match source {
            DiffSource::WorkingTree | DiffSource::Untracked => Self::Worktree,
            DiffSource::Staged => Self::Index,
            DiffSource::Commit(oid) => Self::Commit(oid.clone()),
            DiffSource::Compare(_) => Self::Head,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let short = |oid: &'_ str| oid.get(..SHORT_ID).unwrap_or(oid).to_owned();
        match self {
            Self::Worktree => f.write_str("worktree"),
            Self::Index => f.write_str("index"),
            Self::Head => f.write_str("HEAD"),
            Self::Commit(oid) => f.write_str(&short(oid)),
            Self::Parent(oid) => write!(f, "{}^", short(oid)),
            Self::MergeBase(base) => write!(f, "merge base of {base} and HEAD"),
        }
    }
}

/// Lines of one file in a worktree. It travels with the clipboard as JSON metadata, so a
/// terminal of the same worktree can name the file relative to its own directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeReference {
    host: Host,
    /// Absolute; UTF-8 without control characters, so the text and the JSON never fail.
    worktree: String,
    /// Worktree-relative, normal components only.
    path: String,
    start: usize,
    end: usize,
    version: Version,
}

impl CodeReference {
    /// Lines `lines` (one-based, in either order) of `path` in `worktree` on `host`.
    pub fn new(
        host: Host,
        worktree: &Path,
        path: &Path,
        lines: RangeInclusive<usize>,
        version: Version,
    ) -> Result<Self, PasteError> {
        if !worktree.is_absolute() {
            return Err(PasteError::InvalidPath(worktree.to_path_buf()));
        }
        if path.as_os_str().is_empty()
            || !path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(PasteError::InvalidPath(path.to_path_buf()));
        }
        let (first, last) = lines.into_inner();
        Ok(Self {
            host,
            worktree: paste::path_text(worktree)?.to_owned(),
            path: paste::path_text(path)?.to_owned(),
            start: first.min(last).max(1),
            end: first.max(last).max(1),
            version,
        })
    }

    /// The reference with the file's absolute path, which means the same from anywhere.
    pub fn to_text(&self) -> String {
        self.text(&Path::new(&self.worktree).join(&self.path).to_string_lossy())
    }

    /// The reference for a terminal of `worktree` on `host` in `directory`: relative to that
    /// directory when the terminal is in this reference's worktree, absolute otherwise.
    pub fn to_text_in(&self, host: &Host, worktree: &Path, directory: &Path) -> String {
        let root = Path::new(&self.worktree);
        if self.host != *host || root != worktree {
            return self.to_text();
        }
        match paste::relative_path(root, Path::new(&self.path), directory) {
            Ok(relative) => self.text(&relative.to_string_lossy()),
            Err(_) => self.to_text(),
        }
    }

    /// The reference with the worktree-relative path, for showing what was copied.
    pub fn label(&self) -> String {
        self.text(&self.path).trim_end().to_owned()
    }

    fn text(&self, path: &str) -> String {
        let lines = if self.start == self.end {
            format!("L{}", self.start)
        } else {
            format!("L{}-{}", self.start, self.end)
        };
        // Claude Code takes `@` up to the first whitespace unless the mention is quoted. The
        // trailing space ends the mention before whatever the user types next.
        match &self.version {
            Version::Worktree if path.contains(char::is_whitespace) => {
                format!("@\"{path}#{lines}\" ")
            }
            Version::Worktree => format!("@{path}#{lines} "),
            version => format!("{path}#{lines} ({version}) "),
        }
    }
}

/// The one-based lines that `range` of `text` covers. A range ending right after a newline
/// stops at that line, as a selection of whole lines does; an empty one is its line.
pub fn selected_lines(text: &str, range: Range<usize>) -> Option<RangeInclusive<usize>> {
    let selected = text.get(range.clone())?;
    let start = text[..range.start].matches('\n').count() + 1;
    Some(start..=start + selected.trim_end_matches('\n').matches('\n').count())
}

/// The lines of a diff of `source` between two selected endpoints. A selection crossing from
/// one side to the other, as a unified view allows, names the modified lines it covers.
pub fn diff_lines(
    diff: &FileDiff,
    source: &DiffSource,
    start: Line,
    end: Line,
) -> Option<(Version, RangeInclusive<usize>)> {
    match (start, end) {
        (Line::Original(first), Line::Original(last)) => Some((
            Version::original(source)?,
            first.min(last)..=first.max(last),
        )),
        (Line::Modified(first), Line::Modified(last)) => {
            Some((Version::modified(source), first.min(last)..=first.max(last)))
        }
        _ => {
            let lines = diff.body_lines();
            let (first, last) = (position(&lines, start)?, position(&lines, end)?);
            let covered = &lines[first.min(last)..=first.max(last)];
            // The modified endpoint is among them, so there is at least one.
            let modified = span(covered.iter().filter_map(|line| line.modified))?;
            Some((Version::modified(source), modified))
        }
    }
}

/// Where selected `text` of a diff of `source` is: on the modified side when it is there
/// exactly once, else on the original side when it is there exactly once. Text found more
/// than once, or across hunks, has no single place.
pub fn find_in_diff(
    diff: &FileDiff,
    source: &DiffSource,
    text: &str,
) -> Option<(Version, RangeInclusive<usize>)> {
    let needle = text.trim_end_matches('\n');
    if needle.trim().is_empty() {
        return None;
    }
    let lines = diff.body_lines();
    match occurrences(&lines, needle, |line| line.modified).as_slice() {
        [found] => Some((Version::modified(source), found.clone())),
        [] => match occurrences(&lines, needle, |line| line.original).as_slice() {
            [found] => Some((Version::original(source)?, found.clone())),
            _ => None,
        },
        _ => None,
    }
}

fn position(lines: &[BodyLine], endpoint: Line) -> Option<usize> {
    lines.iter().position(|line| match endpoint {
        Line::Original(number) => line.original == Some(number),
        Line::Modified(number) => line.modified == Some(number),
    })
}

fn span(numbers: impl Iterator<Item = usize>) -> Option<RangeInclusive<usize>> {
    numbers.fold(None, |span, number| match span {
        None => Some(number..=number),
        Some(span) => Some(*span.start().min(&number)..=*span.end().max(&number)),
    })
}

/// Every place `needle` occurs in one side's lines, each hunk searched on its own.
fn occurrences(
    lines: &[BodyLine],
    needle: &str,
    number: fn(&BodyLine) -> Option<usize>,
) -> Vec<RangeInclusive<usize>> {
    let mut found = Vec::new();
    for hunk in lines.chunk_by(|a, b| a.hunk == b.hunk) {
        let side: Vec<(usize, &str)> = hunk
            .iter()
            .filter_map(|line| Some((number(line)?, line.text)))
            .collect();
        let mut text = String::new();
        let mut starts = Vec::with_capacity(side.len());
        for (_, line) in &side {
            if !starts.is_empty() {
                text.push('\n');
            }
            starts.push(text.len());
            text.push_str(line);
        }
        let line_at = |offset: usize| side[starts.partition_point(|&start| start <= offset) - 1].0;
        let mut from = 0;
        while let Some(offset) = text[from..].find(needle).map(|offset| from + offset) {
            found.push(line_at(offset)..=line_at(offset + needle.len() - 1));
            // Overlapping matches count too: "aa" is in "aaa" twice.
            from = offset + needle.chars().next().map_or(1, char::len_utf8);
        }
    }
    found
}

#[cfg(test)]
mod tests;
