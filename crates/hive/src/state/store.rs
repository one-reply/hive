use gpui_kit::*;
use hive_core::host::Host;
use hive_core::identity::ProjectIdentity;
use hive_core::settings::{Scrollbars, ThemeChoice};
use hive_core::state::workspace::{ProjectWorkspace, Selection};
use hive_core::state::{LoadedState, ProjectRef, State, StateError};
use std::path::{Path, PathBuf};
use std::time::Instant;

mod recovery;
mod writer;
use writer::Writer;

/// Persisted state. Every mutation saves to disk and notifies observers.
pub struct Store {
    pub state: State,
    pub(crate) notice: Option<String>,
    writer: Option<Writer>,
    recovery: recovery::RecoveryWriter,
    _recovery_errors: Task<()>,
    stopping: bool,
    _errors: Option<Task<()>>,
}

pub(crate) struct SaveError(pub(crate) String);
impl EventEmitter<SaveError> for Store {}

impl Store {
    pub(crate) fn take_release_notes(&mut self, cx: &mut Context<Self>) -> bool {
        if self.writer.is_none() || self.stopping {
            return false;
        }
        if !self
            .state
            .mark_release_seen(hive_core::release_notes::VERSION)
        {
            return false;
        }
        self.commit(cx);
        true
    }

    pub(crate) fn set_files_settings(
        &mut self,
        settings: hive_core::settings::FilesSettings,
        cx: &mut Context<Self>,
    ) {
        if self.state.settings.files != settings {
            self.state.settings.files = settings;
            self.commit(cx);
        }
    }

    pub(crate) fn replace_recovery(
        &self,
        key: String,
        draft: Option<hive_core::files::recovery::Draft>,
        retired: Vec<String>,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.recovery.replace(key, draft, retired)
    }
    pub(crate) fn set_default_cli_agent(
        &mut self,
        agent: hive_core::agent::AgentKind,
        cx: &mut Context<Self>,
    ) {
        if self.state.settings.default_cli_agent() == agent {
            return;
        }
        if let Err(error) = self.state.settings.set_default_cli_agent(agent) {
            tracing::error!(%error, "could not select default CLI agent");
            return;
        }
        self.commit(cx);
    }
    pub(crate) fn set_selection(&mut self, selection: Option<Selection>, cx: &mut Context<Self>) {
        if self.state.workspace.selection != selection {
            self.state.workspace.selection = selection;
            self.commit(cx);
        }
    }

    pub(crate) fn set_project_workspace(
        &mut self,
        workspace: ProjectWorkspace,
        cx: &mut Context<Self>,
    ) {
        let projects = &mut self.state.workspace.projects;
        if let Some(previous) = projects
            .iter_mut()
            .find(|p| p.project == workspace.project && p.host == workspace.host)
        {
            if *previous == workspace {
                return;
            }
            *previous = workspace;
        } else {
            projects.push(workspace);
        }
        self.commit(cx);
    }

    pub(crate) fn set_commit_generator(
        &mut self,
        generator: Option<hive_core::commit_message::Generator>,
        cx: &mut Context<Self>,
    ) -> Result<(), hive_core::commit_message::Error> {
        let recipe = &mut self.state.settings.commit_message;
        if recipe.generator() == generator.as_ref() {
            return Ok(());
        }
        recipe.set_generator(generator)?;
        self.commit(cx);
        Ok(())
    }

