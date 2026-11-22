use super::{Opened, PullRequestEvent, PullRequests, key};
use crate::state::{git::Git, projects::Projects, store::Store};
use gpui_kit as gpui;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use hive_core::git::{changes::Action, pull_request::Draft};
use hive_core::{host::Host, projects::Target, state::StateError};
use std::sync::{Arc, Mutex};

/// Source control and the request state over a project list that never touches the disk.
fn states(cx: &mut TestAppContext) -> (Entity<Git>, Entity<PullRequests>) {
    let store = cx.new(|cx| {
        Store::new(
            Err(StateError::Io {
                operation: "test",
                path: "/unused".into(),
                source: std::io::Error::other("persistence disabled for test"),
            }),
            cx,
        )
    });
    let projects = cx.new(|cx| Projects::new(&store, cx));
    let git = cx.new(|cx| Git::new(projects.clone(), cx));
    let requests = cx.new(|cx| PullRequests::new(projects, git.clone(), cx));
    (git, requests)
}

fn worktree(path: &str) -> Target {
    Target {
        host: Host::Local,
        project: path.into(),
        path: path.into(),
    }
}

fn draft(title: &str) -> Draft {
    Draft {
        title: title.into(),
        ..Draft::default()
    }
}

/// The titles of the requests a fake hosting service was asked to open.
type Asked = Arc<Mutex<Vec<String>>>;

/// A hosting service that records what it was asked to open and answers `answer`.
fn hosting(
    answer: Result<&'static str, &'static str>,
) -> (
    Asked,
    impl Fn(&Target, &Draft) -> Result<Opened, String> + Send + 'static,
) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let record = asked.clone();
    let request = move |_: &Target, draft: &Draft| {
        record.lock().unwrap().push(draft.title.clone());
        answer
            .map(|url| Opened {
                branch: "feature".into(),
                url: url.into(),
            })
            .map_err(String::from)
    };
    (asked, request)
}

/// The events `requests` emits from now on.
fn events(
    requests: &Entity<PullRequests>,
    cx: &mut TestAppContext,
) -> Arc<Mutex<Vec<PullRequestEvent>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    cx.update(|cx| {
        cx.subscribe(requests, move |_, event: &PullRequestEvent, _| {
            record.lock().unwrap().push(event.clone());
        })
        .detach();
    });
    seen
}

#[gpui::test]
fn an_opened_request_is_announced_and_its_draft_is_spent(cx: &mut TestAppContext) {
    let (git, requests) = states(cx);
    let target = worktree("/unused/worktree");
    let (asked, hosting) = hosting(Ok("https://example.invalid/pull/1"));
    let seen = events(&requests, cx);

    requests.update(cx, |requests, cx| {
        requests.open_with(
            git.clone(),
            target.clone(),
            draft("Add menu"),
            None,
            hosting,
            cx,
        );
        assert!(requests.is_opening(&target));
        // A second click while the first request is on its way opens nothing more.
        let (_, again) = super::tests::hosting(Ok("https://example.invalid/pull/2"));
        requests.open_with(git.clone(), target.clone(), draft("Again"), None, again, cx);
    });
    cx.run_until_parked();

    assert_eq!(*asked.lock().unwrap(), ["Add menu"]);
    assert_eq!(
        *seen.lock().unwrap(),
        [PullRequestEvent::Opened {
            branch: "feature".into(),
            url: "https://example.invalid/pull/1".into(),
        }]
    );
    requests.read_with(cx, |requests, _| {
        assert!(!requests.is_opening(&target));
        assert!(!requests.drafts.contains_key(&key(&target)));
        // The card is read again at once instead of at the next minute.
        assert!(requests.retry.contains(&key(&target)));
    });
}

#[gpui::test]
async fn a_failed_request_keeps_the_draft_and_lands_in_its_worktrees_error(
    cx: &mut TestAppContext,
) {
    let (git, requests) = states(cx);
    let target = worktree("/unused/worktree");
    let (_, hosting) = hosting(Err("gh: HTTP 422 a request already exists"));
    let seen = events(&requests, cx);
    // The empty project list settles first; its notification would clear the target.
    cx.run_until_parked();
    git.update(cx, |git, _| git.target = Some(target.clone()));

    requests.update(cx, |requests, cx| {
        requests.open_with(
            git.clone(),
            target.clone(),
            draft("Add menu"),
            None,
            hosting,
            cx,
        );
    });
    cx.run_until_parked();

    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(
        git.read_with(cx, |git, _| git.error.clone()).as_deref(),
        Some("gh: HTTP 422 a request already exists")
    );
    let kept = requests.read_with(cx, |requests, cx| {
        assert!(!requests.is_opening(&target));
        requests.prefill(&target, "develop".into(), cx)
    });
    // The next dialog starts from what the user wrote, not from the commit subject.
    assert_eq!(kept.await, draft("Add menu"));
}

#[gpui::test]
fn a_push_that_cannot_start_opens_nothing(cx: &mut TestAppContext) {
    let (git, requests) = states(cx);
    let target = worktree("/unused/worktree");
    let (asked, hosting) = hosting(Ok("https://example.invalid/pull/1"));
    let push = Action::Push {
        remote: "origin".into(),
        publish: false,
    };
    // Staging nothing runs no process, and the worktree is busy until the task is polled.
    git.update(cx, |git, cx| {
        git.perform_at(target.clone(), Action::Stage(vec![]), cx);
        assert!(git.is_busy_at(&target));
    });

    requests.update(cx, |requests, cx| {
        requests.open_with(
            git.clone(),
            target.clone(),
            draft("Add menu"),
            Some(push),
            hosting,
            cx,
        );
        assert!(!requests.is_opening(&target));
        // Neither does a request without a push, while another mutation is running.
        let (_, direct) = super::tests::hosting(Ok("https://example.invalid/pull/2"));
        requests.open_with(
            git.clone(),
            target.clone(),
            draft("Add menu"),
            None,
            direct,
            cx,
        );
        assert!(!requests.is_opening(&target));
    });
    assert!(asked.lock().unwrap().is_empty());
}

#[gpui::test]
fn a_request_waits_for_its_push_and_is_dropped_when_the_push_fails(cx: &mut TestAppContext) {
    let (git, requests) = states(cx);
    // Git cannot start in a folder that does not exist, so the push fails without a remote.
    let missing = std::env::temp_dir().join(format!("hive-request-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    let target = Target {
        host: Host::Local,
        project: missing.clone(),
        path: missing,
    };
    let (asked, hosting) = hosting(Ok("https://example.invalid/pull/1"));
    let seen = events(&requests, cx);
    let push = Action::Push {
        remote: "origin".into(),
        publish: false,
    };

    requests.update(cx, |requests, cx| {
        requests.open_with(
            git.clone(),
            target.clone(),
            draft("Add menu"),
            Some(push),
            hosting,
            cx,
        );
        assert!(requests.is_opening(&target));
    });
    // The hosting service is not asked while the push is still running.
    assert!(asked.lock().unwrap().is_empty());
    cx.run_until_parked();

    assert!(asked.lock().unwrap().is_empty());
    assert!(seen.lock().unwrap().is_empty());
    requests.read_with(cx, |requests, _| {
        assert!(!requests.is_opening(&target));
        // What the user wrote survives the failed push.
        assert!(requests.drafts.contains_key(&key(&target)));
    });
}
