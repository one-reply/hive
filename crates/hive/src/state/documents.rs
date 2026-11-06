//! Open buffers have independent entities; the registry never owns a second copy of text.
use super::store::Store;
use gpui_kit::component::input::Position;
use gpui_kit::*;
use hive_core::code_navigation::Navigator;
use hive_core::files::recovery::{self, Record};
use hive_core::projects::Target;
use std::path::PathBuf;
use std::sync::Arc;

mod definitions;
mod document;
pub(crate) use document::{Content, Document, PreviewState, SvgMode};

#[derive(Clone)]
pub(crate) struct OpenFile {
    pub target: Target,
    pub path: PathBuf,
    /// Zero-based cursor target, applied once the text is loaded.
    pub position: Option<Position>,
    pub preview: bool,
    pub placement: TabPlacement,
}

/// Where a newly opened file's tab goes; a file that already has a tab is focused instead.
#[derive(Clone, Default)]
pub(crate) enum TabPlacement {
    /// The dock's default position.
    #[default]
    Default,
    /// Right after the tab showing this document (⌘⇧-click Go to Definition).
    Beside(WeakEntity<Document>),
    /// In place of the tab showing this document, if it has no unsaved edits; beside it
    /// otherwise (⌘-click Go to Definition).
    Replacing(WeakEntity<Document>),
}

pub(crate) enum DocumentEvent {
    Open(OpenFile),
    Activated(Target, PathBuf),
    Changed(Target),
    Saved(Target),
    Deleted(Target, PathBuf),
}

pub(crate) struct Documents {
    pub recovered: Vec<Record>,
    pub recovery_errors: Vec<String>,
    open: Vec<WeakEntity<Document>>,
    pub(crate) mutating_paths: Vec<(Target, PathBuf)>,
    store: Entity<Store>,
    /// Shared by every document so `package.json`/`tsconfig` parses are reused.
    navigator: Arc<Navigator>,
}
impl EventEmitter<DocumentEvent> for Documents {}

impl Documents {
    pub(crate) fn in_path(
        &self,
        target: &Target,
        path: &std::path::Path,
        cx: &App,
    ) -> Vec<Entity<Document>> {
        self.open
            .iter()
            .filter_map(WeakEntity::upgrade)
            .filter(|document| {
                let document = document.read(cx);
                document.target.host == target.host
                    && document.target.path == target.path
                    && document.path.starts_with(path)
            })
            .collect()
    }

    pub(crate) fn emit(&self, event: DocumentEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }
    pub fn new(store: Entity<Store>, (records, errors): (Vec<Record>, Vec<String>)) -> Self {
        Self {
            open: Vec::new(),
            mutating_paths: Vec::new(),
            recovered: records,
            recovery_errors: errors,
            store,
            navigator: Arc::default(),
        }
    }

    pub fn request(&self, mut request: OpenFile, cx: &mut Context<Self>) {
        if self.mutating_paths.iter().any(|(target, path)| {
            target.host == request.target.host
                && target.path == request.target.path
                && request.path.starts_with(path)
        }) {
            tracing::warn!(path = ?request.path, "cannot open a document while its path is being modified");
            return;
        }
        if let Some(document) = self.open.iter().filter_map(WeakEntity::upgrade).find(|d| {
            let d = d.read(cx);
            d.target.host == request.target.host
                && d.target.path == request.target.path
                && d.path == request.path
        }) {
            request.target.project = document.read(cx).target.project.clone();
        }
        cx.emit(DocumentEvent::Open(request));
    }

    pub fn get(
        &mut self,
        request: &OpenFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Document> {
        self.open.retain(|document| document.upgrade().is_some());
        if let Some(document) = self.open.iter().filter_map(WeakEntity::upgrade).find(|d| {
            let d = d.read(cx);
            d.target.host == request.target.host
                && d.target.path == request.target.path
                && d.path == request.path
        }) {
            if let Some(position) = request.position {
                document.update(cx, |document, cx| document.goto(position, window, cx));
            }
            return document;
        }
        let recovered = self
            .recovered
            .iter()
            .position(|r| {
                r.draft.host == request.target.host
                    && r.draft.root == request.target.path
                    && r.draft.path == request.path
            })
            .map(|index| self.recovered.remove(index));
        let document =
            cx.new(|cx| Document::new(request.clone(), recovered, self.store.clone(), window, cx));
        definitions::install(&document, cx.weak_entity(), self.navigator.clone(), cx);
        self.open.push(document.downgrade());
        document
    }

    pub fn dirty(&self, cx: &App) -> Vec<Entity<Document>> {
        self.open
            .iter()
            .filter_map(WeakEntity::upgrade)
            .filter(|d| d.read(cx).dirty)
            .collect()
    }

    pub fn flush(&self, cx: &mut App) {
        for document in self.dirty(cx) {
            document.update(cx, |d, cx| d.flush_recovery(cx));
        }
        let documents: Vec<_> = self.open.iter().filter_map(WeakEntity::upgrade).collect();
        let projects = self.store.read(cx).state.workspace.projects.clone();
        for mut project in projects {
            if let Some(layout) = &project.layout {
                project.layout = layout.filter_map(&mut |tabs| {
                    let mut tabs = tabs.clone();
                    for tab in &mut tabs.items {
                        if let hive_core::state::workspace::Tab::File {
                            worktree,
                            path,
                            cursor,
                            scroll,
                        } = tab
                            && let Some(document) = documents.iter().find(|d| {
                                let d = d.read(cx);
                                d.target.host == project.host
                                    && d.target.path == *worktree
                                    && d.path == *path
                            })
                        {
                            (*cursor, *scroll) = document.read(cx).saved_position(cx);
                        }
                    }
                    Some(tabs)
                });
                self.store
                    .update(cx, |store, cx| store.set_project_workspace(project, cx));
            }
        }
    }
}
