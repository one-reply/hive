use super::{
    documents::{DocumentEvent, OpenFile},
    git::{GitEvent, Target},
    projects::Projects,
};
use crate::app::AppState;
use hive_core::{files::decorations::Decoration, settings::FilesSettings};
mod listing;
mod mutations;
mod tree;
pub(crate) use mutations::FileOperation;

pub(crate) struct FilesEvent {
    pub target: Target,
    pub operation: FileOperation,
    pub result: Result<(), String>,
}
impl EventEmitter<FilesEvent> for Files {}
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
    pub loading: bool,
    pub busy: bool,
    pub settings: FilesSettings,
    pub decorations: BTreeMap<PathBuf, Decoration>,
    pub ignored: HashSet<PathBuf>,
    pub dirty: HashSet<PathBuf>,
    pub missing: HashSet<PathBuf>,
    pub active: Option<PathBuf>,
    pub destination_for: Option<PathBuf>,
    pending_reveal: Option<PathBuf>,
    pending_refresh: bool,
    generation: u64,
    _operation: Option<Task<()>>,
    _read: Option<Task<()>>,
    _poll: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Files {
    pub fn new(projects: Entity<Projects>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let app = AppState::get(cx);
        let (store, git, documents) = (app.store.clone(), app.git.clone(), app.documents.clone());
        let settings = store.read(cx).state.settings.files.clone();
        let subscriptions = vec![
            cx.observe(&store, |this, store, cx| {
                let settings = store.read(cx).state.settings.files.clone();
                if settings != this.settings {
                    let reveal = settings.auto_reveal && !this.settings.auto_reveal;
                    this.settings = settings;
                    if reveal && let Some(path) = this.active.clone() {
                        this.reveal(path, cx);
                    }
                    this.rebuild(cx);
                    this.refresh(cx);
                    cx.notify();
                }
            }),
            cx.observe(&git, |this, git, cx| {
                let git = git.read(cx);
                let decorations = git
                    .status
                    .as_ref()
                    .filter(|_| git.target == this.target)
                    .map(|s| files::decorations::collect(&s.files))
                    .unwrap_or_default();
                if decorations != this.decorations {
                    this.decorations = decorations;
                    this.rebuild(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&git, |this, _, event, cx| {
                if let GitEvent::Refreshed(target) = event
                    && this.target.as_ref() == Some(target)
                    && this.visible
                {
                    this.refresh(cx);
                }
            }),
            cx.subscribe(&documents, |this, documents, event, cx| {
                if this.destination_for.is_some() {
                    return;
                }
                let target = match event {
                    DocumentEvent::Activated(target, path) => {
                        if this.target.as_ref() == Some(target) {
                            this.active = Some(path.clone());
                            if this.settings.auto_reveal {
                                this.reveal(path.clone(), cx);
                            }
                        }
                        target
                    }
                    DocumentEvent::Changed(target)
                    | DocumentEvent::Saved(target)
                    | DocumentEvent::Deleted(target, _) => target,
                    DocumentEvent::Open(_) => return,
                };
                if this.target.as_ref() != Some(target) {
                    return;
                }
                this.dirty = documents
                    .read(cx)
                    .dirty(cx)
                    .into_iter()
                    .filter_map(|document| {
                        let document = document.read(cx);
                        (document.target == *target).then(|| document.path.clone())
                    })
                    .collect();
                if matches!(event, DocumentEvent::Saved(_)) {
                    this.refresh(cx);
                    let git = AppState::get(cx).git.clone();
                    cx.defer(move |cx| git.update(cx, |git, cx| git.refresh(false, cx)));
                }
                cx.notify();
            }),
            cx.observe(&projects, |this, projects, cx| {
                let target = selected_target(projects.read(cx));
                if this.target != target {
                    this.target = target;
                    this._read = None;
                    this.generation += 1;
                    this.pending_refresh = false;
                    this.pending_reveal = None;
                    this.active = None;
                    this.decorations.clear();
                    this.ignored.clear();
                    this.dirty.clear();
                    this.missing.clear();
                    this.loading = false;
                    this.error = None;
                    this.entries.clear();
                    this.expanded.clear();
                    this.tree
                        .update(cx, |tree, cx| tree.set_items(Vec::new(), cx));
                    this.sync_documents(true, cx);
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
                    this.refresh_all(cx);
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
            busy: false,
            settings,
            decorations: BTreeMap::new(),
            ignored: HashSet::new(),
            dirty: HashSet::new(),
            missing: HashSet::new(),
            active: None,
            destination_for: None,
            pending_reveal: None,
            pending_refresh: false,
            generation: 0,
            _operation: None,
            _read: None,
            _poll: poll,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    fn sync_documents(&mut self, reveal: bool, cx: &mut Context<Self>) {
        let Some(target) = &self.target else { return };
        let documents = AppState::get(cx)
            .documents
            .read(cx)
            .in_path(target, Path::new(""), cx);
        self.dirty = documents
            .iter()
            .filter_map(|document| {
                let document = document.read(cx);
                document.dirty.then(|| document.path.clone())
            })
            .collect();
        self.active = documents.iter().find_map(|document| {
            let document = document.read(cx);
            document.visible.then(|| document.path.clone())
        });
        if reveal
            && self.settings.auto_reveal
            && let Some(path) = self.active.clone()
        {
            self.reveal(path, cx);
        }
    }

    pub fn open(&self, path: PathBuf, preview: bool, cx: &mut Context<Self>) {
        if self.missing.contains(&path) || self.busy {
            return;
        }
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
