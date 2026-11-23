//! Dictation settings: the model that turns a recorded task into text, and its prompt.
//! Keys and model catalogs belong to `AiProviders`, the choice to `Store`.
use crate::{
    app::AppState,
    state::ai_providers::{AiProviders, KeyStatus, Models},
    ui::{
        hover::{ButtonLook, Hover, SmoothButton as _},
        text_field::TextField,
    },
};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::{
    assets::IconName as Lucide,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Sizable as _,
        button::Button,
        h_flex,
        input::{InputEvent, Textarea, TextareaState},
        searchable_list::{SearchableListItem, SearchableVec},
        select::{Select, SelectEvent, SelectState},
        v_flex,
    },
    prelude::FluentBuilder as _,
    *,
};
use hive_core::dictation::{DEFAULT_PROMPT, LANGUAGES, Recipe};
use hive_core::llm::{AUDIO_PROVIDERS, Provider};

/// Enough of the prompt is visible to read its rules without scrolling the field.
const PROMPT_ROWS: usize = 9;

/// A provider that takes audio, as the picker lists it.
#[derive(Clone, IntoElement)]
struct ProviderItem {
    provider: Provider,
    status: &'static str,
    available: bool,
}

impl SearchableListItem for ProviderItem {
    type Value = Provider;
    fn title(&self) -> SharedString {
        format!("{}{}", self.provider.name(), self.status).into()
    }
    fn display_title(&self) -> Option<AnyElement> {
        Some(self.clone().into_any_element())
    }
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.clone()
    }
    fn value(&self) -> &Provider {
        &self.provider
    }
    fn disabled(&self) -> bool {
        !self.available
    }
}

impl RenderOnce for ProviderItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .min_w_0()
            .items_center()
            .gap_2()
            .child(
                Icon::default()
                    .data(super::super::ai::logo(self.provider))
                    .size_4()
                    .flex_none(),
            )
            .child(div().min_w_0().truncate().child(self.provider.name()))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.status),
            )
    }
}

/// The providers to offer: those with a key, and the saved one even when its key is gone,
/// so the picker never shows an empty selection for a saved choice.
fn provider_items(providers: &AiProviders, saved: Option<Provider>) -> Vec<ProviderItem> {
    AUDIO_PROVIDERS
        .into_iter()
        .filter_map(|provider| {
            let (available, status) =
                super::super::ai::availability(providers.provider(provider).status);
            (available || saved == Some(provider)).then_some(ProviderItem {
                provider,
                status,
                available,
            })
        })
        .collect()
}

/// The models to offer: those of the catalog that take a recording, and the saved one even
/// when the catalog failed to load or no longer lists it.
fn model_items(models: &Models, saved: Option<&str>) -> Vec<String> {
    let mut items: Vec<String> = match models {
        Models::Ready(models) => models
            .iter()
            .filter(|model| model.accepts_audio())
            .map(|model| model.id.clone())
            .collect(),
        _ => Vec::new(),
    };
    if let Some(saved) = saved
        && !items.iter().any(|id| id == saved)
    {
        items.insert(0, saved.to_owned());
    }
    items
}

