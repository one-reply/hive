use gpui_kit::*;
use hive_core::usage::{self, Answer, POLL_INTERVAL, Provider, Schedule, Vendor};
use std::time::Instant;

pub struct Usage {
    pub providers: Vec<Provider>,
    pub loading: bool,
    /// Which vendors are being left alone after a failure or a rate limit.
    schedule: Schedule,
}

impl Usage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this.update(cx, |usage, cx| usage.poll(false, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            providers: vec![],
            loading: false,
            schedule: Schedule::default(),
        };
        this.poll(false, cx);
        this
    }

    /// Asks every vendor now, because the user did. Only a vendor whose server said when to
    /// come back is still left alone.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.poll(true, cx);
    }

    fn poll(&mut self, requested: bool, cx: &mut Context<Self>) {
        self.poll_with(requested, |ask| usage::poll(&ask), cx);
    }

    /// [`Self::poll`] with the request to the vendors given by the caller.
    fn poll_with(
        &mut self,
        requested: bool,
        request: impl FnOnce(Vec<Vendor>) -> Vec<(Vendor, Option<Answer>)> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        // The start of the poll, not its end: a backoff counted from the answer would slip
        // past the timer tick it was meant to end on. A wait a server named is counted from
        // that vendor's own answer, whose time each answer carries.
        let started = Instant::now();
        let ask: Vec<Vendor> = Vendor::ALL
            .into_iter()
            .filter(|vendor| self.schedule.allows(*vendor, requested, started))
            .collect();
        cx.spawn(async move |this, cx| {
            let polled = cx
                .background_executor()
                .spawn(async move { request(ask) })
                .await;
            let _ = this.update(cx, |usage, cx| {
                for answer in polled.iter().filter_map(|(_, fresh)| fresh.as_ref()) {
                    usage.schedule.record(answer, started);
                }
                usage.providers = usage::merge(&usage.providers, polled);
                usage.loading = false;
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    // No glob import of `super`: it re-exports `gpui_kit::test`, which `#[gpui::test]` expands to.
    use super::Usage;
    use gpui_kit as gpui;
    use gpui_kit::{AppContext as _, Entity, TestAppContext};
    use hive_core::usage::{Answer, Provider, Schedule, UsageError, Vendor};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Usage that asks nobody by itself: no timer and no first poll.
    fn idle(cx: &mut TestAppContext) -> Entity<Usage> {
        cx.new(|_| Usage {
            providers: vec![],
            loading: false,
            schedule: Schedule::default(),
        })
    }

    /// The vendors each poll asked, in the order of the polls.
    type Asked = Arc<Mutex<Vec<Vec<Vendor>>>>;

    /// Vendors that are all installed; each one asked answers with what `error_of` says.
    fn vendors(
        asked: &Asked,
        error_of: fn(Vendor) -> Option<UsageError>,
    ) -> impl FnOnce(Vec<Vendor>) -> Vec<(Vendor, Option<Answer>)> + Send + 'static {
        let asked = asked.clone();
        move |ask| {
            asked.lock().unwrap().push(ask.clone());
            let answer = |vendor| Answer {
                provider: Provider {
                    vendor,
                    windows: vec![],
                    error: error_of(vendor),
                },
                received: Instant::now(),
            };
            Vendor::ALL
                .into_iter()
                .map(|vendor| (vendor, ask.contains(&vendor).then(|| answer(vendor))))
                .collect()
        }
    }

    fn errors(usage: &Entity<Usage>, cx: &mut TestAppContext) -> Vec<(Vendor, Option<UsageError>)> {
        usage.read_with(cx, |usage, _| {
            let shown = usage.providers.iter();
            shown.map(|p| (p.vendor, p.error.clone())).collect()
        })
    }

    #[gpui::test]
    fn a_refresh_asks_a_vendor_that_failed_but_not_one_whose_server_named_a_wait(
        cx: &mut TestAppContext,
    ) {
        fn refusing(vendor: Vendor) -> Option<UsageError> {
            match vendor {
                Vendor::Codex => Some(UsageError::Http("HTTP 503".into())),
                Vendor::Cursor => Some(UsageError::RateLimited {
                    retry_after: Some(Duration::from_secs(60 * 60)),
                }),
                _ => None,
            }
        }
        let usage = idle(cx);
        let asked = Asked::default();

        usage.update(cx, |usage, cx| {
            usage.poll_with(false, vendors(&asked, refusing), cx);
            assert!(usage.loading);
            // A poll already under way is not doubled.
            usage.poll_with(true, vendors(&asked, refusing), cx);
        });
        cx.run_until_parked();
        assert_eq!(*asked.lock().unwrap(), [Vendor::ALL.to_vec()]);
        usage.read_with(cx, |usage, _| assert!(!usage.loading));
        let refused = errors(&usage, cx);
        assert_eq!(refused.len(), Vendor::ALL.len());

        // The timer leaves both alone; the user's own refresh asks all but the rate-limited one.
        for requested in [false, true] {
            usage.update(cx, |usage, cx| {
                usage.poll_with(requested, vendors(&asked, |_| None), cx);
            });
            cx.run_until_parked();
        }
        let asked = asked.lock().unwrap();
        assert_eq!(asked[1], [Vendor::Claude, Vendor::Antigravity]);
        assert_eq!(
            asked[2],
            [Vendor::Claude, Vendor::Codex, Vendor::Antigravity]
        );
        // What was last known of a vendor that was not asked is still shown.
        let shown = errors(&usage, cx);
        assert_eq!(shown[1], (Vendor::Codex, None));
        assert_eq!(shown[2], refused[2]);
        assert!(matches!(shown[2].1, Some(UsageError::RateLimited { .. })));
    }
}
