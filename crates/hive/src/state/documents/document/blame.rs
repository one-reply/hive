//! Inline Git blame of the open buffer: who last changed each line.
//!
//! The buffer itself is blamed (`--contents -`), so it is re-read once editing pauses rather
//! than mapped through edits; until then the caret line simply shows nothing.
use super::*;
use hive_core::git::GitError;
use hive_core::git::blame::{self, Blame, Origin};
use std::collections::HashMap;

/// A burst of keystrokes costs one `git blame`, once typing pauses this long.
const BLAME_DELAY: Duration = Duration::from_millis(400);

#[derive(Default)]
pub(crate) struct LineBlame {
    current: Option<Arc<Blame>>,
    /// Full commit messages for the hover card by object ID; `None` while loading.
    messages: HashMap<String, Option<SharedString>>,
    /// Dropping it discards a blame of text that has changed since.
    _read: Option<Task<()>>,
}

impl LineBlame {
    /// Origin of 0-based `line` in the latest blame of the current text.
    pub fn line(&self, line: usize) -> Option<&Origin> {
        self.current.as_deref()?.line(line)
    }

    /// The full message of `oid`, once loaded.
    pub fn message(&self, oid: &str) -> Option<&SharedString> {
        self.messages.get(oid)?.as_ref()
    }
}

impl Document {
    pub fn blame(&self) -> &LineBlame {
        &self.blame
    }

    pub fn shows_blame(&self, cx: &App) -> bool {
        self.store.read(cx).state.settings.files.inline_blame && !self.content.is_raster()
    }

    /// The text no longer matches the blame: hide it until the next one arrives.
    pub(super) fn invalidate_blame(&mut self, cx: &mut Context<Self>) {
        self.blame.current = None;
        self.schedule_blame(cx);
    }

    /// Re-blame the current text after a pause; the previous result stays until then.
    pub fn schedule_blame(&mut self, cx: &mut Context<Self>) {
        if !self.shows_blame(cx) || self.base.is_none() {
            self.blame.current = None;
            self.blame._read = None;
            return;
        }
        let host = self.target.host.clone();
        let cwd = self.target.path.clone();
        let path = self.path.clone();
        self.blame._read = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BLAME_DELAY).await;
            let Ok(text) = this.read_with(cx, |this, cx| this.text(cx)) else {
                return;
            };
            // ponytail: a superseded blame still runs to the end and its result is dropped;
            // pass a `Cancellation` through `git::bytes` if overlapping runs ever show up.
            let result = cx
                .background_executor()
                .spawn(async move { blame::read(&host, &cwd, &path, text.as_bytes()) })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(blame) => {
                    this.blame.current = Some(Arc::new(blame));
                    cx.notify();
                }
                // Untracked files and folders outside Git are expected, not failures.
                Err(error @ GitError::Failed { .. }) => {
                    tracing::debug!(%error, "inline blame unavailable");
                }
                Err(error) => tracing::warn!(%error, "inline blame failed"),
            });
        }));
    }

    /// Fetch the full message of `oid` for the hover card, once per commit.
    pub fn load_commit_message(&mut self, oid: &str, cx: &mut Context<Self>) {
        if self.blame.messages.contains_key(oid) {
            return;
        }
        self.blame.messages.insert(oid.to_owned(), None);
        let host = self.target.host.clone();
        let cwd = self.target.path.clone();
        let oid = oid.to_owned();
        cx.spawn(async move |this, cx| {
            let request = oid.clone();
            let result = cx
                .background_executor()
                .spawn(async move { blame::message(&host, &cwd, &request) })
                .await;
            this.update(cx, |this, cx| match result {
                Ok(message) => {
                    this.blame.messages.insert(oid, Some(message.into()));
                    cx.notify();
                }
                Err(error) => {
                    // The card keeps the summary; the next hover retries.
                    this.blame.messages.remove(&oid);
                    tracing::warn!(%error, "commit message read failed");
                }
            })
        })
        .detach_and_log_err(cx);
    }
}
