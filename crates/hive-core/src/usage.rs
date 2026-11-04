//! Usage limits of the Claude Code and Codex CLIs. Both endpoints are undocumented;
//! parsers are lenient; debug logs contain response metadata, never response bodies.

use crate::keychain::{self, CredentialError};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq)]
pub struct UsageWindow {
    pub label: &'static str,
    pub percent: f64,
    pub resets_at: Option<SystemTime>,
}

#[derive(thiserror::Error, Clone, Debug, PartialEq, Eq)]
pub enum UsageError {
    #[error(transparent)]
    Credentials(#[from] CredentialError),
    #[error("request failed: {0}")]
    Http(String),
    #[error("bad response: {0}")]
    Json(String),
}

#[derive(Clone, Debug)]
pub struct Provider {
    pub name: &'static str,
    pub windows: Vec<UsageWindow>,
    pub error: Option<UsageError>,
}

/// Blocking network calls; run on a background executor.
pub fn fetch_all() -> Vec<Provider> {
    vec![provider("Claude", claude()), provider("Codex", codex())]
}

fn provider(name: &'static str, result: Result<Vec<UsageWindow>, UsageError>) -> Provider {
    match result {
        Ok(windows) => Provider {
            name,
            windows,
            error: None,
        },
        Err(error) => {
            tracing::warn!(provider = name, %error, "usage fetch failed");
            Provider {
                name,
                windows: vec![],
                error: Some(error),
            }
        }
    }
}

fn get_json(url: &str, headers: &[(&str, &str)]) -> Result<Value, UsageError> {
    get_json_with_timeout(url, headers, Duration::from_secs(10))
}

fn get_json_with_timeout(
    url: &str,
    headers: &[(&str, &str)],
    timeout: Duration,
) -> Result<Value, UsageError> {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(3).min(timeout)))
        .timeout_global(Some(timeout))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut res = req.call().map_err(|e| UsageError::Http(e.to_string()))?;
    let status = res.status().as_u16();
    let text = res
        .body_mut()
        .read_to_string()
        .map_err(|e| UsageError::Http(e.to_string()))?;
    tracing::debug!(status, response_bytes = text.len(), "usage response");
    serde_json::from_str(&text).map_err(|e| UsageError::Json(e.to_string()))
}

fn claude() -> Result<Vec<UsageWindow>, UsageError> {
    let token = keychain::claude_access_token()?;
    let json = get_json(
        "https://api.anthropic.com/api/oauth/usage",
        &[
            ("Authorization", &format!("Bearer {token}")),
            ("anthropic-beta", "oauth-2025-04-20"),
            ("User-Agent", "claude-code/2.0"),
        ],
    )?;
    Ok(parse_claude(&json))
}

/// Handles both the `five_hour`/`seven_day` objects and the newer `limits[]` array.
pub fn parse_claude(json: &Value) -> Vec<UsageWindow> {
    let mut out = vec![];
    for (key, label) in [("five_hour", "5h"), ("seven_day", "7d")] {
        if let Some(pct) = json[key]["utilization"].as_f64() {
            out.push(UsageWindow {
                label,
                percent: pct,
                resets_at: parse_time(&json[key]["resets_at"]),
            });
        }
    }
    if out.is_empty() {
        for limit in json["limits"].as_array().into_iter().flatten() {
            let label = match limit["kind"].as_str() {
                Some("session") => "5h",
                Some("weekly_all") => "7d",
                _ => continue,
            };
            if let Some(pct) = limit["percent"].as_f64() {
                out.push(UsageWindow {
                    label,
                    percent: pct,
                    resets_at: parse_time(&limit["resets_at"]),
                });
            }
        }
    }
    out
}

fn codex() -> Result<Vec<UsageWindow>, UsageError> {
    let creds = keychain::codex_credentials()?;
    let json = get_json(
        "https://chatgpt.com/backend-api/wham/usage",
        &[
            ("Authorization", &format!("Bearer {}", creds.access_token)),
            ("ChatGPT-Account-ID", &creds.account_id),
        ],
    )?;
    Ok(parse_codex(&json))
}

/// Windows are sorted by their length (5h before 7d); the `primary`/`secondary` names are not trusted.
pub fn parse_codex(json: &Value) -> Vec<UsageWindow> {
    let rl = &json["rate_limit"];
    let mut windows: Vec<(u64, f64, Option<SystemTime>)> = ["primary_window", "secondary_window"]
        .iter()
        .filter_map(|key| {
            let w = &rl[key];
            let pct = w["used_percent"].as_f64()?;
            let secs = w["limit_window_seconds"].as_u64().unwrap_or(0);
            Some((
                secs,
                pct,
                parse_time(&w["reset_at"]).or_else(|| parse_time(&w["resets_at"])),
            ))
        })
        .collect();
    windows.sort_by_key(|w| w.0);
    windows
        .into_iter()
        .map(|(secs, percent, resets_at)| UsageWindow {
            label: if secs > 24 * 3600 { "7d" } else { "5h" },
            percent,
            resets_at,
        })
        .collect()
}

