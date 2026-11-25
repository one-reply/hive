//! Editor commands operating on UTF-8 byte ranges in a normalized, LF-delimited document.
use std::{ops::Range, path::Path};

/// One replacement and the selection the editor should restore afterwards.
pub struct Edit {
    /// Bytes in the original document.
    pub range: Range<usize>,
    /// Replacement text, applied as one undo step.
    pub text: String,
    /// Selection in the resulting document, in bytes.
    pub selection: Range<usize>,
}

/// A command acting on every line touched by a selection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Swap the selected lines with the preceding line.
    MoveUp,
    /// Swap the selected lines with the following line.
    MoveDown,
    /// Insert a copy above, selecting the upper copy.
    DuplicateUp,
    /// Insert a copy below, selecting the lower copy.
    DuplicateDown,
    /// Toggle a comment using the syntax at the selected lines.
    Comment,
}

/// Builds an undoable replacement. Moving beyond a file boundary changes nothing.
pub fn edit(command: Command, path: &Path, text: &str, selection: Range<usize>) -> Option<Edit> {
    let selected = text.get(selection.clone())?;
    let start = line_start(text, selection.start);
    let last = selection.end - usize::from(selected.ends_with('\n'));
    let end = line_end(text, last);
    let block = &text[start..end];
    let (range, replacement, new_start) = match command {
        Command::MoveUp => {
            if start == text.len() && text.ends_with('\n') {
                return None;
            }
            let previous = line_start(text, start.checked_sub(1)?);
            let above = &text[previous..start - 1];
            (previous..end, format!("{block}\n{above}"), previous)
        }
        Command::MoveDown => {
            let next = end.checked_add(1)?;
            if next >= text.len() {
                return None;
            }
            let next_end = line_end(text, next);
            let below = &text[next..next_end];
            (
                start..next_end,
                format!("{below}\n{block}"),
                start + below.len() + 1,
            )
        }
        Command::DuplicateUp | Command::DuplicateDown => {
            let shifted = usize::from(command == Command::DuplicateDown) * (block.len() + 1);
            (start..end, format!("{block}\n{block}"), start + shifted)
        }
        Command::Comment => return comment(path, text, start..end, selection),
    };
    let new_len = text.len() - range.len() + replacement.len();
    let map = |offset: usize| (new_start + (offset - start)).min(new_len);
    Some(Edit {
        range,
        text: replacement,
        selection: map(selection.start)..map(selection.end),
    })
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |n| n + 1)
}

fn line_end(text: &str, offset: usize) -> usize {
    text[offset..].find('\n').map_or(text.len(), |n| offset + n)
}

/// Whether comments need the syntax tree of the whole document, including JSX ancestry.
pub fn uses_jsx_comments(path: &Path) -> bool {
    matches!(
        crate::files::editor_language(path).as_str(),
        "tsx" | "javascript" | "js"
    )
}

