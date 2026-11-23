//! Markdown formatting of the task text: what a toolbar button does to the selection.
//! Offsets are UTF-8 byte offsets, as the editor reports its selection.
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    Bold,
    Italic,
    Code,
    Heading,
    Bullets,
    Numbers,
}

/// One replacement in the text and the selection to leave afterwards.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Edit {
    pub(super) range: Range<usize>,
    pub(super) text: String,
    pub(super) selection: Range<usize>,
}

const HEADING: &str = "## ";
const BULLET: &str = "- ";

/// Toggles `format` on the selection: applied where it is missing, removed where present.
pub(super) fn apply(text: &str, selection: Range<usize>, format: Format) -> Edit {
    let selection = clamp(text, selection);
    match format {
        Format::Bold => inline(text, selection, '*', 2, |run| run >= 2),
        // A run of two stars is bold alone; one or three carry the emphasis.
        Format::Italic => inline(text, selection, '*', 1, |run| run % 2 == 1),
        Format::Code => inline(text, selection, '`', 1, |run| run >= 1),
        Format::Heading | Format::Bullets | Format::Numbers => lines(text, selection, format),
    }
}

/// A selection the editor reported for an older text must not slice out of bounds.
fn clamp(text: &str, selection: Range<usize>) -> Range<usize> {
    let floor = |mut offset: usize| {
        offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    };
    let start = floor(selection.start);
    start..floor(selection.end).max(start)
}

/// Wraps the selection in `width` copies of `mark`, or removes them when `is_on` finds them
/// in the run of marks around it. Marks the user selected along with the text count as
/// surrounding it.
fn inline(
    text: &str,
    selection: Range<usize>,
    mark: char,
    width: usize,
    is_on: impl Fn(usize) -> bool,
) -> Edit {
    let selected = &text[selection.clone()];
    let leading = selected.chars().take_while(|&c| c == mark).count();
    let trailing = selected.chars().rev().take_while(|&c| c == mark).count();
    // `mark` is ASCII, so a count of marks is a count of bytes.
    let inside = leading.min(trailing).min(selected.len() / 2);
    let (start, end) = (selection.start + inside, selection.end - inside);
    let before = text[..start]
        .chars()
        .rev()
        .take_while(|&c| c == mark)
        .count();
    let after = text[end..].chars().take_while(|&c| c == mark).count();
    if is_on(before.min(after)) {
        return Edit {
            range: start - width..end + width,
            text: text[start..end].to_owned(),
            selection: start - width..end - width,
        };
    }
    let marks = mark.to_string().repeat(width);
    Edit {
        range: start..end,
        text: format!("{marks}{}{marks}", &text[start..end]),
        selection: start + width..end + width,
    }
}

/// Bytes of the list or heading marker `line` starts with, after its indentation.
fn marker(line: &str) -> Option<(Format, usize)> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && rest[hashes..].starts_with(' ') {
        return Some((Format::Heading, indent + hashes + 1));
    }
    if ["- ", "* ", "+ "]
        .iter()
        .any(|bullet| rest.starts_with(bullet))
    {
        return Some((Format::Bullets, indent + BULLET.len()));
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && rest[digits..].starts_with(". ") {
        return Some((Format::Numbers, indent + digits + 2));
    }
    None
}

