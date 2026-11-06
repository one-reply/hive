use super::*;
use hive_core::files::{
    search,
    tree::{Contents, NodeKind},
};
use std::{ops::Range, sync::Arc};

/// Search owns its snapshot and expansion, leaving the ordinary navigator untouched.
#[derive(Default)]
pub(super) struct Search {
    pub(super) query: String,
    pub(super) entries: Arc<BTreeMap<PathBuf, Vec<Entry>>>,
    pub(super) matches: search::Matches,
    pub(super) collapsed: HashSet<PathBuf>,
    index: Option<Arc<FileIndex>>,
    generation: u64,
    running: bool,
    error: Option<String>,
    task: Option<Task<()>>,
}

impl Search {
    fn set_query(&mut self, query: String) -> bool {
        if self.query == query {
            return false;
        }
        if query.is_empty() {
            self.reset();
        } else {
            self.query = query;
            self.collapsed.clear();
            self.matches = search::Matches::default();
            self.error = None;
        }
        true
    }

    pub(super) fn reset(&mut self) {
        *self = Self {
            generation: self.generation + 1,
            ..Self::default()
        };
    }

    fn refresh(&mut self) {
        let query = std::mem::take(&mut self.query);
        let collapsed = std::mem::take(&mut self.collapsed);
        self.reset();
        self.query = query;
        self.collapsed = collapsed;
    }

    fn accepts(&self, generation: u64) -> bool {
        !self.query.is_empty() && self.generation == generation
    }

    fn can_start(&self, busy: bool) -> bool {
        !self.query.is_empty() && !self.running && !busy
    }

    /// Cache a completed snapshot; return whether a newer query still needs filtering.
    fn complete(
        &mut self,
        query: &str,
        visibility_changed: bool,
        result: Result<Loaded, String>,
    ) -> bool {
        self.running = false;
        match result {
            Ok(loaded) => {
                self.index = Some(loaded.index);
                self.entries = loaded.entries;
                if self.query != query || visibility_changed {
                    return true;
                }
                if let Some(error) = &loaded.error {
                    tracing::warn!(%error, "file search directory unavailable");
                }
                self.matches = loaded.matches;
                self.error = loaded.error;
            }
            Err(error) => {
                tracing::warn!(%error, "file search failed");
                self.error = Some(error);
            }
        }
        false
    }
}

struct Loaded {
    index: Arc<FileIndex>,
    entries: Arc<BTreeMap<PathBuf, Vec<Entry>>>,
    matches: search::Matches,
    error: Option<String>,
}

impl Files {
    pub(crate) fn search_query(&self) -> &str {
        &self.search.query
    }
    pub(crate) fn search_loading(&self) -> bool {
        self.search.running
    }
    pub(crate) fn search_error(&self) -> Option<&str> {
        self.search.error.as_deref()
    }
    pub(crate) fn search_truncated(&self) -> bool {
        self.search
            .index
            .as_ref()
            .is_some_and(|index| index.truncated)
    }
    pub(crate) fn search_highlights(&self, path: &Path) -> &[Range<usize>] {
        self.search.matches.highlights(path)
    }

    pub(crate) fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.destination_for.is_some() || !self.search.set_query(query) {
            return;
        }
        self.pending_reveal = None;
        self.rebuild(cx);
        self.search(cx);
    }

    pub(super) fn refresh_search(&mut self, cx: &mut Context<Self>) {
        if self.search.query.is_empty() {
            return;
        }
        self.search.refresh();
        self.rebuild(cx);
        self.search(cx);
    }

    pub(super) fn search(&mut self, cx: &mut Context<Self>) {
        if !self.search.can_start(self.busy) {
            return;
        }
        let Some(target) = self
            .target
            .clone()
            .filter(|target| target.host == hive_core::host::Host::Local)
        else {
            return;
        };
        let generation = self.search.generation;
        let query = self.search.query.clone();
        let show_hidden = self.settings.show_hidden;
        let index = self.search.index.clone();
        let entries = self.search.entries.clone();
        self.search.running = true;
        self.search.task = Some(cx.spawn(async move |this, cx| {
            let read_target = target.clone();
            let read_query = query.clone();
            let result = cx
                .background_executor()
                .spawn(async move { load(&read_target, &read_query, show_hidden, index, entries) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.target.as_ref() != Some(&target) || !this.search.accepts(generation) {
                    return;
                }
                if this
                    .search
                    .complete(&query, this.settings.show_hidden != show_hidden, result)
                {
                    this.search(cx);
                    return;
                }
                this.rebuild(cx);
            });
        }));
        cx.notify();
    }

    pub(crate) fn first_search_result(&self) -> Option<PathBuf> {
        fn first(nodes: &[Node], files: &Files) -> Option<PathBuf> {
            nodes.iter().find_map(|node| match &node.kind {
                NodeKind::File => (!files.missing.contains(&node.path)
                    && files
                        .entry(&node.path)
                        .is_some_and(|entry| !entry.unsupported))
                .then(|| node.path.clone()),
                NodeKind::Directory {
                    contents: Contents::Children(children),
                    ..
                } => first(children, files),
                _ => None,
            })
        }
        if self.search.query.is_empty() || self.search.running {
            return None;
        }
        first(&self.nodes, self)
    }
}

#[cfg(test)]
mod tests;

fn load(
    target: &Target,
    query: &str,
    show_hidden: bool,
    index: Option<Arc<FileIndex>>,
    mut entries: Arc<BTreeMap<PathBuf, Vec<Entry>>>,
) -> Result<Loaded, String> {
    let index = match index {
        Some(index) => index,
        None => Arc::new(
            files::list_files(&target.host, &target.path).map_err(|error| error.to_string())?,
        ),
    };
    let matches = search::find(&index.paths, query, show_hidden);
    let mut errors = Vec::new();
    for directory in matches.directories() {
        if entries.contains_key(&directory) {
            continue;
        }
        match files::list_directory(&target.host, &target.path, &directory) {
            Ok(listed) => {
                Arc::make_mut(&mut entries).insert(directory, listed);
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    Ok(Loaded {
        index,
        entries,
        matches,
        error: (!errors.is_empty()).then(|| errors.join("\n")),
    })
}
