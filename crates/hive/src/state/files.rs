//! The Files navigator's model: the listed worktree, what is open, what Git says about it.
//! Holds no widget and no window: the panel projects `nodes()` into its own tree state.
use super::{
    documents::{DocumentEvent, OpenFile},
    git::GitEvent,
    projects::Projects,
};
use crate::app::AppState;
use hive_core::{
    files::{decorations::Decoration, tree::Node},
    settings::FilesSettings,
};
mod listing;
mod mutations;
mod tree;
pub(crate) use mutations::FileOperation;

/// A finished file operation. `warning` reports a secondary failure after success, such as
/// editor recovery that could not follow a moved file.
pub(crate) struct FilesEvent {
    pub target: Target,
    pub operation: FileOperation,
    pub result: Result<(), String>,
    pub warning: Option<String>,
}
impl EventEmitter<FilesEvent> for Files {}
use gpui_kit::*;
use hive_core::files::{self, Entry, FileIndex};
use hive_core::projects::Target;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

/// How often a visible navigator re-lists its open directories.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) struct Files {
    pub target: Option<Target>,
    pub error: Option<String>,
    visible: bool,
    window_active: bool,
    entries: BTreeMap<PathBuf, Vec<Entry>>,
    expanded: HashSet<PathBuf>,
    /// The tree as last built from the model; `tree_generation` changes with it.
    nodes: Vec<Node>,
    tree_generation: u64,
    pub loading: bool,
    pub busy: bool,
    pub settings: FilesSettings,
    pub decorations: BTreeMap<PathBuf, Decoration>,
    pub ignored: HashSet<PathBuf>,
    pub dirty: HashSet<PathBuf>,
    pub missing: HashSet<PathBuf>,
    pub active: Option<PathBuf>,
    destination_for: Option<PathBuf>,
    pending_reveal: Option<PathBuf>,
    pending_refresh: bool,
    generation: u64,
    _operation: Option<Task<()>>,
    _read: Option<Task<()>>,
    _poll: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Files {
    /// A navigator for the selected worktree, or, with `destination_for`, the folder picker
    /// for moving that path.
    pub fn new(
        projects: Entity<Projects>,
        destination_for: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
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
                let target = projects.read(cx).target();
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
                    this.rebuild(cx);
                    this.sync_documents(true, cx);
                    this.refresh(cx);
                    cx.notify();
                }
            }),
        ];
        let poll = cx.spawn(async |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        if this.visible && this.window_active {
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
            target: projects.read(cx).target(),
            error: None,
            visible: false,
            window_active: true,
            entries: BTreeMap::new(),
            expanded: HashSet::new(),
            nodes: Vec::new(),
            tree_generation: 0,
            loading: false,
            busy: false,
            settings,
            decorations: BTreeMap::new(),
            ignored: HashSet::new(),
            dirty: HashSet::new(),
            missing: HashSet::new(),
            active: None,
            destination_for,
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

    /// Whether the navigator is on screen; a visible one keeps its listing fresh.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if visible {
            self.refresh(cx);
        }
    }

    /// Tracks the window the navigator lives in, so a hidden window stops polling and a
    /// returning one re-lists at once.
    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.window_active = active;
        if active && self.visible {
            self.refresh_all(cx);
        }
    }

    /// The path this instance picks a destination folder for, if it is that picker.
    pub fn destination_for(&self) -> Option<&Path> {
        self.destination_for.as_deref()
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
                    position: None,
                    preview,
                    placement: Default::default(),
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
