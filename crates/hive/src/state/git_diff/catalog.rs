//! File order and block identity shared by the sidebar and its review.
use crate::state::git::DiffRequest;
use gpui_kit::SharedString;
use hive_core::{
    git::{
        diff::DiffSource,
        history,
        status::{self, Change, FileChange, LineStats},
    },
    projects::Target,
};

pub(crate) const GROUPS: [(&str, DiffSource); 4] = [
    ("Conflicts", DiffSource::WorkingTree),
    ("Staged", DiffSource::Staged),
    ("Changes", DiffSource::WorkingTree),
    ("Untracked", DiffSource::Untracked),
];

pub(crate) fn in_group(file: &FileChange, group: &str) -> bool {
    match group {
        "Conflicts" => file.staged == Some(Change::Conflicted),
        "Staged" => file.staged.is_some_and(|c| c != Change::Conflicted),
        "Changes" => file
            .unstaged
            .is_some_and(|c| !matches!(c, Change::Untracked | Change::Conflicted)),
        _ => file.unstaged == Some(Change::Untracked),
    }
}

pub(super) fn local(source: &DiffSource) -> bool {
    matches!(
        source,
        DiffSource::Staged | DiffSource::WorkingTree | DiffSource::Untracked
    )
}

impl DiffRequest {
    pub(crate) fn block_key(&self) -> SharedString {
        let side = match self.source {
            DiffSource::WorkingTree => "changes",
            DiffSource::Staged => "staged",
            DiffSource::Untracked => "untracked",
            DiffSource::Commit(_) | DiffSource::Compare(_) => "revision",
        };
        format!("{side}/{}", self.path).into()
    }

    pub(super) fn same_review(&self, other: &Self) -> bool {
        self.target == other.target
            && (self.source == other.source || local(&self.source) && local(&other.source))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub request: DiffRequest,
    pub change: Option<Change>,
    pub stats: Option<LineStats>,
}

pub(super) fn entries(target: &Target, source: &DiffSource, files: &[FileChange]) -> Vec<Entry> {
    let entry = |file: &FileChange, source: DiffSource| {
        let working = matches!(source, DiffSource::WorkingTree | DiffSource::Untracked);
        Entry {
            request: DiffRequest::for_file(target, file, source),
            change: if working { file.unstaged } else { file.staged },
            stats: if working {
                file.unstaged_stats
            } else {
                file.staged_stats
            },
        }
    };
    if !local(source) {
        return files
            .iter()
            .map(|file| entry(file, source.clone()))
            .collect();
    }
    let mut ordered = Vec::new();
    for (label, side) in &GROUPS[1..] {
        ordered.extend(
            files
                .iter()
                .filter(|file| file.conflict.is_none() && in_group(file, label))
                .map(|file| entry(file, side.clone())),
        );
    }
    ordered
}

pub(super) fn read(request: &DiffRequest) -> Result<Vec<Entry>, String> {
    let target = &request.target;
    let files = match &request.source {
        DiffSource::Compare(base) => history::compare_files(&target.host, &target.path, base),
        DiffSource::Commit(oid) => history::commit_files(&target.host, &target.path, oid),
        _ => status::read_with_stats(&target.host, &target.path).map(|status| status.files),
    }
    .map_err(|e| e.to_string())?;
    Ok(entries(target, &request.source, &files))
}
