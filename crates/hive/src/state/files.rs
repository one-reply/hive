//! The Files navigator's model: the listed worktree, what is open, what Git says about it.
//! Holds no widget and no window: the panel projects `nodes()` into its own tree state.
//! One navigator lists one worktree for its whole life, so the expanded folders and the
//! search of a worktree survive while another one is shown.
use super::{
    documents::{DocumentEvent, OpenFile},
    git::{GitEvent, Tracking},
};
use crate::app::AppState;
use hive_core::{
    files::{decorations::Decoration, tree::Node},
    settings::FilesSettings,
};
mod content_search;
mod listing;
mod mutations;
mod recent;
mod search;
mod tree;
pub(crate) use content_search::{ContentStatus, SearchMode, SearchOption};
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
    /// Fixed at creation; `None` is the navigator shown while nothing is selected.
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
    /// Files of this worktree opened last, newest first; search ranks them higher.
    recent: Vec<PathBuf>,
    destination_for: Option<PathBuf>,
    pending_reveal: Option<PathBuf>,
    pending_refresh: bool,
    search: search::Search,
    search_mode: SearchMode,
    content: content_search::ContentSearch,
    generation: u64,
    _operation: Option<Task<()>>,
    _read: Option<Task<()>>,
    /// Re-lists the open directories; runs only while the navigator is on screen.
    _poll: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// The badges `tracking` gives, or `None` while it cannot tell.
fn decorations(tracking: Tracking<'_>) -> Option<BTreeMap<PathBuf, Decoration>> {
    match tracking {
        Tracking::Unknown => None,
        Tracking::NoRepository => Some(BTreeMap::new()),
        Tracking::Status(status) => Some(files::decorations::collect(&status.files)),
    }
}

impl Files {
    /// A navigator for `target`, or, with `destination_for`, the folder picker for moving
    /// that path.
    pub fn new(
        target: Option<Target>,
        destination_for: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let app = AppState::get(cx);
        let (store, git, documents) = (app.store.clone(), app.git.clone(), app.documents.clone());
        let settings = store.read(cx).state.settings.files.clone();
        // A status already read for this worktree will not be announced again.
        let known_decorations =
            decorations(git.read(cx).tracking(target.as_ref())).unwrap_or_default();
        let subscriptions = vec![
            // The store notifies on every layout and cursor change; clone only a real change.
            cx.observe(&store, |this, store, cx| {
                let settings = &store.read(cx).state.settings.files;
                if *settings != this.settings {
                    let reveal = settings.auto_reveal && !this.settings.auto_reveal;
                    this.settings = settings.clone();
                    if reveal && let Some(path) = this.active.clone() {
                        this.reveal(path, cx);
                    }
                    this.rebuild(cx);
                    this.search(cx);
                    this.refresh(cx);
                    cx.notify();
                }
            }),
            cx.observe(&git, |this, git, cx| {
                // Badges stay as they were until this worktree's own status arrives.
                let tracking = git.read(cx).tracking(this.target.as_ref());
                let Some(decorations) = decorations(tracking) else {
                    return;
                };
                if decorations != this.decorations {
                    this.decorations = decorations;
                    this.rebuild(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&git, |this, _, event, cx| {
                // An unchanged status leaves the listing to Files' own poll.
                if let GitEvent::Refreshed {
                    target,
                    changed: true,
                } = event
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
                            recent::remember(&mut this.recent, path);
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
                    DocumentEvent::Open(_)
                    | DocumentEvent::Reveal(_, _)
                    | DocumentEvent::External(_, _, _)
                    | DocumentEvent::AgentDraft(_, _, _) => return,
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
        ];
        let mut this = Self {
            target,
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
            decorations: known_decorations,
            ignored: HashSet::new(),
            dirty: HashSet::new(),
            missing: HashSet::new(),
            active: None,
            recent: Vec::new(),
            destination_for,
            pending_reveal: None,
            pending_refresh: false,
            search: search::Search::default(),
            search_mode: SearchMode::default(),
            content: content_search::ContentSearch::default(),
            generation: 0,
            _operation: None,
            _read: None,
            _poll: None,
            _subscriptions: subscriptions,
        };
        if this.destination_for.is_none() {
            this.sync_documents(true, cx);
            if let Some(path) = this.active.clone() {
                recent::remember(&mut this.recent, &path);
            }
        }
        // Revealing the active file has already listed the tree with its folders open.
        if !this.loading {
            this.refresh(cx);
        }
        this
    }

    /// Whether the navigator is on screen; a visible one keeps its listing fresh, a hidden
    /// one stops polling.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.visible = visible;
        if !visible {
            self._poll = None;
            return;
        }
        if self._poll.is_none() {
            self._poll = Some(Self::poll(cx));
        }
        self.refresh(cx);
    }

    fn poll(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this
                    .update(cx, |this, cx| {
                        if this.window_active {
                            this.refresh(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
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
        self.request_open(path, None, preview, cx);
    }

    /// Open a file with the cursor at a zero-based line and char column.
    pub fn open_at(
        &self,
        path: PathBuf,
        line: u32,
        column: u32,
        preview: bool,
        cx: &mut Context<Self>,
    ) {
        let position = gpui_kit::component::input::Position::new(line, column);
        self.request_open(path, Some(position), preview, cx);
    }

    fn request_open(
        &self,
        path: PathBuf,
        position: Option<gpui_kit::component::input::Position>,
        preview: bool,
        cx: &mut Context<Self>,
    ) {
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
                    position,
                    preview,
                    placement: Default::default(),
                },
                cx,
            )
        });
    }
}
