//! Commit recipe form. IO and persistence belong to AiProviders / Store.
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
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::Button,
        h_flex,
        input::{Input, InputEvent, InputState, Textarea, TextareaState},
        searchable_list::SearchableVec,
        select::{Select, SelectEvent, SelectState},
        v_flex,
    },
    prelude::FluentBuilder as _,
    *,
};
use hive_core::commit_message::{DEFAULT_PROMPT, Generator, VARIABLES};

mod choices;
mod view;
use choices::{SourceItem, sources};

const PROMPT_ROWS: usize = 9;

pub(super) struct GitPage {
    providers: Entity<AiProviders>,
    source: Entity<SelectState<SearchableVec<SourceItem>>>,
    model: Entity<SelectState<SearchableVec<String>>>,
    arguments: Entity<InputState>,
    prompt: Entity<TextareaState>,
    argument_error: Option<SharedString>,
    prompt_error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl GitPage {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let providers = AppState::get(cx).ai_providers.clone();
        let recipe = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .commit_message
            .clone();
        let source = cx
            .new(|cx| SelectState::new(SearchableVec::<SourceItem>::new(vec![]), None, window, cx));
        let model = cx.new(|cx| {
            SelectState::new(SearchableVec::<String>::new(vec![]), None, window, cx)
                .searchable(true)
        });
        let arguments = cx.new(|cx| InputState::new(window, cx).placeholder("--model …"));
        let prompt = cx.new(|cx| TextareaState::new(window, cx).rows(PROMPT_ROWS));
        if let Some(Generator::Cli {
            arguments: value, ..
        }) = recipe.generator()
        {
            arguments.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
        }
        prompt.update(cx, |input, cx| {
            input.set_value(recipe.prompt().to_owned(), window, cx)
        });
        let subscriptions = vec![
            cx.observe_in(&providers, window, |this, _, window, cx| {
                this.sync(window, cx);
                cx.notify();
            }),
            cx.subscribe_in(&source, window, |this, _, event, window, cx| {
                let SelectEvent::Confirm(generator) = event;
                this.save(cx);
                let result = AppState::get(cx).store.clone().update(cx, |store, cx| {
                    store.set_commit_generator(generator.clone(), cx)
                });
                this.argument_error = result.err().map(|e| e.to_string().into());
                let value = match generator {
                    Some(Generator::Cli { arguments, .. }) => arguments.clone(),
                    _ => String::new(),
                };
                this.arguments
                    .update(cx, |input, cx| input.set_value(value, window, cx));
                this.sync(window, cx);
                cx.notify();
            }),
            cx.subscribe(&model, |this, _, event, cx| {
                let SelectEvent::Confirm(model) = event;
                let store = AppState::get(cx).store.clone();
                let generator = store
                    .read(cx)
                    .state
                    .settings
                    .commit_message
                    .generator()
                    .cloned();
                if let Some(Generator::Api { provider, .. }) = generator
                    && let Err(error) = store.update(cx, |store, cx| {
                        store.set_commit_generator(
                            Some(Generator::Api {
                                provider,
                                model: model.clone(),
                            }),
                            cx,
                        )
                    })
                {
                    this.argument_error = Some(error.to_string().into());
                }
                cx.notify();
            }),
            cx.subscribe(&arguments, |this, _, event, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    this.save(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&prompt, |this, _, event, cx| {
                if matches!(event, InputEvent::Blur) {
                    this.save(cx);
                    cx.notify();
                }
            }),
            cx.on_release(|this, cx| this.save(cx)),
        ];
        let mut page = Self {
            providers,
            source,
            model,
            arguments,
            prompt,
            argument_error: None,
            prompt_error: None,
            _subscriptions: subscriptions,
        };
        page.sync(window, cx);
        page
    }

    pub(super) fn save(&mut self, cx: &mut App) {
        let store = AppState::get(cx).store.clone();
        let generator = store
            .read(cx)
            .state
            .settings
            .commit_message
            .generator()
            .cloned();
        if let Some(Generator::Cli { agent, .. }) = generator {
            let arguments = self.arguments.read(cx).value().to_string();
            self.argument_error = store
                .update(cx, |store, cx| {
                    store.set_commit_generator(Some(Generator::Cli { agent, arguments }), cx)
                })
                .err()
                .map(|e| e.to_string().into());
        }
        let prompt = self.prompt.read(cx).value().to_string();
        self.prompt_error = store
            .update(cx, |store, cx| store.set_commit_prompt(prompt, cx))
            .err()
            .map(|e| e.to_string().into());
    }

    pub(super) fn refresh(&self, cx: &mut Context<Self>) {
        self.providers
            .update(cx, |providers, cx| providers.refresh(cx));
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let generator = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .commit_message
            .generator()
            .cloned();
        let items = sources(self.providers.read(cx), generator.as_ref());
        self.source.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(items), window, cx);
            if let Some(generator) = &generator {
                select.set_selected_value(generator, window, cx);
            }
        });
        if let Some(Generator::Api { provider, model }) = generator {
            let mut items = match &self.providers.read(cx).provider(provider).models {
                Models::Ready(models) => models.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            // Keep a saved ID visible even when catalog loading fails or the model was retired.
            if let Some(model) = &model
                && !items.contains(model)
            {
                items.insert(0, model.clone());
            }
            self.model.update(cx, |select, cx| {
                select.set_items(SearchableVec::new(items), window, cx);
                if let Some(model) = &model {
                    select.set_selected_value(model, window, cx);
                } else {
                    select.set_selected_index(None, window, cx);
                }
            });
            self.providers.update(cx, |providers, cx| {
                providers.load_models(provider, false, cx)
            });
        }
    }
}
