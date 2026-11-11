use super::{Check, Error, PullRequest, response_error, run_cli};
use crate::host::Host;
use serde::Deserialize;
use std::path::Path;

#[derive(Default, Deserialize)]
#[serde(default)]
struct MergeRequest {
    iid: u64,
    web_url: String,
    state: String,
    head_pipeline: Option<Pipeline>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Pipeline {
    status: String,
    web_url: String,
}

pub(super) fn read(host: &Host, cwd: &Path) -> Result<Option<PullRequest>, Error> {
    let output = run_cli(host, cwd, "glab", &["mr", "view", "--output", "json"])?;
    response(output)
}

fn response(output: crate::command::Output) -> Result<Option<PullRequest>, Error> {
    if output.code != Some(0) {
        // glab uses this exact message for an empty branch search, including --state any.
        if String::from_utf8_lossy(&output.stderr).contains("no open merge request available for ")
        {
            return Ok(None);
        }
        return Err(response_error("glab", &output.stderr));
    }
    parse(&output.stdout).map(Some)
}

fn parse(bytes: &[u8]) -> Result<PullRequest, Error> {
    let mr: MergeRequest = serde_json::from_slice(bytes)
        .map_err(|error| Error::Response(format!("Invalid glab response: {error}")))?;
    let request = PullRequest {
        number: mr.iid,
        url: mr.web_url,
        state: mr.state,
        status_check_rollup: mr.head_pipeline.map(|pipeline| {
            let conclusion = match pipeline.status.as_str() {
                "success" => "SUCCESS",
                "skipped" => "SKIPPED",
                "failed" => "FAILURE",
                "canceled" => "CANCELLED",
                _ => "",
            };
            vec![Check {
                status: "COMPLETED".into(),
                conclusion: conclusion.into(),
                details_url: pipeline.web_url,
                ..Default::default()
            }]
        }),
    };
    if request.number == 0 || request.web_url().is_none() {
        return Err(Error::Response("Invalid MR IID or URL".into()));
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_branch_is_distinct_from_auth_server_and_ambiguous_mr_errors() {
        let output = |stderr: &str| crate::command::Output {
            code: Some(1),
            stdout: vec![],
            stderr: stderr.as_bytes().into(),
        };
        assert!(
            response(output("no open merge request available for \"topic\""))
                .unwrap()
                .is_none()
        );
        for error in [
            "401 Unauthorized",
            "failed to get open merge request for topic: 500",
            "you must select a merge request: merge request ID number required",
        ] {
            assert!(response(output(error)).is_err());
        }
    }

    #[test]
    fn captured_gitlab_merge_request_uses_project_iid_and_head_pipeline() {
        // glab 1.130.0 mr view 4383 --repo gitlab.com/gitlab-org/cli --output json,
        // captured 2026-11-11; retained the unmodified fields this adapter consumes.
        let request = parse(include_bytes!("../fixtures/gitlab-mr-4383.json")).unwrap();
        assert_eq!(request.number, 4383);
        assert_eq!(request.state, "merged");
        assert_eq!(
            request.web_url(),
            Some("https://gitlab.com/gitlab-org/cli/-/merge_requests/4383")
        );
        assert_eq!(request.checks(), "Checks passed");
    }

    #[test]
    fn self_managed_urls_and_pipeline_outcomes_remain_conservative() {
        for (status, expected) in [
            ("success", "Checks passed"),
            ("skipped", "Checks passed"),
            ("failed", "Checks failed"),
            ("canceled", "Checks failed"),
            ("running", "Checks pending"),
            ("pending", "Checks pending"),
            ("manual", "Checks pending"),
            ("future", "Checks pending"),
        ] {
            let value = serde_json::json!({"iid": 9, "web_url": "https://code.example:8443/group/sub/repo/-/merge_requests/9", "state": "opened", "head_pipeline": {"status": status, "web_url": "https://code.example:8443/group/sub/repo/-/pipelines/20"}});
            let request = parse(&serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(request.checks(), expected, "{status}");
            assert_eq!(
                request.failed_check_url().is_some(),
                expected == "Checks failed"
            );
        }
    }

    #[test]
    fn missing_pipeline_unknown_fields_and_missing_state_are_supported() {
        for pipeline in [serde_json::Value::Null, serde_json::json!({})] {
            let value = serde_json::json!({"iid": 1, "web_url": "https://gitlab.com/a/b/-/merge_requests/1", "head_pipeline": pipeline, "future": true});
            let request = parse(&serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(request.state, "");
            assert!(matches!(request.checks(), "No checks" | "Checks pending"));
        }
        let request =
            parse(br#"{"iid":1,"web_url":"https://gitlab.com/a/b/-/merge_requests/1"}"#).unwrap();
        assert_eq!(request.checks(), "No checks");
    }

    #[test]
    fn malformed_responses_and_unsafe_links_are_rejected() {
        for value in [
            "",
            "{",
            "null",
            "[]",
            "{}",
            r#"{"id":7,"web_url":"https://gitlab.com/a/b"}"#,
            r#"{"iid":7,"web_url":"file:///tmp/a"}"#,
            r#"{"iid":7,"web_url":"https://user:password@code.example/a"}"#,
        ] {
            assert!(parse(value.as_bytes()).is_err(), "{value}");
        }
        let request = parse(br#"{"iid":1,"web_url":"https://code.example/a/b","head_pipeline":{"status":"failed","web_url":"javascript:alert(1)"}}"#).unwrap();
        assert_eq!(request.checks(), "Checks failed");
        assert_eq!(request.failed_check_url(), None);
    }
}
