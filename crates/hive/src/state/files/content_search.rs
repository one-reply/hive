//! Project-wide content search: debounced, streamed in path order, cancelled when replaced.
//! Results are never refreshed behind the user's back: agents edit files constantly, and a
//! full re-scan on every change would compete with them for CPU and disk.
use super::*;
use hive_core::{
    command::Cancellation,
    files::content_search::{self as engine, FileMatches, Query},
};

/// Typing pause before a search starts; toggles, Enter and refresh start at once.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// What the Files panel's search field filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SearchMode {
    /// File names, filtering the tree.
    #[default]
    Names,
    /// File contents, replacing the tree with results.
    Content,
}

/// A content search comparison toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchOption {
    MatchCase,
    WholeWord,
    Regex,
}

/// Where the current content search stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ContentStatus {
    /// Nothing to search for.
    #[default]
    Idle,
    Searching,
    Done {
        /// Results stopped at the line cap.
        truncated: bool,
        /// The worktree has more files than the index holds.
        index_truncated: bool,
    },
    Failed(String),
    /// The worktree is on an SSH host, which content search does not reach yet.
    Remote,
}

/// A finished run, before it is accepted into the model.
pub(super) struct Finished {
    truncated: bool,
    index_truncated: bool,
}

#[derive(Default)]
pub(super) struct ContentSearch {
    query: Query,
    results: Vec<FileMatches>,
    matched_lines: usize,
    status: ContentStatus,
    /// Results belong to an earlier query until the new run delivers its first batch, so
    /// the list does not flash empty on every keystroke.
    stale: bool,
    generation: u64,
    /// Changes when a new result set replaces the old one.
    epoch: u64,
    /// Changes whenever the results change.
    revision: u64,
    task: Option<Task<()>>,
}

impl ContentSearch {
    fn begin(&mut self) -> u64 {
        self.generation += 1;
        self.status = ContentStatus::Searching;
        self.stale = true;
        self.generation
    }

    fn replace_stale(&mut self) {
        if self.stale {
            self.stale = false;
            self.results.clear();
            self.matched_lines = 0;
            self.epoch += 1;
        }
    }

    fn receive(&mut self, generation: u64, batch: Vec<FileMatches>) -> bool {
        if generation != self.generation || batch.is_empty() {
            return false;
        }
        self.replace_stale();
        self.matched_lines += batch.iter().map(|file| file.lines.len()).sum::<usize>();
        self.results.extend(batch);
        self.revision += 1;
        true
    }

    fn finish(&mut self, generation: u64, result: Result<Finished, String>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.replace_stale();
        self.status = match result {
            Ok(finished) => ContentStatus::Done {
                truncated: finished.truncated,
                index_truncated: finished.index_truncated,
            },
            Err(error) => {
                tracing::warn!(%error, "content search failed");
                self.results.clear();
                self.matched_lines = 0;
                self.epoch += 1;
                ContentStatus::Failed(error)
            }
        };
        self.revision += 1;
        true
    }

    /// Drop results and any run in flight; the query and its options stay.
    fn clear(&mut self) {
        self.generation += 1;
        self.task = None;
        self.results.clear();
        self.matched_lines = 0;
        self.status = ContentStatus::Idle;
        self.stale = false;
        self.epoch += 1;
        self.revision += 1;
    }

    /// Forget the query text too, keeping the comparison options.
    pub(super) fn reset(&mut self) {
        self.clear();
        self.query.text.clear();
    }

    fn toggle(&mut self, option: SearchOption) {
        let flag = match option {
            SearchOption::MatchCase => &mut self.query.case_sensitive,
            SearchOption::WholeWord => &mut self.query.whole_word,
            SearchOption::Regex => &mut self.query.regex,
        };
        *flag = !*flag;
    }
}

impl Files {
    pub(crate) fn search_mode(&self) -> SearchMode {
        self.search_mode
    }

    /// The text of whichever search the field drives.
    pub(crate) fn search_text(&self) -> &str {
        match self.search_mode {
            SearchMode::Names => self.search_query(),
            SearchMode::Content => &self.content.query.text,
        }
    }

