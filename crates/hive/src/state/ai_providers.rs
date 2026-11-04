//! App-owned credential operations outlive the Settings window; secrets are not cached here.

use std::collections::BTreeMap;

use gpui_kit::*;
use hive_core::llm::{self, ApiKey, Provider};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyStatus {
    Unknown,
    Checking,
    Missing,
    Configured,
    Saving,
    Deleting,
}

impl KeyStatus {
    pub(crate) fn is_busy(self) -> bool {
        matches!(self, Self::Checking | Self::Saving | Self::Deleting)
    }
}

pub(crate) struct ProviderState {
    pub(crate) status: KeyStatus,
    pub(crate) error: Option<SharedString>,
    task: Option<Task<()>>,
}

enum Operation {
    Check,
    Save(ApiKey),
    Delete,
}

pub(crate) struct AiProviders {
    providers: BTreeMap<Provider, ProviderState>,
}

impl AiProviders {
    pub(crate) fn new(_: &mut Context<Self>) -> Self {
        Self {
            providers: Provider::ALL
                .into_iter()
                .map(|provider| {
                    (
                        provider,
                        ProviderState {
                            status: KeyStatus::Unknown,
                            error: None,
                            task: None,
                        },
                    )
                })
                .collect(),
        }
    }

    pub(crate) fn provider(&self, provider: Provider) -> &ProviderState {
        self.providers
            .get(&provider)
            .expect("every Provider has an entry")
    }

    fn provider_mut(&mut self, provider: Provider) -> &mut ProviderState {
        self.providers
            .get_mut(&provider)
            .expect("every Provider has an entry")
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        for provider in Provider::ALL {
            self.check(provider, cx);
        }
    }

    pub(crate) fn check(&mut self, provider: Provider, cx: &mut Context<Self>) {
        self.run(provider, Operation::Check, cx);
    }

    pub(crate) fn save(&mut self, provider: Provider, value: String, cx: &mut Context<Self>) {
        if self.provider(provider).status != KeyStatus::Missing {
            return;
        }
        match ApiKey::try_from(value) {
            Ok(key) => self.run(provider, Operation::Save(key), cx),
            Err(error) => {
                self.provider_mut(provider).error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    pub(crate) fn delete(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if self.provider(provider).status == KeyStatus::Configured {
            self.run(provider, Operation::Delete, cx);
        }
    }

    fn run(&mut self, provider: Provider, operation: Operation, cx: &mut Context<Self>) {
        let entry = self.provider_mut(provider);
        if entry.status.is_busy() {
            return;
        }
        let previous = entry.status;
        entry.status = match &operation {
            Operation::Check => KeyStatus::Checking,
            Operation::Save(_) => KeyStatus::Saving,
            Operation::Delete => KeyStatus::Deleting,
        };
        entry.error = None;
        entry.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match operation {
                        Operation::Check => llm::has_key(provider),
                        Operation::Save(key) => {
                            llm::verify_and_save_key(provider, &key).map(|()| true)
                        }
                        Operation::Delete => llm::delete_key(provider).map(|()| false),
                    }
                })
                .await;
            if let Err(error) = &result {
                tracing::warn!(%provider, %error, "AI credential operation failed");
            }
            let _ = this.update(cx, |this, cx| {
                let entry = this.provider_mut(provider);
                match result {
                    Ok(configured) => {
                        entry.status = if configured {
                            KeyStatus::Configured
                        } else {
                            KeyStatus::Missing
                        }
                    }
                    Err(error) => {
                        entry.status = previous;
                        entry.error = Some(error.to_string().into());
                    }
                }
                entry.task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}
