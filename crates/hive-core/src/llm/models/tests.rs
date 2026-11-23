use super::*;
use crate::llm::{ApiKey, test_server::server};

#[test]
fn google_pages_filter_capabilities_and_encode_cursors() {
    let (url, task) = server(vec![
        (200, r#"{"models":[{"name":"models/gemini-flash","displayName":"Gemini Flash","supportedGenerationMethods":["generateContent"]},{"name":"models/embedding","supportedGenerationMethods":["embedContent"]}],"nextPageToken":"a+/&b"}"#.into()),
        (200, r#"{"models":[{"name":"models/gemini-pro","supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-flash","supportedGenerationMethods":["generateContent"]},{}]}"#.into()),
    ], Duration::ZERO);
    let client = Client::with_key(
        Provider::GoogleAi,
        ApiKey::try_from("test".to_owned()).unwrap(),
        Duration::from_secs(2),
    );
    let models = collect(&client, &url).unwrap();
    assert_eq!(
        models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["gemini-flash", "gemini-pro"]
    );
    let requests = task.join().unwrap();
    assert!(
        requests[1]
            .lines()
            .next()
            .unwrap()
            .contains("pageToken=a%2B%2F%26b")
    );
}

#[test]
fn anthropic_pagination_and_text_filters() {
    let (url, task) = server(vec![
        (200, r#"{"data":[{"id":"claude-a","display_name":"Claude A"}],"has_more":true,"last_id":"claude-a"}"#.into()),
        (200, r#"{"data":[{"id":"claude-b"},{"id":"retired","lifecycle":"retired"}],"has_more":false}"#.into()),
    ], Duration::ZERO);
    let client = Client::with_key(
        Provider::Anthropic,
        ApiKey::try_from("test".to_owned()).unwrap(),
        Duration::from_secs(2),
    );
    assert_eq!(collect(&client, &url).unwrap().len(), 2);
    assert!(task.join().unwrap()[1].contains("after_id=claude-a"));
    let entry: Entry = serde_json::from_str(r#"{"id":"vendor/image","architecture":{"input_modalities":["text"],"output_modalities":["image"]}}"#).unwrap();
    assert!(model(Provider::OpenRouter, entry).is_none());
    for id in [
        "text-embedding-3-small",
        "gpt-image-1",
        "gpt-realtime",
        "whisper-1",
    ] {
        assert!(
            model(
                Provider::OpenAi,
                Entry {
                    id: id.into(),
                    ..Default::default()
                }
            )
            .is_none()
        );
    }
}

#[test]
fn repeated_cursor_is_an_error_not_partial_success() {
    let page = r#"{"models":[],"nextPageToken":"same"}"#.to_owned();
    let (url, task) = server(vec![(200, page.clone()), (200, page)], Duration::ZERO);
    let client = Client::with_key(
        Provider::GoogleAi,
        ApiKey::try_from("test".to_owned()).unwrap(),
        Duration::from_secs(2),
    );
    assert!(collect(&client, &url).is_err());
    task.join().unwrap();
}

fn listed(provider: Provider, entry: &str) -> Option<Model> {
    model(provider, serde_json::from_str(entry).unwrap())
}

fn named(provider: Provider, id: &str) -> Option<Model> {
    let field = if provider == Provider::GoogleAi {
        format!(r#"{{"name":"models/{id}","supportedGenerationMethods":["generateContent"]}}"#)
    } else {
        format!(r#"{{"id":"{id}"}}"#)
    };
    listed(provider, &field)
}

#[test]
fn openrouter_models_accept_audio_only_when_their_input_modalities_say_so() {
    let entry = |input: &str| {
        let entry = format!(
            r#"{{"id":"vendor/model","architecture":{{"input_modalities":{input},"output_modalities":["text"]}}}}"#
        );
        listed(Provider::OpenRouter, &entry)
    };
    let hears = entry(r#"["text","image","audio"]"#).unwrap();
    assert_eq!(hears.capability, Capability::TextAndAudio);
    assert!(hears.accepts_audio() && hears.generates_text());
    let reads = entry(r#"["text","image"]"#).unwrap();
    assert_eq!(reads.capability, Capability::Text);
    assert!(!reads.accepts_audio() && reads.generates_text());
    // The prompt travels as text beside the recording, so audio alone is not enough.
    assert!(entry(r#"["audio"]"#).is_none());
    // An entry without modalities, and a name that merely mentions audio, promise nothing.
    let unknown = named(Provider::OpenRouter, "vendor/audio-model").unwrap();
    assert_eq!(unknown.capability, Capability::Text);
}

#[test]
fn gemini_models_accept_audio_and_gemma_does_not() {
    for id in ["gemini-2.5-flash", "gemini-3-pro-preview"] {
        let model = named(Provider::GoogleAi, id).unwrap();
        assert!(model.accepts_audio() && model.generates_text(), "{id}");
    }
    let gemma = named(Provider::GoogleAi, "gemma-3-27b-it").unwrap();
    assert!(!gemma.accepts_audio() && gemma.generates_text());
    for id in [
        "gemini-2.5-flash-preview-tts",
        "gemini-2.5-flash-native-audio-preview",
        "gemini-2.5-flash-image",
    ] {
        assert!(named(Provider::GoogleAi, id).is_none(), "{id}");
    }
}

#[test]
fn openai_audio_models_accept_audio_and_stay_out_of_text_generation() {
    for id in [
        "gpt-audio",
        "gpt-audio-mini",
        "gpt-4o-audio-preview",
        "gpt-transcribe",
    ] {
        let model = named(Provider::OpenAi, id).unwrap();
        assert_eq!(model.capability, Capability::Audio, "{id}");
        assert!(model.accepts_audio() && !model.generates_text(), "{id}");
    }
    let text = named(Provider::OpenAi, "gpt-5").unwrap();
    assert!(text.generates_text() && !text.accepts_audio());
    for id in [
        "gpt-realtime-audio",
        "gpt-4o-realtime-preview",
        "gpt-4o-mini-tts",
        "gpt-4o-transcribe",
        "whisper-1",
    ] {
        assert!(named(Provider::OpenAi, id).is_none(), "{id}");
    }
}

#[test]
fn anthropic_models_never_accept_audio() {
    let model = named(Provider::Anthropic, "claude-audio-test").unwrap();
    assert_eq!(model.capability, Capability::Text);
}
