use super::*;

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
        assert_eq!(
            extract(provider, serde_json::from_str(sample).unwrap()).unwrap(),
            "feat: fixture"
        );
        assert!(extract(provider, serde_json::from_str("{}").unwrap()).is_err());
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
        assert!(extract(provider, serde_json::from_str(&incomplete).unwrap()).is_err());
        assert!(
            extract(
                provider,
                serde_json::from_str(&sample.replace("feat: fixture", "")).unwrap()
            )
            .is_err()
        );
    }
    assert!(extract(Provider::OpenAi, serde_json::from_str(r#"{"status":"completed","output":[{"type":"message","content":[{"type":"refusal","refusal":"no"}]}]}"#).unwrap()).is_err());
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
