use super::*;
use crate::state::documents::{DocumentEvent, OpenFile};
use crate::state::files::{FileOperation, Files, FilesEvent, SearchMode};
use crate::ui::files_panel::FilesPanel;
use right_tab::keeps_panel;

/// One worktree's Files panel. Dropping it drops the navigator and its listeners.
pub(super) struct WorktreeFiles {
    panel: Entity<FilesPanel>,
    _events: Subscription,
}

impl WorktreeFiles {
    pub(super) fn new(
        target: Option<Target>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let files = cx.new(|cx| Files::new(target, None, cx));
        let events = cx.subscribe(&files, |this, _, event: &FilesEvent, _| {
            if event.result.is_ok()
                && let FileOperation::Move { from, to } = &event.operation
            {
                for center in this.centers.values_mut() {
                    center.navigation.relocate(&event.target, from, to);
                }
            }
        });
        Self {
            panel: cx.new(|cx| FilesPanel::new(files, window, cx)),
            _events: events,
        }
    }

    pub(super) fn panel(&self) -> Entity<FilesPanel> {
        self.panel.clone()
    }
}

impl Workspace {
    /// Shows the right sidebar the selected worktree had: its own Files panel and the tab
    /// that was open there. Panels of worktrees that no longer exist are dropped.
    pub(super) fn sync_right_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let projects = self.projects.read(cx);
        let target = projects.target();
        self.files_panels.retain(|panel, _| {
            keeps_panel(panel.as_ref(), target.as_ref(), |worktree| {
                projects.contains_target(worktree)
            })
        });
        let previous = self.files_panel.read(cx).files.read(cx).target().cloned();
        if previous == target {
            return;
        }
        self.set_files_visible(false, cx);
        self.files_panel = self
            .files_panels
            .entry(target.clone())
            .or_insert_with(|| WorktreeFiles::new(target.clone(), window, cx))
            .panel();
        let shown_tab = RightTab::selected(cx);
        let tab = self
            .right_tabs
            .follow(previous.as_ref(), target.as_ref(), shown_tab);
        self.set_right_tab(tab, cx);
        if tab != shown_tab {
            // The refresh `Git` started for this worktree counted on the other tab.
            self.refresh_right_tab(cx);
        }
        cx.notify();
    }

    pub(super) fn choose_right_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if tab == RightTab::Git && AppState::get(cx).git.read(cx).has_no_repository() {
            return;
        }
        self.set_right_tab(tab, cx);
        self.refresh_right_tab(cx);
        cx.notify();
    }

    /// Makes `tab` the right sidebar's tab.
    fn set_right_tab(&self, tab: RightTab, cx: &mut Context<Self>) {
        if RightTab::selected(cx) != tab {
            let store = AppState::get(cx).store.clone();
            store.update(cx, |store, cx| store.set_files_selected(tab.is_files(), cx));
        }
        self.sync_right_tab_visibility(tab, cx);
    }

    /// Tells the Git and Files models which of them is on screen: the one behind `tab`
    /// while the sidebar is open, neither while it is closed.
    pub(super) fn sync_right_tab_visibility(&self, tab: RightTab, cx: &mut Context<Self>) {
        let open = self.git_open;
        AppState::get(cx).git.clone().update(cx, |git, _| {
            git.panel_visible = open && tab == RightTab::Git;
            git.files_visible = open && tab == RightTab::Files;
        });
        self.set_files_visible(open && tab == RightTab::Files, cx);
    }

    /// Reads Git again for the tab that is shown: with change statistics for the Git tab,
    /// the plain status for Files' badges.
    pub(super) fn refresh_right_tab(&self, cx: &mut Context<Self>) {
        AppState::get(cx).git.clone().update(cx, |git, cx| {
            if git.files_visible {
                git.refresh(false, cx);
            }
            if git.panel_visible {
                git.refresh(true, cx);
            }
        });
    }

    fn set_files_visible(&self, visible: bool, cx: &mut Context<Self>) {
        self.files_panel
            .read(cx)
            .files
            .clone()
            .update(cx, |files, cx| files.set_visible(visible, cx));
    }

    /// Selects `tab` in the right sidebar, opening the sidebar if it is closed.
    pub(super) fn show_right_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        self.choose_right_tab(tab, cx);
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
        self.restore_dock(window, cx);
        self.show_right_tab(RightTab::Files, cx);
        let panel = self.files_panel.clone();
        window.defer(cx, move |window, cx| {
            panel.update(cx, |panel, cx| panel.open_search(mode, window, cx))
        });
    }

    pub(super) fn watch_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_files_visible(self.git_open && RightTab::selected(cx).is_files(), cx);
        let documents = AppState::get(cx).documents.clone();
        self._observers.push(
            cx.subscribe_in(
                &documents,
                window,
                |this, _, event, window, cx| match event {
                    DocumentEvent::Open(request) => this.open_file(request.clone(), window, cx),
                    DocumentEvent::External(target, path, position) => {
                        crate::ui::editor_panel::source_preview::open(
                            target.clone(),
                            path.clone(),
                            *position,
                            window,
                            cx,
                        )
                    }
                    DocumentEvent::AgentDraft(target, agent, prompt) => {
                        this.activate_target(target, cx);
                        this.center(&(target.host.clone(), target.project.clone()), window, cx)
                            .prepare_agent_draft(&target.path, *agent, prompt.clone(), window, cx);
                        this.sync_active_worktrees(cx);
                        this.save_center(
                            &(target.host.clone(), target.project.clone()),
                            window,
                            cx,
                        );
                        cx.notify();
                    }
                    DocumentEvent::Reveal(target, path) => {
                        this.activate_target(target, cx);
                        // The file's worktree gets its panel now, not when observers run.
                        this.sync_right_sidebar(window, cx);
                        this.show_right_tab(RightTab::Files, cx);
                        let files = this.files_panel.read(cx).files.clone();
                        files.update(cx, |files, cx| {
                            if files.target() == Some(target) {
                                files.reveal_in_tree(path.clone(), cx);
                            }
                        });
                    }
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
        let project = (request.target.host.clone(), request.target.project.clone());
        if request.position.is_some()
            && !preview
            && let Some(center) = self.centers.get_mut(&project)
            && let Some(from) = center.current_file(window, cx)
        {
            center.navigation.record(from);
        }
        let documents = AppState::get(cx).documents.clone();
        let document = documents.update(cx, |documents, cx| documents.get(&request, window, cx));
        self.activate_target(&request.target, cx);
        let project = (request.target.host.clone(), request.target.project.clone());
        self.center(&project, window, cx)
            .show_file(request, window, cx);
        self.save_center(&project, window, cx);
        if preview {
            self.sync_right_sidebar(window, cx);
            let panel = self.files_panel.clone();
            window.defer(cx, move |window, cx| {
                panel.update(cx, |panel, cx| panel.focus_list(window, cx))
            });
        }
        drop(document);
        cx.notify();
    }

    pub(super) fn navigate_file(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.projects.read(cx).target() else {
            return;
        };
        let project = (target.host, target.project);
        let Some(center) = self.centers.get_mut(&project) else {
            return;
        };
        let current = center.current_file(window, cx);
        let Some(request) = center.navigation.step(forward, current) else {
            return;
        };
        self.activate_target(&request.target, cx);
        self.center(&project, window, cx)
            .show_file(request, window, cx);
        self.save_center(&project, window, cx);
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
