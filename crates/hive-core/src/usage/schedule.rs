//! When each vendor may be asked for its limits again. The endpoints are undocumented and
//! free to refuse: one that failed is asked less often, and one that said how long to wait
//! is left alone for that long.

use super::{Answer, UsageError, Vendor};
use std::time::{Duration, Instant};

/// How often every vendor is asked while it answers. Anthropic's usage endpoint rate-limits
/// aggressively, so polling is slow to begin with.
pub const POLL_INTERVAL: Duration = Duration::from_secs(300);
/// The longest a vendor that keeps failing goes unasked.
const LONGEST_BACKOFF: Duration = Duration::from_secs(60 * 60);
/// The longest wait a server is believed about: a header asking for more is a mistake, and
/// a restart forgets the wait anyway.
const LONGEST_RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// The vendors that are being left alone after a failure.
#[derive(Debug, Default)]
pub struct Schedule {
    waits: Vec<(Vendor, Wait)>,
}

#[derive(Clone, Copy, Debug)]
struct Wait {
    /// Failures in a row, at least one.
    failures: u32,
    until: Instant,
    /// The server refused with a rate limit; not even an explicit refresh asks it sooner.
    rate_limited: bool,
}

impl Schedule {
    /// Whether `vendor` may be asked at `now`. A poll on the timer waits out every failure;
    /// a refresh the user `requested` skips the backoff, but not a server's rate limit.
    pub fn allows(&self, vendor: Vendor, requested: bool, now: Instant) -> bool {
        match self.wait(vendor) {
            Some(wait) if now < wait.until => requested && !wait.rate_limited,
            _ => true,
        }
    }

    /// Records what a vendor answered in the poll that began at `polled`.
    ///
    /// Only a failed request holds a vendor back. Missing or expired credentials are found
    /// without a request, and are looked at again on every poll so that signing in shows at once.
    pub fn record(&mut self, answer: &Answer, polled: Instant) {
        let provider = &answer.provider;
        let failures = self.wait(provider.vendor).map_or(0, |wait| wait.failures) + 1;
        self.waits.retain(|(vendor, _)| *vendor != provider.vendor);
        let (retry_after, rate_limited) = match &provider.error {
            None | Some(UsageError::Credentials(_)) => return,
            Some(UsageError::RateLimited { retry_after }) => (*retry_after, true),
            Some(UsageError::Http(_) | UsageError::Json(_)) => (None, false),
        };
        // The wait a server named runs from its answer. The vendors are asked one after
        // another, so counted from the start of the poll it would end early by as long as
        // the vendors before this one took.
        let named = retry_after.map(|wait| {
            let answered_after = answer.received.saturating_duration_since(polled);
            answered_after.saturating_add(wait.min(LONGEST_RETRY_AFTER))
        });
        // Our own backoff runs from the start of the poll, which is a tick of the timer.
        // Counted from the answer it would end just after the tick it was meant to end on,
        // and the vendor would go unasked for one interval more.
        let wait = backoff(failures).max(named.unwrap_or_default());
        // A wait too long for the clock to hold is no wait: the next poll simply asks.
        if let Some(until) = polled.checked_add(wait) {
            self.waits.push((
                provider.vendor,
                Wait {
                    failures,
                    until,
                    rate_limited,
                },
            ));
        }
    }

    fn wait(&self, vendor: Vendor) -> Option<&Wait> {
        self.waits
            .iter()
            .find_map(|(held, wait)| (*held == vendor).then_some(wait))
    }
}

