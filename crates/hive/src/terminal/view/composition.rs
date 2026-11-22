//! Text that arrives through the system's text input. What the input method has finished
//! goes to the program; what it is still composing — a dead key's accent, kana before their
//! conversion — is held here and drawn at the cursor until it is finished or dropped.
use std::ops::Range;

use gpui_kit::Context;

use super::TerminalView;

impl TerminalView {
    pub(in crate::terminal) fn marked_text(&self) -> Option<&str> {
        self.marked_text.as_deref()
    }

    /// The composed text as the system addresses it: UTF-16 units of a document that holds
    /// nothing else. While this is `Some`, the system offers Enter, Backspace and the arrows
    /// to the input method before the terminal sees them.
    pub(in crate::terminal) fn marked_text_range(&self) -> Option<Range<usize>> {
        self.marked_text
            .as_deref()
            .map(|text| 0..text.encode_utf16().count())
    }

    /// Replaces the text being composed; an empty one ends the composition.
    pub(in crate::terminal) fn set_marked_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let marked = (!text.is_empty()).then(|| text.to_owned());
        if self.marked_text == marked {
            return;
        }
        self.marked_text = marked;
        self.restart_cursor_blink(cx);
    }

    /// Drops the composition without sending it.
    pub(in crate::terminal) fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        self.set_marked_text("", cx);
    }

    /// Sends finished text to the program and ends the composition it came from.
    pub(in crate::terminal) fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.marked_text = None;
        if !text.is_empty() {
            let bytes = text.as_bytes().to_vec();
            self.session
                .update(cx, |session, cx| session.write_input(bytes, cx));
            self.stop_selection();
        }
        self.restart_cursor_blink(cx);
    }
}

#[cfg(test)]
mod tests;
