//! Opening a request for the branch a worktree has checked out, once the user asks for it.
use super::{Key, PullRequests};
use crate::state::git::Git;
use gpui_kit::*;
use hive_core::git::{
    changes::Action,
    pull_request::{self, Draft},
};
use hive_core::projects::Target;

/// What became of a request the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PullRequestEvent {
    /// `branch` has a request at `url`: created now, or the one it already had.
    Opened { branch: String, url: String },
}

impl EventEmitter<PullRequestEvent> for PullRequests {}

/// The request a branch ended up with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Opened {
    /// The branch as the remote names it.
    branch: String,
    url: String,
}

fn key(target: &Target) -> Key {
    (target.host.clone(), target.path.clone())
}

/// Blocking: opens a request for the branch checked out at `target`, or finds the one it
/// already has, so asking twice never opens two.
fn request(target: &Target, draft: &Draft) -> Result<Opened, String> {
    let (host, path) = (&target.host, target.path.as_path());
    let source = pull_request::published_branch(host, path).map_err(|error| error.to_string())?;
    let existing = pull_request::read_published(
        host,
        path,
        &source.repository,
        &source.branch,
        &source.commit,
    )
    .map_err(|error| error.to_string())?;
    let url = match existing.request().and_then(|request| request.web_url()) {
        Some(url) => url.to_owned(),
        None => pull_request::create(
            existing.provider(),
            host,
            path,
            &source.repository,
            &source.branch,
            draft,
        )
        .map_err(|error| match error {
            // The CLI was cut short; whether the service acted is not known.
            pull_request::Error::Command(_) => format!(
                "{error}\nThe request may have been created anyway; Create PR again finds it instead of opening a second one."
            ),
            error => error.to_string(),
        })?,
    };
    Ok(Opened {
        branch: source.branch,
        url,
    })
}

impl PullRequests {
    pub(crate) fn is_opening(&self, target: &Target) -> bool {
        self.opening.contains(&key(target))
    }

    /// What the request dialog starts from: the text of an attempt that did not go through,
    /// or the subject of the checked-out commit with `base` to merge into.
    pub(crate) fn prefill(&self, target: &Target, base: String, cx: &App) -> Task<Draft> {
        if let Some(draft) = self.drafts.get(&key(target)) {
            return Task::ready(draft.clone());
        }
        let (host, path) = (target.host.clone(), target.path.clone());
        cx.background_executor().spawn(async move {
            let title = pull_request::suggested_title(&host, &path).unwrap_or_else(|error| {
                tracing::warn!(%error, ?path, "no commit subject for the request title");
                String::new()
            });
            Draft {
                title,
                base,
                ..Draft::default()
            }
        })
    }

    /// Opens a request for the branch checked out at `target`, after `push` when one is
    /// given. A failed push is reported by `git` like any other; a failed request lands in
    /// the same place, where its text can be copied.
    pub(crate) fn open(
        &mut self,
        git: Entity<Git>,
        target: Target,
        draft: Draft,
        push: Option<Action>,
        cx: &mut Context<Self>,
    ) {
        self.open_with(git, target, draft, push, request, cx);
    }

    /// [`PullRequests::open`] with the call to the hosting service given by the caller.
    fn open_with(
        &mut self,
        git: Entity<Git>,
        target: Target,
        draft: Draft,
        push: Option<Action>,
        request: impl Fn(&Target, &Draft) -> Result<Opened, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let key = key(&target);
        if self.opening.contains(&key) {
            return;
        }
        // Kept until the request exists, so a failure does not cost the user their text.
        self.drafts.insert(key.clone(), draft.clone());
        let pushed = match push {
            Some(push) => {
                let started =
                    git.update(cx, |git, cx| git.perform_awaited(target.clone(), push, cx));
                let Some(pushed) = started else {
                    // Another mutation holds the worktree; nothing was started.
                    return;
                };
                Some(pushed)
            }
            None if git.read(cx).is_busy_at(&target) => return,
            None => None,
        };
        self.opening.insert(key.clone());
        let work = cx.spawn(async move |this, cx| {
            let result = match pushed {
                // `git` has already told the user why the push failed.
                Some(pushed) if !pushed.recv().await.unwrap_or(false) => None,
                _ => {
                    let target = target.clone();
                    let asked = cx
                        .background_executor()
                        .spawn(async move { request(&target, &draft) });
                    Some(asked.await)
                }
            };
            let _ = this.update(cx, |this, cx| this.finish(&git, &target, result, cx));
        });
        self.requests.insert(key, work);
        cx.notify();
    }

    /// `result` is `None` when the push the request waited for failed.
    fn finish(
        &mut self,
        git: &Entity<Git>,
        target: &Target,
        result: Option<Result<Opened, String>>,
        cx: &mut Context<Self>,
    ) {
        let key = key(target);
        self.opening.remove(&key);
        match result {
            None => {}
            Some(Ok(Opened { branch, url })) => {
                self.drafts.remove(&key);
                // Read the card now rather than at the next minute.
                self.retry.insert(key);
                cx.emit(PullRequestEvent::Opened { branch, url });
            }
            Some(Err(message)) => {
                tracing::warn!(path = ?target.path, error = %message, "request was not opened");
                // A request cut short may exist all the same.
                self.retry.insert(key);
                git.update(cx, |git, cx| git.report_message(target, message, cx));
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
