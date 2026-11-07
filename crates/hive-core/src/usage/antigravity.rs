//! Antigravity's quota, from the Cloud Code `retrieveUserQuotaSummary` call that `agy` itself
//! makes, with the Google session it saved and its Cloud Code project context.

use super::{UsageError, UsageWindow, json_of, parse_time, post_text};
use crate::keychain;
use serde_json::Value;

const API_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:";

pub(super) fn fetch() -> Result<Vec<UsageWindow>, UsageError> {
    let token = keychain::antigravity_access_token()?;
    let authorization = format!("Bearer {token}");
    let headers = [
        ("Authorization", authorization.as_str()),
        ("Accept", "application/json"),
        ("User-Agent", "antigravity"),
    ];
    fetch_quota(|method, body| post_text(&format!("{API_URL}{method}"), &headers, body))
}

fn fetch_quota(
    mut post: impl FnMut(&str, &str) -> Result<(u16, String), UsageError>,
) -> Result<Vec<UsageWindow>, UsageError> {
    let (status, text) = post(
        "loadCodeAssist",
        r#"{"metadata":{"ideType":"ANTIGRAVITY"}}"#,
    )?;
    let account = json_of(status, &text)?;
    let project = account["cloudaicompanionProject"]
        .as_str()
        .filter(|project| !project.trim().is_empty())
        .ok_or_else(|| {
            UsageError::Json("Antigravity project unavailable; open `agy` to sign in".into())
        })?;
    // Without project, Google can return SUBSCRIPTION_REQUIRED even for an eligible account.
    let body = serde_json::json!({ "project": project }).to_string();
    let (status, text) = post("retrieveUserQuotaSummary", &body)?;
    let json = json_of(status, &text)?;
    Ok(parse(&json))
}

/// Model groups (Gemini; Claude and GPT; plan-specific pools) each hold a 5-hour and a weekly
/// bucket of the same account-wide quota. Per window the bar shows the most used bucket: that
/// is the one that stops work first.
fn parse(json: &Value) -> Vec<UsageWindow> {
    let groups = json["response"]["groups"]
        .as_array()
        .or_else(|| json["groups"].as_array());
    let buckets = groups
        .into_iter()
        .flatten()
        .filter_map(|group| group["buckets"].as_array())
        .flatten();
    let mut windows: Vec<UsageWindow> = Vec::new();
    for bucket in buckets {
        let Some(window) = bucket_window(bucket) else {
            continue;
        };
        match windows.iter_mut().find(|known| known.label == window.label) {
            Some(known) if known.percent >= window.percent => {}
            Some(known) => *known = window,
            None => windows.push(window),
        }
    }
    windows.sort_by_key(|window| window.label != "5h");
    windows
}