/// Prefixes every line the selection touches, replacing another marker a line already has;
/// when they all carry `format` already, the marker comes off instead.
fn lines(text: &str, selection: Range<usize>, format: Format) -> Edit {
    let start = text[..selection.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[selection.end..]
        .find('\n')
        .map_or(text.len(), |at| selection.end + at);
    let block: Vec<&str> = text[start..end].split('\n').collect();
    // Blank lines between paragraphs stay blank; a block of nothing but blank lines is
    // where the user is about to type.
    let all_blank = block.iter().all(|line| line.trim().is_empty());
    let takes = |line: &str| all_blank || !line.trim().is_empty();
    let is_on = !all_blank
        && block
            .iter()
            .filter(|line| takes(line))
            .all(|line| marker(line).is_some_and(|(kind, _)| kind == format));
    let mut number = 0;
    let edited: Vec<String> = block
        .iter()
        .map(|line| {
            if !takes(line) {
                return (*line).to_owned();
            }
            let indent = line.len() - line.trim_start().len();
            let body = &line[marker(line).map_or(indent, |(_, len)| len)..];
            if is_on {
                return format!("{}{body}", &line[..indent]);
            }
            let prefix = match format {
                Format::Heading => HEADING.to_owned(),
                Format::Numbers => {
                    number += 1;
                    format!("{number}. ")
                }
                _ => BULLET.to_owned(),
            };
            format!("{}{prefix}{body}", &line[..indent])
        })
        .collect();
    let replacement = edited.join("\n");
    let selection = if selection.is_empty() {
        // The caret stays on the same character of its line.
        let shift = edited[0].len() as isize - block[0].len() as isize;
        let caret = (selection.start as isize + shift).max(start as isize) as usize;
        caret..caret
    } else {
        start..start + replacement.len()
    };
    Edit {
        range: start..end,
        text: replacement,
        selection,
    }
}

#[cfg(test)]
mod tests {
    use super::{Edit, Format, apply};
    use std::ops::Range;

    /// The text after the edit, with the new selection between `[` and `]`.
    fn after(text: &str, selection: Range<usize>, format: Format) -> String {
        let Edit {
            range,
            text: replacement,
            selection,
        } = apply(text, selection, format);
        let mut result = format!(
            "{}{replacement}{}",
            &text[..range.start],
            &text[range.end..]
        );
        result.insert(selection.end, ']');
        result.insert(selection.start, '[');
        result
    }

    #[test]
    fn bold_wraps_the_selection_and_keeps_it_selected() {
        assert_eq!(after("fix login", 4..9, Format::Bold), "fix **[login]**");
    }

    #[test]
    fn an_empty_selection_gets_the_marks_with_the_caret_between_them() {
        assert_eq!(after("fix ", 4..4, Format::Bold), "fix **[]**");
        assert_eq!(after("", 0..0, Format::Code), "`[]`");
    }

    #[test]
    fn bold_is_removed_whether_or_not_the_marks_were_selected() {
        assert_eq!(after("fix **login**", 6..11, Format::Bold), "fix [login]");
        assert_eq!(after("fix **login**", 4..13, Format::Bold), "fix [login]");
    }

    #[test]
    fn italic_is_added_to_bold_text_and_removed_without_touching_the_bold() {
        assert_eq!(after("**login**", 2..7, Format::Italic), "***[login]***");
        assert_eq!(after("***login***", 3..8, Format::Italic), "**[login]**");
        assert_eq!(after("*login*", 1..6, Format::Italic), "[login]");
    }

    #[test]
    fn bold_comes_off_bold_italic_text_leaving_the_italic() {
        assert_eq!(after("***login***", 3..8, Format::Bold), "*[login]*");
    }

    #[test]
    fn code_toggles_backticks() {
        assert_eq!(
            after("run cargo test", 4..14, Format::Code),
            "run `[cargo test]`"
        );
        assert_eq!(
            after("run `cargo test`", 5..15, Format::Code),
            "run [cargo test]"
        );
    }

    #[test]
    fn multibyte_text_is_wrapped_on_character_boundaries() {
        let text = "виправ логін";
        let start = text.find("логін").unwrap();
        assert_eq!(
            after(text, start..text.len(), Format::Bold),
            "виправ **[логін]**"
        );
    }

    #[test]
    fn a_selection_past_the_text_or_inside_a_character_is_clamped() {
        assert_eq!(after("ок", 1..99, Format::Bold), "**[ок]**");
    }

    #[test]
    fn a_selection_of_marks_alone_is_never_sliced_out_of_bounds() {
        assert_eq!(after("**", 0..2, Format::Italic), "[]");
        assert_eq!(after("*", 0..1, Format::Bold), "**[*]**");
    }

    #[test]
    fn heading_toggles_on_the_line_of_the_caret_and_the_caret_keeps_its_place() {
        assert_eq!(
            after("intro\nGoal here", 10..10, Format::Heading),
            "intro\n## Goal[] here"
        );
        assert_eq!(
            after("intro\n## Goal here", 13..13, Format::Heading),
            "intro\nGoal[] here"
        );
    }

    #[test]
    fn bullets_prefix_every_selected_line_and_skip_blank_ones() {
        assert_eq!(
            after("one\n\ntwo\nrest", 1..7, Format::Bullets),
            "[- one\n\n- two]\nrest"
        );
        assert_eq!(
            after("- one\n\n- two", 0..12, Format::Bullets),
            "[one\n\ntwo]"
        );
    }

    #[test]
    fn numbers_count_the_selected_lines_and_replace_bullets() {
        assert_eq!(
            after("- one\n- two", 0..11, Format::Numbers),
            "[1. one\n2. two]"
        );
        assert_eq!(
            after("1. one\n2. two", 0..13, Format::Numbers),
            "[one\ntwo]"
        );
    }

    #[test]
    fn a_mixed_list_becomes_the_chosen_kind_instead_of_losing_its_markers() {
        assert_eq!(after("- one\ntwo", 0..9, Format::Bullets), "[- one\n- two]");
    }

    #[test]
    fn an_empty_line_gets_the_marker_so_typing_continues_after_it() {
        assert_eq!(after("intro\n", 6..6, Format::Bullets), "intro\n- []");
        assert_eq!(after("", 0..0, Format::Numbers), "1. []");
    }

    #[test]
    fn indentation_stays_in_front_of_the_marker() {
        assert_eq!(after("  nested", 2..2, Format::Bullets), "  - []nested");
        assert_eq!(after("  - nested", 4..4, Format::Bullets), "  []nested");
    }
}
