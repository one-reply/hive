//! Read-only GitHub/GitLab CLI metadata using the host's existing authentication.
mod gitlab;
use crate::{command, host::Host};
use serde::Deserialize;
use std::{path::Path, time::Duration};

pub use super::hosting::Provider;

/// A branch's metadata, retaining the provider even when it has no request.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    provider: Provider,
    request: Option<PullRequest>,
}
impl Snapshot {
    /// Hosting service used for this read.
    pub fn provider(&self) -> Provider {
        self.provider
    }
    /// The current branch's pull or merge request, if one exists.
    pub fn request(&self) -> Option<&PullRequest> {
        self.request.as_ref()
    }
}

/// Pull or merge request metadata normalized from the hosting CLI.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PullRequest {
    /// Repository-local PR number or MR IID, zero only for an invalid response.
    pub number: u64,
    /// Web URL; only exposed through `web_url`.
    url: String,
    /// Hosting service's request state; unknown values remain visible.
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
    /// The hosting CLI was not found on the project host.
    #[error(
        "Install gh and run gh auth login for GitHub, or install glab and run glab auth login --hostname <host> for GitLab."
    )]
    Missing,
    /// The bounded subprocess failed.
    #[error("Could not read PR/MR: {0}")]
    Command(#[from] command::Error),
    /// The hosting service rejected credentials or denied account access.
    #[error("{provider:?}: {failure}")]
    Authentication {
        /// Provider whose API rejected the request.
        provider: Provider,
        /// Authentication failure category.
        failure: AuthFailure,
    },
    /// CLI/API response could not be interpreted.
    #[error("{0}")]
    Response(String),
}

/// Provider authentication failure; distinct from network and repository errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthFailure {
    /// Credentials are absent or expired.
    #[error("Sign in to your hosting account.")]
    SignedOut,
    /// The account lacks access.
    #[error("The server denied access; check account permissions.")]
    AccessDenied,
}
impl Error {
    /// Provider for which signing in or checking access may resolve this failure.
    pub fn authentication_provider(&self) -> Option<Provider> {
        match self {
            Self::Authentication { provider, .. } => Some(*provider),
            _ => None,
        }
    }
}

/// Blocking: read the branch's PR/MR without changing authentication or the repository.
pub fn read(host: &Host, cwd: &Path) -> Result<Snapshot, Error> {
    resolve(read_github(host, cwd), || gitlab::read(host, cwd))
}

fn resolve(
    github: Result<Option<PullRequest>, Error>,
    gitlab: impl FnOnce() -> Result<Option<PullRequest>, Error>,
) -> Result<Snapshot, Error> {
    let (provider, request) = match github {
        Ok(request) => (Provider::GitHub, request),
        Err(error) if try_gitlab(&error) => (Provider::GitLab, gitlab()?),
        Err(error) => return Err(error),
    };
    Ok(Snapshot { provider, request })
}

// Let the CLIs resolve SSH aliases and configured enterprise hosts themselves.
// API/network/parse failures on a recognized GitHub repository must stay visible.
fn try_gitlab(error: &Error) -> bool {
    match error {
        Error::Missing => true,
        Error::Response(message) => message.contains(
            "none of the git remotes configured for this repository point to a known GitHub host",
        ) || message
            .contains("To get started with GitHub CLI, please run:"),
        Error::Command(_) | Error::Authentication { .. } => false,
    }
}

fn run_cli(
    host: &Host,
    cwd: &Path,
    provider: Provider,
    args: &[&str],
) -> Result<command::Output, Error> {
    let command =
        super::hosting::cli_command_on(provider, host, Some(cwd), args).ok_or(Error::Missing)?;
    let output = command::run(
        command,
        &[],
        &command::Cancellation::default(),
        Duration::from_secs(15),
        1024 * 1024,
    )?;
    if output.code == Some(127) {
        return Err(Error::Missing);
    }
    Ok(output)
}

