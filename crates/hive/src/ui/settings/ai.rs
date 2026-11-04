//! API key forms. Credential IO and validation belong to AiProviders / hive-core.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::llm::Provider;

use crate::app::AppState;
use crate::state::ai_providers::{AiProviders, KeyStatus};
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use crate::ui::text_field::TextField;

const INPUT_MIN_WIDTH: Pixels = px(150.);

struct ProviderInput {
    provider: Provider,
    input: Entity<InputState>,
}

pub(super) struct AiPage {
    providers: Entity<AiProviders>,
    inputs: Vec<ProviderInput>,
    _subscriptions: Vec<Subscription>,
}

impl AiPage {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let providers = AppState::get(cx).ai_providers.clone();
        let mut subscriptions = vec![cx.observe_in(&providers, window, |this, _, window, cx| {
            for row in &this.inputs {
                if this.providers.read(cx).provider(row.provider).status == KeyStatus::Configured
                    && !row.input.read(cx).value().is_empty()
                {
                    row.input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
            }
            cx.notify();
        })];
        let inputs = Provider::ALL
            .into_iter()
            .map(|provider| {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder("Enter API key")
                });
                subscriptions.push(cx.subscribe(&input, move |this, _, event, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.save(provider, cx);
                    }
                    cx.notify();
                }));
                ProviderInput { provider, input }
            })
            .collect();
        Self {
            providers,
            inputs,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn clear(&self, window: &mut Window, cx: &mut Context<Self>) {
        for row in &self.inputs {
            row.input
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub(super) fn refresh(&self, cx: &mut Context<Self>) {
        self.providers
            .update(cx, |providers, cx| providers.refresh(cx));
    }

    fn save(&self, provider: Provider, cx: &mut Context<Self>) {
        let Some(row) = self.inputs.iter().find(|row| row.provider == provider) else {
            return;
        };
        let value = row.input.read(cx).value().to_string();
        self.providers
            .update(cx, |providers, cx| providers.save(provider, value, cx));
    }

    fn render_provider(
        &self,
        row: &ProviderInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let provider = row.provider;
        let state = self.providers.read(cx).provider(provider);
        let (status, error) = (state.status, state.error.clone());
        let can_save = !status.is_busy() && !row.input.read(cx).value().is_empty();
        let save_hover = Hover::new(("save-key", provider as usize), window, cx).enabled(can_save);
        let action_hover =
            Hover::new(("key-action", provider as usize), window, cx).enabled(!status.is_busy());
        let theme = cx.theme();
        let (muted, border, success, danger) = (
            theme.muted_foreground,
            theme.border,
            theme.success,
            theme.danger,
        );
        let configured = matches!(status, KeyStatus::Configured | KeyStatus::Deleting);
        let body = match status {
            KeyStatus::Missing | KeyStatus::Saving => h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    div()
                        .flex_1()
                        .min_w(INPUT_MIN_WIDTH)
                        .child(TextField::input(
                            Input::new(&row.input)
                                .disabled(status.is_busy())
                                .aria_label(format!("{} API key", provider.name())),
                            &row.input,
                            cx,
                        )),
                )
                .child(
                    Button::new(("save-key", provider as usize))
                        .cursor_pointer()
                        .small()
                        .label("Verify & Save")
                        .loading(status == KeyStatus::Saving)
                        .disabled(!can_save)
                        .on_click(cx.listener(move |this, _, _, cx| this.save(provider, cx)))
                        .smooth_hover(ButtonLook::Primary, &save_hover, cx),
                ),
            _ => {
                let providers = self.providers.clone();
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_1()
                    .border_color(border)
                    .rounded_lg()
                    .when(configured, |row| {
                        row.child(Icon::new(Lucide::Check).size_4().text_color(success))
                    })
                    .child(div().flex_1().min_w_0().text_sm().child(if configured {
                        "API Key Configured"
                    } else if status == KeyStatus::Checking {
                        "Checking Keychain…"
                    } else {
                        "Keychain unavailable"
                    }))
                    .child(
                        Button::new(("key-action", provider as usize))
                            .cursor_pointer()
                            .small()
                            .label(if configured { "Reset Key" } else { "Retry" })
                            .icon(Lucide::RotateCcw)
                            .loading(status.is_busy())
                            .disabled(status.is_busy())
                            .on_click(move |_, window, cx| {
                                if configured {
                                    confirm_reset(providers.clone(), provider, window, cx);
                                } else {
                                    providers
                                        .update(cx, |providers, cx| providers.check(provider, cx));
                                }
                            })
                            .smooth_hover(ButtonLook::Ghost, &action_hover, cx),
                    )
            }
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .pb_2()
                    .border_b_1()
                    .border_color(border)
                    .text_color(muted)
                    .child(Icon::default().data(logo(provider)).size_4())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(provider.name()),
                    ),
            )
            .child(body)
            .when_some(error, |block, error| {
                block.child(div().text_sm().text_color(danger).child(error))
            })
    }
}

impl Render for AiPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_6().pt_2().children(
            self.inputs
                .iter()
                .map(|row| self.render_provider(row, window, cx))
                .collect::<Vec<_>>(),
        )
    }
}

fn confirm_reset(
    providers: Entity<AiProviders>,
    provider: Provider,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        let providers = providers.clone();
        alert.confirm()
            .title(format!("Reset {} API key?", provider.name()))
            .description("Remove this key from Hive's Keychain storage. This does not revoke it with the provider.")
            .ok_text("Reset Key")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                providers.update(cx, |providers, cx| providers.delete(provider, cx));
                true
            })
    });
}

pub(super) fn logo(provider: Provider) -> &'static [u8] {
    match provider {
        Provider::GoogleAi => include_bytes!("../../../assets/google-ai.svg"),
        Provider::OpenAi => include_bytes!("../../../assets/openai.svg"),
        Provider::OpenRouter => include_bytes!("../../../assets/openrouter.svg"),
        Provider::Anthropic => include_bytes!("../../../assets/anthropic.svg"),
    }
}