/// Accepts unix seconds (number) or an RFC 3339 string.
fn parse_time(v: &Value) -> Option<SystemTime> {
    if let Some(secs) = v.as_u64() {
        return Some(UNIX_EPOCH + Duration::from_secs(secs));
    }
    let dt = chrono::DateTime::parse_from_rfc3339(v.as_str()?).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(dt.timestamp().max(0) as u64))
}

/// "31m", "12h 3m", "5d 14h" until `t`; "now" if already passed.
pub fn until(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::now())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    match (d, h, m) {
        (0, 0, 0) => "now".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, m) => format!("{h}h {m}m"),
        (d, h, _) => format!("{d}d {h}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_debug_log_does_not_expose_credentials_or_response_body() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};

        #[derive(Clone)]
        struct Log(Arc<Mutex<Vec<u8>>>);

        impl Write for Log {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let log = Log(Arc::default());
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/usage", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 2048];
            assert!(stream.read(&mut request).unwrap() > 0);
            let body = r#"{"account":"private-account-marker"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        tracing::subscriber::with_default(subscriber, || {
            let value =
                get_json(&url, &[("Authorization", "Bearer private-token-marker")]).unwrap();
            assert_eq!(value["account"], "private-account-marker");
        });
        server.join().unwrap();
        let bytes = log.0.lock().unwrap();
        let output = std::str::from_utf8(&bytes).unwrap();
        assert!(output.contains("usage response"));
        assert!(output.contains("status=200"));
        assert!(output.contains("response_bytes="));
        assert!(!output.contains("private-account-marker"));
        assert!(!output.contains("private-token-marker"));
    }

    #[test]
    fn http_deadline_includes_headers_and_body_and_allows_retry() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Instant;

        for delay_body in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/usage", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                for attempt in 0..2 {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut request = [0; 2048];
                    assert!(stream.read(&mut request).unwrap() > 0);
                    if attempt == 0 && !delay_body {
                        std::thread::sleep(Duration::from_millis(300));
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n",
                    );
                    if attempt == 0 && delay_body {
                        std::thread::sleep(Duration::from_millis(300));
                    }
                    let _ = stream.write_all(br#"{"ok":true}"#);
                }
            });
            let start = Instant::now();
            let error = get_json_with_timeout(&url, &[], Duration::from_millis(100)).unwrap_err();
            assert!(matches!(error, UsageError::Http(_)), "{error}");
            assert!(error.to_string().contains("timeout"), "{error}");
            assert!(start.elapsed() < Duration::from_millis(250));
            assert_eq!(
                get_json_with_timeout(&url, &[], Duration::from_secs(2)).unwrap(),
                json!({"ok": true})
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn claude_old_and_new_shapes() {
        let old = json!({"five_hour": {"utilization": 2.0, "resets_at": "2026-11-03T22:00:00Z"},
                         "seven_day": {"utilization": 15.5, "resets_at": null}});
        let w = parse_claude(&old);
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].label, w[0].percent), ("5h", 2.0));
        assert!(w[0].resets_at.is_some() && w[1].resets_at.is_none());

        let new = json!({"five_hour": null, "seven_day": null,
                         "limits": [{"kind": "weekly_all", "percent": 40, "resets_at": "2026-11-08T00:00:00+00:00"},
                                    {"kind": "session", "percent": 3, "resets_at": "2026-11-03T22:00:00Z"}]});
        let w = parse_claude(&new);
        assert_eq!(
            w.iter().map(|w| (w.label, w.percent)).collect::<Vec<_>>(),
            vec![("7d", 40.0), ("5h", 3.0)]
        );
    }

    #[test]
    fn codex_sorts_by_window_length() {
        let v = json!({"plan_type": "plus", "rate_limit": {
            "primary_window": {"used_percent": 40, "limit_window_seconds": 604800, "reset_at": 1762300000},
            "secondary_window": {"used_percent": 83, "limit_window_seconds": 18000, "reset_at": 1762200000}}});
        let w = parse_codex(&v);
        assert_eq!(
            w.iter().map(|w| (w.label, w.percent)).collect::<Vec<_>>(),
            vec![("5h", 83.0), ("7d", 40.0)]
        );
    }

    #[test]
    fn until_formats() {
        let now = SystemTime::now();
        assert_eq!(until(now + Duration::from_secs(31 * 60 + 5)), "31m");
        assert_eq!(
            until(now + Duration::from_secs(12 * 3600 + 3 * 60 + 5)),
            "12h 3m"
        );
        assert_eq!(
            until(now + Duration::from_secs(5 * 86400 + 14 * 3600 + 5)),
            "5d 14h"
        );
        assert_eq!(until(now), "now");
    }
}
