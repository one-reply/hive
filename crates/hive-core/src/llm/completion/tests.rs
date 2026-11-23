use super::*;
use crate::llm::{ApiKey, test_server::server};

// Provider response envelopes from their API references, with synthetic message text.
const SAMPLES: [(Provider, &str); 4] = [
    (
        Provider::OpenAi,
        r#"{"status":"completed","output":[{"type":"reasoning","summary":[]},{"type":"message","content":[{"type":"output_text","text":"feat: fixture"}]}]}"#,
    ),
    (
        Provider::Anthropic,
        r#"{"content":[{"type":"thinking","thinking":"private"},{"type":"text","text":"feat: fixture"}],"stop_reason":"end_turn"}"#,
    ),
    (
        Provider::OpenRouter,
        r#"{"choices":[{"message":{"role":"assistant","content":"feat: fixture","reasoning":"private"},"finish_reason":"stop"}]}"#,
    ),
    (
        Provider::GoogleAi,
        r#"{"candidates":[{"content":{"parts":[{"text":"private","thought":true},{"text":"feat: fixture"}],"role":"model"},"finishReason":"STOP"}]}"#,
    ),
];

#[test]
fn final_text_ignores_reasoning_and_unknown_fields() {
    for (provider, sample) in SAMPLES {
        let reply = Reply::for_text(provider);
        assert_eq!(
            extract(provider, reply, serde_json::from_str(sample).unwrap()).unwrap(),
            "feat: fixture"
        );
        assert!(extract(provider, reply, serde_json::from_str("{}").unwrap()).is_err());
    }
}

