//! Commit recipe layout and controls.
use super::*;

impl GitPage {
    fn backend_control(&self, generator: Option<&Generator>, cx: &mut Context<Self>) -> Div {
        match generator {
            Some(Generator::Cli { agent, .. }) => {
                let help = argument_hint(*agent).1;
                v_flex()
                    .gap_2()
                    .child("Arguments")
                    .child(TextField::input(
                        Input::new(&self.arguments).aria_label("Commit generator arguments"),
                        &self.arguments,
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(help),
                    )
                    .when_some(self.argument_error.clone(), |col, error| {
                        col.child(div().text_sm().text_color(cx.theme().danger).child(error))
                    })
            }
            Some(Generator::Api { provider, model }) => {
                let provider = *provider;
                let state = self.providers.read(cx).provider(provider);
                let loading = matches!(state.models, Models::Loading) || state.status.is_busy();
                let error = match &state.models {
                    Models::Failed(error) => Some(error.clone()),
                    Models::Ready(models) if models.is_empty() => Some("No text models are available for this provider.".into()),
                    Models::Ready(models) if model.as_ref().is_some_and(|id| !models.iter().any(|m| &m.id == id)) => Some("Saved model is no longer listed. Select another model if generation fails.".into()),
                    _ => None,
                };
                v_flex()
                    .gap_2()
                    .child("Model")
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Select::new(&self.model)
                                    .flex_1()
                                    .min_w_0()
                                    .placeholder(if loading {
                                        "Loading models…"
                                    } else {
                                        "Select a model"
                                    })
                                    .search_placeholder("Search models…")
                                    .accessibility_label("Commit generation model")
                                    .disabled(loading || state.status != KeyStatus::Configured),
                            )
                            .child(
                                Button::new("reload-commit-models")
                                    .cursor_pointer()
                                    .small()
                                    .ghost()
                                    .icon(Lucide::RefreshCw)
                                    .loading(loading)
                                    .disabled(loading || state.status != KeyStatus::Configured)
                                    .tooltip("Reload models / Retry")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.providers
                                            .update(cx, |p, cx| p.load_models(provider, true, cx))
                                    })),
                            ),
                    )
                    .when_some(error, |col, error| {
                        col.child(div().text_sm().text_color(cx.theme().danger).child(error))
                    })
            }
            None => div(),
        }
    }
}

impl Render for GitPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let generator = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .commit_message
            .generator()
            .cloned();
        let providers = self.providers.read(cx);
        let notice: Option<SharedString> = match &generator {
            Some(Generator::Cli { agent, .. })
                if !providers.scanning_cli && !providers.cli_agents.contains(agent) =>
            {
                Some("Selected CLI is unavailable. Install it and refresh the generators.".into())
            }
            Some(Generator::Api { provider, .. }) => match providers.provider(*provider).status {
                KeyStatus::Missing => {
                    Some("Selected provider has no API key. Add it in AI settings.".into())
                }
                KeyStatus::Unknown => {
                    Some("Keychain is unavailable. Check AI settings and retry.".into())
                }
                _ => None,
            },
            None => Some("Select an installed CLI or add an API key in AI settings.".into()),
            _ => None,
        };
        let reset_hover = Hover::new("reset-commit-prompt", window, cx);
        let variables: Vec<_> = VARIABLES
            .into_iter()
            .enumerate()
            .map(|(index, (name, description))| {
                let hover = Hover::new(("commit-variable", index), window, cx);
                Button::new(("commit-variable", index))
                    .cursor_pointer()
                    .small()
                    .label(name)
                    .tooltip(description)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.prompt.update(cx, |input, cx| {
                            input.focus(window, cx);
                            input.insert(name, window, cx);
                        });
                        this.save(cx);
                        cx.notify();
                    }))
                    .smooth_hover(ButtonLook::Ghost, &hover, cx)
            })
            .collect();
        let control = self.backend_control(generator.as_ref(), cx);
        v_flex()
            .gap_4()
            .p_4()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_lg()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Commit message"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Generate a commit message from staged changes."),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Select::new(&self.source)
                            .flex_1()
                            .min_w_0()
                            .placeholder("Select a generator")
                            .accessibility_label("Commit message generator")
                            .cleanable(true),
                    )
                    .child(
                        Button::new("refresh-commit-generators")
                            .cursor_pointer()
                            .small()
                            .ghost()
                            .icon(Lucide::RefreshCw)
                            .tooltip("Refresh generators")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .when_some(notice, |col, notice| {
                col.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(notice),
                )
            })
            .child(control)
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child("Prompt template")
                    .child(
                        Button::new("reset-commit-prompt")
                            .cursor_pointer()
                            .small()
                            .icon(Lucide::RotateCcw)
                            .label("Reset prompt")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.prompt.update(cx, |input, cx| {
                                    input.set_value(DEFAULT_PROMPT, window, cx)
                                });
                                this.save(cx);
                                cx.notify();
                            }))
                            .smooth_hover(ButtonLook::Ghost, &reset_hover, cx),
                    ),
            )
            .child(TextField::textarea(
                Textarea::new(&self.prompt).aria_label("Commit message prompt template"),
                &self.prompt,
                cx,
            ))
            .when_some(self.prompt_error.clone(), |col, error| {
                col.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Variables"),
                    )
                    .children(variables),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Changes save automatically when you finish editing."),
            )
    }
}
