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
fn audio_requests_carry_the_prompt_as_the_system_instruction_and_the_wav_as_base64() {
    let wav = b"RIFF\x00\xffWAVE recording";
    let audio = STANDARD.encode(wav);
    for provider in AUDIO_PROVIDERS {
        let (url, request) =
            audio_request(provider, "fixture-model", "Transcribe.", None, wav).unwrap();
        let (origin, server) = server(vec![(200, transcript_sample(provider))], Duration::ZERO);
        assert_eq!(
            request.send(&client(provider), &origin).unwrap(),
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
                sent["systemInstruction"],
                json!({"parts":[{"text":"Transcribe."}]})
            );
            assert_eq!(
                sent["contents"],
                json!([{"role":"user","parts":[
                    {"text":"Transcribe this recording."},
                    {"inlineData":{"mimeType":"audio/wav","data":audio}},
                ]}])
            );
            assert_eq!(sent["generationConfig"]["maxOutputTokens"], 8192);
            continue;
        }
        assert_eq!(sent["model"], "fixture-model");
        assert_eq!(
            sent["messages"],
            json!([
                {"role":"system","content":"Transcribe."},
                {"role":"user","content":[
                    {"type":"text","text":"Transcribe this recording."},
                    {"type":"input_audio","input_audio":{"data":audio,"format":"wav"}},
                ]},
            ])
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
            audio_request(provider, "fixture-model", "Transcribe.", None, b"wav").is_ok(),
            AUDIO_PROVIDERS.contains(&provider),
            "{provider}"
        );
    }
}

#[test]
fn file_transcription_posts_the_original_wav_and_uses_the_prompt_as_context() {
    let wav = b"RIFF\x00\x80\xffWAVE\r\n";
    let prompt = "Hive, GPUI.\nПроєкт «Вулик».";
    let provider = Provider::OpenAi;
    for language in [Some(("Ukrainian", "uk")), None] {
        let (url, request) =
            audio_request(provider, "gpt-transcribe", prompt, language, wav).unwrap();
        let path = url.strip_prefix("https://api.openai.com").unwrap();
        let (origin, server) = crate::llm::test_server::capture_requests(
            vec![(
                200,
                r#"{"text":"  Привіт!  ","languages":[{"code":"uk"}]}"#.into(),
            )],
            Duration::ZERO,
            std::convert::identity,
        );
        assert_eq!(
            request
                .send(&client(provider), &format!("{origin}{path}"))
                .unwrap(),
            "Привіт!"
        );
        let sent = server.join().unwrap().remove(0);
        let end = sent.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&sent[..end]).unwrap();
        assert!(headers.starts_with("POST /v1/audio/transcriptions HTTP/1.1\r\n"));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-secret")
        );
        let boundary = headers
            .lines()
            .find_map(|header| {
                header
                    .split_once("multipart/form-data; boundary=")
                    .map(|(_, value)| value)
            })
            .unwrap();
        let mut expected = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\n").into_bytes();
        expected.extend_from_slice(wav);
        let mut fields = vec![("model", "gpt-transcribe"), ("prompt", prompt)];
        if language.is_some() {
            fields.push(("languages[]", "uk"));
        }
        for (field, value) in fields {
            expected.extend(format!("\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}").bytes());
        }
        expected.extend(format!("\r\n--{boundary}--\r\n").bytes());
        assert_eq!(&sent[end + 4..], expected);
    }
}

#[test]
fn chat_models_keep_the_language_in_the_system_instructions() {
    let language = Some(("Ukrainian", "uk"));
    for (provider, model) in [
        (Provider::OpenAi, "gpt-audio"),
        (Provider::GoogleAi, "gemini-flash"),
        (Provider::OpenRouter, "openai/gpt-transcribe"),
    ] {
        let (_, request) = audio_request(provider, model, "Terms.", language, b"wav").unwrap();
        let AudioRequest::Chat(body, _) = request else {
            panic!("expected a chat request")
        };
        let instructions = if provider == Provider::GoogleAi {
            &body["systemInstruction"]["parts"][0]["text"]
        } else {
            &body["messages"][0]["content"]
        };
        let instructions = instructions.as_str().unwrap();
        assert!(instructions.starts_with("Terms.\n\nThe recording is in Ukrainian."));
        assert!(instructions.contains("Write the transcription in Ukrainian;"));
    }
}

#[test]
fn file_transcription_rejects_empty_replies_and_reports_http_failures_safely() {
    let provider = Provider::OpenAi;
    let (origin, server) = server(
        vec![
            (401, "private".into()),
            (429, "private".into()),
            (200, "{}".into()),
            (200, r#"{"text":" \n "}"#.into()),
            (200, r#"{"text":"Retry worked"}"#.into()),
        ],
        Duration::ZERO,
    );
    let client = client(provider);
    let send = || {
        audio_request(provider, "gpt-transcribe", "Terms.", None, b"wav")
            .unwrap()
            .1
            .send(&client, &origin)
    };
    for expected in [401, 429] {
        let error = send().unwrap_err();
        assert!(matches!(error, RequestError::Http { status, .. } if status == expected));
        assert!(!error.to_string().contains("private"));
    }
    for _ in 0..2 {
        assert!(
            send()
                .unwrap_err()
                .to_string()
                .contains("No transcription was returned")
        );
    }
    assert_eq!(send().unwrap(), "Retry worked");
    assert_eq!(server.join().unwrap().len(), 5);
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
            let (_, request) =
                audio_request(provider, "fixture-model", "Transcribe.", None, b"wav").unwrap();
            let (origin, server) = server(vec![(200, body)], Duration::ZERO);
            let error = request.send(&client(provider), &origin).unwrap_err();
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
    let (_, request) = audio_request(
        Provider::OpenAi,
        "fixture-model",
        "Transcribe.",
        None,
        b"wav",
    )
    .unwrap();
    let (origin, server) = server(
        vec![(400, "fixture-secret and the recording".into())],
        Duration::ZERO,
    );
    let error = request
        .send(&client(Provider::OpenAi), &origin)
        .unwrap_err();
    assert!(matches!(error, RequestError::Http { status: 400, .. }));
    let printed = format!("{error} {error:?}");
    assert!(printed.contains("Check the selected model and request size in Settings."));
    assert!(!printed.contains("fixture-secret") && !printed.contains("the recording"));
    server.join().unwrap();
}
