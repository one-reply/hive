//! Opening a pull or merge request for the branch a worktree has checked out.
use super::{Error, Provider, response_error, run_cli_within, web_url};
use crate::git::branches;
use crate::host::Host;
use std::path::Path;
use std::time::Duration;

/// The hosting API answers in seconds; this bounds a request nobody can interrupt.
const CREATE_TIMEOUT: Duration = Duration::from_secs(60);

/// What to open for the current branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    /// One-line title.
    pub title: String,
    /// Description, as Markdown.
    pub body: String,
    /// Branch to merge into; empty leaves it to the hosting CLI, which uses the
    /// repository's default branch.
    pub base: String,
}

/// Open a request from an explicitly named, published branch in the supplied hosting repository.
pub fn create(
    provider: Provider,
    host: &Host,
    cwd: &Path,
    repository: &str,
    branch: &str,
    draft: &Draft,
) -> Result<String, Error> {
    if draft.title.trim().is_empty() {
        return Err(Error::Response("Enter a title for the request.".into()));
    }
    let base = draft.base.trim();
    if !base.is_empty() && !branches::is_valid_name(base) {
        return Err(Error::Response(format!("“{base}” is not a branch name.")));
    }
    if !branches::is_valid_name(branch) || repository.is_empty() {
        return Err(Error::Response(
            "Invalid published source branch or repository.".into(),
        ));
    }
    let repository = super::repository_argument(provider, repository);
    let output = run_cli_within(
        host,
        cwd,
        provider,
        &arguments(provider, &repository, branch, draft),
        CREATE_TIMEOUT,
    )?;
    if output.code != Some(0) {
        return Err(response_error(provider.cli(), &output.stderr));
    }
    created_url(&output.stdout).ok_or_else(|| {
        Error::Response(format!(
            "{} did not report the new request's address.",
            provider.cli()
        ))
    })
}

/// The CLI arguments that open `draft` without a prompt, an editor or a push.
fn arguments<'a>(
    provider: Provider,
    repository: &'a str,
    branch: &'a str,
    draft: &'a Draft,
) -> Vec<&'a str> {
    let (title, body, base) = (draft.title.trim(), draft.body.as_str(), draft.base.trim());
    let mut arguments = match provider {
        Provider::GitHub => vec!["pr", "create", "--title", title, "--body", body],
        Provider::GitLab => vec![
            "mr",
            "create",
            "--title",
            title,
            "--description",
            body,
            "--yes",
            "--push=false",
        ],
    };
    arguments.extend([
        "--repo",
        repository,
        match provider {
            Provider::GitHub => "--head",
            Provider::GitLab => "--source-branch",
        },
        branch,
    ]);
    if !base.is_empty() {
        arguments.push(match provider {
            Provider::GitHub => "--base",
            Provider::GitLab => "--target-branch",
        });
        arguments.push(base);
    }
    arguments
}

/// The request's address in what the CLI printed: `gh` prints it alone, `glab` after a
/// summary line.
fn created_url(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .split_whitespace()
        .rev()
        .find_map(web_url)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(base: &str) -> Draft {
        Draft {
            title: "  Add login  ".into(),
            body: "## Task\nAdd login".into(),
            base: base.into(),
        }
    }

    #[test]
    fn each_provider_gets_its_own_flags_and_glab_is_told_not_to_push_or_prompt() {
        assert_eq!(
            arguments(Provider::GitHub, "host/o/r", "topic", &draft("")),
            [
                "pr",
                "create",
                "--title",
                "Add login",
                "--body",
                "## Task\nAdd login",
                "--repo",
                "host/o/r",
                "--head",
                "topic"
            ]
        );
        assert_eq!(
            arguments(Provider::GitHub, "host/o/r", "topic", &draft(" develop "))[10..],
            ["--base", "develop"]
        );
        let merge_request = draft("develop");
        let gitlab = arguments(Provider::GitLab, "host/o/r", "topic", &merge_request);
        assert_eq!(gitlab[..2], ["mr", "create"]);
        assert!(gitlab.contains(&"--yes") && gitlab.contains(&"--push=false"));
        assert_eq!(gitlab[gitlab.len() - 2..], ["--target-branch", "develop"]);
        assert!(!gitlab.contains(&"--fill") && !gitlab.contains(&"--base"));
    }

    #[test]
    fn the_new_address_is_found_in_either_clis_output() {
        assert_eq!(
            created_url(b"https://github.com/acme/app/pull/12\n").as_deref(),
            Some("https://github.com/acme/app/pull/12")
        );
        let glab = b"!7 Add login (feature/login)\n https://gitlab.example.com/acme/app/-/merge_requests/7\n\n";
        assert_eq!(
            created_url(glab).as_deref(),
            Some("https://gitlab.example.com/acme/app/-/merge_requests/7")
        );
        assert_eq!(created_url(b"Creating pull request...\n"), None);
        assert_eq!(created_url(b""), None);
    }

    #[test]
    fn a_draft_without_a_title_or_with_an_impossible_base_never_reaches_the_cli() {
        let nowhere = Path::new("/nonexistent-hive-test-directory");
        for (title, base, expected) in [
            ("   ", "", "Enter a title"),
            ("Add login", "bad name", "is not a branch name"),
            ("Add login", "--web", "is not a branch name"),
        ] {
            let draft = Draft {
                title: title.into(),
                body: String::new(),
                base: base.into(),
            };
            let error = create(
                Provider::GitHub,
                &Host::Local,
                nowhere,
                "host/o/r",
                "topic",
                &draft,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }
}
