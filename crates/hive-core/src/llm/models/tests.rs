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
