use super::{AUDIO_PROVIDERS, Provider, RequestError, request::Client};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

/// A recording is uploaded whole and read in one pass, which is slower than a text prompt.
const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(180);
/// Output limit of a transcription; the longest recording is a few thousand tokens of text.
const TRANSCRIPT_TOKENS: u32 = 8192;

/// Reply envelopes of the provider APIs. OpenAI answers `generate` through Responses and
/// `transcribe` through Chat Completions, so the provider alone does not name the parser.
#[derive(Clone, Copy)]
enum Reply {
    Responses,
    Messages,
    ChatCompletion,
    GenerateContent,
}

impl Reply {
    /// The envelope each provider answers `generate` with.
    fn for_text(provider: Provider) -> Self {
        match provider {
            Provider::OpenAi => Self::Responses,
            Provider::Anthropic => Self::Messages,
            Provider::OpenRouter => Self::ChatCompletion,
            Provider::GoogleAi => Self::GenerateContent,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Response {
    status: String,
    output: Vec<Output>,
    content: Vec<Block>,
    stop_reason: Option<String>,
    choices: Vec<Choice>,
    candidates: Vec<Candidate>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Output {
    #[serde(rename = "type")]
    kind: String,
    content: Vec<Block>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    text: String,
    thought: bool,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Choice {
    message: ChatMessage,
    finish_reason: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct ChatMessage {
    content: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Candidate {
    content: Content,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct Content {
    parts: Vec<Block>,
}

/// Blocking: generates a single text response without tools, streaming or conversation history.
pub fn generate(provider: Provider, model: &str, prompt: &str) -> Result<String, RequestError> {
    let client = Client::new(provider, Duration::from_secs(120))?;
    let (url, body) = request(provider, model, prompt)?;
    let response: Response = client.post(&url, &body)?;
    extract(provider, Reply::for_text(provider), response)
}

/// Blocking: turns a WAV recording into text in one request, guided by `prompt`.
pub fn transcribe(
    provider: Provider,
    model: &str,
    prompt: &str,
    wav: &[u8],
) -> Result<String, RequestError> {
    // Built before the key is read: a provider or model that cannot answer costs no Keychain
    // access and no request.
    let (url, body, reply) = audio_request(provider, model, prompt, wav)?;
    let client = Client::new(provider, TRANSCRIBE_TIMEOUT)?;
    transcription(&client, &url, &body, reply)
}

fn transcription(
    client: &Client,
    url: &str,
    body: &serde_json::Value,
    reply: Reply,
) -> Result<String, RequestError> {
    let response: Response = client.post(url, body)?;
    // `extract` fails only on an unfinished or empty reply. For a recording that means silence
    // or a cut-off transcript, so `generate`'s advice to shorten the prompt would mislead.
    extract(client.provider, reply, response).map_err(|_| RequestError::Response {
        provider: client.provider,
        message: "No transcription was returned. Check the selected model or record again.",
    })
}

fn audio_request(
    provider: Provider,
    model: &str,
    prompt: &str,
    wav: &[u8],
) -> Result<(String, serde_json::Value, Reply), RequestError> {
    if !AUDIO_PROVIDERS.contains(&provider) {
        return Err(RequestError::Response {
            provider,
            message: "This provider does not accept audio.",
        });
    }
    let audio = STANDARD.encode(wav);
    if provider == Provider::GoogleAi {
        return Ok((
            google_url(model)?,
            json!({
                "contents":[{"role":"user","parts":[
                    {"text":prompt},
                    {"inlineData":{"mimeType":"audio/wav","data":audio}},
                ]}],
                "generationConfig":{"maxOutputTokens":TRANSCRIPT_TOKENS},
            }),
            Reply::GenerateContent,
        ));
    }
    let messages = json!([{"role":"user","content":[
        {"type":"text","text":prompt},
        {"type":"input_audio","input_audio":{"data":audio,"format":"wav"}},
    ]}]);
    let (url, body) = if provider == Provider::OpenAi {
        (
            "https://api.openai.com/v1/chat/completions",
            // Audio models can also speak; only the text is wanted. Recordings are not kept
            // for OpenAI's distillation or evals.
            json!({"model":model,"messages":messages,"modalities":["text"],"store":false,"max_completion_tokens":TRANSCRIPT_TOKENS}),
        )
    } else {
        (
            "https://openrouter.ai/api/v1/chat/completions",
            json!({"model":model,"messages":messages,"max_tokens":TRANSCRIPT_TOKENS}),
        )
    };
    Ok((url.into(), body, Reply::ChatCompletion))
}

/// The id becomes a path segment, so anything that could alter the path or query is refused.
fn google_url(model: &str) -> Result<String, RequestError> {
    if model.is_empty()
        || !model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(invalid(Provider::GoogleAi));
    }
    Ok(format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
    ))
}

fn request(
    provider: Provider,
    model: &str,
    prompt: &str,
) -> Result<(String, serde_json::Value), RequestError> {
    let messages = json!([{"role":"user","content":prompt}]);
    let (url, body) = match provider {
        Provider::OpenAi => (
            "https://api.openai.com/v1/responses".into(),
            json!({"model":model,"input":prompt,"store":false,"max_output_tokens":4096}),
        ),
        Provider::Anthropic => (
            "https://api.anthropic.com/v1/messages".into(),
            json!({"model":model,"messages":messages,"max_tokens":4096}),
        ),
        Provider::OpenRouter => (
            "https://openrouter.ai/api/v1/chat/completions".into(),
            json!({"model":model,"messages":messages,"max_tokens":4096}),
        ),
        Provider::GoogleAi => (
            google_url(model)?,
            json!({"contents":[{"role":"user","parts":[{"text":prompt}]}],"generationConfig":{"maxOutputTokens":4096}}),
        ),
    };
    Ok((url, body))
}

fn invalid(provider: Provider) -> RequestError {
    RequestError::Response {
        provider,
        message: "No complete text response was returned. Check the selected model or shorten the prompt and try again.",
    }
}

fn extract(provider: Provider, reply: Reply, response: Response) -> Result<String, RequestError> {
    let text = match reply {
        Reply::Responses if response.status == "completed" => response
            .output
            .into_iter()
            .filter(|o| o.kind == "message")
            .flat_map(|o| o.content)
            .filter(|c| c.kind == "output_text")
            .map(|c| c.text)
            .collect::<Vec<_>>()
            .join("\n"),
        Reply::Messages if response.stop_reason.as_deref() == Some("end_turn") => response
            .content
            .into_iter()
            .filter(|c| c.kind == "text")
            .map(|c| c.text)
            .collect::<Vec<_>>()
            .join("\n"),
        Reply::ChatCompletion => {
            let choice = response
                .choices
                .into_iter()
                .next()
                .ok_or_else(|| invalid(provider))?;
            if choice.finish_reason.as_deref() != Some("stop") {
                return Err(invalid(provider));
            }
            choice.message.content.unwrap_or_default()
        }
        Reply::GenerateContent => {
            let candidate = response
                .candidates
                .into_iter()
                .next()
                .ok_or_else(|| invalid(provider))?;
            if candidate.finish_reason.as_deref() != Some("STOP") {
                return Err(invalid(provider));
            }
            candidate
                .content
                .parts
                .into_iter()
                .filter(|p| !p.thought)
                .map(|p| p.text)
                .collect::<Vec<_>>()
                .join("")
        }
        _ => return Err(invalid(provider)),
    };
    if text.trim().is_empty() {
        return Err(invalid(provider));
    }
    Ok(text)
}

#[cfg(test)]
mod tests;
