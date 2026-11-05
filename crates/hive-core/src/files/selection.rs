use std::{ops::Range, path::Path};

/// A local, editable agent draft for a UTF-8 selection. Ranges are byte offsets;
/// invalid boundaries and empty selections do not produce a prompt.
pub fn selection_prompt(
    worktree: &Path,
    path: &Path,
    text: &str,
    range: Range<usize>,
    unsaved: bool,
) -> Option<String> {
    let selected = text.get(range.clone()).filter(|s| !s.is_empty())?;
    let start = text
        .get(..range.start)?
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1;
    let end = start + selected.bytes().filter(|b| *b == b'\n').count()
        - usize::from(selected.ends_with('\n'));
    let longest = selected
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    Some(format!(
        "Worktree: {}\nFile: {}\nLines: {start}–{end}\n{}\n\n{fence}\n{selected}\n{fence}\n\nComment: ",
        worktree.display(),
        path.display(),
        if unsaved {
            "Source: unsaved editor buffer (different from disk)"
        } else {
            "Source: saved file"
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_boundaries_inclusive_lines_and_nested_fences() {
        let text = "привіт\n```rs\nlet a = 1;\n```\n";
        let prompt = selection_prompt(
            Path::new("/worktree"),
            Path::new("a.md"),
            text,
            0..text.len(),
            true,
        )
        .unwrap();
        assert!(prompt.contains("Lines: 1–4"));
        assert!(prompt.contains("unsaved editor buffer"));
        assert!(prompt.contains("\n````\n"));
        assert!(selection_prompt(Path::new("/w"), Path::new("a"), text, 1..2, false).is_none());
        assert!(selection_prompt(Path::new("/w"), Path::new("a"), text, 0..0, false).is_none());
    }
}
