use super::*;
use crate::llm::test_server::server;

fn client(provider: Provider, timeout: Duration) -> Client {
    Client::with_key(
        provider,
        ApiKey::try_from("fixture-secret".to_owned()).unwrap(),
        timeout,
    )
}

#[test]
fn requests_authenticate_in_headers_and_encode_json_without_leaking_keys() {
    for provider in Provider::ALL {
        let (url, server) = server(vec![(200, "{}".into())], Duration::ZERO);
        let _: serde_json::Value = client(provider, Duration::from_secs(2))
            .post(&url, &serde_json::json!({"input":"quoted \"text\"\nλ"}))
            .unwrap();
        let request = server.join().unwrap().remove(0);
        assert!(request.starts_with("POST / HTTP/1.1"));
        let lower = request.to_lowercase();
        assert!(lower.contains(if provider == Provider::GoogleAi {
            "x-goog-api-key: fixture-secret"
        } else {
            "authorization: bearer fixture-secret"
        }));
        assert_eq!(
            lower.contains("anthropic-version: 2026-10-21"),
            provider == Provider::Anthropic
        );
        let body = request.split_once("\r\n\r\n").unwrap().1;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap()["input"],
            "quoted \"text\"\nλ"
        );
        assert!(!body.contains("fixture-secret"));
    }
}

#[test]
fn remote_errors_malformed_json_redirect_and_timeout_are_safe() {
    for status in [302, 400, 401, 403, 404, 429, 500] {
        let (url, server) = server(
            vec![(status, "fixture-secret and private prompt".into())],
            Duration::ZERO,
        );
        let error = client(Provider::OpenAi, Duration::from_secs(2))
            .get::<serde_json::Value>(&url, &[])
            .unwrap_err();
        let printed = format!("{error} {error:?}");
        assert!(!printed.contains("fixture-secret"));
        assert!(!printed.contains("private prompt"));
        assert!(matches!(error, RequestError::Http { status: s, .. } if s == status));
        server.join().unwrap();
    }
    let (url, task) = server(vec![(200, "invalid fixture-secret".into())], Duration::ZERO);
    assert!(matches!(
        client(Provider::OpenAi, Duration::from_secs(2)).get::<serde_json::Value>(&url, &[]),
        Err(RequestError::Response { .. })
    ));
    task.join().unwrap();
    let (url, task) = server(vec![(200, "{}".into())], Duration::from_millis(150));
    assert!(
        client(Provider::OpenAi, Duration::from_millis(40))
            .get::<serde_json::Value>(&url, &[])
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    task.join().unwrap();
}
