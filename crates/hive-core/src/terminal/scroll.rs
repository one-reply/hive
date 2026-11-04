use super::{Column, Point, TermMode, Terminal};
use alacritty_terminal::grid::{Dimensions as _, Scroll};

/// Keyboard modifiers held during a wheel event.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollModifiers {
    /// Bypass application mouse reporting and scroll the terminal history.
    pub shift: bool,
    /// Report the Alt/Meta modifier to the application.
    pub alt: bool,
    /// Report the Control modifier to the application.
    pub control: bool,
}

impl Terminal {
    /// Scroll history or return wheel input for the caller's PTY input queue.
    /// Positive `lines` means up; `point` is a zero-based viewport cell.
    pub fn scroll_wheel(
        &self,
        lines: i32,
        point: Point<usize>,
        modifiers: ScrollModifiers,
    ) -> Option<Vec<u8>> {
        if lines == 0 {
            return None;
        }
        let mut term = self.term.lock();
        let mode = *term.mode();
        if modifiers.shift || !mode.intersects(TermMode::MOUSE_MODE | TermMode::ALT_SCREEN) {
            term.scroll_display(Scroll::Delta(lines));
            return None;
        }
        let bytes = if mode.intersects(TermMode::MOUSE_MODE) {
            let point = Point::new(
                point.line.min(term.screen_lines() - 1),
                Column(point.column.0.min(term.columns() - 1)),
            );
            let button = if lines > 0 { 64 } else { 65 };
            let button = button + u8::from(modifiers.alt) * 8 + u8::from(modifiers.control) * 16;
            wheel_report(mode, point, button)?
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            vec![
                0x1b,
                if mode.contains(TermMode::APP_CURSOR) {
                    b'O'
                } else {
                    b'['
                },
                if lines > 0 { b'A' } else { b'B' },
            ]
        } else {
            return None;
        };
        // A single gesture event must not allocate an unbounded PTY write.
        Some(bytes.repeat((lines.unsigned_abs() as usize).min(term.screen_lines())))
    }
}

