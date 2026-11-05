use super::*;
use hive_core::git::status::Change;

impl Files {
    pub(crate) fn entry(&self, path: &Path) -> Option<&Entry> {
        self.entries
            .get(path.parent().unwrap_or(Path::new("")))?
            .iter()
            .find(|entry| entry.path == path)
    }

    pub(crate) fn selected(&self, cx: &App) -> Option<PathBuf> {
        self.tree
            .read(cx)
            .selected_entry()
            .filter(|e| !e.is_disabled())
            .map(|e| PathBuf::from(e.item().id.as_ref()))
    }

    pub(crate) fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.expanded.clear();
        self.pending_reveal = None;
        self.rebuild(cx);
    }

    pub(crate) fn expand(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.expanded.insert(path) {
            self.rebuild(cx);
            self.refresh(cx);
        }
    }

    pub(crate) fn toggle_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
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
            self.expanded.insert(parent.into());
        }
        self.pending_reveal = Some(path);
        self.rebuild(cx);
        self.refresh(cx);
    }

    pub(super) fn rebuild(&mut self, cx: &mut Context<Self>) {
        let mut entries = self.entries.clone();
        self.missing.clear();
        if self.settings.git_decorations && self.destination_for.is_none() {
            for (path, decoration) in &self.decorations {
                if decoration.staged != Some(Change::Deleted)
                    && decoration.unstaged != Some(Change::Deleted)
                {
                    continue;
                }
                if self.entry(path).is_some() {
                    continue;
                }
                self.missing.insert(path.clone());
                let mut child = path.clone();
                let mut directory = false;
                while let Some(parent) = child.parent() {
                    let siblings = entries.entry(parent.into()).or_default();
                    if !siblings.iter().any(|entry| entry.path == child) {
                        siblings.push(Entry {
                            path: child.clone(),
                            directory,
                            unsupported: false,
                        });
                    }
                    child = parent.into();
                    directory = true;
                    if child.as_os_str().is_empty() {
                        break;
                    }
                }
            }
        }
        let selected = self
            .tree
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone());
        let mut items = self.items(Path::new(""), &entries);
        if self.destination_for.is_some() {
            if items.is_empty() {
                items.push(TreeItem::new("/", "No subfolders").disabled(true));
            }
            items = vec![
                TreeItem::new("", "Worktree root")
                    .children(items)
                    .expanded(true),
            ];
        }
        let reveal = self.pending_reveal.clone();
        let revealed = self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            if let Some(id) = selected {
                tree.set_selected_index(tree.index_of(&id), cx);
            }
            if let Some(path) = &reveal {
                let id: SharedString = path.to_string_lossy().into_owned().into();
                if let Some(ix) = tree.index_of(&id) {
                    tree.set_selected_index(Some(ix), cx);
                    tree.scroll_to_item(ix, ScrollStrategy::Nearest);
                    return true;
                }
            }
            false
        });
        if revealed {
            self.pending_reveal = None;
        }
        cx.notify();
    }

    fn items(&self, directory: &Path, entries: &BTreeMap<PathBuf, Vec<Entry>>) -> Vec<TreeItem> {
        let mut children = entries
            .get(directory)
            .into_iter()
            .flatten()
            .filter(|entry| {
                if let Some(source) = &self.destination_for {
                    return entry.directory && !entry.path.starts_with(source);
                }
                (self.settings.show_hidden
                    || !entry
                        .path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with('.')))
                    && (self.settings.show_ignored || !self.ignored.contains(&entry.path))
            })
            .collect::<Vec<_>>();
        children.sort_by(|a, b| {
            b.directory
                .cmp(&a.directory)
                .then_with(|| a.path.cmp(&b.path))
        });
        children
            .into_iter()
            .map(|entry| {
                let id = entry.path.to_string_lossy().into_owned();
                let mut item = TreeItem::new(
                    id.clone(),
                    entry
                        .path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
                if entry.directory {
                    let children = self.items(&entry.path, entries);
                    let empty = if entries.contains_key(&entry.path) {
                        "Empty folder"
                    } else {
                        "Loading…"
                    };
                    item = item
                        .children(if children.is_empty() {
                            vec![TreeItem::new(format!("{id}/"), empty).disabled(true)]
                        } else {
                            children
                        })
                        .expanded(self.expanded.contains(&entry.path));
                }
                item
            })
            .collect()
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
