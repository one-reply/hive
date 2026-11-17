//! UTF-16 LSP positions and file URIs; GPUI uses Unicode scalar columns instead.
use lsp_types::{Position, Uri};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

/// Converts a UTF-8 byte boundary into the protocol's UTF-16 position.
pub fn at(text: &str, offset: usize) -> Option<Position> {
    let prefix = text.get(..offset)?;
    let line = prefix.bytes().filter(|b| *b == b'\n').count();
    let start = prefix.rfind('\n').map_or(0, |offset| offset + 1);
    Some(Position::new(
        line.try_into().ok()?,
        text[start..offset].encode_utf16().count().try_into().ok()?,
    ))
}

/// Rejects lines/columns outside the document and positions in the middle of a surrogate pair.
pub fn offset(text: &str, position: Position) -> Option<usize> {
    let mut start = 0;
    for _ in 0..position.line {
        start += text.get(start..)?.find('\n')? + 1;
    }
    column_offset(text, start, position.character)
}

/// The byte offset of a UTF-16 column on the line that begins at `start`.
fn column_offset(text: &str, start: usize, character: u32) -> Option<usize> {
    let line = text
        .get(start..)?
        .split('\n')
        .next()?
        .trim_end_matches('\r');
    let mut column = 0;
    for (byte, scalar) in line.char_indices() {
        if column == character {
            return Some(start + byte);
        }
        column += scalar.len_utf16() as u32;
    }
    (column == character).then_some(start + line.len())
}

/// The line starts of one text. A completion list or a diagnostics set converts hundreds of
/// positions in the same buffer; walking from the top for each one is quadratic.
pub struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    /// Indexes `text` once; each conversion afterwards costs one line.
    pub fn new(text: &'a str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(offset, _)| offset + 1))
            .collect();
        Self { text, starts }
    }

    /// The indexed text.
    pub fn text(&self) -> &'a str {
        self.text
    }

    /// Same result as [`at`].
    pub fn at(&self, offset: usize) -> Option<Position> {
        let prefix = self.text.get(..offset)?;
        // `starts[0]` is 0, so at least one start is not after `offset`.
        let line = self.starts.partition_point(|start| *start <= offset) - 1;
        Some(Position::new(
            line.try_into().ok()?,
            prefix[self.starts[line]..]
                .encode_utf16()
                .count()
                .try_into()
                .ok()?,
        ))
    }

    /// Same result as [`offset`].
    pub fn offset(&self, position: Position) -> Option<usize> {
        column_offset(self.text, self.start(position)?, position.character)
    }

    /// Same result as [`editor_position`].
    pub fn editor_position(&self, position: Position) -> Option<Position> {
        let start = self.start(position)?;
        let offset = column_offset(self.text, start, position.character)?;
        Some(Position::new(
            position.line,
            self.text[start..offset].chars().count().try_into().ok()?,
        ))
    }

    /// Same result as [`editor_range`].
    pub fn editor_range(&self, range: lsp_types::Range) -> Option<lsp_types::Range> {
        if range.start > range.end {
            return None;
        }
        Some(lsp_types::Range::new(
            self.editor_position(range.start)?,
            self.editor_position(range.end)?,
        ))
    }

    fn start(&self, position: Position) -> Option<usize> {
        self.starts
            .get(usize::try_from(position.line).ok()?)
            .copied()
    }
}

/// Converts a protocol position to the scalar-column positions expected by the editor widget.
pub fn editor_position(text: &str, position: Position) -> Option<Position> {
    let offset = offset(text, position)?;
    let prefix = &text[..offset];
    let start = prefix.rfind('\n').map_or(0, |offset| offset + 1);
    Some(Position::new(
        position.line,
        text[start..offset].chars().count().try_into().ok()?,
    ))
}

/// Converts an ordered protocol range, rejecting reversed or non-scalar boundaries.
pub fn editor_range(text: &str, range: lsp_types::Range) -> Option<lsp_types::Range> {
    if range.start > range.end {
        return None;
    }
    Some(lsp_types::Range::new(
        editor_position(text, range.start)?,
        editor_position(text, range.end)?,
    ))
}

/// An absolute source path encoded as a file URI on the server's host.
pub fn uri(path: &Path) -> Option<Uri> {
    if !path.is_absolute() {
        return None;
    }
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Uri::from_str(&uri).ok()
}

