//! Verification of the exact remote branch that will be supplied to the hosting CLI.
use super::*;
use crate::pipeline::{Phase, Run};

/// Hosted repository and reviewed source branch checked against the remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedBranch {
    /// Hosting CLI repository selector (host/owner/repository).
    pub repository: String,
    /// Published source branch.
    pub branch: String,
    /// Verified commit at the remote branch tip.
    pub commit: String,
}

fn hosted_repository(
    url: &str,
    resolve_ssh: impl FnOnce(&str) -> Result<String, GitError>,
) -> Result<String, GitError> {
    let invalid =
        || GitError::Invalid("Cannot determine the hosting repository from the Git remote.".into());
    let (parts, ssh) = if let Some(rest) = url.strip_prefix("https://") {
        (rest.split_once('/'), false)
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        (rest.split_once('/'), true)
    } else {
        (url.split_once(':'), true)
    };
    let (authority, path) = parts.ok_or_else(invalid)?;
    let server = git::hosting::Server::parse(authority.rsplit('@').next().unwrap_or_default())
        .map_err(|_| invalid())?;
    let path = path
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_end_matches('/'));
    if !path.contains('/')
        || !path.split('/').all(|part| {
            !matches!(part, "" | "." | "..")
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        })
    {
        return Err(invalid());
    }
    let server = if ssh {
        let name = server.as_str().split(':').next().ok_or_else(invalid)?;
        git::hosting::Server::parse(web_host(&resolve_ssh(name)?)).map_err(|_| invalid())?
    } else {
        server
    };
    Ok(format!("{}/{path}", server.as_str()))
}

/// The hosting CLIs sign in to the service's own name, not to the SSH endpoint it offers on
/// port 443 for networks that block port 22.
fn web_host(ssh_host: &str) -> &str {
    if ssh_host.eq_ignore_ascii_case("ssh.github.com") {
        "github.com"
    } else if ssh_host.eq_ignore_ascii_case("altssh.gitlab.com") {
        "gitlab.com"
    } else {
        ssh_host
    }
}

/// The host OpenSSH would connect to, from the effective configuration `ssh -G` prints.
fn configured_hostname(config: &str) -> Option<&str> {
    config
        .lines()
        .find_map(|line| line.strip_prefix("hostname "))
        .map(str::trim)
        .filter(|host| !host.is_empty())
}

fn ssh_hostname(alias: &str) -> Result<String, GitError> {
    // OpenSSH expands Include, Match and wildcards locally without opening a connection.
    let output = command::run(
        Host::Local.command("ssh", &["-G", "--", alias], None),
        &[],
        &Cancellation::default(),
        git::READ_TIMEOUT,
        256 * 1024,
    )
    .map_err(|e| GitError::Invalid(format!("Resolving SSH host {alias}: {e}")))?;
    if output.code == Some(0)
        && let Some(host) = configured_hostname(&String::from_utf8_lossy(&output.stdout))
    {
        return Ok(host.to_owned());
    }
    Err(GitError::Invalid(format!(
        "Could not resolve SSH host {alias}."
    )))
}

/// Recheck the registered checkout, reviewed tree and live remote ref immediately before delivery.
pub fn publication(run: &Run, index: usize) -> Result<PublishedBranch, GitError> {
    let (url, branch, commit) = published_remote(run, index)?;
    Ok(PublishedBranch {
        repository: hosted_repository(&url, ssh_hostname)?,
        branch,
        commit,
    })
}

pub(super) fn published_remote(
    run: &Run,
    index: usize,
) -> Result<(String, String, String), GitError> {
    let lane = run
        .lanes()
        .get(index)
        .filter(|l| run.phase() == Phase::Accepted && !l.assignment.is_empty())
        .ok_or_else(|| GitError::Invalid("Accept this lane before opening a request.".into()))?;
    verify(run.id(), lane)?;
    let cwd = &lane.worktree;
    let name = branch(run.id(), lane);
    let reference = format!("refs/heads/{name}");
    let head = git::branches::resolve(&Host::Local, cwd, &reference)?;
    if !git::status::read(&Host::Local, cwd)?.is_clean() {
        return Err(GitError::Invalid(
            "Commit the accepted changes in the worktree first.".into(),
        ));
    }
    let actual = git::run(
        &Host::Local,
        cwd,
        &["rev-parse", "--verify", &format!("{head}^{{tree}}")],
    )?;
    let matches = match run.review_trees().get(index) {
        Some(expected) => expected == &actual,
        None => {
            run.review_revision()
                == Some(revision(run.id(), run.lanes(), &Cancellation::default())?.as_str())
        }
    };
    if !matches {
        return Err(GitError::Invalid(
            "The committed files differ from the accepted review. Inspect the worktree.".into(),
        ));
    }
    let upstream = git::run(
        &Host::Local,
        cwd,
        &[
            "for-each-ref",
            "--format=%(upstream:remotename)",
            &reference,
        ],
    )?;
    let remote = if upstream.is_empty() {
        "origin"
    } else {
        &upstream
    };
    let url = git::run(&Host::Local, cwd, &["remote", "get-url", "--", remote])?;
    let published = git::run(
        &Host::Local,
        cwd,
        &["ls-remote", "--refs", "--", remote, &reference],
    )?;
    if published.split_once('\t') != Some((head.as_str(), reference.as_str())) {
        return Err(GitError::Invalid(
            "Push the accepted commit to this branch before opening its request.".into(),
        ));
    }
    Ok((url, name, head))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hosting_selector_is_bound_to_the_git_remote_and_excludes_credentials() {
        for url in [
            "git@github.com:acme/tool.git",
            "ssh://git@github.com/acme/tool.git",
            "https://user@github.com/acme/tool",
        ] {
            assert_eq!(
                hosted_repository(url, |h| Ok(h.to_owned())).unwrap(),
                "github.com/acme/tool"
            );
        }
        assert_eq!(
            hosted_repository("git@work:acme/tool.git", |_| Ok("github.com".into())).unwrap(),
            "github.com/acme/tool"
        );
        // The port-443 SSH endpoints are not hosts the CLIs sign in to.
        for (ssh, web) in [
            ("ssh.github.com", "github.com/acme/tool"),
            ("altssh.gitlab.com", "gitlab.com/acme/tool"),
            ("git.example.com", "git.example.com/acme/tool"),
        ] {
            assert_eq!(
                hosted_repository("git@host:acme/tool.git", |_| Ok(ssh.into())).unwrap(),
                web
            );
        }
        for url in [
            "/local/path",
            "file:///local/repo",
            "https://github.com/a/../b",
        ] {
            assert!(hosted_repository(url, |h| Ok(h.to_owned())).is_err());
        }
    }

    #[test]
    fn ssh_configuration_names_the_host_and_malformed_output_names_none() {
        let config = "host work\nuser git\nhostname ssh.github.com\nport 443\nhostkeyalias x\n";
        assert_eq!(configured_hostname(config), Some("ssh.github.com"));
        for output in [
            "",
            "user git\nport 22\n",
            "hostname \n",
            "hostkeyalias github.com\n",
        ] {
            assert_eq!(configured_hostname(output), None, "{output:?}");
        }
    }
}
