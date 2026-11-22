use gpui_kit::*;
use hive_core::host::Host;
use hive_core::identity::ProjectIdentity;
use hive_core::settings::{DarkTheme, LightTheme, Scrollbars, ThemeChoice};
use hive_core::state::workspace::{ProjectWorkspace, Selection};
use hive_core::state::{Access, LoadedState, ProjectRef, State, StateError};
use std::path::{Path, PathBuf};
use std::time::Instant;

mod recovery;
mod startup;
mod writer;
use startup::{Opened, open};
use writer::Writer;

/// Where the Hive that holds the data folder keeps its editor drafts.
fn recovery_directory() -> PathBuf {
    hive_core::state::hive_dir().join("editor-recovery")
}

/// Blocking: the editor drafts this session restores, with the records that could not be
/// read. A folder another Hive holds gives none: those drafts are that Hive's.
pub(crate) fn load_drafts(
    access: &Access,
) -> (Vec<hive_core::files::recovery::Record>, Vec<String>) {
    if startup::keeps_drafts(access) {
        hive_core::files::recovery::load(&recovery_directory())
    } else {
        Default::default()
    }
}

/// Persisted state. Every mutation saves to disk and notifies observers.
pub struct Store {
    pub state: State,
    pub(crate) notice: Option<String>,
    writer: Option<Writer>,
    /// The data folder lock, kept for as long as `writer` may save.
    _access: Option<Access>,
    recovery: recovery::RecoveryWriter,
    _recovery_errors: Task<()>,
    stopping: bool,
    _errors: Option<Task<()>>,
}

pub(super) type Checkpoint = async_channel::Receiver<Result<(), String>>;

pub(crate) struct SaveError(pub(crate) String);
impl EventEmitter<SaveError> for Store {}

impl Store {
    #[cfg(test)]
    pub(crate) fn with_held_recovery(cx: &mut Context<Self>) -> Self {
        Self {
            state: State::default(),
            notice: None,
            writer: None,
            _access: None,
            recovery: recovery::RecoveryWriter::held(cx.background_executor()),
            _recovery_errors: Task::ready(()),
            stopping: false,
            _errors: None,
        }
    }

    #[cfg(test)]
    pub(super) fn set_test_writer(
        &mut self,
        cx: &Context<Self>,
        save: impl Fn(&State) -> Result<(), StateError> + Send + 'static,
    ) {
        self.writer = Some(Writer::with_save(cx.background_executor(), save).0);
    }

    #[cfg(test)]
    pub(crate) fn complete_held_recovery(&self) {
        self.recovery.complete_held();
    }

