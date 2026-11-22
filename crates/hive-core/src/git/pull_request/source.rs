//! Where a request is opened from: the hosting repository behind a Git remote, and the
//! published branch that holds exactly the commit to be reviewed.
use crate::command::{self, Cancellation};
use crate::git::{self, GitError, changes, hosting, status};
use crate::host::Host;
use std::path::Path;

/// Hosted repository and source branch checked against the remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedBranch {
    /// Hosting CLI repository selector (host/owner/repository).
    pub repository: String,
    /// Published source branch, as the remote names it.
    pub branch: String,
    /// Verified commit at the remote branch tip.
    pub commit: String,
}

/// Blocking: the hosting repository and remote branch that hold exactly the commit checked
/// out in `cwd`, which is what a request for the current branch is opened from.
pub fn published_branch(host: &Host, cwd: &Path) -> Result<PublishedBranch, GitError> {
    let source = remote_branch(host, cwd)?;
    Ok(PublishedBranch {
        repository: hosted_repository(host, &source.url)?,
        branch: source.branch,
        commit: source.commit,
    })
}

/// Blocking: the subject of the commit checked out in `cwd`, as a title to start a request
/// from. Empty when there is no commit yet.
pub fn suggested_title(host: &Host, cwd: &Path) -> Result<String, GitError> {
    if status::read(host, cwd)?.oid.is_none() {
        return Ok(String::new());
    }
    git::run(host, cwd, &["log", "-1", "--format=%s"])
}

/// The current branch as its remote holds it.
#[derive(Debug, PartialEq, Eq)]
struct RemoteBranch {
    /// Where the remote lives, as Git would contact it.
    url: String,
    /// What the remote calls the branch, which may differ from its local name.
    branch: String,
    /// The commit both sides agree on.
    commit: String,
}

fn remote_branch(host: &Host, cwd: &Path) -> Result<RemoteBranch, GitError> {
    let head = status::read(host, cwd)?;
    let commit = head
        .oid
        .ok_or_else(|| GitError::Invalid("Create the first commit first.".into()))?;
    let local = head
        .branch
        .ok_or_else(|| GitError::Invalid("Check out a branch before opening a request.".into()))?;
    let unpublished =
        || GitError::Invalid("Publish this branch before opening its request.".into());
    if head.upstream.is_none() {
        return Err(unpublished());
    }
    // A branch that tracks a local one has nothing on a hosting service yet.
    let remote = changes::upstream_remote(host, cwd, &local)?.ok_or_else(unpublished)?;
    let reference = changes::push_destination(host, cwd, &remote, false)?;
    let branch = reference
        .strip_prefix("refs/heads/")
        .ok_or_else(unpublished)?
        .to_owned();
    if !is_published(host, cwd, &remote, &reference, &commit)? {
        return Err(GitError::Invalid(format!(
            "Push {local} before opening its request: {remote}/{branch} does not hold its latest commit."
        )));
    }
    let url = git::run(host, cwd, &["remote", "get-url", "--", &remote])?;
    Ok(RemoteBranch {
        url,
        branch,
        commit,
    })
}

/// Blocking: whether `reference` on `remote` points exactly at `commit` right now, asked of
/// the remote itself rather than of the last fetch.
pub(crate) fn is_published(
    host: &Host,
    cwd: &Path,
    remote: &str,
    reference: &str,
    commit: &str,
) -> Result<bool, GitError> {
    let listed = git::run(
        host,
        cwd,
        &[
            "-c",
            "credential.interactive=false",
            "ls-remote",
            "--refs",
            "--",
            remote,
            reference,
        ],
    )?;
    // The pattern also matches longer refs that end the same way, so the name is compared.
    let tip = listed
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .find_map(|(oid, name)| (name == reference).then_some(oid));
    Ok(tip == Some(commit))
}

/// Blocking: the hosting CLI's selector (host/owner/repository) for the repository at `url`,
/// a Git remote of a repository on `host`. An SSH alias is resolved with that host's OpenSSH
/// configuration.
pub fn hosted_repository(host: &Host, url: &str) -> Result<String, GitError> {
    repository_selector(url, |alias| ssh_hostname(host, alias))
}

fn repository_selector(
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
    let server = hosting::Server::parse(authority.rsplit('@').next().unwrap_or_default())
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
        hosting::Server::parse(web_host(&resolve_ssh(name)?)).map_err(|_| invalid())?
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

fn ssh_hostname(host: &Host, alias: &str) -> Result<String, GitError> {
    // OpenSSH expands Include, Match and wildcards locally without opening a connection.
    let output = command::run(
        host.command("ssh", &["-G", "--", alias], None),
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

#[cfg(test)]
mod tests;
