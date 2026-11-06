//! Usage limits of the installed agent CLIs: Claude Code, Codex, Cursor Agent and Antigravity.
//! Every endpoint is undocumented; parsers are lenient; debug logs contain response metadata,
//! never response bodies.

mod antigravity;
mod cursor;

use crate::command;
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

/// A vendor whose plan limits Hive reads with the credentials of its CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Claude,
    Codex,
    Cursor,
    Antigravity,
}

impl Vendor {
    /// Every vendor, in status bar order.
    pub const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::Cursor, Self::Antigravity];

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Antigravity => "Antigravity",
        }
    }

    /// The CLI whose presence decides whether the vendor is shown at all.
    pub fn executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor-agent",
            Self::Antigravity => "agy",
        }
    }

    fn fetch(self) -> Result<Vec<UsageWindow>, UsageError> {
        match self {
            Self::Claude => claude(),
            Self::Codex => codex(),
            Self::Cursor => cursor::fetch(),
            Self::Antigravity => antigravity::fetch(),
        }
    }
}

/// The limits of one vendor, or why they could not be read.
#[derive(Clone, Debug)]
pub struct Provider {
    pub vendor: Vendor,
    pub windows: Vec<UsageWindow>,
    pub error: Option<UsageError>,
}

/// Blocking: the limits of every vendor whose CLI is installed. A CLI that is installed but
/// signed out still shows, with the command that signs in; one that is missing is left out.
pub fn fetch_all() -> Vec<Provider> {
    Vendor::ALL
        .into_iter()
        .filter(|vendor| command::executable(vendor.executable()).is_some())
        .map(|vendor| provider(vendor, vendor.fetch()))
        .collect()
}

fn provider(vendor: Vendor, result: Result<Vec<UsageWindow>, UsageError>) -> Provider {
    match result {
        Ok(windows) => Provider {
            vendor,
            windows,
            error: None,
        },
        Err(error) => {
            tracing::warn!(provider = vendor.name(), %error, "usage fetch failed");
            Provider {
                vendor,
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
    let mut req = agent(timeout).get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let (status, text) = response_text(req.call())?;
    json_of(status, &text)
}

/// Blocking: POSTs `body` as JSON; the status and text, for callers that read error bodies.
fn post_text(url: &str, headers: &[(&str, &str)], body: &str) -> Result<(u16, String), UsageError> {
    let mut req = agent(Duration::from_secs(10))
        .post(url)
        .content_type("application/json");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    response_text(req.send(body))
}

/// Non-2xx statuses come back as responses, so a caller can tell them apart by their body.
fn agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(3).min(timeout)))
        .timeout_global(Some(timeout))
        .build();
    ureq::Agent::new_with_config(config)
}