pub(super) struct DictationCard {
    providers: Entity<AiProviders>,
    provider: Entity<SelectState<SearchableVec<ProviderItem>>>,
    model: Entity<SelectState<SearchableVec<String>>>,
    language: Entity<SelectState<SearchableVec<String>>>,
    prompt: Entity<TextareaState>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

/// The saved dictation settings.
fn recipe(cx: &App) -> &Recipe {
    &AppState::get(cx).store.read(cx).state.settings.dictation
}

/// Changes the saved dictation settings; `Store` writes them when they differ.
fn edit<T>(cx: &mut App, change: impl FnOnce(&mut Recipe) -> T) -> T {
    let store = AppState::get(cx).store.clone();
    store.update(cx, |store, cx| store.edit_dictation(cx, change))
}

impl DictationCard {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let providers = AppState::get(cx).ai_providers.clone();
        let provider = cx.new(|cx| {
            SelectState::new(SearchableVec::<ProviderItem>::new(vec![]), None, window, cx)
        });
        let model = cx.new(|cx| {
            SelectState::new(SearchableVec::<String>::new(vec![]), None, window, cx)
                .searchable(true)
        });
        let language = cx.new(|cx| {
            let languages = LANGUAGES.map(str::to_owned).to_vec();
            let mut select =
                SelectState::new(SearchableVec::new(languages), None, window, cx).searchable(true);
            if let Some(language) = recipe(cx).language() {
                select.set_selected_value(&language.to_owned(), window, cx);
            }
            select
        });
        let prompt = cx.new(|cx| TextareaState::new(window, cx).rows(PROMPT_ROWS));
        let saved = recipe(cx).prompt().to_owned();
        prompt.update(cx, |input, cx| input.set_value(saved, window, cx));
        let subscriptions = vec![
            cx.observe_in(&providers, window, |card, _, window, cx| {
                card.sync(window, cx);
                cx.notify();
            }),
            cx.subscribe_in(&provider, window, |card, _, event, window, cx| {
                let SelectEvent::Confirm(provider) = event;
                card.error = edit(cx, |recipe| recipe.set_provider(*provider))
                    .err()
                    .map(|error| error.to_string().into());
                card.sync(window, cx);
                cx.notify();
            }),
            cx.subscribe(&model, |_, _, event, cx| {
                let SelectEvent::Confirm(model) = event;
                edit(cx, |recipe| recipe.set_model(model.clone()));
                cx.notify();
            }),
            cx.subscribe(&language, |card, _, event, cx| {
                let SelectEvent::Confirm(language) = event;
                card.error = edit(cx, |recipe| recipe.set_language(language.clone()))
                    .err()
                    .map(|error| error.to_string().into());
                cx.notify();
            }),
            cx.subscribe(&prompt, |card, _, event, cx| {
                if matches!(event, InputEvent::Blur) {
                    card.save_prompt(cx);
                    cx.notify();
                }
            }),
            cx.on_release(|card, cx| card.save_prompt(cx)),
        ];
        let mut card = Self {
            providers,
            provider,
            model,
            language,
            prompt,
            error: None,
            _subscriptions: subscriptions,
        };
        card.sync(window, cx);
        card
    }

    /// Re-reads which providers have a key: it changes on the AI page, outside this card.
    pub(super) fn refresh(&self, cx: &mut Context<Self>) {
        self.providers.update(cx, |providers, cx| {
            for provider in AUDIO_PROVIDERS {
                providers.check(provider, cx);
            }
        });
    }

    fn save_prompt(&mut self, cx: &mut App) {
        let prompt = self.prompt.read(cx).value().to_string();
        self.error = edit(cx, |recipe| recipe.set_prompt(prompt))
            .err()
            .map(|error| error.to_string().into());
    }

    /// Fills both pickers from the saved choice and what the providers offer now.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = recipe(cx).clone();
        let items = provider_items(self.providers.read(cx), saved.provider());
        self.provider.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(items), window, cx);
            match saved.provider() {
                Some(provider) => select.set_selected_value(&provider, window, cx),
                None => select.set_selected_index(None, window, cx),
            }
        });
        let Some(provider) = saved.provider() else {
            return;
        };
        let items = model_items(
            &self.providers.read(cx).provider(provider).models,
            saved.model(),
        );
        self.model.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(items), window, cx);
            match saved.model() {
                Some(model) => select.set_selected_value(&model.to_owned(), window, cx),
                None => select.set_selected_index(None, window, cx),
            }
        });
        self.providers.update(cx, |providers, cx| {
            providers.load_models(provider, false, cx)
        });
    }

    fn model_control(
        &self,
        provider: Provider,
        saved: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = self.providers.read(cx).provider(provider);
        let configured = state.status == KeyStatus::Configured;
        let loading = matches!(state.models, Models::Loading) || state.status.is_busy();
        let listed = |id: &str| match &state.models {
            Models::Ready(models) => models.iter().any(|m| m.id == id && m.accepts_audio()),
            _ => true,
        };
        let error: Option<SharedString> = match &state.models {
            Models::Failed(error) => Some(error.clone()),
            Models::Ready(models) if !models.iter().any(|m| m.accepts_audio()) => {
                Some("This provider lists no model that takes audio.".into())
            }
            _ if saved.is_some_and(|id| !listed(id)) => Some(
                "Saved model is no longer listed. Select another model if dictation fails.".into(),
            ),
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
                            .accessibility_label("Dictation model")
                            .disabled(loading || !configured),
                    )
                    .child(
                        Button::new("reload-dictation-models")
                            .cursor_pointer()
                            .small()
                            .ghost()
                            .icon(Lucide::RefreshCw)
                            .loading(loading)
                            .disabled(loading || !configured)
                            .tooltip("Reload models / Retry")
                            .on_click(cx.listener(move |card, _, _, cx| {
                                card.providers.update(cx, |providers, cx| {
                                    providers.load_models(provider, true, cx)
                                })
                            })),
                    ),
            )
            .when_some(error, |column, error| {
                column.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
    }
}

