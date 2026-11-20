//! Lookup for an explicit same-repository source, independent of the current checkout.
use super::*;
use serde::Deserialize;

/// Find the open request or a finished request for this exact commit and repository.
pub fn read_published(
    host: &Host,
    cwd: &Path,
    repository: &str,
    branch: &str,
    commit: &str,
) -> Result<Snapshot, Error> {
    let provider = match repository.split('/').next() {
        Some("github.com") => Provider::GitHub,
        Some("gitlab.com") => Provider::GitLab,
        _ => read(host, cwd)?.provider(),
    };
    let selector = repository_argument(provider, repository);
    let repository = selector.as_ref();
    let args = match provider {
        Provider::GitHub => vec![
            "pr",
            "list",
            "--repo",
            repository,
            "--head",
            branch,
            "--json",
            "number,url,state,statusCheckRollup,isCrossRepository,headRefOid",
            "--state",
            "all",
        ],
        Provider::GitLab => vec![
            "mr",
            "list",
            "--repo",
            repository,
            "--all",
            "--source-branch",
            branch,
            "--output",
            "json",
        ],
    };
    let output = run_cli(host, cwd, provider, &args)?;
    if output.code != Some(0) {
        return Err(response_error(provider.cli(), &output.stderr));
    }
    let request = match provider {
        Provider::GitHub => same_repository_pr(&output.stdout, commit)?,
        Provider::GitLab => {
            let Some(iid) = same_project_mr(&output.stdout, commit)? else {
                return Ok(Snapshot {
                    provider,
                    request: None,
                });
            };
            let output = run_cli(
                host,
                cwd,
                provider,
                &[
                    "mr",
                    "view",
                    &iid.to_string(),
                    "--repo",
                    repository,
                    "--output",
                    "json",
                ],
            )?;
            if output.code != Some(0) {
                return Err(response_error(provider.cli(), &output.stderr));
            }
            Some(gitlab::parse(&output.stdout)?)
        }
    };
    Ok(Snapshot { provider, request })
}

/// Refresh a previously delivered request by its permanent web address even if its branch was deleted.
pub fn read_link(host: &Host, cwd: &Path, url: &str) -> Result<PullRequest, Error> {
    let invalid = || Error::Response("Invalid saved request address.".into());
    let address = web_url(url)
        .and_then(|s| s.strip_prefix("https://"))
        .ok_or_else(invalid)?;
    let (provider, (repository, number)) =
        if let Some(parts) = address.rsplit_once("/-/merge_requests/") {
            (Provider::GitLab, parts)
        } else {
            (
                Provider::GitHub,
                address.rsplit_once("/pull/").ok_or_else(invalid)?,
            )
        };
    if !repository.contains('/') || number.parse::<u64>().ok().filter(|i| *i > 0).is_none() {
        return Err(invalid());
    }
    let selector = repository_argument(provider, repository);
    let args = match provider {
        Provider::GitHub => vec![
            "pr",
            "view",
            number,
            "--repo",
            &selector,
            "--json",
            "number,url,state,statusCheckRollup",
        ],
        Provider::GitLab => vec![
            "mr", "view", number, "--repo", &selector, "--output", "json",
        ],
    };
    let response = run_cli(host, cwd, provider, &args)?;
    if response.code != Some(0) {
        return Err(response_error(provider.cli(), &response.stderr));
    }
    match provider {
        Provider::GitHub => parse_github(&response.stdout),
        Provider::GitLab => gitlab::parse(&response.stdout),
    }
}

fn same_repository_pr(bytes: &[u8], commit: &str) -> Result<Option<PullRequest>, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Candidate {
        is_cross_repository: bool,
        head_ref_oid: String,
        #[serde(flatten)]
        request: PullRequest,
    }
    let candidates: Vec<Candidate> = serde_json::from_slice(bytes)
        .map_err(|e| Error::Response(format!("Invalid PR list: {e}")))?;
    let found = candidates
        .into_iter()
        .find(|c| !c.is_cross_repository && (c.request.state == "OPEN" || c.head_ref_oid == commit))
        .map(|c| c.request);
    if found
        .as_ref()
        .is_some_and(|pr| pr.number == 0 || pr.web_url().is_none())
    {
        return Err(Error::Response("Invalid PR number or URL".into()));
    }
    Ok(found)
}

fn same_project_mr(bytes: &[u8], commit: &str) -> Result<Option<u64>, Error> {
    #[derive(Deserialize)]
    struct Entry {
        iid: u64,
        source_project_id: Option<u64>,
        target_project_id: u64,
        state: String,
        sha: Option<String>,
    }
    let entries: Vec<Entry> = serde_json::from_slice(bytes)
        .map_err(|e| Error::Response(format!("Invalid MR list: {e}")))?;
    Ok(entries
        .into_iter()
        .find(|e| {
            e.iid > 0
                && e.source_project_id == Some(e.target_project_id)
                && (e.state == "opened" || e.sha.as_deref() == Some(commit))
        })
        .map(|e| e.iid))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finished_requests_match_the_verified_commit_after_restart() {
        let rows=br#"[{"number":2,"url":"https://github.com/o/r/pull/2","state":"MERGED","isCrossRepository":true,"headRefOid":"accepted"},{"number":1,"url":"https://github.com/o/r/pull/1","state":"MERGED","isCrossRepository":false,"headRefOid":"accepted"}]"#;
        assert_eq!(
            same_repository_pr(rows, "accepted")
                .unwrap()
                .unwrap()
                .number,
            1
        );
        assert!(same_repository_pr(rows, "later").unwrap().is_none());
    }

    #[test]
    fn same_named_fork_branch_is_excluded() {
        let rows=br#"[{"iid":1,"source_project_id":4,"target_project_id":3,"state":"opened"},{"iid":2,"source_project_id":3,"target_project_id":3,"state":"opened"}]"#;
        assert_eq!(same_project_mr(rows, "sha").unwrap(), Some(2));
        assert!(same_project_mr(b"{}", "sha").is_err());
        assert_eq!(
            repository_argument(Provider::GitLab, "gitlab.com/team/nested/repo"),
            "https://gitlab.com/team/nested/repo"
        );
    }
}
