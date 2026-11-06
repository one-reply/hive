use super::*;
use hive_core::files::tree::{self, Source};
use hive_core::git::status::Change;

impl Files {
    pub(crate) fn entry(&self, path: &Path) -> Option<&Entry> {
        self.listing(path.parent().unwrap_or(Path::new("")))?
            .iter()
            .find(|entry| entry.path == path)
    }

    pub(crate) fn listing(&self, directory: &Path) -> Option<&Vec<Entry>> {
        if !self.search.query.is_empty()
            && let Some(entries) = self.search.entries.get(directory)
        {
            return Some(entries);
        }
        self.entries.get(directory)
    }

    /// Paths listed in `directory` so far; nothing when it has not been listed.
    pub(crate) fn listed(&self, directory: &Path) -> impl Iterator<Item = &Path> {
        self.listing(directory)
            .into_iter()
            .flatten()
            .map(|entry| entry.path.as_path())
    }

    /// The tree as the panel should show it.
    pub(crate) fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Changes whenever [`Files::nodes`] does, so a panel can skip rebuilding its rows.
    pub(crate) fn tree_generation(&self) -> u64 {
        self.tree_generation
    }

    /// The row the panel should select and scroll to once it exists.
    pub(crate) fn reveal_request(&self) -> Option<&Path> {
        self.pending_reveal.as_deref()
    }

    /// The panel has shown the requested row.
    pub(crate) fn revealed(&mut self) {
        self.pending_reveal = None;
    }

    /// The user opened or closed a directory in the panel; its children are listed on demand.
    pub(crate) fn set_expanded(&mut self, path: PathBuf, expanded: bool, cx: &mut Context<Self>) {
        if !self.search.query.is_empty() {
            if expanded {
                self.search.collapsed.remove(&path);
            } else {
                self.search.collapsed.insert(path);
            }
            self.rebuild(cx);
            return;
        }
        if expanded {
            self.expanded.insert(path);
        } else {
            self.expanded.remove(&path);
        }
        self.refresh(cx);
    }

    pub(crate) fn collapse_all(&mut self, cx: &mut Context<Self>) {
        if !self.search.query.is_empty() {
            self.search.collapsed = self.search.matches.directories().into_iter().collect();
        } else {
            self.expanded.clear();
        }
        self.pending_reveal = None;
        self.rebuild(cx);
    }

    pub(crate) fn expand(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.search.query.is_empty() {
            self.set_expanded(path, true, cx);
            return;
        }
        if self.expanded.insert(path) {
            self.rebuild(cx);
            self.refresh(cx);
        }
    }

    pub(crate) fn toggle_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.search.query.is_empty() {
            let expanded = self.search.collapsed.contains(&path);
            self.set_expanded(path, expanded, cx);
            return;
        }
        if self.expanded.remove(&path) {
            self.rebuild(cx);
        } else {
            self.expand(path, cx);
        }
    }

    pub(crate) fn reveal(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        for parent in path
            .ancestors()
            .skip(1)
            .filter(|p| !p.as_os_str().is_empty())
        {
            if self.search.query.is_empty() {
                self.expanded.insert(parent.into());
            } else {
                self.search.collapsed.remove(parent);
            }
        }
        self.pending_reveal = Some(path);
        self.rebuild(cx);
        self.refresh(cx);
    }

    /// Rebuilds the rows from the model; the panel picks them up on its next observation.
    pub(super) fn rebuild(&mut self, cx: &mut Context<Self>) {
        let searching = !self.search.query.is_empty();
        let mut built = tree::build(&Source {
            entries: if searching {
                &self.search.entries
            } else {
                &self.entries
            },
            expanded: &self.expanded,
            ignored: &self.ignored,
            decorations: &self.decorations,
            settings: &self.settings,
            destination_for: self.destination_for.as_deref(),
        });
        if searching {
            self.search
                .matches
                .filter(&mut built.nodes, &self.search.collapsed);
        }
        self.nodes = built.nodes;
        self.missing = built.missing;
        self.tree_generation += 1;
        cx.notify();
    }

    pub(crate) fn show_changes(&self, path: &Path, cx: &mut Context<Self>) {
        let git = AppState::get(cx).git.clone();
        let state = git.read(cx);
        if state.target != self.target {
            return;
        }
        let file = state
            .status
            .as_ref()
            .and_then(|s| s.files.iter().find(|f| Path::new(&f.path) == path))
            .cloned();
        if let Some(file) = file {
            let source = if file.unstaged == Some(Change::Untracked) {
                hive_core::git::diff::DiffSource::Untracked
            } else if file.unstaged.is_some() {
                hive_core::git::diff::DiffSource::WorkingTree
            } else {
                hive_core::git::diff::DiffSource::Staged
            };
            git.update(cx, |git, cx| git.open_diff(&file, source, cx));
        }
    }
}