fn bucket_window(bucket: &Value) -> Option<UsageWindow> {
    let label = match bucket["window"].as_str()? {
        "5h" => "5h",
        "weekly" => "7d",
        _ => return None,
    };
    let remaining = bucket["remainingFraction"]
        .as_f64()
        .filter(|fraction| (0. ..=1.).contains(fraction))?;
    Some(UsageWindow {
        label,
        percent: (1. - remaining) * 100.,
        resets_at: parse_time(&bucket["resetTime"]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `RetrieveUserQuotaSummary` response (2026-07-22, `agy` 1.1.5) as ai-usagebar
    /// captured and trimmed it.
    const QUOTA_JSON: &str = r#"{
      "response": {
        "groups": [
          {
            "displayName": "Gemini Models",
            "buckets": [
              {"bucketId": "gemini-weekly", "displayName": "Weekly Limit",
               "window": "weekly", "remainingFraction": 0.9191212,
               "resetTime": "2026-07-28T17:39:58Z"},
              {"bucketId": "gemini-5h", "displayName": "Five Hour Limit",
               "window": "5h", "remainingFraction": 0.5672253,
               "resetTime": "2026-07-22T17:47:00Z"}
            ]
          },
          {
            "displayName": "Claude and GPT models",
            "buckets": [
              {"bucketId": "3p-weekly", "window": "weekly",
               "remainingFraction": 1, "resetTime": "2026-07-29T12:47:00Z"},
              {"bucketId": "3p-5h", "window": "5h",
               "remainingFraction": 0.25, "resetTime": "2026-07-22T17:47:00Z"}
            ]
          }
        ]
      }
    }"#;

    #[test]
    fn resolves_the_account_project_before_requesting_quota() {
        let mut calls = Vec::new();
        let windows = fetch_quota(|method, body| {
            calls.push(method.to_owned());
            let body: Value = serde_json::from_str(body).unwrap();
            match method {
                "loadCodeAssist" => {
                    assert_eq!(body["metadata"]["ideType"], "ANTIGRAVITY");
                    Ok((
                        200,
                        r#"{"cloudaicompanionProject":"project-\"one","unknown":true}"#.into(),
                    ))
                }
                "retrieveUserQuotaSummary" => {
                    assert_eq!(body, serde_json::json!({"project": "project-\"one"}));
                    Ok((200, QUOTA_JSON.into()))
                }
                _ => panic!("unexpected method"),
            }
        })
        .unwrap();
        assert_eq!(calls, ["loadCodeAssist", "retrieveUserQuotaSummary"]);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].percent, 75.);
    }

    #[test]
    fn missing_or_invalid_project_never_sends_an_unscoped_quota_request() {
        for response in [
            "{}",
            r#"{"cloudaicompanionProject":null}"#,
            r#"{"cloudaicompanionProject":123}"#,
            r#"{"cloudaicompanionProject":"  "}"#,
            "{",
        ] {
            let error = fetch_quota(|method, _| {
                assert_eq!(method, "loadCodeAssist");
                Ok((200, response.into()))
            })
            .unwrap_err();
            assert!(matches!(error, UsageError::Json(_)));
        }
    }

    #[test]
    fn project_lookup_errors_stop_without_exposing_the_account() {
        for status in [401, 403, 429, 500] {
            let error = fetch_quota(|method, _| {
                assert_eq!(method, "loadCodeAssist");
                Ok((status, r#"{"account":"private-account-marker"}"#.into()))
            })
            .unwrap_err();
            assert_eq!(error, UsageError::Http(format!("HTTP {status}")));
        }
        let error = fetch_quota(|_, _| Err(UsageError::Http("timeout".into()))).unwrap_err();
        assert_eq!(error, UsageError::Http("timeout".into()));
    }

    #[test]
    fn quota_denial_is_not_misreported_as_a_plan_without_quota() {
        let error = fetch_quota(|method, _| match method {
            "loadCodeAssist" => Ok((200, r#"{"cloudaicompanionProject":"project-one"}"#.into())),
            _ => Ok((403, r#"{"error":{"details":[{"reason":"SUBSCRIPTION_REQUIRED"}],"message":"private-account-marker"}}"#.into())),
        })
        .unwrap_err();
        assert_eq!(error, UsageError::Http("HTTP 403".into()));
    }

    #[test]
    fn reads_the_live_project_scoped_weekly_summary() {
        // Captured 2026-11-07; only public model labels and quota fields are retained.
        let response = serde_json::json!({"groups": [{
            "displayName": "Claude and GPT models",
            "buckets": [{"displayName": "Weekly Limit Remaining", "window": "weekly",
                "remainingFraction": 1, "resetTime": "2026-11-14T20:30:36Z"}]
        }]});
        let windows = parse(&response);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "7d");
        assert_eq!(windows[0].remaining_percent(), 100.);
        assert_eq!(
            windows[0].resets_at,
            parse_time(&serde_json::json!("2026-11-14T20:30:36Z"))
        );
    }

    #[test]
    fn shows_the_most_used_bucket_of_each_window() {
        let windows = parse(&serde_json::from_str(QUOTA_JSON).unwrap());
        let summary: Vec<_> = windows
            .iter()
            .map(|window| (window.label, window.percent.round()))
            .collect();
        assert_eq!(summary, [("5h", 75.), ("7d", 8.)]);
        assert_eq!(
            windows[1].resets_at,
            parse_time(&serde_json::json!("2026-07-28T17:39:58Z"))
        );
    }

    #[test]
    fn tolerates_an_unwrapped_summary_and_skips_unknown_buckets() {
        let json = serde_json::json!({"groups": [{"buckets": [
            {"window": "weekly", "remainingFraction": 0.5},
            {"window": "daily", "remainingFraction": 0},
            {"window": "5h", "remainingFraction": 7},
            {"window": "5h"}
        ]}]});
        let windows = parse(&json);
        assert_eq!(windows.len(), 1);
        assert_eq!((windows[0].label, windows[0].percent), ("7d", 50.));
        assert!(parse(&serde_json::json!({})).is_empty());
    }
}
