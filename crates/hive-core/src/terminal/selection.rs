use super::{Column, Line, Listener, Point, SelectionType, Side, Terminal};
use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::selection::Selection;
use alacritty_terminal::term::{Term, viewport_to_point};

impl Terminal {
    /// Start a selection at a visible cell, accounting for scrollback.
    pub fn start_selection(&self, point: Point<usize>, side: Side, kind: SelectionType) {
        let mut term = self.term.lock();
        let point = grid_point(&term, point);
        term.selection = Some(Selection::new(kind, point, side));
    }

    /// Move the selection endpoint to a visible cell, clamped to the current grid.
    pub fn update_selection(&self, point: Point<usize>, side: Side) {
        let mut term = self.term.lock();
        let point = grid_point(&term, point);
        if let Some(selection) = &mut term.selection {
            selection.update(point, side);
        }
    }

    /// Selected text with terminal wrapping, wide characters and combining marks preserved.
    pub fn selected_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// The last `count` lines, scrollback included, as plain text without trailing blank
    /// lines. Soft-wrapped lines stay joined, as in a selection.
    pub fn last_lines(&self, count: usize) -> String {
        if count == 0 {
            return String::new();
        }
        let term = self.term.lock();
        let bottom = term.bottommost_line();
        let count = i32::try_from(count).unwrap_or(i32::MAX);
        let top = Line(bottom.0.saturating_sub(count - 1)).max(term.topmost_line());
        let text = term.bounds_to_string(
            Point::new(top, Column(0)),
            Point::new(bottom, term.last_column()),
        );
        text.trim_end().to_owned()
    }

    /// Clear the selection, returning whether one existed.
    pub fn clear_selection(&self) -> bool {
        self.term.lock().selection.take().is_some()
    }
}

fn grid_point(term: &Term<Listener>, point: Point<usize>) -> Point {
    let point = Point::new(
        point.line.min(term.screen_lines() - 1),
        Column(point.column.0.min(term.columns() - 1)),
    );
    viewport_to_point(term.grid().display_offset(), point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TerminalSize;
    use alacritty_terminal::vte::ansi::Processor;

    fn terminal(text: &str, cols: u16, rows: u16) -> Terminal {
        // cat leaves the grid untouched until input arrives, unlike an interactive shell.
        let terminal =
            Terminal::spawn(None, Some("/bin/cat"), TerminalSize::new(cols, rows)).unwrap();
        let mut parser: Processor = Processor::new();
        parser.advance(&mut *terminal.term.lock(), text.as_bytes());
        terminal
    }

    fn point(row: usize, col: usize) -> Point<usize> {
        Point::new(row, Column(col))
    }

    #[test]
    fn drag_selects_in_both_directions_and_a_click_clears_it() {
        let terminal = terminal("alpha beta\r\nsecond line", 20, 3);
        terminal.start_selection(point(0, 0), Side::Left, SelectionType::Simple);
        assert_eq!(terminal.selected_text(), None);
        terminal.update_selection(point(0, 4), Side::Right);
        assert_eq!(terminal.selected_text().as_deref(), Some("alpha"));
        terminal.start_selection(point(1, 5), Side::Right, SelectionType::Simple);
        terminal.update_selection(point(0, 6), Side::Left);
        assert_eq!(terminal.selected_text().as_deref(), Some("beta\nsecond"));
        terminal.start_selection(point(0, 1), Side::Left, SelectionType::Simple);
        assert_eq!(terminal.selected_text(), None);
        assert!(terminal.clear_selection());
        assert!(!terminal.clear_selection());
    }

    #[test]
    fn selection_preserves_soft_wraps_and_unicode() {
        let terminal = terminal("abcd界e\u{301}fg", 6, 3);
        terminal.start_selection(point(0, 0), Side::Left, SelectionType::Simple);
        terminal.update_selection(point(1, 2), Side::Right);
        assert_eq!(
            terminal.selected_text().as_deref(),
            Some("abcd界e\u{301}fg")
        );
        terminal.start_selection(point(0, 5), Side::Left, SelectionType::Simple);
        terminal.update_selection(point(0, 5), Side::Right);
        assert_eq!(terminal.selected_text().as_deref(), Some("界"));
    }

    #[test]
    fn selection_uses_scrollback_and_clamps_stale_viewport_coordinates() {
        let terminal = terminal("first\r\nsecond\r\nthird\r\nfourth", 10, 2);
        terminal.scroll(2);
        terminal.start_selection(point(0, 0), Side::Left, SelectionType::Simple);
        terminal.update_selection(point(usize::MAX, usize::MAX), Side::Right);
        assert_eq!(terminal.selected_text().as_deref(), Some("first\nsecond"));
    }

    #[test]
    fn last_lines_reach_into_scrollback_and_drop_trailing_blank_rows() {
        let scrolled = terminal("first\r\nsecond\r\nthird\r\nfourth", 10, 3);
        assert_eq!(scrolled.last_lines(2), "third\nfourth");
        assert_eq!(scrolled.last_lines(3), "second\nthird\nfourth");
        assert_eq!(scrolled.last_lines(100), "first\nsecond\nthird\nfourth");
        assert_eq!(scrolled.last_lines(0), "");
        let short = terminal("only", 10, 4);
        assert_eq!(short.last_lines(4), "only");
    }

    #[test]
    fn word_and_line_selection_are_isolated_per_terminal() {
        let first = terminal("alpha beta", 20, 2);
        let second = terminal("another pane", 20, 2);
        first.start_selection(point(0, 7), Side::Left, SelectionType::Semantic);
        assert_eq!(first.selected_text().as_deref(), Some("beta"));
        assert_eq!(second.selected_text(), None);
        first.start_selection(point(0, 7), Side::Left, SelectionType::Lines);
        assert_eq!(first.selected_text().as_deref(), Some("alpha beta\n"));
    }
}
