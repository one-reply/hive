//! When a terminal last printed output and last received input, so a caller can tell a program
//! that is producing output on its own (an agent streaming or showing a spinner) from one that
//! only echoes keystrokes or redraws after a resize.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Output this recent counts as the program being busy.
const OUTPUT_WINDOW: Duration = Duration::from_millis(1500);
/// Output this soon after input is the terminal answering it (echo, redraw), not work.
const ECHO_WINDOW: Duration = Duration::from_millis(400);
/// Clock value for "never happened".
const NEVER: u64 = 0;

/// Shared, lock-free record of a terminal's last output and input. Cheap to clone and `Send`,
/// so a background poller can read it while the PTY thread writes it.
#[derive(Clone, Debug)]
pub struct Activity(Arc<Clock>);

#[derive(Debug)]
struct Clock {
    origin: Instant,
    /// Milliseconds since `origin`, offset by one so that `NEVER` stays distinct from zero.
    last_output: AtomicU64,
    last_input: AtomicU64,
}

impl Default for Activity {
    fn default() -> Self {
        Self(Arc::new(Clock {
            origin: Instant::now(),
            last_output: AtomicU64::new(NEVER),
            last_input: AtomicU64::new(NEVER),
        }))
    }
}

impl Activity {
    pub(super) fn record_output(&self) {
        self.0.last_output.store(self.stamp(), Ordering::Relaxed);
    }

    /// Keystrokes, pastes and resizes: anything the terminal will redraw in answer to.
    pub(super) fn record_input(&self) {
        self.0.last_input.store(self.stamp(), Ordering::Relaxed);
    }

    /// Whether the program printed output of its own accord within the last moment.
    pub fn is_streaming(&self, now: Instant) -> bool {
        let read = |stamp: &AtomicU64| match stamp.load(Ordering::Relaxed) {
            NEVER => None,
            millis => Some(Duration::from_millis(millis - 1)),
        };
        is_streaming(
            read(&self.0.last_output),
            read(&self.0.last_input),
            now.saturating_duration_since(self.0.origin),
        )
    }

    /// Whether the terminal received input (keystrokes, pastes, resizes) within `window`.
    pub fn had_input_within(&self, window: Duration, now: Instant) -> bool {
        match self.0.last_input.load(Ordering::Relaxed) {
            NEVER => false,
            millis => {
                now.saturating_duration_since(self.0.origin)
                    .saturating_sub(Duration::from_millis(millis - 1))
                    <= window
            }
        }
    }

    fn stamp(&self) -> u64 {
        let millis = u64::try_from(self.0.origin.elapsed().as_millis()).unwrap_or(u64::MAX - 1);
        millis + 1
    }
}

/// Times are offsets from one origin. Output only counts when it is recent and came well after
/// the latest input, so typing into an idle prompt does not look like work.
fn is_streaming(
    last_output: Option<Duration>,
    last_input: Option<Duration>,
    now: Duration,
) -> bool {
    let Some(output) = last_output else {
        return false;
    };
    if now.saturating_sub(output) > OUTPUT_WINDOW {
        return false;
    }
    last_input.is_none_or(|input| output.saturating_sub(input) > ECHO_WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn no_output_is_idle() {
        assert!(!is_streaming(None, None, ms(5_000)));
        assert!(!is_streaming(None, Some(ms(4_000)), ms(5_000)));
    }

    #[test]
    fn recent_output_without_input_is_streaming() {
        assert!(is_streaming(Some(ms(4_800)), None, ms(5_000)));
    }

    #[test]
    fn output_goes_quiet_after_the_window() {
        assert!(!is_streaming(Some(ms(3_000)), None, ms(5_000)));
    }

    #[test]
    fn echo_of_a_keystroke_is_not_work() {
        assert!(!is_streaming(Some(ms(4_950)), Some(ms(4_900)), ms(5_000)));
    }

    #[test]
    fn output_long_after_the_last_keystroke_is_work() {
        // The user pressed Enter, the agent keeps printing its spinner.
        assert!(is_streaming(Some(ms(4_900)), Some(ms(2_000)), ms(5_000)));
    }

    #[test]
    fn clock_records_and_reads_back() {
        let activity = Activity::default();
        assert!(!activity.is_streaming(Instant::now()));
        activity.record_output();
        assert!(activity.is_streaming(Instant::now()));
        activity.record_input();
        activity.record_output();
        assert!(!activity.is_streaming(Instant::now()));
    }

    #[test]
    fn recent_input_is_reported_until_the_window_passes() {
        let activity = Activity::default();
        let now = Instant::now();
        assert!(!activity.had_input_within(Duration::from_secs(2), now));
        activity.record_input();
        let now = Instant::now();
        assert!(activity.had_input_within(Duration::from_secs(2), now));
        assert!(!activity.had_input_within(Duration::from_secs(2), now + Duration::from_secs(3)));
    }
}