    pub(crate) fn set_commit_prompt(
        &mut self,
        prompt: String,
        cx: &mut Context<Self>,
    ) -> Result<(), hive_core::commit_message::Error> {
        let recipe = &mut self.state.settings.commit_message;
        if recipe.prompt() == prompt {
            return Ok(());
        }
        recipe.set_prompt(prompt)?;
        self.commit(cx);
        Ok(())
    }
    pub(crate) fn set_git_panel_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.state.settings.set_git_panel_width(width);
        self.commit(cx);
    }

    pub(crate) fn set_git_panel_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.state.settings.git_panel_open = open;
        self.commit(cx);
    }

    pub(crate) fn set_diff_split(&mut self, split: bool, cx: &mut Context<Self>) {
        self.state.settings.diff_split = split;
        self.commit(cx);
    }
    pub(crate) fn set_confirm_close_running_agent(
        &mut self,
        confirm: bool,
        cx: &mut Context<Self>,
    ) {
        self.state.settings.confirm_close_running_agent = confirm;
        self.commit(cx);
    }

    pub(crate) fn set_files_selected(&mut self, files: bool, cx: &mut Context<Self>) {
        self.state.settings.files_selected = files;
        self.commit(cx);
    }
    pub(crate) fn new(loaded: Result<LoadedState, StateError>, cx: &mut Context<Self>) -> Self {
        let (recovery, recovery_errors) = recovery::RecoveryWriter::new(cx.background_executor());
        let recovery_errors = cx.spawn(async move |this, cx| {
            while let Ok(error) = recovery_errors.recv().await {
                if this.update(cx, |_, cx| cx.emit(SaveError(error))).is_err() {
                    break;
                }
            }
        });
        let (state, notice) = match loaded {
            Ok(loaded) => (
                loaded.state,
                loaded.recovery.map(|recovery| recovery_notice(&recovery)),
            ),
            Err(error) => {
                tracing::error!(%error, "state loading failed; persistence disabled");
                return Self {
                    state: State::default(),
                    notice: Some(format!(
                        "{error}. Saving is disabled to protect the original state. Restart Hive after fixing the file."
                    )),
                    writer: None,
                    recovery,
                    _recovery_errors: recovery_errors,
                    stopping: false,
                    _errors: None,
                };
            }
        };
        let (writer, errors) = Writer::new(cx.background_executor());
        let errors = cx.spawn(async move |this, cx| {
            while let Ok(error) = errors.recv().await {
                if this.update(cx, |_, cx| cx.emit(SaveError(error))).is_err() {
                    break;
                }
            }
        });
        Self {
            state,
            notice,
            writer: Some(writer),
            recovery,
            _recovery_errors: recovery_errors,
            stopping: false,
            _errors: Some(errors),
        }
    }

    pub(crate) fn stop(&mut self) {
        self.stopping = true;
        self.recovery.close();
        if let Some(writer) = &self.writer {
            writer.close();
        }
    }

    pub(crate) fn wait_until(&self, deadline: Instant) -> bool {
        let saved = self
            .writer
            .as_ref()
            .is_none_or(|writer| writer.wait_until(deadline));
        self.recovery.wait_until(deadline) && saved
    }

    pub(crate) fn save_recovery(
        &self,
        key: String,
        draft: Option<hive_core::files::recovery::Draft>,
    ) {
        self.recovery.submit(key, draft);
    }

    pub fn set_theme(&mut self, theme: ThemeChoice, cx: &mut Context<Self>) {
        self.state.settings.theme = theme;
        self.commit(cx);
    }

    pub fn set_sidebar_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.state.settings.set_sidebar_width(width);
        self.commit(cx);
    }

    pub fn set_ui_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.state.settings.set_ui_font_size(size);
        self.commit(cx);
    }

    pub fn set_terminal_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        self.state.settings.set_terminal_font_size(size);
        self.commit(cx);
    }

    pub fn set_ui_font_family(&mut self, family: Option<&str>, cx: &mut Context<Self>) {
        self.state.settings.set_ui_font_family(family);
        self.commit(cx);
    }

    pub fn set_code_font_family(&mut self, family: Option<&str>, cx: &mut Context<Self>) {
        self.state.settings.set_code_font_family(family);
        self.commit(cx);
    }

    pub fn set_scrollbars(&mut self, scrollbars: Scrollbars, cx: &mut Context<Self>) {
        self.state.settings.scrollbars = scrollbars;
        self.commit(cx);
    }

    /// Assigns `keystrokes` to the action named `action`, replacing its default.
    pub fn set_keybinding(&mut self, action: &str, keystrokes: &str, cx: &mut Context<Self>) {
        self.state.settings.set_keybinding(action, keystrokes);
        self.commit(cx);
    }

    /// Returns the action named `action` to its default shortcut.
    pub fn reset_keybinding(&mut self, action: &str, cx: &mut Context<Self>) {
        if self.state.settings.reset_keybinding(action) {
            self.commit(cx);
        }
    }

    pub fn add_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.state.add_project(ProjectRef::new(path, Host::Local)) {
            self.commit(cx);
        }
    }

    pub fn remove_project(&mut self, path: &Path, cx: &mut Context<Self>) {
        let before = self.state.projects.len();
        self.state.projects.retain(|p| p.path != path);
        if self.state.projects.len() != before {
            self.state.workspace.projects.retain(|p| p.project != path);
            if self
                .state
                .workspace
                .selection
                .as_ref()
                .is_some_and(|s| s.project == path)
            {
                self.state.workspace.selection = None;
            }
            self.commit(cx);
        }
    }

    pub(crate) fn move_project(
        &mut self,
        project: &ProjectRef,
        before: Option<&ProjectRef>,
        cx: &mut Context<Self>,
    ) {
        if self.state.move_project(project, before) {
            self.commit(cx);
        }
    }

    /// Switches a project between listing only Hive's worktrees and listing all of them.
    pub fn toggle_worktree_filter(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Some(project) = self.state.projects.iter_mut().find(|p| p.path == path) {
            project.worktrees = project.worktrees.toggled();
            self.commit(cx);
        }
    }

    /// Edits how a project presents itself; saves only when something changed.
    pub(crate) fn update_identity(
        &mut self,
        path: &Path,
        edit: impl FnOnce(&mut ProjectIdentity),
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.state.projects.iter_mut().find(|p| p.path == path) else {
            return;
        };
        let before = project.identity.clone();
        edit(&mut project.identity);
        if project.identity != before {
            self.commit(cx);
        }
    }

    /// Native folder picker; every chosen folder becomes a project.
    pub fn prompt_add_projects(store: Entity<Store>, cx: &mut App) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn(async move |cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                cx.update(|cx| {
                    store.update(cx, |store, cx| {
                        for path in paths {
                            store.add_project(path, cx);
                        }
                    })
                });
            }
        })
        .detach();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        if self.stopping {
            return;
        }
        if let Some(writer) = &self.writer {
            writer.submit(self.state.clone());
        }
        cx.notify();
    }
}

/// What the user is told when `state.json` could not be used as written.
fn recovery_notice(recovery: &hive_core::state::Recovery) -> String {
    use hive_core::state::RecoveryCause;
    let backup = recovery.backup.display();
    match &recovery.cause {
        RecoveryCause::Corrupt => format!(
            "State was damaged. The original was preserved at {backup}. Default settings are in use."
        ),
        RecoveryCause::Partial(lost) => format!(
            "Part of the saved state could not be read and was reset: {}. The original was preserved at {backup}.",
            lost.join(", ")
        ),
        RecoveryCause::Newer { version } => format!(
            "The saved state was written by a newer Hive (format {version}). Anything this version does not understand is kept only in the backup at {backup}."
        ),
    }
}