#[test]
fn truncated_refused_and_empty_responses_are_not_used() {
    for (provider, sample) in SAMPLES {
        let incomplete = sample
            .replace("completed", "incomplete")
            .replace("end_turn", "max_tokens")
            .replace("\"stop\"", "\"length\"")
            .replace("STOP", "MAX_TOKENS");
        let reply = Reply::for_text(provider);
        assert!(extract(provider, reply, serde_json::from_str(&incomplete).unwrap()).is_err());
        assert!(
            extract(
                provider,
                reply,
                serde_json::from_str(&sample.replace("feat: fixture", "")).unwrap()
            )
            .is_err()
        );
    }
    assert!(extract(Provider::OpenAi, Reply::Responses, serde_json::from_str(r#"{"status":"completed","output":[{"type":"message","content":[{"type":"refusal","refusal":"no"}]}]}"#).unwrap()).is_err());
}

#[test]
fn request_bodies_use_selected_models_without_history_or_tools() {
    for provider in Provider::ALL {
        let (url, body) = request(provider, "fixture-model", "staged patch").unwrap();
        assert!(url.starts_with("https://"));
        assert!(body.get("tools").is_none());
        if provider == Provider::OpenAi {
            assert_eq!(body["store"], false);
        }
        if provider != Provider::GoogleAi {
            assert_eq!(body["model"], "fixture-model");
        }
        assert!(body.to_string().contains("staged patch"));
    }
    assert!(request(Provider::GoogleAi, "bad/path?key=secret", "text").is_err());
}

// The chat-completion and generateContent envelopes above are also what an audio request is
// answered with; OpenAI replies to it in the first one, unlike for `generate`.
fn transcript_sample(provider: Provider) -> String {
    let envelope = if provider == Provider::GoogleAi {
        Provider::GoogleAi
    } else {
        Provider::OpenRouter
    };
    let (_, sample) = SAMPLES.iter().find(|(p, _)| *p == envelope).unwrap();
    sample.replace("feat: fixture", "open `main.rs`")
}

fn client(provider: Provider) -> Client {
    Client::with_key(
        provider,
        ApiKey::try_from("fixture-secret".to_owned()).unwrap(),
        Duration::from_secs(2),
    )
}

fn posted_body(request: &str) -> serde_json::Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[test]
fn audio_requests_carry_the_prompt_and_base64_wav_for_each_audio_provider() {
    let wav = b"RIFF\x00\xffWAVE recording";
    let audio = STANDARD.encode(wav);
    for provider in AUDIO_PROVIDERS {
        let (url, body, reply) =
            audio_request(provider, "fixture-model", "Transcribe.", wav).unwrap();
        let (origin, server) = server(vec![(200, transcript_sample(provider))], Duration::ZERO);
        assert_eq!(
            transcription(&client(provider), &origin, &body, reply).unwrap(),
            "open `main.rs`",
            "{provider}"
        );
        let sent = posted_body(&server.join().unwrap()[0]);
        if provider == Provider::GoogleAi {
            assert_eq!(
                url,
                "https://generativelanguage.googleapis.com/v1beta/models/fixture-model:generateContent"
            );
            assert_eq!(
                sent["contents"][0]["parts"],
                json!([
                    {"text":"Transcribe."},
                    {"inlineData":{"mimeType":"audio/wav","data":audio}},
                ])
            );
            assert_eq!(sent["generationConfig"]["maxOutputTokens"], 8192);
            continue;
        }
        assert_eq!(sent["model"], "fixture-model");
        assert_eq!(
            sent["messages"],
            json!([{"role":"user","content":[
                {"type":"text","text":"Transcribe."},
                {"type":"input_audio","input_audio":{"data":audio,"format":"wav"}},
            ]}])
        );
        if provider == Provider::OpenAi {
            assert_eq!(url, "https://api.openai.com/v1/chat/completions");
            assert_eq!(sent["modalities"], json!(["text"]));
            assert_eq!(sent["store"], false);
            assert_eq!(sent["max_completion_tokens"], 8192);
        } else {
            assert_eq!(url, "https://openrouter.ai/api/v1/chat/completions");
            assert_eq!(sent["max_tokens"], 8192);
            assert!(sent.get("modalities").is_none());
        }
    }
}

#[test]
fn providers_without_audio_input_are_refused_before_any_request() {
    // Reaching the Keychain or the network would fail differently, or hang, in a test.
    let error =
        transcribe(Provider::Anthropic, "fixture-model", "Transcribe.", b"wav").unwrap_err();
    assert!(matches!(
        error,
        RequestError::Response {
            provider: Provider::Anthropic,
            message: "This provider does not accept audio."
        }
    ));
    for provider in Provider::ALL {
        assert_eq!(
            audio_request(provider, "fixture-model", "Transcribe.", b"wav").is_ok(),
            AUDIO_PROVIDERS.contains(&provider),
            "{provider}"
        );
    }
}

#[test]
fn a_google_model_id_that_would_change_the_request_path_is_refused_before_any_request() {
    for model in ["", "bad/path?key=secret", "a:b", "a b"] {
        assert!(transcribe(Provider::GoogleAi, model, "Transcribe.", b"wav").is_err());
    }
}

#[test]
fn an_unfinished_or_empty_transcription_is_an_error_about_the_recording() {
    for provider in AUDIO_PROVIDERS {
        let sample = transcript_sample(provider);
        let cut_off = sample
            .replace("\"stop\"", "\"length\"")
            .replace("STOP", "MAX_TOKENS");
        let silent = sample.replace("open `main.rs`", r" \n");
        for body in [cut_off, silent, "{}".to_owned()] {
            let (_, request, reply) =
                audio_request(provider, "fixture-model", "Transcribe.", b"wav").unwrap();
            let (origin, server) = server(vec![(200, body)], Duration::ZERO);
            let error = transcription(&client(provider), &origin, &request, reply).unwrap_err();
            assert!(
                error.to_string().contains("No transcription was returned"),
                "{provider}: {error}"
            );
            server.join().unwrap();
        }
    }
}

#[test]
fn a_rejected_audio_request_reports_the_status_without_the_remote_body() {
    let (_, request, reply) =
        audio_request(Provider::OpenAi, "fixture-model", "Transcribe.", b"wav").unwrap();
    let (origin, server) = server(
        vec![(400, "fixture-secret and the recording".into())],
        Duration::ZERO,
    );
    let error = transcription(&client(Provider::OpenAi), &origin, &request, reply).unwrap_err();
    assert!(matches!(error, RequestError::Http { status: 400, .. }));
    let printed = format!("{error} {error:?}");
    assert!(printed.contains("Check the selected model and request size in Settings."));
    assert!(!printed.contains("fixture-secret") && !printed.contains("the recording"));
    server.join().unwrap();
}