fn comment(path: &Path, text: &str, lines: Range<usize>, selection: Range<usize>) -> Option<Edit> {
    let language = crate::files::editor_language(path);
    let jsx = uses_jsx_comments(path);
    let marker = match language.as_str() {
        "rs" | "ts" | "typescript" | "tsx" | "js" | "javascript" | "c" | "h" | "cpp" | "java"
        | "go" => "//",
        "py" | "rb" | "sh" | "bash" | "toml" | "yaml" | "yml" | "make" => "#",
        _ => return None,
    };
    let block = text.get(lines.clone())?;
    let body = block.trim();
    let indent = block.len() - block.trim_start().len();
    let trimmed = lines.start + indent..lines.start + indent + body.len();
    if jsx && jsx_children(text, &language, &trimmed) {
        let (value, shift) =
            if let Some(inner) = body.strip_prefix("{/*").and_then(|s| s.strip_suffix("*/}")) {
                let prefix = 3 + isize::from(inner.starts_with(' '));
                let inner = inner.strip_prefix(' ').unwrap_or(inner);
                (inner.strip_suffix(' ').unwrap_or(inner).to_owned(), -prefix)
            } else {
                if body.contains("*/") {
                    return None;
                }
                (format!("{{/* {body} */}}"), 4)
            };
        let map = |p: usize| {
            if p < trimmed.start {
                p
            } else if p > trimmed.end {
                p - trimmed.len() + value.len()
            } else {
                p.saturating_add_signed(shift)
                    .clamp(trimmed.start, trimmed.start + value.len())
            }
        };
        let selection = map(selection.start)..map(selection.end);
        return Some(Edit {
            range: trimmed,
            text: value,
            selection,
        });
    }
    let nonblank = block.lines().filter(|line| !line.trim().is_empty());
    let remove = !body.is_empty()
        && nonblank
            .clone()
            .all(|line| line.trim_start().starts_with(marker));
    let mut result = String::new();
    let mut offsets = [selection.start, selection.end];
    let mut old_start = lines.start;
    for (index, line) in block.split('\n').enumerate() {
        if index != 0 {
            result.push('\n');
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = &line[indent..];
        let (deleted, inserted) = if remove {
            let len = trimmed
                .strip_prefix(marker)
                .map_or(0, |rest| marker.len() + usize::from(rest.starts_with(' ')));
            (len, "")
        } else if trimmed.is_empty() && !body.is_empty() {
            (0, "")
        } else {
            (0, if marker == "//" { "// " } else { "# " })
        };
        let old_end = old_start + line.len();
        let new_start = lines.start + result.len();
        for (i, original) in [selection.start, selection.end].into_iter().enumerate() {
            if (old_start..=old_end).contains(&original) {
                let col = original - old_start;
                offsets[i] = new_start
                    + if col < indent {
                        col
                    } else {
                        indent + inserted.len() + (col - indent).saturating_sub(deleted)
                    };
            }
        }
        result.push_str(&line[..indent]);
        result.push_str(inserted);
        result.push_str(&trimmed[deleted..]);
        old_start = old_end + 1;
    }
    if selection.end > lines.end {
        offsets[1] = lines.start + result.len() + 1;
    }
    Some(Edit {
        range: lines,
        text: result,
        selection: offsets[0]..offsets[1],
    })
}

/// Tests the syntactic container outside the selected source. A JSX child expression
/// may itself contain ordinary TypeScript, so stop at its braces when working inside it.
fn jsx_children(text: &str, language: &str, range: &Range<usize>) -> bool {
    let grammar = if language == "tsx" {
        tree_sitter_typescript::LANGUAGE_TSX
    } else {
        tree_sitter_javascript::LANGUAGE
    };
    let Some(tree) = crate::completion::parse(text, &grammar.into()) else {
        return false;
    };
    let mut node = tree
        .root_node()
        .descendant_for_byte_range(range.start, range.end);
    while let Some(n) = node {
        match n.kind() {
            "jsx_text" => return true,
            "jsx_element" | "jsx_fragment" if n.byte_range() != *range => {
                // An opening/closing tag may span several lines; attributes aren't children.
                return true;
            }
            "jsx_expression"
            | "jsx_opening_element"
            | "jsx_closing_element"
            | "jsx_self_closing_element"
                if n.byte_range() != *range =>
            {
                return false;
            }
            _ => node = n.parent(),
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(
        command: Command,
        path: &str,
        source: &str,
        selection: Range<usize>,
    ) -> (String, Range<usize>) {
        let edit = edit(command, Path::new(path), source, selection).unwrap();
        let mut output = source.to_owned();
        output.replace_range(edit.range, &edit.text);
        (output, edit.selection)
    }

    #[test]
    fn move_and_duplicate_preserve_unicode_and_final_newline() {
        let src = "a\nβ\nc\n";
        assert_eq!(
            apply(Command::MoveUp, "a.rs", src, 2..4),
            ("β\na\nc\n".into(), 0..2)
        );
        assert_eq!(
            apply(Command::MoveDown, "a.rs", src, 2..4),
            ("a\nc\nβ\n".into(), 4..6)
        );
        assert_eq!(
            apply(Command::DuplicateDown, "a.rs", src, 2..5),
            ("a\nβ\nβ\nc\n".into(), 5..8)
        );
        assert_eq!(
            apply(Command::DuplicateUp, "a.rs", "a\nb", 2..3),
            ("a\nb\nb".into(), 2..3)
        );
        assert!(
            edit(
                Command::MoveUp,
                Path::new("a.rs"),
                src,
                src.len()..src.len()
            )
            .is_none()
        );
        assert!(edit(Command::MoveDown, Path::new("a.rs"), src, 5..5).is_none());
        assert!(edit(Command::MoveUp, Path::new("a.rs"), src, 0..0).is_none());
    }

    #[test]
    fn comments_only_touch_the_selected_lines_and_round_trip() {
        let original = "  first\n\n    second\nlast";
        let (commented, selection) = apply(Command::Comment, "a.rs", original, 2..20);
        assert_eq!(commented, "  // first\n\n    // second\nlast");
        assert_eq!(
            apply(Command::Comment, "a.rs", &commented, selection),
            (original.into(), 2..20)
        );
    }

    #[test]
    fn blank_jsx_children_and_adjacent_unicode_stay_inside_the_comment() {
        let source = "const view = <div>\n  \n  λλ\n</div>;";
        let blank = source.find("  \n").unwrap() + 2;
        let (commented, cursor) = apply(Command::Comment, "a.tsx", source, blank..blank);
        assert!(commented.contains("  {/*  */}"));
        assert_eq!(cursor, blank + 4..blank + 4);
        assert_eq!(
            apply(Command::Comment, "a.tsx", &commented, cursor),
            (source.to_owned(), blank..blank)
        );
        let start = source.find("λλ").unwrap();
        let (commented, sel) = apply(Command::Comment, "a.tsx", source, start..start + 4);
        assert_eq!(&commented[sel.clone()], "λλ");
        assert_eq!(
            apply(Command::Comment, "a.tsx", &commented, sel),
            (source.to_owned(), start..start + 4)
        );
    }

    #[test]
    fn nested_jsx_and_syntax_looking_strings_follow_their_lexical_container() {
        let source = "const tag = '<Ghost />';\nconst view = <section>\n  {rows.map(row => (\n    <p>\n      Hello {row.name}\n    </p>\n  ))}\n</section>;";
        for (line, wanted) in [
            ("const tag", "// const tag = '<Ghost />';"),
            ("Hello", "{/* Hello {row.name} */}"),
        ] {
            let offset = source.find(line).unwrap();
            let (commented, selection) =
                apply(Command::Comment, "component.jsx", source, offset..offset);
            assert!(commented.contains(wanted));
            assert_eq!(
                apply(Command::Comment, "component.jsx", &commented, selection).0,
                source
            );
        }
        let begin = source.find("    <p>").unwrap();
        let end = source.find("</p>").unwrap() + 4;
        let (commented, _) = apply(Command::Comment, "component.tsx", source, begin..end);
        assert!(commented.contains("    // <p>\n      // Hello {row.name}\n    // </p>"));
    }

    #[test]
    fn jsx_children_and_typescript_expressions_use_distinct_comments() {
        let source = "const v = <div>\n  Hello\n  <Button />\n  {items.map(x => {\n    return x;\n  })}\n</div>;";
        for (needle, inserted) in [
            ("Hello", "{/* Hello */}"),
            ("<Button />", "{/* <Button /> */}"),
            ("return x;", "// return x;"),
        ] {
            let start = source.find(needle).unwrap();
            let (commented, sel) = apply(Command::Comment, "a.TSX", source, start..start);
            assert!(commented.contains(inserted), "{commented}");
            assert_eq!(apply(Command::Comment, "a.TSX", &commented, sel).0, source);
        }
    }
}
