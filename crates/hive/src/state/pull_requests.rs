//! Periodic PR/MR cards, separate from the frequently refreshed local Git panel.
use super::git::Git;
use super::projects::Projects;
use gpui_kit::*;
use hive_core::{
    git::pull_request::{self, Provider, Snapshot},
    host::Host,
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// How often a worktree's PR is read again: its checks change while it is open.
const RECHECK: Duration = Duration::from_secs(60);
/// The longest pause between reads of a worktree whose reads keep failing.
const FAILURE_RECHECK_CAP: Duration = Duration::from_secs(30 * 60);

/// The last read of one worktree's PR.
struct Attempt {
    head: String,
    at: Instant,
    /// Failed reads in a row on this `head`.
    failures: u32,
}

/// The pause after `failures` failed reads in a row. An unavailable hosting service
/// fails the same way every time, so each further failure doubles the pause up to the cap;
/// the first one retries as soon as a success would, in case it was a network blip.
fn recheck_after(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(u32::BITS - 1);
    RECHECK
        .saturating_mul(1 << doublings)
        .min(FAILURE_RECHECK_CAP)
}

/// Whether the worktree's PR should be read now. A new `head` is read at once.
fn is_due(attempt: Option<&Attempt>, head: &str, now: Instant) -> bool {
    attempt.is_none_or(|attempt| {
        attempt.head != head
            || now.saturating_duration_since(attempt.at) >= recheck_after(attempt.failures)
    })
}

/// Whether a read failed in a way that repeating it soon will not fix. Missing hosting CLIs
/// are checked again within a minute of installation; no PR/MR is a normal answer.
fn backs_off(result: &Result<Snapshot, pull_request::Error>) -> bool {
    matches!(
        result,
        Err(pull_request::Error::Command(_)
            | pull_request::Error::Response(_)
            | pull_request::Error::Authentication { .. })
    )
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Status {
    Ready(Snapshot),
    Unavailable {
        message: String,
        authentication: Option<Provider>,
    },
}

pub(crate) struct PullRequests {
    cards: HashMap<(Host, PathBuf), (String, Status)>,
    _poll: Task<()>,
    refresh_generations: HashMap<Provider, u64>,
    retry: HashSet<(Host, PathBuf)>,
}
impl PullRequests {
    /// Polls only while the main window is active, like `Git`: nobody sees the cards
    /// otherwise, and each check runs `gh`/`glab` (over `ssh` for remote projects). A worktree whose
    /// reads keep failing is asked ever more rarely, see `recheck_after`.
    pub(crate) fn new(
        projects: Entity<Projects>,
        git: Entity<Git>,
        cx: &mut Context<Self>,
    ) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            let mut attempted = HashMap::<(Host, PathBuf), Attempt>::new();
            loop {
                let Ok(generations) =
                    this.read_with(cx, |this, _| this.refresh_generations.clone())
                else {
                    return;
                };
                let retry = this
                    .update(cx, |this, _| std::mem::take(&mut this.retry))
                    .unwrap_or_default();
                for key in retry {
                    attempted.remove(&key);
                }
                let targets = projects.read_with(cx, |projects, _| {
                    projects
                        .items
                        .iter()
                        .flat_map(|p| {
                            p.worktrees.iter().map(move |w| {
                                (p.host.clone(), w.path.clone(), format!("{:?}", w.head))
                            })
                        })
                        .collect::<Vec<_>>()
                });
                let _ = this.update(cx, |this, cx| {
                    let count = this.cards.len();
                    this.cards.retain(|(host, path), _| {
                        targets.iter().any(|(h, p, _)| h == host && p == path)
                    });
                    if this.cards.len() != count {
                        cx.notify();
                    }
                });
                attempted.retain(|(host, path), _| {
                    targets.iter().any(|(h, p, _)| h == host && p == path)
                });
                let window_active = git.read_with(cx, |git, _| git.window_active);
                for (host, path, head) in targets.into_iter().filter(|_| window_active) {
                    let key = (host.clone(), path.clone());
                    let now = Instant::now();
                    if !is_due(attempted.get(&key), &head, now) {
                        continue;
                    }
                    let failures = attempted
                        .get(&key)
                        .filter(|attempt| attempt.head == head)
                        .map_or(0, |attempt| attempt.failures);
                    let (h, p) = (host.clone(), path.clone());
                    let result = cx
                        .background_executor()
                        .spawn(async move { pull_request::read(&h, &p) })
                        .await;
                    let stale = host == Host::Local
                        && result
                            .as_ref()
                            .err()
                            .and_then(pull_request::Error::authentication_provider)
                            .is_some_and(|provider| {
                                this.read_with(cx, |this, _| {
                                    this.refresh_generations.get(&provider)
                                        != generations.get(&provider)
                                })
                                .unwrap_or(true)
                            });
                    if stale {
                        continue;
                    }
                    let failures = if backs_off(&result) {
                        failures.saturating_add(1)
                    } else {
                        0
                    };
                    let attempt = Attempt {
                        head: head.clone(),
                        at: now,
                        failures,
                    };
                    attempted.insert(key, attempt);
                    let status = match result {
                        Ok(pr) => Status::Ready(pr),
                        Err(error) => Status::Unavailable {
                            message: error.to_string(),
                            authentication: error.authentication_provider(),
                        },
                    };
                    if this
                        .update(cx, |this, cx| {
                            let key = (host, path);
                            let value = (head, status);
                            if this.cards.get(&key) != Some(&value) {
                                if let Status::Unavailable { message: error, .. } = &value.1 {
                                    tracing::warn!(?key, %error, "PR/MR metadata unavailable");
                                }
                                this.cards.insert(key, value);
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
        Self {
            cards: HashMap::new(),
            _poll: poll,
            refresh_generations: HashMap::new(),
            retry: HashSet::new(),
        }
    }
    pub(crate) fn refresh(&mut self, provider: Provider, _: &mut Context<Self>) {
        let generation = self.refresh_generations.entry(provider).or_default();
        *generation = generation.wrapping_add(1);
        self.retry
            .extend(self.cards.iter().filter_map(|(key, (_, status))| {
                can_connect(&key.0, status)
                    .filter(|p| *p == provider)
                    .map(|_| key.clone())
            }));
    }
    pub(crate) fn status(&self, host: &Host, path: &Path, head: &str) -> Option<&Status> {
        self.cards
            .get(&(host.clone(), path.to_owned()))
            .filter(|(h, _)| h == head)
            .map(|(_, status)| status)
    }
}

pub(crate) fn can_connect(host: &Host, status: &Status) -> Option<Provider> {
    match (host, status) {
        (Host::Local, Status::Unavailable { authentication, .. }) => *authentication,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Attempt, FAILURE_RECHECK_CAP, RECHECK, backs_off, is_due, recheck_after};
    use hive_core::git::pull_request::Error;
    use std::time::{Duration, Instant};

    #[gpui::test]
    fn reconnect_retries_only_local_provider_errors_and_keeps_all_cards_visible(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use super::{Host, Provider, PullRequests, Status};
        use gpui_kit::AppContext as _;
        let failure = |provider| Status::Unavailable {
            message: "signed out".into(),
            authentication: Some(provider),
        };
        let local = Host::Local;
        let remote = Host::Ssh {
            target: "test-host".into(),
        };
        let state = cx.new(|_| PullRequests {
            cards: [
                (
                    (local.clone(), "/github".into()),
                    ("main".into(), failure(Provider::GitHub)),
                ),
                (
                    (local.clone(), "/gitlab".into()),
                    ("main".into(), failure(Provider::GitLab)),
                ),
                (
                    (remote.clone(), "/ssh".into()),
                    ("main".into(), failure(Provider::GitHub)),
                ),
                (
                    (local.clone(), "/timeout".into()),
                    (
                        "main".into(),
                        Status::Unavailable {
                            message: "timeout".into(),
                            authentication: None,
                        },
                    ),
                ),
            ]
            .into(),
            _poll: gpui_kit::Task::ready(()),
            refresh_generations: Default::default(),
            retry: Default::default(),
        });
        state.update(cx, |state, cx| {
            state.refresh(Provider::GitHub, cx);
            assert_eq!(state.cards.len(), 4);
            assert_eq!(state.retry, [(local.clone(), "/github".into())].into());
            assert_eq!(state.refresh_generations.get(&Provider::GitHub), Some(&1));
            assert_eq!(state.refresh_generations.get(&Provider::GitLab), None);
            assert_eq!(
                super::can_connect(&remote, &failure(Provider::GitHub)),
                None
            );
            assert_eq!(
                super::can_connect(
                    &local,
                    &Status::Unavailable {
                        message: "ambiguous MR".into(),
                        authentication: None
                    }
                ),
                None
            );
        });
    }

    fn attempt(failures: u32) -> Attempt {
        Attempt {
            head: "main".into(),
            at: Instant::now(),
            failures,
        }
    }

    #[test]
    fn successful_read_is_rechecked_after_a_minute() {
        let read = attempt(0);
        assert!(!is_due(Some(&read), "main", read.at));
        assert!(!is_due(
            Some(&read),
            "main",
            read.at + RECHECK - Duration::from_secs(1)
        ));
        assert!(is_due(Some(&read), "main", read.at + RECHECK));
    }

    #[test]
    fn first_failure_retries_after_a_minute_then_the_wait_doubles() {
        let waits: Vec<_> = (0..=5).map(|n| recheck_after(n).as_secs()).collect();
        assert_eq!(waits, [60, 60, 120, 240, 480, 960]);
        let failed = attempt(3);
        assert!(!is_due(Some(&failed), "main", failed.at + RECHECK * 3));
        assert!(is_due(Some(&failed), "main", failed.at + RECHECK * 4));
    }

    #[test]
    fn failure_wait_is_capped_at_thirty_minutes_without_overflow() {
        assert_eq!(recheck_after(6), FAILURE_RECHECK_CAP);
        assert_eq!(recheck_after(40), FAILURE_RECHECK_CAP);
        assert_eq!(recheck_after(u32::MAX), FAILURE_RECHECK_CAP);
    }

    #[test]
    fn worktree_never_read_and_changed_head_are_due_at_once() {
        let failed = attempt(9);
        assert!(is_due(None, "main", failed.at));
        assert!(is_due(Some(&failed), "feature", failed.at));
    }

    #[test]
    fn only_failed_commands_and_responses_back_off() {
        assert!(backs_off(&Err(Error::Response("no GitHub remote".into()))));
        assert!(backs_off(&Err(Error::Command(
            hive_core::command::Error::TimedOut
        ))));
        assert!(!backs_off(&Err(Error::Missing)));
    }
}
