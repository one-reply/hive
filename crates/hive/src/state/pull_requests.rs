//! Periodic PR cards, separate from the frequently refreshed local Git panel.
use super::git::Git;
use super::projects::Projects;
use gpui_kit::*;
use hive_core::{
    git::pull_request::{self, PullRequest},
    host::Host,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Status {
    Ready(Option<PullRequest>),
    Unavailable(String),
}

pub(crate) struct PullRequests {
    cards: HashMap<(Host, PathBuf), (String, Status)>,
    _poll: Task<()>,
}
impl PullRequests {
    /// Polls only while the main window is active, like `Git`: nobody sees the cards
    /// otherwise, and each check runs `gh` (over `ssh` for remote projects).
    pub(crate) fn new(
        projects: Entity<Projects>,
        git: Entity<Git>,
        cx: &mut Context<Self>,
    ) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            let mut attempted = HashMap::<(Host, PathBuf), (String, std::time::Instant)>::new();
            loop {
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
                    if attempted.get(&key).is_some_and(|(h, time)| {
                        *h == head && time.elapsed() < Duration::from_secs(60)
                    }) {
                        continue;
                    }
                    attempted.insert(key, (head.clone(), std::time::Instant::now()));
                    let (h, p) = (host.clone(), path.clone());
                    let result = cx
                        .background_executor()
                        .spawn(async move { pull_request::read(&h, &p) })
                        .await;
                    let status = match result {
                        Ok(pr) => Status::Ready(pr),
                        Err(error) => Status::Unavailable(error.to_string()),
                    };
                    if this
                        .update(cx, |this, cx| {
                            let key = (host, path);
                            let value = (head, status);
                            if this.cards.get(&key) != Some(&value) {
                                if let Status::Unavailable(error) = &value.1 {
                                    tracing::warn!(?key, %error, "PR metadata unavailable");
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
        }
    }
    pub(crate) fn status(&self, host: &Host, path: &Path, head: &str) -> Option<&Status> {
        self.cards
            .get(&(host.clone(), path.to_owned()))
            .filter(|(h, _)| h == head)
            .map(|(_, status)| status)
    }
}
