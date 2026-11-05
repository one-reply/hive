//! Antigravity's quota, from the Cloud Code `retrieveUserQuotaSummary` call that `agy` itself
//! makes, with the Google session it saved. Shape as reverse-engineered by ai-usagebar.

use super::{UsageError, UsageWindow, json_of, parse_time, post_text};
use crate::keychain;
use serde_json::Value;

const QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
/// What Google answers with when the account's plan has no Antigravity quota.
const SUBSCRIPTION_REQUIRED: &str = "SUBSCRIPTION_REQUIRED";

pub(super) fn fetch() -> Result<Vec<UsageWindow>, UsageError> {
    let token = keychain::antigravity_access_token()?;
    let (status, text) = post_text(
        QUOTA_URL,
        &[
            ("Authorization", &format!("Bearer {token}")),
            ("Accept", "application/json"),
            ("User-Agent", "antigravity"),
        ],
        "{}",
    )?;
    if status == 403 && text.contains(SUBSCRIPTION_REQUIRED) {
        return Err(UsageError::Http(
            "this plan has no Antigravity quota".into(),
        ));
    }
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
