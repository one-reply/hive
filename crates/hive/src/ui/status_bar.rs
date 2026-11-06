//! Bottom bar: usage limits per provider + refresh, and the settings gear.

use gpui_kit::base::motion::{Presence, Transition};
use gpui_kit::component::button::Button;
use gpui_kit::component::hover_card::HoverCard;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, ThemeStyled as _, h_flex, v_flex,
};
use gpui_kit::*;
use hive_core::usage::{self, Provider, UsageLevel, UsageWindow, Vendor};
use std::time::Duration;

use crate::app::AppState;
use crate::state::usage::Usage;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use crate::ui::settings;

/// Width of the per-provider bar in the status bar.
const BAR_WIDTH: Pixels = px(56.);
/// Width of the details card shown on hover.
const CARD_WIDTH: Pixels = px(240.);
/// The card should feel like a tooltip, not a deliberate hover.
const CARD_OPEN_DELAY: Duration = Duration::from_millis(150);
/// How long the card takes to fade and rise in, and to sink and fade out.
const CARD_TRANSITION: Duration = Duration::from_millis(180);
/// Outlasts `CARD_TRANSITION`, so the card is removed only once it has faded out.
const CARD_CLOSE_DELAY: Duration = Duration::from_millis(200);
/// How far below its place the card starts rising from.
const CARD_RISE: Pixels = px(6.);

pub struct StatusBarView {
    usage: Entity<Usage>,
    _observer: Subscription,
}

impl StatusBarView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let usage = AppState::get(cx).usage.clone();
        let observer = cx.observe(&usage, |_, _, cx| cx.notify());
        Self {
            usage,
            _observer: observer,
        }
    }
}

fn logo_svg(vendor: Vendor) -> &'static [u8] {
    match vendor {
        Vendor::Claude => include_bytes!("../../assets/claude.svg"),
        Vendor::Codex => include_bytes!("../../assets/openai.svg"),
        Vendor::Cursor => include_bytes!("../../assets/cursor.svg"),
        Vendor::Antigravity => include_bytes!("../../assets/antigravity.svg"),
    }
}

fn vendor_title(vendor: Vendor) -> Div {
    h_flex()
        .gap_2()
        .items_center()
        .child(Icon::default().data(logo_svg(vendor)).xsmall())
        .child(div().font_weight(FontWeight::SEMIBOLD).child(vendor.name()))
}

fn level_color(level: UsageLevel, cx: &App) -> Hsla {
    let theme = cx.theme();
    match level {
        UsageLevel::Ok => theme.success,
        UsageLevel::Warning => theme.warning,
        UsageLevel::Critical => theme.danger,
    }
}

/// Whether the pointer is over a provider's bar or its card: the card shows while either is.
#[derive(Clone, Copy, Default, PartialEq)]
struct CardHover {
    trigger: bool,
    card: bool,
}

impl CardHover {
    fn is_hovered(self) -> bool {
        self.trigger || self.card
    }
}

fn set_card_hover(hover: &Entity<CardHover>, cx: &mut App, change: impl FnOnce(&mut CardHover)) {
    hover.update(cx, |hover, cx| {
        let before = *hover;
        change(hover);
        if *hover != before {
            cx.notify();
        }
    });
}