/// Decodes only absolute file URIs without authority, query, fragment or NUL bytes.
pub fn path(uri: &Uri) -> Option<PathBuf> {
    decode(uri.as_str())
}

/// [`path`] for a URI as a server wrote it. rust-analyzer leaves `[`, `(` or `+` raw, which the
/// typed [`Uri`] either rejects or keeps in a spelling that differs from [`uri`].
pub(super) fn decode(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?;
    if !encoded.starts_with('/') || encoded.contains(['?', '#']) {
        return None;
    }
    let encoded = encoded.as_bytes();
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'%' {
            bytes.push(
                u8::from_str_radix(
                    std::str::from_utf8(encoded.get(index + 1..index + 3)?).ok()?,
                    16,
                )
                .ok()?,
            );
            index += 3;
        } else {
            bytes.push(encoded[index]);
            index += 1;
        }
    }
    if bytes.contains(&0) {
        return None;
    }
    Some(String::from_utf8(bytes).ok()?.into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn rejects_reversed_ranges_and_surrogate_midpoints() {
        use lsp_types::{Position, Range};
        assert!(
            super::editor_range("😀x", Range::new(Position::new(0, 3), Position::new(0, 2)))
                .is_none()
        );
        assert!(
            super::editor_range("😀x", Range::new(Position::new(0, 1), Position::new(0, 2)))
                .is_none()
        );
        assert_eq!(
            super::editor_range("😀x", Range::new(Position::new(0, 0), Position::new(0, 2)))
                .unwrap()
                .end
                .character,
            1
        );
    }
    use super::*;
    #[test]
    fn a_line_index_converts_exactly_like_the_single_position_functions() {
        use lsp_types::Range;
        for text in ["", "a😀e\u{301}\r\n日本語\n", "one\ntwo", "\n\n", "\r\n x"] {
            let lines = Lines::new(text);
            for offset in 0..=text.len() + 1 {
                assert_eq!(lines.at(offset), at(text, offset), "{text:?} at {offset}");
            }
            for line in 0..5 {
                for character in 0..8 {
                    let position = Position::new(line, character);
                    assert_eq!(lines.offset(position), offset(text, position));
                    assert_eq!(
                        lines.editor_position(position),
                        editor_position(text, position)
                    );
                    for end in [Position::new(line, character + 1), Position::new(0, 0)] {
                        let range = Range::new(position, end);
                        assert_eq!(lines.editor_range(range), editor_range(text, range));
                    }
                }
            }
        }
    }
    #[test]
    fn server_spellings_of_a_file_uri_decode_to_the_same_path() {
        let source = Path::new("/work/rs (proj)+@[x]/a.rs");
        let ours = uri(source).unwrap();
        assert_eq!(
            ours.as_str(),
            "file:///work/rs%20%28proj%29%2B%40%5Bx%5D/a.rs"
        );
        for spelling in [
            ours.as_str(),
            "file:///work/rs%20(proj)+@[x]/a.rs",
            "file:///work/rs%20(proj)+@%5bx%5d/a.rs",
        ] {
            assert_eq!(decode(spelling).as_deref(), Some(source), "{spelling}");
        }
        assert!(decode("untitled:a").is_none());
    }
    #[test]
    fn emoji_combining_characters_crlf_and_empty_last_lines_round_trip() {
        let text = "a😀e\u{301}\r\n日本語\n";
        for offset in [0, 1, 5, 6, 8, 10, 13, 16, 19, 20] {
            let position = at(text, offset).unwrap();
            assert_eq!(super::offset(text, position), Some(offset));
        }
        assert_eq!(
            editor_position(text, Position::new(0, 3)),
            Some(Position::new(0, 2))
        );
        assert_eq!(super::offset(text, Position::new(0, 2)), None);
        assert_eq!(super::offset(text, Position::new(99, 0)), None);
    }
    #[test]
    fn uris_preserve_paths_without_crossing_into_other_schemes_or_hosts() {
        let source = Path::new("/work/日本語 #?.rs");
        assert_eq!(path(&uri(source).unwrap()).as_deref(), Some(source));
        for value in [
            "https://host/a",
            "file://server/a",
            "file:///a?query",
            "file:///a%00",
        ] {
            assert!(path(&Uri::from_str(value).unwrap()).is_none());
        }
    }
}
