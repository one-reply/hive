//! Open buffers have independent entities; the registry never owns a second copy of text.
use super::{git::Target, store::Store};
use gpui_kit::*;
use hive_core::files::recovery::{self, Record};
use std::path::PathBuf;

mod document;
pub(crate) use document::Document;

#[derive(Clone)]
pub(crate) struct OpenFile {
    pub target: Target,
    pub path: PathBuf,
    pub line: Option<usize>,
    pub preview: bool,
}

pub(crate) enum DocumentEvent {
    Open(OpenFile),
}

pub(crate) struct Documents {
    pub recovered: Vec<Record>,
    pub recovery_errors: Vec<String>,
    open: Vec<WeakEntity<Document>>,
    store: Entity<Store>,
}
impl EventEmitter<DocumentEvent> for Documents {}

impl Documents {
    pub fn new(store: Entity<Store>, (records, errors): (Vec<Record>, Vec<String>)) -> Self {
        Self {
            open: Vec::new(),
            recovered: records,
            recovery_errors: errors,
            store,
        }
    }

    pub fn request(&self, mut request: OpenFile, cx: &mut Context<Self>) {
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
                            let editor = document.read(cx).editor.read(cx);
                            let position = editor.cursor_position();
                            let offset = editor.scroll_offset();
                            *cursor = [position.line, position.character];
                            *scroll = [offset.x.into(), offset.y.into()];
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
