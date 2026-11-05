//! Read-only GitHub CLI metadata; no authentication or repository mutations.
use crate::{command, host::Host};
use serde::Deserialize;
use std::{path::Path, time::Duration};

/// PR metadata returned by `gh pr view --json`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PullRequest {
    /// PR number, zero only for an invalid response.
    pub number: u64,
    /// Web URL; only exposed through `web_url`.
    url: String,
    /// GitHub's OPEN/CLOSED/MERGED state, unknown values remain visible.
    pub state: String,
    status_check_rollup: Option<Vec<Check>>,
}
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Check {
    status: String,
    conclusion: String,
    state: String,
    details_url: String,
    target_url: String,
}

impl PullRequest {
    /// A safe URL for the operating system's web browser.
    pub fn web_url(&self) -> Option<&str> {
        web_url(&self.url)
    }
    /// Current check summary, deliberately conservative for future status values.
    pub fn checks(&self) -> &'static str {
        let checks = self.status_check_rollup.as_deref().unwrap_or_default();
        if checks.is_empty() {
            return "No checks";
        }
        if checks.iter().any(Check::failed) {
            return "Checks failed";
        }
        if checks.iter().all(Check::passed) {
            "Checks passed"
        } else {
            "Checks pending"
        }
    }
    /// First failing check's web page, if the server supplied a valid web URL.
    pub fn failed_check_url(&self) -> Option<&str> {
        self.status_check_rollup
            .as_ref()?
            .iter()
            .filter(|c| c.failed())
            .find_map(|c| web_url(&c.details_url).or_else(|| web_url(&c.target_url)))
    }
}
impl Check {
    fn failed(&self) -> bool {
        matches!(
            self.conclusion.as_str(),
            "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
        ) || matches!(self.state.as_str(), "FAILURE" | "ERROR")
    }
    fn passed(&self) -> bool {
        (self.status == "COMPLETED"
            && matches!(self.conclusion.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED"))
            || self.state == "SUCCESS"
    }
}
fn web_url(value: &str) -> Option<&str> {
    let host = value.strip_prefix("https://")?.split('/').next()?;
    (!host.is_empty() && !value.chars().any(char::is_whitespace) && !host.contains('@'))
        .then_some(value)
}

/// Failed metadata retrieval; the UI keeps it distinct from a branch without a PR.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No local GitHub CLI was found.
    #[error("Install gh and run gh auth login to show PR checks.")]
    Missing,
    /// The bounded subprocess failed.
    #[error("Could not read PR: {0}")]
    Command(#[from] command::Error),
    /// CLI/API response could not be interpreted.
    #[error("{0}")]
    Response(String),
}

/// Blocking: read the current branch's PR using the host's existing gh authentication.
pub fn read(host: &Host, cwd: &Path) -> Result<Option<PullRequest>, Error> {
    let executable = if *host == Host::Local {
        command::executable("gh").ok_or(Error::Missing)?
    } else {
        "gh".into()
    };
    let args = ["pr", "view", "--json", "number,url,state,statusCheckRollup"];
    let mut cmd = host.command(&executable.to_string_lossy(), &args, Some(cwd));
    cmd.env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("NO_COLOR", "1");
    let output = command::run(
        cmd,
        &[],
        &command::Cancellation::default(),
        Duration::from_secs(15),
        1024 * 1024,
    )?;
    if output.code != Some(0) {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("no pull requests found for branch") {
            return Ok(None);
        }
        return Err(Error::Response(error.trim().chars().take(500).collect()));
    }
    let pr: PullRequest = serde_json::from_slice(&output.stdout)
        .map_err(|error| Error::Response(format!("Invalid gh response: {error}")))?;
    if pr.number == 0 || pr.web_url().is_none() {
        return Err(Error::Response("Invalid PR number or URL".into()));
    }
    Ok(Some(pr))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_github_cli_response() {
        // gh pr view 5 --repo vitaliits-stack/hive, captured 2026-11-05.
        let pr: PullRequest = serde_json::from_str(include_str!("fixtures/gh-pr-5.json")).unwrap();
        assert_eq!(pr.number, 5);
        assert_eq!(pr.state, "MERGED");
        assert_eq!(pr.checks(), "Checks passed");
        assert_eq!(
            pr.web_url(),
            Some("https://github.com/vitaliits-stack/hive/pull/5")
        );
        assert_eq!(pr.failed_check_url(), None);
    }

    #[test]
    fn understands_checks_and_legacy_status_contexts_without_false_success() {
        let mut pr: PullRequest = serde_json::from_str(r#"{"number":7,"url":"https://github.com/a/b/pull/7","state":"OPEN","statusCheckRollup":[{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"},{"__typename":"StatusContext","state":"FAILURE","targetUrl":"https://ci.example/run/7"}]}"#).unwrap();
        assert_eq!(pr.checks(), "Checks failed");
        assert_eq!(pr.failed_check_url(), Some("https://ci.example/run/7"));
        pr.status_check_rollup = Some(vec![Check {
            state: "future".into(),
            ..Default::default()
        }]);
        assert_eq!(pr.checks(), "Checks pending");
        pr.status_check_rollup = None;
        assert_eq!(pr.checks(), "No checks");
        assert!(web_url("file:///tmp/a").is_none());
        assert!(web_url("https://").is_none());
    }
}
