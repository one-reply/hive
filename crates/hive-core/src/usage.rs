//! Usage limits of the Claude Code and Codex CLIs. Both endpoints are undocumented;
//! parsers are lenient and the raw JSON is logged at debug level.

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
    let mut req = ureq::get(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let mut res = req.call().map_err(|e| UsageError::Http(e.to_string()))?;
    let text = res
        .body_mut()
        .read_to_string()
        .map_err(|e| UsageError::Http(e.to_string()))?;
    tracing::debug!(url, body = %text, "usage response");
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
