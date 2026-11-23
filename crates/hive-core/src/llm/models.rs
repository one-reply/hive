use super::{Provider, RequestError, request::Client};
use serde::Deserialize;
use std::{collections::HashSet, time::Duration};

/// A model offered by a provider. IDs are opaque outside its API adapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    /// Stable identifier passed back to the provider.
    pub id: String,
    /// Human-readable name, falling back to the identifier.
    pub name: String,
    /// What the model can be asked to do.
    pub capability: Capability,
}

impl Model {
    /// Whether `generate` can use it: a plain text prompt in, text out.
    pub fn generates_text(&self) -> bool {
        self.capability != Capability::Audio
    }

    /// Whether `transcribe` can use it: a recording in, text out.
    pub fn accepts_audio(&self) -> bool {
        self.capability != Capability::Text
    }
}

/// What a listed model can be asked to do. A model fit for neither use is not listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    /// Text generation only.
    Text,
    /// Takes a recording, and is not offered for text generation: an audio-only family.
    Audio,
    /// Text generation that also takes a recording.
    TextAndAudio,
}

/// Families of the catalogs filtered by name that neither generate text nor take a recording
/// in a chat request.
const SPECIALIZED: [&str; 12] = [
    "embedding",
    "whisper",
    "tts",
    "realtime",
    "transcribe",
    "moderation",
    "dall-e",
    "image",
    "sora",
    "veo",
    "imagen",
    "aqa",
];

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
    let id = if provider == Provider::GoogleAi {
        entry.name.trim_start_matches("models/").to_owned()
    } else {
        entry.id
    };
    if id.is_empty() || id.chars().any(char::is_control) {
        return None;
    }
    let capability = capability(provider, &id, entry.architecture.as_ref())?;
    let name = if !entry.display_name.is_empty() {
        entry.display_name
    } else if provider == Provider::OpenRouter && !entry.name.is_empty() {
        entry.name
    } else {
        id.clone()
    };
    Some(Model {
        id,
        name,
        capability,
    })
}

fn capability(
    provider: Provider,
    id: &str,
    architecture: Option<&Architecture>,
) -> Option<Capability> {
    if provider.uses_file_transcription(id) {
        return Some(Capability::Audio);
    }
    if let Some(architecture) = architecture
        && (!architecture.input_modalities.iter().any(|m| m == "text")
            || !architecture.output_modalities.iter().any(|m| m == "text"))
    {
        return None;
    }
    let listens =
        |architecture: &Architecture| architecture.input_modalities.iter().any(|m| m == "audio");
    Some(match provider {
        Provider::OpenRouter if architecture.is_some_and(listens) => Capability::TextAndAudio,
        Provider::OpenRouter | Provider::Anthropic => Capability::Text,
        // ponytail: neither OpenAI nor Google reports modalities, so names decide from here on
        // ("audio" in an OpenAI id, a `gemini-` prefix at Google) and a family named otherwise
        // is misfiled. Read the modalities from the catalog once these APIs return them.
        Provider::OpenAi | Provider::GoogleAi if SPECIALIZED.iter().any(|s| id.contains(s)) => {
            return None;
        }
        // `gpt-audio`, `gpt-4o-audio-preview`: chat models that answer a recording.
        Provider::OpenAi if id.contains("audio") => Capability::Audio,
        Provider::OpenAi => Capability::Text,
        // Native-audio Gemini models hold live conversations instead of answering a request.
        Provider::GoogleAi if id.contains("audio") => return None,
        // Gemma lists `generateContent` too, but takes no audio.
        Provider::GoogleAi if id.starts_with("gemini-") => Capability::TextAndAudio,
        Provider::GoogleAi => Capability::Text,
    })
}

#[cfg(test)]
mod tests;