fn response_error(program: &str, stderr: &[u8]) -> Error {
    let message = String::from_utf8_lossy(stderr);
    let lower = message.to_ascii_lowercase();
    let failure =
        if super::hosting::has_http_status(&lower, "401") || lower.contains("not logged in") {
            Some(AuthFailure::SignedOut)
        } else if super::hosting::has_http_status(&lower, "403") {
            Some(AuthFailure::AccessDenied)
        } else {
            None
        };
    if let Some(failure) = failure {
        return Error::Authentication {
            provider: if program == "gh" {
                Provider::GitHub
            } else {
                Provider::GitLab
            },
            failure,
        };
    }
    Error::Response(format!(
        "{program}: {}",
        message.trim().chars().take(500).collect::<String>()
    ))
}

fn read_github(host: &Host, cwd: &Path) -> Result<Option<PullRequest>, Error> {
    let output = run_cli(
        host,
        cwd,
        Provider::GitHub,
        &["pr", "view", "--json", "number,url,state,statusCheckRollup"],
    )?;
    github_response(output)
}

fn github_response(output: command::Output) -> Result<Option<PullRequest>, Error> {
    if output.code != Some(0) {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("no pull requests found for branch") {
            return Ok(None);
        }
        return Err(response_error("gh", &output.stderr));
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
    fn authentication_failures_keep_provider_and_do_not_match_dns_errors() {
        for (program, provider) in [("gh", Provider::GitHub), ("glab", Provider::GitLab)] {
            for status in ["HTTP 401: Unauthorized", "403 Forbidden", "not logged in"] {
                assert_eq!(
                    response_error(program, status.as_bytes()).authentication_provider(),
                    Some(provider)
                );
            }
            for failure in [
                "lookup git4013.example.com failed",
                "dial host:4401: refused",
                "ambiguous merge requests",
                "SSH timeout",
            ] {
                assert_eq!(
                    response_error(program, failure.as_bytes()).authentication_provider(),
                    None
                );
            }
        }
    }
    #[test]
    fn provider_is_retained_without_a_request_and_github_errors_do_not_fall_through() {
        let github = resolve(Ok(None), || panic!("GitHub already resolved")).unwrap();
        assert_eq!(github.provider(), Provider::GitHub);
        assert!(github.request().is_none());
        let gitlab = resolve(Err(Error::Missing), || Ok(None)).unwrap();
        assert_eq!(gitlab.provider(), Provider::GitLab);
        assert!(gitlab.request().is_none());
        assert!(
            resolve(Err(Error::Command(command::Error::TimedOut)), || panic!(
                "must preserve GitHub failure"
            ))
            .is_err()
        );
        assert!(matches!(
            resolve(Err(Error::Missing), || Err(Error::Missing)),
            Err(Error::Missing)
        ));
    }
    #[test]
    fn only_unavailable_github_context_tries_gitlab() {
        assert!(try_gitlab(&Error::Missing));
        for error in [
            "none of the git remotes configured for this repository point to a known GitHub host. To tell gh about a new GitHub host, please use `gh auth login`",
            "To get started with GitHub CLI, please run:  gh auth login",
        ] {
            assert!(try_gitlab(&response_error("gh", error.as_bytes())));
        }
        for error in [
            "HTTP 401: Bad credentials",
            "HTTP 500",
            "connection refused",
            "Invalid gh response",
        ] {
            assert!(!try_gitlab(&response_error("gh", error.as_bytes())));
        }
        assert!(!try_gitlab(&Error::Command(command::Error::TimedOut)));
    }

    #[test]
    fn github_no_request_and_failures_are_distinct() {
        let output = |code, stdout: &str, stderr: &str| command::Output {
            code: Some(code),
            stdout: stdout.as_bytes().into(),
            stderr: stderr.as_bytes().into(),
        };
        assert!(
            github_response(output(1, "", "no pull requests found for branch \"topic\""))
                .unwrap()
                .is_none()
        );
        assert!(github_response(output(1, "", "HTTP 403: Forbidden")).is_err());
        for response in [
            "",
            "{",
            "null",
            "{}",
            r#"{"number":1,"url":"file:///tmp/a"}"#,
        ] {
            assert!(github_response(output(0, response, "")).is_err());
        }
    }
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
