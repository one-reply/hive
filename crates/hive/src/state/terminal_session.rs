//! A terminal's asynchronous startup, input and retained event subscription.
use gpui_kit::*;
use hive_core::host::Host;
use hive_core::terminal::{Point, ScrollModifiers, SelectionType, Side, Terminal, TerminalSize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use crate::app::AppState;
use hive_core::agent::{
    api, hooks,
    launch::{Launch, ResumeSession},
};

mod draft;
mod inbox;
mod input;
pub(crate) use draft::{AgentDraft, DraftStatus};
pub(crate) use inbox::{Message, Recipient};
use input::Input;

pub(crate) enum SessionStatus {
    Starting,
    Ready(Terminal),
    Failed(String),
}

pub(crate) enum SessionEvent {
    TitleChanged,
    DirectoryChanged,
    Exited,
    PasteFailed(String),
}

pub(crate) struct TerminalSession {
    pub(crate) host: Host,
    pub(crate) status: SessionStatus,
    pub(crate) title: Option<String>,
    pub(crate) exited: bool,
    pub(crate) directory: Option<PathBuf>,
    worktree: Option<PathBuf>,
    /// The conversation this terminal's agent is running, kept so a restart can resume it.
    pub(crate) resume: Option<ResumeSession>,
    focused: bool,
    pub(crate) last_focused: Option<std::time::Instant>,
    pub(crate) launched_agent: Option<hive_core::agent::AgentKind>,
    pub(crate) agent_draft: Option<AgentDraft>,
    draft_task: Option<Task<()>>,
    /// The name `hive agent start` gave this pane; persisted with it.
    pub(crate) name: Option<String>,
    /// Messages from other agents, delivered one at a time.
    inbox: VecDeque<Message>,
    inbox_task: Option<Task<()>>,
    _start: Task<()>,
    _events: Option<Task<()>>,
    _directory: Option<Task<()>>,
    pending_input: VecDeque<Input>,
    paste_task: Option<Task<()>>,
}

impl EventEmitter<SessionEvent> for TerminalSession {}

impl TerminalSession {
    pub(crate) fn new(
        host: Host,
        cwd: Option<PathBuf>,
        worktree: Option<PathBuf>,
        launch: Option<Launch>,
        connect: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let starting = AppState::get(cx).terminals.begin();
        let directory = cwd.clone();
        let endpoint = AppState::get(cx).agents.read(cx).endpoint();
        let terminal_id = cx.entity_id().as_u64();
        let launch = if host.is_local() {
            launch.map(Launch::with_known_session)
        } else {
            launch
        };
        let launched_agent = launch.as_ref().map(Launch::agent);
        let resume = launch.as_ref().and_then(Launch::session).cloned();
        let remote_host = host.clone();
        let start = cx.background_executor().spawn(async move {
            if !connect {
                return Err(
                    "SSH session is disconnected. Open a new SSH session to continue.".to_owned(),
                );
            }
            let Some(starting) = starting else {
                return Err("Hive is shutting down".to_owned());
            };
            let result = if !remote_host.is_local() {
                let directory = cwd.as_deref().ok_or("Choose a remote directory")?;
                let command = match launch {
                    Some(launch) => launch
                        .remote_command(&remote_host, directory)
                        .map_err(|e| e.to_string())?,
                    None => remote_host
                        .terminal_command(directory, None)
                        .map_err(|e| e.to_string())?,
                };
                Terminal::spawn_command(command, cwd.as_deref(), TerminalSize::new(80, 24))
            } else {
                let executable = std::env::current_exe()
                    .inspect_err(
                        |error| tracing::warn!(%error, "hooks and hive agent need the app path"),
                    )
                    .ok();
                let mut command = match launch {
                    Some(launch) => {
                        let settings = endpoint.as_ref().zip(executable.as_ref()).map(
                            |(endpoint, executable)| {
                                hooks::settings(executable, &endpoint.hooks, terminal_id)
                            },
                        );
                        launch
                            .command(
                                cwd.as_deref()
                                    .ok_or("Choose a worktree before starting an agent")?,
                                settings.as_deref(),
                            )
                            .map_err(|error| error.to_string())?
                    }
                    None => Terminal::shell_command(cwd.as_deref())
                        .map_err(|error| error.to_string())?,
                };
                api::clear_environment(&mut command);
                if let (Some(endpoint), Some(executable)) = (&endpoint, &executable)
                    && let Some(socket) = &endpoint.api
                {
                    api::environment(
                        &mut command,
                        socket,
                        terminal_id,
                        endpoint.launcher.as_deref(),
                        executable,
                    );
                }
                Terminal::spawn_command(command, cwd.as_deref(), TerminalSize::new(80, 24))
            };
            if let Ok(terminal) = &result {
                starting.register(terminal);
            }
            result.map_err(|error| error.to_string())
        });
        let registered_worktree = worktree.clone();
        let start = cx.spawn(async move |this, cx| {
            let result = start.await;
            // If the view closed while spawning, dropping the result closes the late PTY.
            let _ = this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(mut terminal) => {
                        if this.host.is_local() { this.watch_directory(terminal.shell_pid(), cx); }
                        terminal.set_focused(this.focused);
                        if let Some(worktree) = &registered_worktree {
                            let session = cx.entity();
                            AppState::get(cx).agents.clone().update(cx, |agents, _| {
                                agents.watch(&session, &this.host, this.launched_agent, worktree, &terminal)
                            });
                        }
                        if let Some(events) = terminal.events() {
                            this._events = Some(cx.spawn(async move |this, cx| {
                                while let Ok(mut update) = events.recv().await {
                                    cx.background_executor()
                                        .timer(Duration::from_millis(16))
                                        .await;
                                    if let Ok(latest) = events.try_recv() {
                                        update = latest;
                                    }
                                    let alive = this.update(cx, |this, cx| {
                                        if this.title.as_deref() != update.title() {
                                            this.title = update.title().map(str::to_owned);
                                            cx.emit(SessionEvent::TitleChanged);
                                        }
                                        if update.exited() && !this.exited {
                                            this.exited = true;
                                            this.pending_input.clear();
                                            this.paste_task = None;
                                            this.draft_task = None;
                                            this.fail_inbox(cx);
                                            cx.emit(SessionEvent::Exited);
                                        }
                                        cx.notify();
                                        !this.exited
                                    });
                                    if !matches!(alive, Ok(true)) {
                                        break;
                                    }
                                }
                            }));
                        }
                        SessionStatus::Ready(terminal)
                    }
                    Err(error) => {
                        tracing::error!(directory = ?this.directory, %error, "failed to start terminal");
                        SessionStatus::Failed(error)
                    },
                };
                cx.notify();
            });
        });
        Self {
            host,
            status: SessionStatus::Starting,
            title: None,
            exited: !connect,
            directory,
            worktree,
            resume,
            focused: false,
            last_focused: None,
            launched_agent,
            agent_draft: None,
            draft_task: None,
            name: None,
            inbox: VecDeque::new(),
            inbox_task: None,
            _start: start,
            _events: None,
            _directory: None,
            pending_input: VecDeque::new(),
            paste_task: None,
        }
    }

    /// Records the conversation the agent runs now, or that none runs; persisted with the tile.
    pub(crate) fn set_resume(&mut self, resume: Option<ResumeSession>, cx: &mut Context<Self>) {
        if self.resume != resume {
            self.resume = resume;
            cx.emit(SessionEvent::DirectoryChanged);
        }
    }

    /// Names the pane for `hive agent`; persisted with the tile like its conversation.
    pub(crate) fn set_name(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        if self.name != name {
            self.name = name;
            cx.emit(SessionEvent::DirectoryChanged);
            cx.notify();
        }
    }

    fn watch_directory(&mut self, pid: u32, cx: &mut Context<Self>) {
        // shortcut: sampled each second; use shell directory events if immediate cd-and-quit must be exact.
        self._directory = Some(cx.spawn(async move |this, cx| {
            let mut reported = false;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if !matches!(this.update(cx, |this, _| !this.exited), Ok(true)) {
                    break;
                }
                let directory = cx
                    .background_executor()
                    .spawn(async move { hive_core::terminal::working_directory(pid) })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.exited {
                            return;
                        }
                        match directory {
                            Ok(directory) => {
                                reported = false;
                                if this.directory.as_ref() != Some(&directory) {
                                    this.directory = Some(directory);
                                    cx.emit(SessionEvent::DirectoryChanged);
                                }
                            }
                            Err(error) if !reported => {
                                tracing::warn!(%error, "keeping last known terminal directory");
                                reported = true;
                            }
                            Err(_) => {}
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    pub(crate) fn terminal(&self) -> Option<&Terminal> {
        match &self.status {
            SessionStatus::Ready(terminal) => Some(terminal),
            _ => None,
        }
    }

    pub(crate) fn set_focused(&mut self, focused: bool) {
        if focused && !self.focused {
            self.last_focused = Some(std::time::Instant::now());
        }
        self.focused = focused;
        if let Some(terminal) = self.terminal() {
            terminal.set_focused(focused);
        }
    }

    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        if let SessionStatus::Ready(terminal) = &mut self.status {
            terminal.resize(TerminalSize::new(cols, rows));
        }
    }

    pub(crate) fn scroll_wheel(
        &mut self,
        lines: i32,
        point: Point<usize>,
        modifiers: ScrollModifiers,
        cx: &mut Context<Self>,
    ) {
        if let Some(bytes) = self
            .terminal()
            .and_then(|terminal| terminal.scroll_wheel(lines, point, modifiers))
        {
            self.write_input(bytes, cx);
        }
    }

    pub(crate) fn start_selection(
        &mut self,
        point: Point<usize>,
        side: Side,
        kind: SelectionType,
        cx: &mut Context<Self>,
    ) {
        if let Some(terminal) = self.terminal() {
            terminal.start_selection(point, side, kind);
            cx.notify();
        }
    }

    pub(crate) fn update_selection(
        &mut self,
        point: Point<usize>,
        side: Side,
        cx: &mut Context<Self>,
    ) {
        if let Some(terminal) = self.terminal() {
            terminal.update_selection(point, side);
            cx.notify();
        }
    }

    pub(crate) fn copy(&self, cx: &mut Context<Self>) {
        if let Some(text) = self.terminal().and_then(Terminal::selected_text) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
}