    /// Apply one pipeline mutation to the same snapshot as the rest of the workspace.
    pub(crate) fn edit_pipelines<T>(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut hive_core::pipeline::Library) -> T,
    ) -> T {
        let result = edit(&mut self.state.pipelines);
        self.commit(cx);
        result
    }

    /// [`Self::edit_pipelines`] for an edit that can be rejected. A rejected edit leaves the
    /// library as it was, so nothing is saved or announced.
    pub(crate) fn try_edit_pipelines<T, E>(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut hive_core::pipeline::Library) -> Result<T, E>,
    ) -> Result<T, E> {
        let result = edit(&mut self.state.pipelines)?;
        self.commit(cx);
        Ok(result)
    }

    /// False after a failed load (saving would overwrite the original file) or during shutdown.
    pub(crate) fn can_save(&self) -> bool {
        self.writer.is_some() && !self.stopping
    }

    /// Submit the mutation and its acknowledgment together so a transition clones State once.
    pub(super) fn checkpoint_pipelines(
        &mut self,
        cx: &mut Context<Self>,
        edit: impl FnOnce(&mut hive_core::pipeline::Library),
    ) -> Checkpoint {
        edit(&mut self.state.pipelines);
        cx.notify();
        self.pipeline_checkpoint()
    }

    /// Wait asynchronously for this pipeline state (or a later snapshot) to reach disk.
    pub(crate) fn pipeline_checkpoint(&self) -> Checkpoint {
        if !self.stopping
            && let Some(writer) = &self.writer
        {
            return writer.checkpoint(self.state.clone());
        }
        let (send, recv) = async_channel::bounded(1);
        let _ = send.try_send(Err("Workspace state cannot be saved".into()));
        recv
    }

    pub(crate) fn set_reviewed(
        &mut self,
        revision: hive_core::git::reviewed::Revision,
        reviewed: bool,
        cx: &mut Context<Self>,
    ) {
        if hive_core::git::reviewed::set(&mut self.state.reviewed, revision, reviewed) {
            self.commit(cx);
        }
    }
    pub(crate) fn set_hosting_server(
        &mut self,
        provider: hive_core::git::hosting::Provider,
        server: hive_core::git::hosting::Server,
        cx: &mut Context<Self>,
    ) {
        if self.state.settings.hosting_servers.set(provider, server) {
            self.commit(cx);
        }
    }

    pub(crate) fn set_project_expanded(
        &mut self,
        host: &Host,
        path: &Path,
        expanded: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.set_project_expanded(host, path, expanded) {
            self.commit(cx);
        }
    }

    pub(crate) fn take_release_notes(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.can_save() {
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

    pub(crate) fn set_option_as_meta(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.state.settings.option_as_meta = enabled;
        self.commit(cx);
    }

    pub(crate) fn set_agent_notifications(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.state.settings.agent_notifications = enabled;
        self.commit(cx);
    }

    pub(crate) fn set_keep_awake_while_agents_work(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.state.settings.keep_awake_while_agents_work = enabled;
        self.commit(cx);
    }

    pub(crate) fn set_files_selected(&mut self, files: bool, cx: &mut Context<Self>) {
        self.state.settings.files_selected = files;
        self.commit(cx);
    }
    /// `access` is this process's right to write the data folder, taken before `loaded` was read.
    pub(crate) fn new(
        loaded: Result<LoadedState, StateError>,
        access: Access,
        cx: &mut Context<Self>,
    ) -> Self {
        let Opened {
            state,
            notice,
            access,
            keeps_drafts,
        } = open(loaded, access);
        let drafts = keeps_drafts.then(recovery_directory);
        let (recovery, recovery_errors) =
            recovery::RecoveryWriter::new(cx.background_executor(), drafts);
        let recovery_errors = cx.spawn(async move |this, cx| {
            while let Ok(error) = recovery_errors.recv().await {
                if this.update(cx, |_, cx| cx.emit(SaveError(error))).is_err() {
                    break;
                }
            }
        });
        let Some(access) = access else {
            return Self {
                state,
                notice,
                writer: None,
                _access: None,
                recovery,
                _recovery_errors: recovery_errors,
                stopping: false,
                _errors: None,
            };
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
            _access: Some(access),
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
        if self.state.settings.theme != theme {
            self.state.settings.theme = theme;
            self.commit(cx);
        }
    }

    pub(crate) fn set_light_theme(&mut self, theme: LightTheme, cx: &mut Context<Self>) {
        if self.state.settings.light_theme() != theme {
            self.state.settings.set_light_theme(theme);
            self.commit(cx);
        }
    }

    pub(crate) fn set_dark_theme(&mut self, theme: DarkTheme, cx: &mut Context<Self>) {
        if self.state.settings.dark_theme() != theme {
            self.state.settings.set_dark_theme(theme);
            self.commit(cx);
        }
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

    pub fn add_project(&mut self, host: Host, path: PathBuf, cx: &mut Context<Self>) {
        if self.state.add_project(ProjectRef::new(path, host)) {
            self.commit(cx);
        }
    }

    pub fn remove_project(&mut self, host: &Host, path: &Path, cx: &mut Context<Self>) {
        if self.state.remove_project(host, path) {
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
    pub fn toggle_worktree_filter(&mut self, host: &Host, path: &Path, cx: &mut Context<Self>) {
        if let Some(project) = self
            .state
            .projects
            .iter_mut()
            .find(|p| p.host == *host && p.path == path)
        {
            project.worktrees = project.worktrees.toggled();
            self.commit(cx);
        }
    }

    /// Remembers the worktree setup command a local project last ran, to offer it again.
    pub(crate) fn set_project_setup(&mut self, path: &Path, setup: &str, cx: &mut Context<Self>) {
        let Some(project) = self
            .state
            .projects
            .iter_mut()
            .find(|p| p.host.is_local() && p.path == path)
        else {
            return;
        };
        if project.setup != setup {
            project.setup = setup.to_owned();
            self.commit(cx);
        }
    }

    /// Edits how a project presents itself; saves only when something changed.
    pub(crate) fn update_identity(
        &mut self,
        host: &Host,
        path: &Path,
        edit: impl FnOnce(&mut ProjectIdentity),
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self
            .state
            .projects
            .iter_mut()
            .find(|p| p.host == *host && p.path == path)
        else {
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
                            store.add_project(Host::Local, path, cx);
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
