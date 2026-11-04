use super::{Provider, RequestError, request::Client};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

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
    extract(provider, response)
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
        Provider::GoogleAi => {
            if model.is_empty()
                || !model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err(invalid(provider));
            }
            (
                format!(
                    "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
                ),
                json!({"contents":[{"role":"user","parts":[{"text":prompt}]}],"generationConfig":{"maxOutputTokens":4096}}),
            )
        }
    };
    Ok((url, body))
}

fn invalid(provider: Provider) -> RequestError {
    RequestError::Response {
        provider,
        message: "No complete text response was returned. Check the selected model or shorten the prompt and try again.",
    }
}

fn extract(provider: Provider, response: Response) -> Result<String, RequestError> {
    let text = match provider {
        Provider::OpenAi if response.status == "completed" => response
            .output
            .into_iter()
            .filter(|o| o.kind == "message")
            .flat_map(|o| o.content)
            .filter(|c| c.kind == "output_text")
            .map(|c| c.text)
            .collect::<Vec<_>>()
            .join("\n"),
        Provider::Anthropic if response.stop_reason.as_deref() == Some("end_turn") => response
            .content
            .into_iter()
            .filter(|c| c.kind == "text")
            .map(|c| c.text)
            .collect::<Vec<_>>()
            .join("\n"),
        Provider::OpenRouter => {
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
        Provider::GoogleAi => {
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
