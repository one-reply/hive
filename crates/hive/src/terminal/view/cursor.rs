//! A stepped fade, armed only by a visible, focused terminal view and only for a while after
//! the last input.
use std::time::Duration;

use gpui_kit::Context;
use hive_core::terminal::Terminal;

use super::TerminalView;

const HOLD_INTERVAL: Duration = Duration::from_millis(240);
const FADE_INTERVAL: Duration = Duration::from_millis(40);
// Four eased steps over 120 ms, with a quiet hold at either end of the blink.
const OPACITIES: [f32; 8] = [1., 0.85, 0.5, 0.15, 0., 0.15, 0.5, 0.85];
/// Blink cycles after the last input, about ten seconds; the cursor then stays lit. Every
/// step redraws the window (a notify dirties every ancestor view), so a cursor that blinked
/// for as long as it had focus would keep an idle Hive drawing eleven frames a second.
const IDLE_CYCLES: usize = 14;
const IDLE_STEPS: usize = IDLE_CYCLES * OPACITIES.len();

impl TerminalView {
    pub(in crate::terminal) fn cursor_opacity(&self) -> f32 {
        OPACITIES[self.cursor_step % OPACITIES.len()]
    }

    /// Lights the cursor and starts a new blink, for input and focus changes.
    pub(super) fn restart_cursor_blink(&mut self, cx: &mut Context<Self>) {
        self.stop_cursor_blink();
        // Only `render` arms the next step.
        cx.notify();
    }

    fn stop_cursor_blink(&mut self) {
        self.cursor_tick = None;
        self.cursor_step = 0;
    }

    /// Called from `render`, so a view that is not drawn keeps no timer running.
    pub(super) fn sync_cursor_blink(&mut self, active: bool, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let blinking = active
            && !cx.reduce_motion()
            && !session.exited
            && session
                .terminal()
                .is_some_and(Terminal::has_blinking_cursor);
        if !blinking {
            self.stop_cursor_blink();
            return;
        }
        if self.cursor_tick.is_some() || self.cursor_step >= IDLE_STEPS {
            return;
        }
        let interval = if self.cursor_step.is_multiple_of(OPACITIES.len() / 2) {
            HOLD_INTERVAL
        } else {
            FADE_INTERVAL
        };
        let timer = cx.background_executor().timer(interval);
        self.cursor_tick = Some(cx.spawn(async move |this, cx| {
            timer.await;
            // A view that is gone has nothing left to redraw.
            let _ = this.update(cx, |this, cx| {
                this.cursor_tick = None;
                this.cursor_step += 1;
                cx.notify();
            });
        }));
    }
}

#[cfg(test)]
mod tests;
