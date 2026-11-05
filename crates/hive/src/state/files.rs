use super::{documents::OpenFile, git::Target, projects::Projects};
use gpui_kit::component::tree::{TreeEvent, TreeItem, TreeState};
use gpui_kit::*;
use hive_core::files::{self, Entry, FileIndex};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) struct Files {
    pub target: Option<Target>,
    pub tree: Entity<TreeState>,
    pub error: Option<String>,
    pub visible: bool,
    entries: BTreeMap<PathBuf, Vec<Entry>>,
    expanded: HashSet<PathBuf>,
    loading: bool,
    _read: Option<Task<()>>,
    _poll: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Files {
    pub fn new(projects: Entity<Projects>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let subscriptions = vec![
            cx.observe(&projects, |this, projects, cx| {
                let target = selected_target(projects.read(cx));
                if this.target != target {
                    this.target = target;
                    this._read = None;
                    this.loading = false;
                    this.error = None;
                    this.entries.clear();
                    this.expanded.clear();
                    this.tree
                        .update(cx, |tree, cx| tree.set_items(Vec::new(), cx));
                    this.refresh(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&tree, |this, _, event, cx| {
                match event {
                    TreeEvent::Expanded(path) => {
                        this.expanded.insert(PathBuf::from(path.as_ref()));
                    }
                    TreeEvent::Collapsed(path) => {
                        this.expanded.remove(Path::new(path.as_ref()));
                    }
                }
                this.refresh(cx);
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() && this.visible {
                    this.refresh(cx);
                }
            }),
        ];
        let poll = cx.spawn_in(window, async |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                if this
                    .update_in(cx, |this, window, cx| {
                        if this.visible && window.is_window_active() {
                            this.refresh(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut this = Self {
            target: selected_target(projects.read(cx)),
            tree,
            error: None,
            visible: false,
            entries: BTreeMap::new(),
            expanded: HashSet::new(),
            loading: false,
            _read: None,
            _poll: poll,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(target) = self.target.clone() else {
            return;
        };
        self.loading = true;
        let paths: Vec<_> = std::iter::once(PathBuf::new())
            .chain(self.expanded.iter().cloned())
            .collect();
        self._read = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut directories = BTreeMap::new();
                    let mut errors = Vec::new();
                    for path in paths {
                        match files::list_directory(&target.host, &target.path, &path) {
                            Ok(entries) => {
                                directories.insert(path, entries);
                            }
                            Err(error) if path.as_os_str().is_empty() => {
                                return Err(error.to_string());
                            }
                            Err(error) => {
                                directories.insert(path, Vec::new());
                                errors.push(error.to_string());
                            }
                        }
                    }
                    Ok::<_, String>((directories, (!errors.is_empty()).then(|| errors.join("\n"))))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((directories, error)) => {
                        let error_changed = this.error != error;
                        if error_changed && let Some(error) = &error {
                            tracing::warn!(%error, "file tree directory unavailable");
                        }
                        let changed = directories
                            .iter()
                            .any(|(path, entries)| this.entries.get(path) != Some(entries));
                        this.entries.extend(directories);
                        this.error = error;
                        if changed {
                            let selected = this
                                .tree
                                .read(cx)
                                .selected_item()
                                .map(|item| item.id.clone());
                            let items = this.items(Path::new(""));
                            this.tree.update(cx, |tree, cx| {
                                tree.set_items(items, cx);
                                if let Some(id) = selected {
                                    tree.set_selected_index(tree.index_of(&id), cx);
                                }
                            });
                        }
                        if changed || error_changed {
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        if this.error.as_ref() != Some(&error) {
                            tracing::warn!(%error, "file tree failed");
                            this.error = Some(error);
                            cx.notify();
                        }
                    }
                }
            });
        }));
    }

    fn items(&self, directory: &Path) -> Vec<TreeItem> {
        self.entries
            .get(directory)
            .into_iter()
            .flatten()
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
                    let children = self.items(&entry.path);
                    item = item
                        .children(if children.is_empty() {
                            vec![
                                TreeItem::new(
                                    format!("{id}/"),
                                    if self.entries.contains_key(&entry.path) {
                                        "Empty folder"
                                    } else {
                                        "Loading…"
                                    },
                                )
                                .disabled(true),
                            ]
                        } else {
                            children
                        })
                        .expanded(self.expanded.contains(&entry.path));
                }
                item
            })
            .collect()
    }

    pub fn open(&self, path: PathBuf, preview: bool, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let documents = crate::app::AppState::get(cx).documents.clone();
        documents.update(cx, |documents, cx| {
            documents.request(
                OpenFile {
                    target,
                    path,
                    line: None,
                    preview,
                },
                cx,
            )
        });
    }

    pub fn index(&self, cx: &App) -> Option<(Target, Task<Result<FileIndex, String>>)> {
        let target = self.target.clone()?;
        let index_target = target.clone();
        Some((
            target,
            cx.background_executor().spawn(async move {
                files::list_files(&index_target.host, &index_target.path).map_err(|e| e.to_string())
            }),
        ))
    }
}

fn selected_target(projects: &Projects) -> Option<Target> {
    let (project, worktree) = projects.selection()?;
    Some(Target {
        project: project.path.clone(),
        path: worktree.map_or_else(|| project.path.clone(), |w| w.path.clone()),
        host: project.host.clone(),
    })
}
