use gpui_kit::*;
use hive_core::usage::{self, POLL_INTERVAL, Provider, Schedule, Vendor};
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
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        // The start of the poll, not its end: a wait counted from the answer would slip
        // past the timer tick it was meant to end on.
        let asked = Instant::now();
        let ask: Vec<Vendor> = Vendor::ALL
            .into_iter()
            .filter(|vendor| self.schedule.allows(*vendor, requested, asked))
            .collect();
        cx.spawn(async move |this, cx| {
            let polled = cx
                .background_executor()
                .spawn(async move { usage::poll(&ask) })
                .await;
            let _ = this.update(cx, |usage, cx| {
                for provider in polled.iter().filter_map(|(_, fresh)| fresh.as_ref()) {
                    usage.schedule.record(provider, asked);
                }
                usage.providers = usage::merge(&usage.providers, polled);
                usage.loading = false;
                cx.notify();
            });
        })
        .detach();
    }
}
