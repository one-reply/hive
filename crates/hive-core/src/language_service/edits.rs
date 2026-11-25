//! Simultaneous UTF-16 edits against one immutable document.
use crate::{files::MAX_FILE_BYTES, language_service::position::Lines};
use lsp_types::TextEdit;

/// Applies edits to their common source revision. Invalid, overlapping and unsavable edits fail.
pub fn apply(text: &str, edits: &[TextEdit]) -> Option<String> {
    compose(text, edits, None).map(|(text, _)| text)
}

/// Applies completion edits and returns the caret after the main insertion. Extra edits are
/// ordered before the main one when both insert at the same empty position.
pub fn completion(
    text: &str,
    main: TextEdit,
    mut additional: Vec<TextEdit>,
) -> Option<(String, usize)> {
    let main_index = additional.len();
    additional.push(main);
    compose(text, &additional, Some(main_index))
}

fn compose(text: &str, edits: &[TextEdit], primary: Option<usize>) -> Option<(String, usize)> {
    let lines = Lines::new(text);
    let mut ordered = Vec::with_capacity(edits.len());
    for (index, edit) in edits.iter().enumerate() {
        let start = lines.offset(edit.range.start)?;
        let end = lines.offset(edit.range.end)?;
        if start > end || edit.new_text.contains('\0') {
            return None;
        }
        ordered.push((start, end, index, &edit.new_text));
    }
    ordered.sort_by_key(|&(start, end, index, _)| (start, end, index));
    let mut out = String::with_capacity(text.len().min(MAX_FILE_BYTES));
    let (mut from, mut caret) = (0, 0);
    for (start, end, index, replacement) in ordered {
        let unchanged = text.get(from..start)?;
        if out.len() + unchanged.len() + replacement.len() > MAX_FILE_BYTES {
            return None;
        }
        out.push_str(unchanged);
        out.push_str(replacement);
        if primary == Some(index) {
            caret = out.len();
        }
        from = end;
    }
    if out.len() + text.len() - from > MAX_FILE_BYTES {
        return None;
    }
    out.push_str(&text[from..]);
    Some((out, caret))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};
    fn edit(start: u32, end: u32, text: &str) -> TextEdit {
        TextEdit::new(
            Range::new(Position::new(0, start), Position::new(0, end)),
            text.into(),
        )
    }
    #[test]
    fn completion_moves_the_caret_past_imports_and_respects_unicode() {
        let (text, cursor) = completion(
            "😀Ha!",
            edit(2, 4, "HashMap"),
            vec![edit(0, 0, "use m;\n"), edit(5, 5, "// tail")],
        )
        .unwrap();
        assert_eq!(&text[..cursor], "use m;\n😀HashMap");
        assert_eq!(&text[cursor..], "!// tail");
        assert_eq!(
            completion("", edit(0, 0, "T"), vec![edit(0, 0, "use T;\n")])
                .unwrap()
                .0,
            "use T;\nT"
        );
    }
    #[test]
    fn refuses_overlap_surrogate_midpoints_and_invalid_replacements() {
        for edits in [
            vec![edit(0, 3, ""), edit(2, 4, "x")],
            vec![edit(1, 2, "")],
            vec![edit(2, 0, "")],
            vec![edit(5, 5, "")],
            vec![edit(4, 4, "\0")],
        ] {
            assert!(apply("😀ab", &edits).is_none());
        }
    }
}