/// Two poll intervals after the first failure, doubling up to [`LONGEST_BACKOFF`].
fn backoff(failures: u32) -> Duration {
    let doubled = 2u32
        .checked_pow(failures)
        .and_then(|factor| POLL_INTERVAL.checked_mul(factor));
    doubled.map_or(LONGEST_BACKOFF, |wait| wait.min(LONGEST_BACKOFF))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::CredentialError;
    use crate::usage::Provider;

    /// What `vendor` answered `after` the poll that began at `polled` did.
    fn answered_after(
        vendor: Vendor,
        error: Option<UsageError>,
        polled: Instant,
        after: Duration,
    ) -> Answer {
        Answer {
            provider: Provider {
                vendor,
                windows: vec![],
                error,
            },
            received: polled + after,
        }
    }

    /// Records an answer that came as soon as the poll began.
    fn record(schedule: &mut Schedule, vendor: Vendor, error: Option<UsageError>, polled: Instant) {
        schedule.record(
            &answered_after(vendor, error, polled, Duration::ZERO),
            polled,
        );
    }

    fn failure() -> Option<UsageError> {
        Some(UsageError::Http("HTTP 503".into()))
    }

    /// The first scheduled poll, counted in poll intervals from `start`, that asks `vendor`.
    fn next_poll(schedule: &Schedule, vendor: Vendor, start: Instant) -> u32 {
        (1..100)
            .find(|tick| schedule.allows(vendor, false, start + POLL_INTERVAL * *tick))
            .expect("every wait ends")
    }

    #[test]
    fn repeated_failures_double_the_wait_up_to_an_hour_and_an_answer_resets_it() {
        let start = Instant::now();
        let mut schedule = Schedule::default();
        assert!(schedule.allows(Vendor::Claude, false, start));
        let mut asked = start;
        let mut skipped = Vec::new();
        for _ in 0..6 {
            record(&mut schedule, Vendor::Claude, failure(), asked);
            let ticks = next_poll(&schedule, Vendor::Claude, asked);
            skipped.push(ticks);
            asked += POLL_INTERVAL * ticks;
        }
        // 10, 20 and 40 minutes, then an hour however long it goes on.
        assert_eq!(skipped, [2, 4, 8, 12, 12, 12]);
        // The others are asked on every poll all along.
        assert!(schedule.allows(Vendor::Codex, false, start));

        record(&mut schedule, Vendor::Claude, None, asked);
        assert!(schedule.allows(Vendor::Claude, false, asked));
        record(&mut schedule, Vendor::Claude, failure(), asked);
        assert_eq!(next_poll(&schedule, Vendor::Claude, asked), 2);
    }

    #[test]
    fn a_rate_limited_vendor_is_left_alone_for_as_long_as_the_server_asked() {
        let start = Instant::now();
        let limited = |retry_after| {
            let mut schedule = Schedule::default();
            let error = UsageError::RateLimited { retry_after };
            record(&mut schedule, Vendor::Claude, Some(error), start);
            schedule
        };
        let hour = Duration::from_secs(60 * 60);
        let schedule = limited(Some(hour));
        assert!(!schedule.allows(Vendor::Claude, false, start + hour - Duration::from_secs(1)));
        assert!(schedule.allows(Vendor::Claude, false, start + hour));
        // Not even the user's own refresh asks a server that said when to come back.
        assert!(!schedule.allows(Vendor::Claude, true, start + POLL_INTERVAL));
        assert!(schedule.allows(Vendor::Claude, true, start + hour));

        // A short or missing wait still backs off like any failure.
        for retry_after in [None, Some(Duration::from_secs(5))] {
            assert_eq!(next_poll(&limited(retry_after), Vendor::Claude, start), 2);
        }
        // An absurd one is bounded.
        let forever = limited(Some(Duration::MAX));
        assert!(forever.allows(Vendor::Claude, false, start + LONGEST_RETRY_AFTER));
    }

    #[test]
    fn a_wait_the_server_named_runs_from_its_answer_not_from_the_start_of_the_poll() {
        let start = Instant::now();
        // The vendors asked before this one took this long to answer.
        let late = Duration::from_secs(25);
        let hour = Duration::from_secs(60 * 60);
        let second = Duration::from_secs(1);
        let limited = |retry_after| {
            let mut schedule = Schedule::default();
            let error = Some(UsageError::RateLimited { retry_after });
            schedule.record(&answered_after(Vendor::Cursor, error, start, late), start);
            schedule
        };
        let schedule = limited(Some(hour));
        // An hour after the poll began is not yet an hour after the server asked for one.
        assert!(!schedule.allows(Vendor::Cursor, false, start + hour));
        assert!(!schedule.allows(Vendor::Cursor, true, start + hour + late - second));
        assert!(schedule.allows(Vendor::Cursor, false, start + hour + late));
        // A day is the most a server is believed about, from its answer too.
        let forever = limited(Some(Duration::MAX));
        assert!(!forever.allows(Vendor::Cursor, false, start + LONGEST_RETRY_AFTER));
        assert!(forever.allows(Vendor::Cursor, false, start + LONGEST_RETRY_AFTER + late));
    }

    #[test]
    fn a_backoff_ends_on_the_tick_it_was_meant_to_however_late_in_the_poll_the_vendor_failed() {
        let start = Instant::now();
        let late = Duration::from_secs(25);
        for error in [
            failure(),
            Some(UsageError::RateLimited { retry_after: None }),
            // Named, and shorter than the backoff: the backoff decides.
            Some(UsageError::RateLimited {
                retry_after: Some(Duration::from_secs(5)),
            }),
        ] {
            let mut schedule = Schedule::default();
            schedule.record(&answered_after(Vendor::Cursor, error, start, late), start);
            // Ten minutes: the second tick, not the third it would be if counted from the answer.
            assert!(schedule.allows(Vendor::Cursor, false, start + POLL_INTERVAL * 2));
            assert_eq!(next_poll(&schedule, Vendor::Cursor, start), 2);
        }
        // An answer stamped before its poll began (two clocks, a stale value) adds nothing.
        let mut schedule = Schedule::default();
        let early = Answer {
            received: start,
            ..answered_after(Vendor::Cursor, failure(), start, Duration::ZERO)
        };
        schedule.record(&early, start + late);
        assert_eq!(next_poll(&schedule, Vendor::Cursor, start + late), 2);
    }

    #[test]
    fn a_refresh_the_user_asked_for_does_not_wait_out_a_plain_failure() {
        let start = Instant::now();
        let mut schedule = Schedule::default();
        record(&mut schedule, Vendor::Codex, failure(), start);
        assert!(!schedule.allows(Vendor::Codex, false, start + POLL_INTERVAL));
        assert!(schedule.allows(Vendor::Codex, true, start));
    }

    #[test]
    fn a_signed_out_cli_is_looked_at_on_every_poll() {
        let start = Instant::now();
        let mut schedule = Schedule::default();
        for error in [
            CredentialError::NotLoggedIn("claude login"),
            CredentialError::Expired("claude"),
            CredentialError::Unavailable("keychain".into()),
        ] {
            let signed_out = Some(UsageError::Credentials(error));
            record(&mut schedule, Vendor::Claude, signed_out, start);
            assert!(schedule.allows(Vendor::Claude, false, start));
        }
        // And it ends a wait an earlier failure began: the request that failed is not the
        // one that would be made now.
        record(&mut schedule, Vendor::Claude, failure(), start);
        let signed_out = UsageError::Credentials(CredentialError::NotLoggedIn("claude login"));
        record(&mut schedule, Vendor::Claude, Some(signed_out), start);
        assert!(schedule.allows(Vendor::Claude, false, start));
    }

    #[test]
    fn a_malformed_answer_backs_off_like_a_failed_request() {
        let start = Instant::now();
        let mut schedule = Schedule::default();
        let garbled = UsageError::Json("expected value".into());
        record(&mut schedule, Vendor::Cursor, Some(garbled), start);
        assert_eq!(next_poll(&schedule, Vendor::Cursor, start), 2);
    }
}
