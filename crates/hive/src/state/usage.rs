use gpui_kit::*;
use hive_core::usage::{self, Provider};
use std::time::Duration;

/// Anthropic's usage endpoint rate-limits aggressively; keep polling slow.
const POLL_INTERVAL: Duration = Duration::from_secs(300);

pub struct Usage {
    pub providers: Vec<Provider>,
    pub loading: bool,
}

impl Usage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this.update(cx, |usage, cx| usage.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            providers: vec![],
            loading: false,
        };
        this.refresh(cx);
        this
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let providers = cx
                .background_executor()
                .spawn(async { usage::fetch_all() })
                .await;
            let _ = this.update(cx, |usage, cx| {
                usage.providers = providers;
                usage.loading = false;
                cx.notify();
            });
        })
        .detach();
    }
}
