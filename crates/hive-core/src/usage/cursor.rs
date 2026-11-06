//! Cursor's included plan usage, from the dashboard's `GET cursor.com/api/usage-summary`,
//! authenticated with the `cursor-agent login` session as the dashboard's cookie.
//! Shape as reverse-engineered by CodexBar and ai-usagebar.

use super::{UsageError, UsageWindow, get_json, parse_time};
use crate::keychain;
use serde_json::Value;

const USAGE_URL: &str = "https://cursor.com/api/usage-summary";
/// The endpoint sits behind a WAF that turns away clients without a browser's headers.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

pub(super) fn fetch() -> Result<Vec<UsageWindow>, UsageError> {
    let session = keychain::cursor_session()?;
    let json = get_json(
        USAGE_URL,
        &[
            ("Cookie", &session.cookie()),
            ("Origin", "https://cursor.com"),
            ("Referer", "https://cursor.com/dashboard"),
            ("User-Agent", BROWSER_USER_AGENT),
            ("Accept", "application/json, text/plain, */*"),
        ],
    )?;
    Ok(parse(&json))
}

/// One window for the billing cycle. `totalPercentUsed` is already in percent; older
/// payloads carry only the `auto` and `api` pools, whose mean the dashboard shows.
/// Team and enterprise seats have no `plan` block, so they show no window.
fn parse(json: &Value) -> Vec<UsageWindow> {
    let plan = &json["individualUsage"]["plan"];
    let pools = (
        plan["autoPercentUsed"].as_f64(),
        plan["apiPercentUsed"].as_f64(),
    );
    let percent = plan["totalPercentUsed"].as_f64().or(match pools {
        (Some(auto), Some(api)) => Some((auto + api) / 2.),
        (auto, api) => auto.or(api),
    });
    percent
        .filter(|percent| percent.is_finite())
        .map(|percent| UsageWindow {
            label: "Plan",
            percent: percent.clamp(0., 100.),
            resets_at: parse_time(&json["billingCycleEnd"]),
        })
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Trimmed from the live Ultra response ai-usagebar's wire types document.
    #[test]
    fn reads_the_plan_total_and_the_cycle_end() {
        let json = json!({
            "billingCycleStart": "2026-07-04T00:35:51.000Z",
            "billingCycleEnd": "2026-08-04T00:35:51.000Z",
            "membershipType": "ultra",
            "isUnlimited": false,
            "individualUsage": {
                "plan": {"autoPercentUsed": 98.1, "apiPercentUsed": 100, "totalPercentUsed": 98.5},
                "onDemand": {"enabled": false}
            }
        });
        let windows = parse(&json);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Plan");
        assert_eq!(windows[0].percent, 98.5);
        assert_eq!(
            windows[0].resets_at,
            parse_time(&json!("2026-08-04T00:35:51.000Z"))
        );
    }

    #[test]
    fn falls_back_to_the_pools_and_skips_seats_without_a_plan() {
        let pools =
            json!({"individualUsage": {"plan": {"autoPercentUsed": 40, "apiPercentUsed": 20}}});
        assert_eq!(parse(&pools)[0].percent, 30.);
        let over = json!({"individualUsage": {"plan": {"autoPercentUsed": 142.7}}});
        assert_eq!(parse(&over)[0].percent, 100.);
        let team = json!({"membershipType": "team", "teamUsage": {"onDemand": {"enabled": true}}});
        assert!(parse(&team).is_empty());
    }
}
