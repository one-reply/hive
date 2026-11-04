use super::*;
use std::{
    cell::Cell,
    io::{Read, Write},
    net::TcpListener,
    thread,
};

const TEST_KEY: &str = "hive-test-secret";

fn key() -> ApiKey {
    ApiKey::try_from(TEST_KEY.to_owned()).unwrap()
}

#[test]
fn keys_are_trimmed_and_never_echoed() {
    assert_eq!(
        ApiKey::try_from(" \n new-format-key\t ".to_owned())
            .unwrap()
            .as_str(),
        "new-format-key"
    );
    for value in ["", " \n\t"] {
        assert_eq!(
            ApiKey::try_from(value.to_owned()).unwrap_err(),
            InvalidApiKey::Empty
        );
    }
    for value in [
        "a b",
        "a\nb",
        "a\rb",
        "a\tb",
        "a\0b",
        "a\u{2003}b",
        "a\u{7f}b",
    ] {
        let error = ApiKey::try_from(value.to_owned()).unwrap_err();
        assert_eq!(error, InvalidApiKey::InvalidCharacters);
        assert!(!format!("{error:?} {error}").contains(value));
    }
    assert!(!format!("{:?}", key()).contains(TEST_KEY));
    for provider in Provider::ALL {
        let (_, header) = authentication(provider, &key());
        assert!(header.is_sensitive());
        assert!(!format!("{header:?}").contains(TEST_KEY));
    }
}

#[test]
fn provider_storage_and_endpoints_are_stable() {
    let expected = [
        (
            "google-ai",
            "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1",
        ),
        ("openai", "https://api.openai.com/v1/models"),
        ("openrouter", "https://openrouter.ai/api/v1/key"),
        ("anthropic", "https://api.anthropic.com/v1/models?limit=1"),
    ];
    for (provider, (account, endpoint)) in Provider::ALL.into_iter().zip(expected) {
        assert_eq!(provider.account(), account);
        assert_eq!(provider.verification_url(), endpoint);
    }
}

fn server(status: u16, delay: Duration) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
        }
        thread::sleep(delay);
        // A response body containing a secret must never become a UI/log error.
        let response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nLocation: http://127.0.0.1:1/leak\r\nConnection: close\r\n\r\n{TEST_KEY}",
            TEST_KEY.len()
        );
        let _ = stream.write_all(response.as_bytes());
        String::from_utf8(request).unwrap()
    });
    (format!("http://{address}"), handle)
}

#[test]
fn verification_sends_provider_headers_before_saving() {
    for provider in Provider::ALL {
        let (origin, server) = server(200, Duration::ZERO);
        let uri: ureq::http::Uri = provider.verification_url().parse().unwrap();
        let path = uri.path_and_query().unwrap().as_str();
        let saved = Cell::new(false);
        verify_and_save_with(
            provider,
            &key(),
            &format!("{origin}{path}"),
            Duration::from_secs(2),
            || {
                saved.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(saved.get());
        let request = server.join().unwrap().to_lowercase();
        assert!(request.starts_with(&format!("get {} http/1.1", path.to_lowercase())));
        let (header, prefix) = if provider == Provider::GoogleAi {
            ("x-goog-api-key", "")
        } else {
            ("authorization", "bearer ")
        };
        assert!(request.contains(&format!("{header}: {prefix}{TEST_KEY}\r\n")));
        assert_eq!(
            request.contains("anthropic-version: 2026-10-21\r\n"),
            provider == Provider::Anthropic
        );
        assert!(!request.lines().next().unwrap().contains(TEST_KEY));
    }
}

#[test]
fn unsuccessful_verification_never_saves_or_exposes_response() {
    for status in [301, 302, 307, 400, 401, 403, 429, 500, 503] {
        let (url, server) = server(status, Duration::ZERO);
        let error = verify_and_save_with(
            Provider::OpenAi,
            &key(),
            &url,
            Duration::from_secs(2),
            || panic!("a failed verification must not write to Keychain"),
        )
        .unwrap_err();
        assert!(matches!(error, Error::Verification { status: actual, .. } if actual == status));
        assert!(!format!("{error:?} {error}").contains(TEST_KEY));
        server.join().unwrap();
    }
}

#[test]
fn timeout_does_not_save_and_next_attempt_can_succeed() {
    let (url, server) = server(200, Duration::from_millis(250));
    let error = verify_and_save_with(
        Provider::GoogleAi,
        &key(),
        &url,
        Duration::from_millis(50),
        || panic!("a timed out verification must not save"),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Timeout { .. }));
    server.join().unwrap();
    let (url, server) = self::server(200, Duration::ZERO);
    let error = verify_and_save_with(
        Provider::GoogleAi,
        &key(),
        &url,
        Duration::from_secs(2),
        || {
            Err(Error::Keychain {
                provider: Provider::GoogleAi,
                operation: "save",
                code: -25293,
            })
        },
    )
    .unwrap_err();
    assert!(matches!(
        error,
        Error::Keychain {
            operation: "save",
            ..
        }
    ));
    server.join().unwrap();
}