impl Render for DictationCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let saved = recipe(cx).clone();
        let providers = self.providers.read(cx);
        let notice: Option<&'static str> = match saved.provider() {
            None => Some("Choose a provider that has an API key in AI settings."),
            Some(provider) if providers.provider(provider).status == KeyStatus::Missing => {
                Some("Selected provider has no API key. Add it in AI settings.")
            }
            Some(_) if saved.model().is_none() => Some("Choose a model to turn dictation on."),
            _ => None,
        };
        let reset_hover = Hover::new("reset-dictation-prompt", window, cx);
        let muted = cx.theme().muted_foreground;
        let model = saved
            .provider()
            .map(|provider| self.model_control(provider, saved.model(), cx));
        let file_transcription = saved
            .provider()
            .zip(saved.model())
            .is_some_and(|(provider, model)| provider.uses_file_transcription(model));
        let theme = cx.theme();
        v_flex()
            .gap_4()
            .p_4()
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Dictation"))
                    .child(div().text_sm().text_color(muted).child(
                        "Record a task with the microphone in New run. The selected model \
                         transcribes the recording into the Task field.",
                    )),
            )
            .child(
                v_flex().gap_2().child("Provider").child(
                    Select::new(&self.provider)
                        .placeholder("Select a provider")
                        .accessibility_label("Dictation provider")
                        .cleanable(true),
                ),
            )
            .when_some(notice, |card, notice| {
                card.child(div().text_sm().text_color(muted).child(notice))
            })
            .children(model)
            .child(
                v_flex()
                    .gap_2()
                    .child("Language")
                    .child(
                        Select::new(&self.language)
                            .placeholder("Detect automatically")
                            .search_placeholder("Search languages…")
                            .accessibility_label("Dictation language")
                            .cleanable(true),
                    )
                    .child(div().text_xs().text_color(muted).child(
                        "Name the language you dictate in: a model left to detect it mistakes \
                         close languages for each other.",
                    )),
            )
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child("Prompt")
                    .child(
                        Button::new("reset-dictation-prompt")
                            .cursor_pointer()
                            .small()
                            .icon(Lucide::RotateCcw)
                            .label("Reset prompt")
                            .on_click(cx.listener(|card, _, window, cx| {
                                card.prompt.update(cx, |input, cx| {
                                    input.set_value(DEFAULT_PROMPT, window, cx)
                                });
                                card.save_prompt(cx);
                                cx.notify();
                            }))
                            .smooth_hover(ButtonLook::Ghost, &reset_hover, cx),
                    ),
            )
            .child(TextField::textarea(
                Textarea::new(&self.prompt).aria_label("Dictation prompt"),
                &self.prompt,
                cx,
            ))
            .when(file_transcription, |card| {
                card.child(div().text_xs().text_color(muted).child(
                    "Use the prompt to give context, such as project names and technical terms you expect to say.",
                ))
            })
            .when_some(self.error.clone(), |card, error| {
                card.child(div().text_sm().text_color(theme.danger).child(error))
            })
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Changes save automatically when you finish editing."),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::model_items;
    use crate::state::ai_providers::Models;
    use hive_core::llm::{Capability, Model};

    fn model(id: &str, capability: Capability) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            capability,
        }
    }

    #[test]
    fn only_models_that_take_audio_are_offered() {
        let models = Models::Ready(vec![
            model("text-only", Capability::Text),
            model("audio-only", Capability::Audio),
            model("both", Capability::TextAndAudio),
        ]);
        assert_eq!(model_items(&models, None), ["audio-only", "both"]);
    }

    #[test]
    fn a_saved_model_stays_offered_when_the_catalog_lacks_it_or_did_not_load() {
        let models = Models::Ready(vec![model("both", Capability::TextAndAudio)]);
        assert_eq!(model_items(&models, Some("retired")), ["retired", "both"]);
        assert_eq!(model_items(&models, Some("both")), ["both"]);
        assert_eq!(
            model_items(&Models::Failed("offline".into()), Some("both")),
            ["both"]
        );
        assert!(model_items(&Models::Loading, None).is_empty());
    }
}
