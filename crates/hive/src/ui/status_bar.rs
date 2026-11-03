//! Bottom bar: usage limits per provider + refresh.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex};
use gpui_kit::*;
use hive_core::usage;

use crate::app::AppState;
use crate::state::usage::Usage;

pub struct StatusBarView {
    usage: Entity<Usage>,
    _observer: Subscription,
}

impl StatusBarView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let usage = AppState::get(cx).usage.clone();
        let observer = cx.observe(&usage, |_, _, cx| cx.notify());
        Self { usage, _observer: observer }
    }
}

fn logo_svg(provider: &str) -> &'static [u8] {
    match provider {
        "Claude" => include_bytes!("../../assets/claude.svg"),
        _ => include_bytes!("../../assets/openai.svg"),
    }
}

impl Render for StatusBarView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.usage.read(cx);
        let providers = state.providers.iter().map(|p| {
            let row = h_flex()
                .gap_2()
                .items_center()
                .child(Icon::default().data(logo_svg(p.name)).xsmall())
                .child(div().font_weight(FontWeight::SEMIBOLD).child(p.name));
            match &p.error {
                Some(err) => row.child(err.to_string()),
                None if p.windows.is_empty() => row.child("no data"),
                None => row.children(p.windows.iter().map(|w| {
                    let reset = w.resets_at.map(usage::until).unwrap_or_else(|| "?".into());
                    format!("{} {:.0}% · {}", w.label, w.percent, reset)
                })),
            }
        });
        let usage = self.usage.clone();
        StatusBar::new()
            .bg(cx.theme().background)
            .left(h_flex().gap_6().child("Used").children(providers))
            .right(
                Button::new("refresh-usage")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .loading(state.loading)
                    .tooltip("Refresh limits")
                    .on_click(move |_, _, cx| usage.update(cx, |u, cx| u.refresh(cx))),
            )
    }
}