fn response_text(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<(u16, String), UsageError> {
    let mut res = response.map_err(|e| UsageError::Http(e.to_string()))?;
    let status = res.status().as_u16();
    let text = res
        .body_mut()
        .read_to_string()
        .map_err(|e| UsageError::Http(e.to_string()))?;
    tracing::debug!(status, response_bytes = text.len(), "usage response");
    Ok((status, text))
}

/// The body of a 2xx response as JSON. Error bodies may name the account, so only the status
/// reaches the message.
fn json_of(status: u16, text: &str) -> Result<Value, UsageError> {
    if !(200..300).contains(&status) {
        return Err(UsageError::Http(format!("HTTP {status}")));
    }
    serde_json::from_str(text).map_err(|e| UsageError::Json(e.to_string()))
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

/// Usage from this percent on is worth a glance.
const WARNING_PERCENT: f64 = 70.0;
/// Usage from this percent on is about to stop work.
const CRITICAL_PERCENT: f64 = 90.0;

/// How close a window is to its limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageLevel {
    Ok,
    Warning,
    Critical,
}

impl UsageWindow {
    /// What is left of the limit, 0..=100; a malformed (NaN) percent reads as all left.
    pub fn remaining_percent(&self) -> f64 {
        if self.percent.is_nan() {
            return 100.0;
        }
        (100.0 - self.percent).clamp(0.0, 100.0)
    }

    /// The level of this window; a malformed (NaN) percent reads as `Ok`.
    pub fn level(&self) -> UsageLevel {
        match self.percent {
            p if p >= CRITICAL_PERCENT => UsageLevel::Critical,
            p if p >= WARNING_PERCENT => UsageLevel::Warning,
            _ => UsageLevel::Ok,
        }
    }
}

/// The window that will stop work first: the most used one, the sooner reset on a tie.
pub fn binding_window(windows: &[UsageWindow]) -> Option<&UsageWindow> {
    let used = |w: &UsageWindow| if w.percent.is_nan() { 0.0 } else { w.percent };
    windows.iter().max_by(|a, b| {
        used(a)
            .total_cmp(&used(b))
            .then_with(|| match (a.resets_at, b.resets_at) {
                (Some(a), Some(b)) => b.cmp(&a),
                (a, b) => a.is_some().cmp(&b.is_some()),
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn window(percent: f64, resets_in_secs: Option<u64>) -> UsageWindow {
        UsageWindow {
            label: "w",
            percent,
            resets_at: resets_in_secs.map(|s| UNIX_EPOCH + Duration::from_secs(s)),
        }
    }

    #[test]
    fn binding_window_is_the_most_used_one() {
        let windows = [window(2.0, Some(10)), window(46.0, Some(1000))];
        assert_eq!(binding_window(&windows), Some(&windows[1]));
    }

    #[test]
    fn binding_window_tie_prefers_sooner_known_reset() {
        let windows = [
            window(50.0, None),
            window(50.0, Some(1000)),
            window(50.0, Some(10)),
        ];
        assert_eq!(binding_window(&windows), Some(&windows[2]));
    }

    #[test]
    fn binding_window_of_nothing_is_none() {
        assert_eq!(binding_window(&[]), None);
    }

    #[test]
    fn binding_window_tolerates_nan_percent() {
        let windows = [window(f64::NAN, None), window(5.0, None)];
        assert_eq!(binding_window(&windows), Some(&windows[1]));
    }

    #[test]
    fn remaining_percent_is_clamped() {
        assert_eq!(window(3.0, None).remaining_percent(), 97.0);
        assert_eq!(window(150.0, None).remaining_percent(), 0.0);
        assert_eq!(window(-5.0, None).remaining_percent(), 100.0);
        assert_eq!(window(f64::NAN, None).remaining_percent(), 100.0);
    }

    #[test]
    fn level_follows_thresholds() {
        let level = |p| window(p, None).level();
        assert_eq!(level(-1.0), UsageLevel::Ok);
        assert_eq!(level(69.9), UsageLevel::Ok);
        assert_eq!(level(70.0), UsageLevel::Warning);
        assert_eq!(level(89.9), UsageLevel::Warning);
        assert_eq!(level(90.0), UsageLevel::Critical);
        assert_eq!(level(150.0), UsageLevel::Critical);
        assert_eq!(level(f64::NAN), UsageLevel::Ok);
    }

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

    /// The deadline must cover a slow header and a slow body alike. A timeout error is the
    /// whole proof: had the client waited out the server's delay, the complete response
    /// would have arrived and the call would have succeeded. No wall-clock bound, which
    /// only made the test flaky on loaded CI runners.
    #[test]
    fn http_deadline_includes_headers_and_body_and_allows_retry() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

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
            let error = get_json_with_timeout(&url, &[], Duration::from_millis(100)).unwrap_err();
            assert!(matches!(error, UsageError::Http(_)), "{error}");
            assert!(error.to_string().contains("timeout"), "{error}");
            assert_eq!(
                get_json_with_timeout(&url, &[], Duration::from_secs(2)).unwrap(),
                json!({"ok": true})
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn error_statuses_never_carry_the_body() {
        let error = json_of(403, r#"{"email":"private-account-marker"}"#).unwrap_err();
        assert_eq!(error, UsageError::Http("HTTP 403".into()));
        assert_eq!(json_of(200, r#"{"ok":1}"#).unwrap(), json!({"ok": 1}));
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