    pub(crate) fn content_query(&self) -> &Query {
        &self.content.query
    }
    pub(crate) fn content_results(&self) -> &[FileMatches] {
        &self.content.results
    }
    pub(crate) fn content_matched_lines(&self) -> usize {
        self.content.matched_lines
    }
    pub(crate) fn content_epoch(&self) -> u64 {
        self.content.epoch
    }
    pub(crate) fn content_revision(&self) -> u64 {
        self.content.revision
    }
    pub(crate) fn content_status(&self) -> ContentStatus {
        match &self.target {
            Some(target) if !target.host.is_local() => ContentStatus::Remote,
            _ => self.content.status.clone(),
        }
    }

    /// Switch what the field searches, carrying the typed text over to the other search.
    pub(crate) fn set_search_mode(&mut self, mode: SearchMode, cx: &mut Context<Self>) {
        if self.destination_for.is_some() || self.search_mode == mode {
            return;
        }
        let text = self.search_text().to_owned();
        self.search_mode = mode;
        match mode {
            SearchMode::Content => {
                self.set_search_query(String::new(), cx);
                self.content.query.text = text;
                self.start_content_search(Duration::ZERO, cx);
            }
            SearchMode::Names => {
                self.content.reset();
                self.set_search_query(text, cx);
            }
        }
        cx.notify();
    }

    /// Route the field's text to the search of the current mode.
    pub(crate) fn set_search_text(&mut self, text: String, cx: &mut Context<Self>) {
        match self.search_mode {
            SearchMode::Names => self.set_search_query(text, cx),
            SearchMode::Content if self.content.query.text != text => {
                self.content.query.text = text;
                self.start_content_search(DEBOUNCE, cx);
            }
            SearchMode::Content => {}
        }
    }

    pub(crate) fn toggle_search_option(&mut self, option: SearchOption, cx: &mut Context<Self>) {
        self.content.toggle(option);
        if self.search_mode == SearchMode::Content {
            self.start_content_search(Duration::ZERO, cx);
        }
        cx.notify();
    }

    /// Search again now, picking up files changed since the last run.
    pub(crate) fn rerun_content_search(&mut self, cx: &mut Context<Self>) {
        if self.search_mode == SearchMode::Content {
            self.start_content_search(Duration::ZERO, cx);
        }
    }

    fn start_content_search(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let target = self.target.clone().filter(|target| target.host.is_local());
        let Some(target) = target.filter(|_| !self.content.query.text.is_empty()) else {
            self.content.clear();
            cx.notify();
            return;
        };
        let generation = self.content.begin();
        let query = self.content.query.clone();
        self.content.task = Some(cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let (sender, receiver) = async_channel::unbounded();
            let executor = cx.background_executor().clone();
            // Dropping this task drops `work`, whose guard cancels the scan's workers.
            let work = cx.background_executor().spawn(crate::state::file_io::run(
                executor,
                move |cancellation| {
                    run(&target, &query, cancellation, |file| {
                        let _ = sender.try_send(file);
                    })
                },
            ));
            while let Ok(file) = receiver.recv().await {
                let mut batch = vec![file];
                batch.extend(std::iter::from_fn(|| receiver.try_recv().ok()));
                let delivered = this.update(cx, |this, cx| {
                    if this.content.receive(generation, batch) {
                        cx.notify();
                    }
                });
                if delivered.is_err() {
                    return;
                }
            }
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.content.finish(generation, result) {
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }
}

/// Blocking: list the worktree afresh, so files agents just created are searched too.
pub(super) fn run(
    target: &Target,
    query: &Query,
    cancellation: &Cancellation,
    emit: impl FnMut(FileMatches),
) -> Result<Finished, String> {
    let index = files::Access::new(&target.host, cancellation)
        .list_files(&target.path)
        .map_err(|error| error.to_string())?;
    let summary = engine::search(&target.path, &index.paths, query, cancellation, emit)
        .map_err(|error| error.to_string())?;
    Ok(Finished {
        truncated: summary.truncated,
        index_truncated: index.truncated,
    })
}

#[cfg(test)]
mod tests;
