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
    /// The setting as last applied, so unrelated settings changes do not re-blame.
    enabled: bool,
    runs: Runs,
    /// The pause after the latest change; dropping it restarts the wait.
    _delay: Option<Task<()>>,
    /// The `git blame` in flight. A process cannot be stopped mid-run, so a newer request waits
    /// for it instead of starting a second one on a large file.
    _run: Option<Task<()>>,
}

impl LineBlame {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    /// Origin of 0-based `line` in the latest blame of the current text.
    pub fn line(&self, line: usize) -> Option<&Origin> {
        self.current.as_deref()?.line(line)
    }

    /// The full message of `oid`, once loaded.
    pub fn message(&self, oid: &str) -> Option<&SharedString> {
        self.messages.get(oid)?.as_ref()
    }
}

/// Keeps at most one `git blame` per document in flight and tells whether its result is current.
#[derive(Debug, Default)]
struct Runs {
    /// Bumped whenever the text or its history may have changed.
    generation: u64,
    in_flight: Option<InFlight>,
}

#[derive(Debug)]
struct InFlight {
    generation: u64,
    /// A newer request arrived while this run was in flight.
    rerun: bool,
}

/// What to do once a run ends.
#[derive(Debug, PartialEq, Eq)]
struct Finished {
    /// Nothing changed since the run started: its result describes the current text.
    is_current: bool,
    /// A request arrived meanwhile and is waiting for this run to end.
    rerun: bool,
}

impl Runs {
    /// Results of runs started before now are stale.
    fn invalidate(&mut self) {
        self.generation += 1;
    }

    /// Whether to start a run now; `false` when one is in flight, which is then followed by
    /// another.
    fn request(&mut self) -> bool {
        if let Some(in_flight) = &mut self.in_flight {
            in_flight.rerun = true;
            return false;
        }
        self.in_flight = Some(InFlight {
            generation: self.generation,
            rerun: false,
        });
        true
    }

    fn finish(&mut self) -> Finished {
        match self.in_flight.take() {
            Some(run) => Finished {
                is_current: run.generation == self.generation,
                rerun: run.rerun,
            },
            None => Finished {
                is_current: false,
                rerun: false,
            },
        }
    }
}

impl Document {
    pub fn blame(&self) -> &LineBlame {
        &self.blame
    }

    pub fn shows_blame(&self, cx: &App) -> bool {
        self.store.read(cx).state.settings.files.inline_blame && !self.content.is_raster()
    }

    /// Settings changed: apply the blame toggle to an already open file.
    pub(super) fn apply_blame_setting(&mut self, cx: &mut Context<Self>) {
        let enabled = self.store.read(cx).state.settings.files.inline_blame;
        if self.blame.enabled == enabled {
            return;
        }
        self.blame.enabled = enabled;
        // A hidden tab is blamed when it is activated.
        if !enabled || self.visible {
            self.schedule_blame(cx);
        }
    }

    /// The text no longer matches the blame: hide it until the next one arrives.
    pub(super) fn invalidate_blame(&mut self, cx: &mut Context<Self>) {
        self.blame.current = None;
        self.schedule_blame(cx);
    }

    /// Re-blame the current text after a pause; the previous result stays until then.
    pub fn schedule_blame(&mut self, cx: &mut Context<Self>) {
        self.blame.runs.invalidate();
        if !self.can_blame(cx) {
            self.blame.current = None;
            self.blame._delay = None;
            return;
        }
        self.blame._delay = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BLAME_DELAY).await;
            let _ = this.update(cx, |this, cx| this.start_blame(cx));
        }));
    }

    fn can_blame(&self, cx: &App) -> bool {
        self.shows_blame(cx) && self.base.is_some()
    }

    fn start_blame(&mut self, cx: &mut Context<Self>) {
        self.blame._delay = None;
        if !self.can_blame(cx) {
            return;
        }
        let Some(base) = self.base.clone() else {
            return;
        };
        if !self.blame.runs.request() {
            return;
        }
        let text = self.text(cx);
        let host = self.target.host.clone();
        let cwd = self.target.path.clone();
        let path = self.path.clone();
        self.blame._run = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { blame::read(&host, &cwd, &path, &base, &text) })
                .await;
            let _ = this.update(cx, |this, cx| this.finish_blame(result, cx));
        }));
    }

    fn finish_blame(&mut self, result: Result<Blame, GitError>, cx: &mut Context<Self>) {
        let finished = self.blame.runs.finish();
        if finished.is_current {
            match result {
                Ok(blame) => {
                    self.blame.current = Some(Arc::new(blame));
                    cx.notify();
                }
                // Untracked files and folders outside Git are expected, not failures.
                Err(error @ GitError::Failed { .. }) => {
                    tracing::debug!(%error, "inline blame unavailable");
                }
                Err(error) => tracing::warn!(%error, "inline blame failed"),
            }
        }
        if finished.rerun {
            self.start_blame(cx);
        }
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

#[cfg(test)]
mod tests {
    use super::{Finished, Runs};

    const CURRENT: Finished = Finished {
        is_current: true,
        rerun: false,
    };

    #[test]
    fn a_lone_run_is_current() {
        let mut runs = Runs::default();
        runs.invalidate();
        assert!(runs.request());
        assert_eq!(runs.finish(), CURRENT);
    }

    #[test]
    fn a_request_during_a_run_waits_for_it_and_its_result_is_dropped() {
        let mut runs = Runs::default();
        assert!(runs.request(), "nothing in flight");
        runs.invalidate();
        assert!(!runs.request(), "a second process must not start");
        assert!(!runs.request());
        assert_eq!(
            runs.finish(),
            Finished {
                is_current: false,
                rerun: true,
            }
        );
        assert!(runs.request(), "the waiting request starts now");
        assert_eq!(runs.finish(), CURRENT);
    }

    #[test]
    fn a_change_without_a_new_request_only_drops_the_result() {
        let mut runs = Runs::default();
        runs.request();
        runs.invalidate();
        assert_eq!(
            runs.finish(),
            Finished {
                is_current: false,
                rerun: false,
            }
        );
    }

    #[test]
    fn finishing_with_nothing_in_flight_changes_nothing() {
        let mut runs = Runs::default();
        assert_eq!(
            runs.finish(),
            Finished {
                is_current: false,
                rerun: false,
            }
        );
        assert!(runs.request());
    }
}
