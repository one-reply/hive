use super::*;
use crate::state::documents::{DocumentEvent, OpenFile};
use crate::state::files::SearchMode;

impl Workspace {
    pub(super) fn choose_files(&mut self, selected: bool, cx: &mut Context<Self>) {
        if !selected && AppState::get(cx).git.read(cx).has_no_repository() {
            return;
        }
        AppState::get(cx)
            .store
            .clone()
            .update(cx, |store, cx| store.set_files_selected(selected, cx));
        AppState::get(cx).git.clone().update(cx, |git, cx| {
            git.panel_visible = self.git_open && !selected;
            git.files_visible = self.git_open && selected;
            if git.files_visible {
                git.refresh(false, cx);
            }
            if git.panel_visible {
                git.refresh(true, cx);
            }
        });
        let visible = self.git_open && selected;
        self.files_panel
            .read(cx)
            .files
            .clone()
            .update(cx, |files, cx| files.set_visible(visible, cx));
        cx.notify();
    }

    /// Selects the right sidebar's Files (`true`) or Git tab, opening the sidebar if closed.
    pub(super) fn show_right_tab(&mut self, files: bool, cx: &mut Context<Self>) {
        self.choose_files(files, cx);
        if !self.git_open {
            self.toggle_git(cx);
        }
    }

    /// ⌘P and ⌘⇧F: the Files tab with its search field in `mode`, text selected.
    pub(super) fn open_file_search(
        &mut self,
        mode: SearchMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_right_tab(true, cx);
        let panel = self.files_panel.clone();
        window.defer(cx, move |window, cx| {
            panel.update(cx, |panel, cx| panel.open_search(mode, window, cx))
        });
    }

    pub(super) fn watch_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .files_selected;
        let visible = self.git_open && selected;
        self.files_panel
            .read(cx)
            .files
            .clone()
            .update(cx, |files, cx| files.set_visible(visible, cx));
        let documents = AppState::get(cx).documents.clone();
        self._observers.push(
            cx.subscribe_in(
                &documents,
                window,
                |this, _, event, window, cx| match event {
                    DocumentEvent::Open(request) => this.open_file(request.clone(), window, cx),
                    DocumentEvent::Deleted(target, path) => {
                        for center in this.centers.values_mut() {
                            center.remove_files(target, path, window, cx);
                        }
                    }
                    _ => {}
                },
            ),
        );
        let projects = self.projects.clone();
        self._observers
            .push(cx.observe_in(&projects, window, |this, _, window, cx| {
                this.restore_drafts(window, cx)
            }));
        self.restore_drafts(window, cx);
    }

    fn open_file(&mut self, request: OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let preview = request.preview;
        let documents = AppState::get(cx).documents.clone();
        let document = documents.update(cx, |documents, cx| documents.get(&request, window, cx));
        self.activate_target(&request.target, cx);
        let project = (request.target.host.clone(), request.target.project.clone());
        self.center(&project, window, cx)
            .show_file(request, window, cx);
        self.save_center(&project, window, cx);
        if preview {
            let panel = self.files_panel.clone();
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.focus_list(window, cx))
            });
        }
        drop(document);
        cx.notify();
    }

    fn restore_drafts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let documents = AppState::get(cx).documents.clone();
        let records = documents.read(cx).recovered.clone();
        for record in records {
            let draft = &record.draft;
            if self
                .projects
                .read(cx)
                .items
                .iter()
                .any(|p| p.path == draft.project && p.host == draft.host)
            {
                self.open_file(
                    OpenFile {
                        target: hive_core::projects::Target {
                            project: draft.project.clone(),
                            path: draft.root.clone(),
                            host: draft.host.clone(),
                        },
                        path: draft.path.clone(),
                        preview: false,
                        position: None,
                        placement: Default::default(),
                    },
                    window,
                    cx,
                );
            } else {
                if AppState::get(cx)
                    .store
                    .read(cx)
                    .state
                    .projects
                    .iter()
                    .any(|p| p.path == draft.project && p.host == draft.host)
                {
                    continue;
                }
                documents.update(cx, |documents, _| {
                    documents.recovered.retain(|r| r.key != record.key)
                });
                super::super::editor_panel::close::recovered(record, window, cx);
            }
        }
        let errors = documents.update(cx, |documents, _| {
            std::mem::take(&mut documents.recovery_errors)
        });
        for error in errors {
            window.push_notification(
                Notification::error(format!("Recovery: {error}. Original record retained.")),
                cx,
            );
        }
    }
}
