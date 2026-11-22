use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A repository in its own temporary directory, removed when dropped.
struct Repo(PathBuf);

impl Repo {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hive-request-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        let repo = Self(path.canonicalize().unwrap());
        for args in [
            &["init", "-b", "main"][..],
            &["config", "user.name", "Hive Test"],
            &["config", "user.email", "hive@example.invalid"],
            &["config", "commit.gpgsign", "false"],
            &["config", "core.hooksPath", "/dev/null"],
        ] {
            repo.git(args);
        }
        repo
    }

    fn git(&self, args: &[&str]) -> String {
        git::run(&Host::Local, &self.0, args).unwrap()
    }

    fn commit(&self, path: &str, content: &str) {
        std::fs::write(self.0.join(path), content).unwrap();
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", &format!("change {path}")]);
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A repository with one commit on `main`, published to the returned bare remote.
fn published() -> (Repo, Repo) {
    let repo = Repo::new();
    let remote = Repo::new();
    remote.git(&["config", "core.bare", "true"]);
    repo.git(&["remote", "add", "origin", remote.0.to_str().unwrap()]);
    repo.commit("base", "base\n");
    repo.git(&["push", "--set-upstream", "origin", "main"]);
    (repo, remote)
}

fn refusal(repo: &Repo) -> String {
    remote_branch(&Host::Local, &repo.0)
        .unwrap_err()
        .to_string()
}

#[test]
fn the_source_is_the_remote_branch_holding_the_checked_out_commit() {
    let (repo, remote) = published();
    let source = remote_branch(&Host::Local, &repo.0).unwrap();
    assert_eq!(source.branch, "main");
    assert_eq!(source.commit, repo.git(&["rev-parse", "HEAD"]));
    assert_eq!(source.url, remote.0.to_str().unwrap());
    assert_eq!(
        suggested_title(&Host::Local, &repo.0).unwrap(),
        "change base"
    );
}

#[test]
fn the_source_is_named_as_the_remote_names_it() {
    let (repo, remote) = published();
    remote.git(&["branch", "review"]);
    repo.git(&["fetch", "origin"]);
    repo.git(&["switch", "-c", "topic", "--track", "origin/review"]);
    assert_eq!(
        remote_branch(&Host::Local, &repo.0).unwrap().branch,
        "review"
    );
}

#[test]
fn a_request_needs_the_latest_commit_on_the_remote() {
    let (repo, remote) = published();
    repo.commit("more", "more\n");
    let error = refusal(&repo);
    assert!(
        error.contains("Push main before opening its request"),
        "{error}"
    );
    assert!(
        error.contains("origin/main does not hold its latest commit"),
        "{error}"
    );
    repo.git(&["push", "origin", "main"]);
    assert!(remote_branch(&Host::Local, &repo.0).is_ok());

    // The remote moved on after the last fetch: the local commit is no longer its tip.
    let other = Repo::new();
    other.git(&["remote", "add", "origin", remote.0.to_str().unwrap()]);
    other.git(&["fetch", "origin"]);
    other.git(&["reset", "--hard", "origin/main"]);
    other.commit("theirs", "theirs\n");
    other.git(&["push", "origin", "HEAD:main"]);
    assert!(refusal(&repo).contains("does not hold its latest commit"));
}

#[test]
fn a_ref_that_only_ends_like_the_branch_is_not_the_branch() {
    let (repo, remote) = published();
    let head = repo.git(&["rev-parse", "HEAD"]);
    // `ls-remote refs/heads/main` also lists this ref.
    remote.git(&["update-ref", "refs/heads/archive/refs/heads/main", &head]);
    let published = |commit: &str| {
        is_published(&Host::Local, &repo.0, "origin", "refs/heads/main", commit).unwrap()
    };
    assert!(published(&head));
    repo.commit("more", "more\n");
    assert!(!published(&repo.git(&["rev-parse", "HEAD"])));
    assert!(!is_published(&Host::Local, &repo.0, "origin", "refs/heads/missing", &head).unwrap());
}

#[test]
fn unborn_detached_unpublished_and_locally_tracked_branches_have_no_source() {
    let unborn = Repo::new();
    assert!(refusal(&unborn).contains("Create the first commit first"));
    assert_eq!(suggested_title(&Host::Local, &unborn.0).unwrap(), "");

    let (repo, _remote) = published();
    repo.git(&["switch", "-c", "unpublished"]);
    assert!(refusal(&repo).contains("Publish this branch before opening its request"));
    repo.git(&["switch", "-c", "local", "--track", "main"]);
    assert!(refusal(&repo).contains("Publish this branch before opening its request"));
    repo.git(&["switch", "--detach"]);
    assert!(refusal(&repo).contains("Check out a branch before opening a request"));
}

#[test]
fn hosting_selector_is_bound_to_the_git_remote_and_excludes_credentials() {
    for url in [
        "git@github.com:acme/tool.git",
        "ssh://git@github.com/acme/tool.git",
        "https://user@github.com/acme/tool",
    ] {
        assert_eq!(
            repository_selector(url, |h| Ok(h.to_owned())).unwrap(),
            "github.com/acme/tool"
        );
    }
    assert_eq!(
        repository_selector("git@work:acme/tool.git", |_| Ok("github.com".into())).unwrap(),
        "github.com/acme/tool"
    );
    // The port-443 SSH endpoints are not hosts the CLIs sign in to.
    for (ssh, web) in [
        ("ssh.github.com", "github.com/acme/tool"),
        ("altssh.gitlab.com", "gitlab.com/acme/tool"),
        ("git.example.com", "git.example.com/acme/tool"),
    ] {
        assert_eq!(
            repository_selector("git@host:acme/tool.git", |_| Ok(ssh.into())).unwrap(),
            web
        );
    }
    for url in [
        "/local/path",
        "file:///local/repo",
        "https://github.com/a/../b",
    ] {
        assert!(repository_selector(url, |h| Ok(h.to_owned())).is_err());
    }
}

#[test]
fn a_local_remote_has_no_hosting_repository() {
    let (repo, _remote) = published();
    let error = published_branch(&Host::Local, &repo.0)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Cannot determine the hosting repository"),
        "{error}"
    );
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
