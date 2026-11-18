//! Wall-clock scale for persisted measurements. A crash has no known end time.
use hive_core::pipeline::StageTiming;
use std::time::{SystemTime, UNIX_EPOCH};
pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
pub(super) fn observed_end(row: &StageTiming, now: u64) -> u64 {
    row.end_ms
        .unwrap_or(if row.failed { row.start_ms } else { now })
        .max(row.start_ms)
}
pub(super) fn range(rows: &[StageTiming], now: u64) -> Option<(u64, u64)> {
    Some((
        rows.iter().map(|r| r.start_ms).min()?,
        rows.iter().map(|r| observed_end(r, now)).max()?,
    ))
}
/// While recording, leave time ahead of the playhead. Keep the axis fixed between zoom-outs.
pub(super) fn chart_range((start, end): (u64, u64), live: bool) -> (u64, u64) {
    if !live {
        return (start, end);
    }
    const MINUTE: u64 = 60_000;
    let minutes = end.saturating_sub(start) / MINUTE + 1;
    let window = minutes.next_power_of_two().saturating_mul(MINUTE);
    (start, start.saturating_add(window))
}
pub(super) fn stopwatch(ms: u64) -> String {
    let seconds = ms / 1000;
    let minutes = seconds / 60;
    if minutes >= 60 {
        format!("{}:{:02}:{:02}", minutes / 60, minutes % 60, seconds % 60)
    } else {
        format!("{minutes}:{:02}", seconds % 60)
    }
}
pub(super) fn span(row: &StageTiming, scale: (u64, u64), now: u64) -> (f32, f32) {
    let length = scale.1.saturating_sub(scale.0).max(1) as f64;
    let x = row.start_ms.saturating_sub(scale.0) as f64 / length;
    let w = observed_end(row, now).saturating_sub(row.start_ms) as f64 / length;
    (x.clamp(0., 1.) as f32, w.clamp(0., 1.) as f32)
}
pub(super) fn duration(ms: u64) -> String {
    match ms / 1000 {
        0 if ms == 0 => "0s".into(),
        0 => format!("{ms}ms"),
        s @ 1..60 => format!("{s}s"),
        s @ 60..3600 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::pipeline::Phase;
    fn sample(start: u64, end: Option<u64>, failed: bool) -> StageTiming {
        StageTiming {
            phase: Phase::Development,
            lane: Some(0),
            attempt: Some(1),
            start_ms: start,
            end_ms: end,
            failed,
        }
    }
    #[::core::prelude::v1::test]
    fn parallel_attempts_share_a_scale_and_interrupted_rows_do_not_gain_time() {
        let rows = [
            sample(100, Some(140), false),
            sample(120, None, false),
            sample(160, None, true),
        ];
        let scale = range(&rows, 180).unwrap();
        assert_eq!(scale, (100, 180));
        assert_eq!(span(&rows[0], scale, 180), (0., 0.5));
        assert_eq!(span(&rows[1], scale, 180), (0.25, 0.75));
        assert_eq!(span(&rows[2], scale, 180), (0.75, 0.));
        assert_eq!(range(&[], 99), None);
    }
    #[::core::prelude::v1::test]
    fn durations_use_readable_units_without_rounding_away_short_attempts() {
        assert_eq!(duration(87), "87ms");
        assert_eq!(duration(61_200), "1m 1s");
        assert_eq!(duration(3_660_000), "1h 1m");
    }
    #[test]
    fn live_track_advances_with_the_clock_and_finished_history_uses_its_exact_extent() {
        let row = sample(1000, None, false);
        let bounds = |end| chart_range((1000, end), true);
        assert_eq!(bounds(1000), bounds(42_000));
        assert_eq!(span(&row, bounds(16_000), 16_000), (0., 0.25));
        assert_eq!(span(&row, bounds(31_000), 31_000), (0., 0.5));
        assert_eq!(bounds(61_000), (1000, 121_000));
        assert_eq!(chart_range((1000, 42_000), false), (1000, 42_000));
        assert_eq!(chart_range((3, u64::MAX), true), (3, u64::MAX));
    }
    #[test]
    fn stopwatch_counts_whole_seconds_even_after_an_hour() {
        assert_eq!(stopwatch(999), "0:00");
        assert_eq!(stopwatch(41_999), "0:41");
        assert_eq!(stopwatch(3_722_099), "1:02:02");
    }
}
