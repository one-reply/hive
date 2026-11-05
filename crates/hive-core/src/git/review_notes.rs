//! A portable review prompt, with source-side coordinates kept independent of the UI.
use super::diff::DiffSource;

/// One endpoint of a comment in the original or modified file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// One-based line in the original file.
    Original(usize),
    /// One-based line in the modified file.
    Modified(usize),
}
impl std::fmt::Display for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Original(n) => write!(f, "original:{n}"),
            Self::Modified(n) => write!(f, "modified:{n}"),
        }
    }
}

/// A comment captured against a specific diff source and inclusive range.
#[derive(Clone, Debug)]
pub struct Note {
    /// Repository-relative path (quoted when formatted).
    pub path: String,
    /// The reviewed revision or worktree area.
    pub source: DiffSource,
    /// First endpoint in patch order.
    pub start: Line,
    /// Last endpoint in patch order.
    pub end: Line,
    /// User-authored feedback.
    pub text: String,
}

/// Build one prompt without losing filenames, deleted-side coordinates or revision identity.
pub fn prompt(notes: &[Note]) -> String {
    let mut prompt = String::from(
        "Please address these review comments. Verify the current file contents before editing: line numbers refer to the reviewed diff and may have moved.\n",
    );
    for (index, note) in notes.iter().enumerate() {
        let source = match &note.source {
            DiffSource::WorkingTree => "unstaged changes".into(),
            DiffSource::Staged => "staged changes".into(),
            DiffSource::Untracked => "untracked file".into(),
            DiffSource::Commit(oid) => format!("commit {oid}"),
            DiffSource::Compare(base) => format!("comparison with {base}"),
        };
        prompt.push_str(&format!(
            "\n{}. {:?} [{}; {}–{}]\n{}\n",
            index + 1,
            note.path,
            source,
            note.start,
            note.end,
            note.text.trim()
        ));
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_multiple_files_sources_and_both_sides() {
        let notes = [
            Note {
                path: "a\nb.rs".into(),
                source: DiffSource::Staged,
                start: Line::Original(4),
                end: Line::Modified(8),
                text: "Handle empty input".into(),
            },
            Note {
                path: "test.rs".into(),
                source: DiffSource::Commit("abc123".into()),
                start: Line::Modified(2),
                end: Line::Modified(3),
                text: "Add a regression case".into(),
            },
        ];
        let value = prompt(&notes);
        assert!(value.contains("\"a\\nb.rs\" [staged changes; original:4–modified:8]"));
        assert!(value.contains("2. \"test.rs\" [commit abc123"));
        assert!(value.contains("Add a regression case"));
    }
}
