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
    let line = text
        .get(start..)?
        .split('\n')
        .next()?
        .trim_end_matches('\r');
    let mut column = 0;
    for (byte, character) in line.char_indices() {
        if column == position.character {
            return Some(start + byte);
        }
        column += character.len_utf16() as u32;
    }
    (column == position.character).then_some(start + line.len())
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
    let encoded = uri.as_str().strip_prefix("file://")?;
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