fn wheel_report(mode: TermMode, point: Point<usize>, button: u8) -> Option<Vec<u8>> {
    if mode.contains(TermMode::SGR_MOUSE) {
        return Some(
            format!("\x1b[<{button};{};{}M", point.column.0 + 1, point.line + 1).into_bytes(),
        );
    }
    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let limit = if utf8 { 2015 } else { 223 };
    if point.column.0 >= limit || point.line >= limit {
        return None;
    }
    let mut bytes = vec![0x1b, b'[', b'M', button + 32];
    for coordinate in [point.column.0, point.line] {
        let encoded = coordinate as u32 + 33;
        if utf8 {
            let character = char::from_u32(encoded).expect("mouse coordinate is below 2048");
            bytes.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
        } else {
            bytes.push(encoded as u8);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TerminalSize;
    use alacritty_terminal::vte::ansi::Processor;

    fn terminal_with_output(output: &str) -> Terminal {
        let terminal = Terminal::spawn(None, Some("/bin/cat"), TerminalSize::new(300, 4)).unwrap();
        let mut parser: Processor = Processor::new();
        parser.advance(&mut *terminal.term.lock(), output.as_bytes());
        terminal
    }

    #[test]
    fn wheel_reports_take_priority_over_alternate_screen_arrows() {
        for tracking in [1000, 1002, 1003] {
            let terminal =
                terminal_with_output(&format!("\x1b[?1049h\x1b[?{tracking}h\x1b[?1006h"));
            assert_eq!(
                terminal.scroll_wheel(2, Point::new(2, Column(10)), ScrollModifiers::default()),
                Some(b"\x1b[<64;11;3M\x1b[<64;11;3M".to_vec())
            );
        }
    }

    #[test]
    fn normal_screen_mouse_reporting_preserves_viewport_coordinates_and_modifiers() {
        let terminal = terminal_with_output("1\r\n2\r\n3\r\n4\r\n5\x1b[?1000h\x1b[?1006h");
        terminal.scroll(1);
        assert_eq!(
            terminal.scroll_wheel(
                -1,
                Point::new(1, Column(299)),
                ScrollModifiers {
                    alt: true,
                    control: true,
                    ..Default::default()
                }
            ),
            Some(b"\x1b[<89;300;2M".to_vec())
        );
        assert_eq!(terminal.term.lock().grid().display_offset(), 1);
    }

    #[test]
    fn shift_wheel_reads_history_without_sending_application_input() {
        let terminal = terminal_with_output("1\r\n2\r\n3\r\n4\r\n5\r\n6\x1b[?1000h");
        let modifiers = ScrollModifiers {
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            terminal.scroll_wheel(2, Point::new(0, Column(0)), modifiers),
            None
        );
        assert_eq!(terminal.term.lock().grid().display_offset(), 2);
        assert_eq!(
            terminal.scroll_wheel(-1, Point::new(0, Column(0)), modifiers),
            None
        );
        assert_eq!(terminal.term.lock().grid().display_offset(), 1);
    }

    #[test]
    fn alternate_scroll_respects_application_cursor_mode_and_disable_sequence() {
        let point = Point::new(0, Column(0));
        let modifiers = ScrollModifiers::default();
        let normal = terminal_with_output("\x1b[?1049h");
        assert_eq!(
            normal.scroll_wheel(1, point, modifiers),
            Some(b"\x1b[A".to_vec())
        );
        let application = terminal_with_output("\x1b[?1049h\x1b[?1h");
        assert_eq!(
            application.scroll_wheel(-1, point, modifiers),
            Some(b"\x1bOB".to_vec())
        );
        let disabled = terminal_with_output("\x1b[?1049h\x1b[?1007l");
        assert_eq!(disabled.scroll_wheel(1, point, modifiers), None);
    }

    #[test]
    fn legacy_and_utf8_reports_respect_protocol_coordinate_limits() {
        let point = Point::new(2, Column(10));
        assert_eq!(
            wheel_report(TermMode::NONE, point, 64),
            Some(b"\x1b[M`+#".to_vec())
        );
        assert_eq!(
            wheel_report(TermMode::NONE, Point::new(0, Column(222)), 65),
            Some(b"\x1b[Ma\xff!".to_vec())
        );
        assert_eq!(
            wheel_report(TermMode::NONE, Point::new(0, Column(223)), 64),
            None
        );
        assert_eq!(
            wheel_report(TermMode::UTF8_MOUSE, Point::new(0, Column(222)), 64),
            Some("\x1b[M`ÿ!".as_bytes().to_vec())
        );
        assert_eq!(
            wheel_report(TermMode::UTF8_MOUSE, Point::new(0, Column(2014)), 64),
            Some("\x1b[M`\u{7ff}!".as_bytes().to_vec())
        );
        assert_eq!(
            wheel_report(TermMode::UTF8_MOUSE, Point::new(2015, Column(0)), 64),
            None
        );
    }

    #[test]
    fn clamps_stale_geometry_and_bounds_large_input_batches() {
        let terminal = terminal_with_output("\x1b[?1000h\x1b[?1006h");
        let point = Point::new(usize::MAX, Column(usize::MAX));
        let modifiers = ScrollModifiers::default();
        assert_eq!(
            terminal.scroll_wheel(i32::MIN, point, modifiers),
            Some(b"\x1b[<65;300;4M".repeat(4))
        );
        assert_eq!(terminal.scroll_wheel(0, point, modifiers), None);
        let shell = terminal_with_output("1\r\n2\r\n3\r\n4\r\n5");
        assert_eq!(shell.scroll_wheel(1, point, modifiers), None);
        assert_eq!(shell.term.lock().grid().display_offset(), 1);
    }
}
