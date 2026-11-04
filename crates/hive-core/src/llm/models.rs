use super::{Provider, RequestError, request::Client};
use serde::Deserialize;
use std::{collections::HashSet, time::Duration};

/// A text model offered by a provider. IDs are opaque outside its API adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    /// Stable identifier passed back to the provider.
    pub id: String,
    /// Human-readable name, falling back to the identifier.
    pub name: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Page {
    data: Vec<Entry>,
    models: Vec<Entry>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Entry {
    id: String,
    name: String,
    #[serde(alias = "displayName")]
    display_name: String,
    #[serde(rename = "supportedGenerationMethods")]
    methods: Vec<String>,
    architecture: Option<Architecture>,
    lifecycle: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Architecture {
    input_modalities: Vec<String>,
    output_modalities: Vec<String>,
}

/// Blocking: fetches all model pages without doing inference. Keys are loaded on demand.
pub fn list_models(provider: Provider) -> Result<Vec<Model>, RequestError> {
    let client = Client::new(provider, Duration::from_secs(15))?;
    let url = match provider {
        Provider::GoogleAi => "https://generativelanguage.googleapis.com/v1beta/models",
        Provider::OpenAi => "https://api.openai.com/v1/models",
        Provider::OpenRouter => "https://openrouter.ai/api/v1/models",
        Provider::Anthropic => "https://api.anthropic.com/v1/models",
    };
    collect(&client, url)
}

fn collect(client: &Client, url: &str) -> Result<Vec<Model>, RequestError> {
    let mut cursor = None::<String>;
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    for _ in 0..100 {
        let mut query = Vec::new();
        match client.provider {
            Provider::GoogleAi => {
                query.push(("pageSize", "1000"));
                if let Some(cursor) = &cursor {
                    query.push(("pageToken", cursor));
                }
            }
            Provider::Anthropic => {
                query.push(("limit", "1000"));
                if let Some(cursor) = &cursor {
                    query.push(("after_id", cursor));
                }
            }
            _ => {}
        }
        let page: Page = client.get(url, &query)?;
        cursor = match client.provider {
            Provider::GoogleAi => page.next_page_token.filter(|s| !s.is_empty()),
            Provider::Anthropic if page.has_more => Some(
                page.last_id
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| client.invalid("Model list is missing its next-page cursor."))?,
            ),
            _ => None,
        };
        models.extend(
            page.data
                .into_iter()
                .chain(page.models)
                .filter_map(|entry| model(client.provider, entry)),
        );
        let Some(next) = &cursor else {
            models.sort_by(|a, b| a.id.cmp(&b.id));
            models.dedup_by(|a, b| a.id == b.id);
            return Ok(models);
        };
        if !seen.insert(next.clone()) {
            return Err(client.invalid("Model catalog repeated a page. Try again."));
        }
    }
    Err(client.invalid("Model catalog exceeds the page limit."))
}

fn model(provider: Provider, entry: Entry) -> Option<Model> {
    if entry.lifecycle.as_deref() == Some("retired") {
        return None;
    }
    if provider == Provider::GoogleAi && !entry.methods.iter().any(|m| m == "generateContent") {
        return None;
    }
    if let Some(architecture) = &entry.architecture
        && (!architecture.input_modalities.iter().any(|m| m == "text")
            || !architecture.output_modalities.iter().any(|m| m == "text"))
    {
        return None;
    }
    let id = if provider == Provider::GoogleAi {
        entry.name.trim_start_matches("models/").to_owned()
    } else {
        entry.id
    };
    if id.is_empty() || id.chars().any(char::is_control) {
        return None;
    }
    // shortcut: OpenAI exposes no modality metadata; exclude known specialized model families until it does.
    if matches!(provider, Provider::OpenAi | Provider::GoogleAi)
        && [
            "embedding",
            "whisper",
            "tts",
            "realtime",
            "transcribe",
            "audio",
            "moderation",
            "dall-e",
            "image",
            "sora",
            "veo",
            "imagen",
            "aqa",
        ]
        .iter()
        .any(|s| id.contains(s))
    {
        return None;
    }
    let name = if !entry.display_name.is_empty() {
        entry.display_name
    } else if provider == Provider::OpenRouter && !entry.name.is_empty() {
        entry.name
    } else {
        id.clone()
    };
    Some(Model { id, name })
}

#[cfg(test)]
mod tests;