/// Logo, name and a bar of what is left of the limit that will stop work first; every window
/// shows on hover.
fn provider_item(ix: usize, provider: &Provider, window: &mut Window, cx: &mut App) -> AnyElement {
    let title = vendor_title(provider.vendor);
    if let Some(err) = &provider.error {
        return title.child(err.to_string()).into_any_element();
    }
    let Some(binding) = usage::binding_window(&provider.windows) else {
        return title.child("no data").into_any_element();
    };
    let bar = Progress::new(("usage-bar", ix))
        .xsmall()
        .w(BAR_WIDTH)
        .value(binding.remaining_percent() as f32)
        .color(level_color(binding.level(), cx));
    let hover = window.use_keyed_state(("usage-card-hover", ix), cx, |_, _| CardHover::default());
    let trigger = div()
        .id(("usage-trigger", ix))
        .child(title.child(bar))
        .on_hover({
            let hover = hover.clone();
            move |hovered, _, cx| set_card_hover(&hover, cx, |h| h.trigger = *hovered)
        });
    let provider = provider.clone();
    // `HoverCard` drops its content the moment it closes, so the card plays its own exit while
    // the pointer is away and the close delay outlasts it. Its presence state lives only while
    // the card is rendered, so every open starts the entrance from the beginning.
    let mut card = HoverCard::new(("usage-card", ix));
    // The library clips its content box, which would cut the popover ring and shadow drawn just
    // outside our card down to its rounded corners.
    card.style().overflow.x = Some(Overflow::Visible);
    card.style().overflow.y = Some(Overflow::Visible);
    card.anchor(Anchor::BottomLeft)
        .open_delay(CARD_OPEN_DELAY)
        .close_delay(CARD_CLOSE_DELAY)
        .appearance(false)
        .trigger(trigger)
        .content(move |_, window, cx| {
            let shown = Presence::new(
                ElementId::from(("usage-card-presence", ix)),
                hover.read(cx).is_hovered(),
            )
            .transition(Transition::new(CARD_TRANSITION).ease(ease_in_out))
            .sample(window, cx)
            .progress;
            let hover = hover.clone();
            usage_card(&provider, cx)
                .popover_style(cx)
                .p_3()
                .relative()
                .opacity(shown)
                .top(CARD_RISE * (1. - shown))
                .id(("usage-card-body", ix))
                .on_hover(move |hovered, _, cx| set_card_hover(&hover, cx, |h| h.card = *hovered))
        })
        .into_any_element()
}

fn usage_card(provider: &Provider, cx: &App) -> Div {
    v_flex()
        .w(CARD_WIDTH)
        .gap_3()
        .child(vendor_title(provider.vendor))
        .children(
            provider
                .windows
                .iter()
                .enumerate()
                .map(|(ix, w)| window_details(ix, w, cx)),
        )
}

fn window_details(ix: usize, window: &UsageWindow, cx: &App) -> Div {
    let reset = window
        .resets_at
        .map(|t| {
            format!(
                "{:.0}% used · resets in {}",
                window.percent,
                usage::until(t)
            )
        })
        .unwrap_or_else(|| format!("{:.0}% used", window.percent));
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .justify_between()
                .child(div().font_weight(FontWeight::MEDIUM).child(window.label))
                .child(format!("{:.0}% left", window.remaining_percent())),
        )
        .child(
            Progress::new(("usage-card-bar", ix))
                .small()
                .value(window.remaining_percent() as f32)
                .color(level_color(window.level(), cx)),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(reset),
        )
}

impl Render for StatusBarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = self.usage.read(cx).loading;
        let refresh_hover = Hover::new("refresh-usage", window, cx).enabled(!loading);
        let settings_hover = Hover::new("settings", window, cx);
        let providers: Vec<AnyElement> = self
            .usage
            .read(cx)
            .providers
            .clone()
            .iter()
            .enumerate()
            .map(|(ix, p)| provider_item(ix, p, window, cx))
            .collect();
        let usage = self.usage.clone();
        StatusBar::new()
            .bg(cx.theme().background)
            .left(h_flex().gap_6().children(providers))
            .right(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("refresh-usage")
                            .cursor_pointer()
                            .xsmall()
                            .icon(IconName::RefreshCw)
                            .loading(loading)
                            .tooltip("Refresh limits")
                            .on_click(move |_, _, cx| usage.update(cx, |u, cx| u.refresh(cx)))
                            .smooth_hover(ButtonLook::Ghost, &refresh_hover, cx),
                    )
                    .child(
                        Button::new("settings")
                            .cursor_pointer()
                            .xsmall()
                            .icon(IconName::Settings)
                            .tooltip("Settings")
                            .on_click(|_, _, cx| settings::open(cx))
                            .smooth_hover(ButtonLook::Ghost, &settings_hover, cx),
                    ),
            )
    }
}
