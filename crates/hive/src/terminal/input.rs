//! The terminal's end of the system's text input, registered by [`super::element`] on every
//! paint of a focused terminal. Printable keys are not written to the PTY as they are
//! pressed ([`super::keys`] leaves them alone): macOS turns them into text here, so dead
//! keys, input methods, the emoji panel and dictation all arrive the same way.

use std::ops::Range;

use gpui_kit::*;

use super::view::TerminalView;

/// The composed text, shaped and placed for one frame.
pub(super) struct Composition {
    line: ShapedLine,
    origin: Point<Pixels>,
    background: Hsla,
}

impl Composition {
    /// Shapes `text` to start in the `cursor` cell. A composition too long for the rest of
    /// the row slides left, so its end — where the input method is working — stays in `grid`.
    pub(super) fn shape(
        text: &str,
        cursor: Bounds<Pixels>,
        grid: Bounds<Pixels>,
        font: Font,
        font_size: Pixels,
        colors: (Hsla, Hsla),
        window: &Window,
    ) -> Self {
        let (foreground, background) = colors;
        let run = TextRun {
            len: text.len(),
            font,
            color: foreground,
            background_color: None,
            // The mark every text field puts under a composition.
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(foreground),
                wavy: false,
            }),
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(text.to_owned().into(), font_size, &[run], None);
        let x = composition_x(cursor.left(), line.width, grid.left(), grid.right());
        Self {
            line,
            origin: point(x, cursor.top()),
            background,
        }
    }

    /// Covers the cells under the composition and draws it over them.
    pub(super) fn paint(&self, line_height: Pixels, window: &mut Window, cx: &mut App) {
        let bounds = Bounds {
            origin: self.origin,
            size: size(self.line.width, line_height),
        };
        window.paint_layer(bounds, |window| {
            window.paint_quad(fill(bounds, self.background))
        });
        // A glyph the atlas rejects leaves a gap in the composition, nothing worse.
        let _ = self
            .line
            .paint(self.origin, line_height, TextAlign::Left, None, window, cx);
    }

    /// Where the UTF-16 offset `offset` of the composed text sits, from its start.
    fn x_for_utf16_offset(&self, offset: usize) -> Pixels {
        self.line
            .x_for_index(utf16_to_byte_offset(&self.line.text, offset))
    }
}

/// The left edge of a composition `width` wide that starts at `cursor`, kept inside
/// `left..right`: its end stays visible, and its start does when the whole of it cannot.
fn composition_x(cursor: Pixels, width: Pixels, left: Pixels, right: Pixels) -> Pixels {
    cursor.min(right - width).max(left)
}

/// The byte offset in `text` of its UTF-16 offset `offset`; past the end is the end, and an
/// offset inside a surrogate pair is the start of that character.
fn utf16_to_byte_offset(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, c) in text.char_indices() {
        if units + c.len_utf16() > offset {
            return index;
        }
        units += c.len_utf16();
    }
    text.len()
}

pub(super) struct TerminalInputHandler {
    view: Entity<TerminalView>,
    /// The cell under the terminal's cursor in window coordinates; `None` when the cursor
    /// is scrolled out of view.
    cursor: Option<Bounds<Pixels>>,
    composition: Option<Composition>,
}

impl TerminalInputHandler {
    pub(super) fn new(
        view: Entity<TerminalView>,
        cursor: Option<Bounds<Pixels>>,
        composition: Option<Composition>,
    ) -> Self {
        Self {
            view,
            cursor,
            composition,
        }
    }
}

impl InputHandler for TerminalInputHandler {
    /// A terminal has no document to select in; an empty selection at its start is what
    /// tells the system there is an insertion point, without which it places no candidate
    /// window. This holds on the alternate screen too, where agent TUIs take their prompts.
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut App,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.view.read(cx).marked_text_range()
    }

    /// The grid is the program's, not text the input method may read back or rewrite.
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| view.commit_text(text, cx));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut App,
    ) {
        self.view
            .update(cx, |view, cx| view.set_marked_text(new_text, cx));
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| view.clear_marked_text(cx));
    }

    /// Where the candidate window goes: the cursor cell, moved along the composition.
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let cursor = self.cursor?;
        let Some(composition) = &self.composition else {
            return Some(cursor);
        };
        let x = composition.origin.x + composition.x_for_utf16_offset(range_utf16.start);
        Some(Bounds {
            origin: point(x, cursor.top()),
            size: cursor.size,
        })
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        None
    }

    /// A held key repeats, as in every terminal, instead of opening the accent popup.
    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{composition_x, utf16_to_byte_offset};
    use gpui_kit::px;

    #[test]
    fn a_composition_starts_at_the_cursor_while_it_fits() {
        assert_eq!(composition_x(px(40.), px(30.), px(4.), px(100.)), px(40.));
    }

    #[test]
    fn a_composition_past_the_right_edge_keeps_its_end_in_view() {
        assert_eq!(composition_x(px(90.), px(30.), px(4.), px(100.)), px(70.));
    }

    #[test]
    fn a_composition_wider_than_the_grid_starts_at_its_left_edge() {
        assert_eq!(composition_x(px(40.), px(300.), px(4.), px(100.)), px(4.));
    }

    #[test]
    fn utf16_offsets_map_to_character_boundaries() {
        // か is one UTF-16 unit and three bytes, 😀 two units and four bytes.
        let text = "aか😀b";
        assert_eq!(utf16_to_byte_offset(text, 0), 0);
        assert_eq!(utf16_to_byte_offset(text, 1), 1);
        assert_eq!(utf16_to_byte_offset(text, 2), 4);
        assert_eq!(
            utf16_to_byte_offset(text, 3),
            4,
            "inside the surrogate pair is the start of the emoji"
        );
        assert_eq!(utf16_to_byte_offset(text, 4), 8);
        assert_eq!(utf16_to_byte_offset(text, 5), 9);
        assert_eq!(utf16_to_byte_offset(text, 99), 9, "past the end is the end");
        assert_eq!(utf16_to_byte_offset("", 3), 0);
    }
}
