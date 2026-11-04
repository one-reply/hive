//! A terminal's asynchronous startup, input and retained event subscription.
use gpui_kit::*;
use hive_core::terminal::{Terminal, TerminalSize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use crate::app::AppState;

mod input;
use input::Input;

pub(crate) enum SessionStatus {
    Starting,
    Ready(Terminal),
    Failed(String),
}

pub(crate) enum SessionEvent {
    TitleChanged,
    Exited,
    PasteFailed(String),
}

pub(crate) struct TerminalSession {
    pub(crate) status: SessionStatus,
    pub(crate) title: Option<String>,
    pub(crate) exited: bool,
    focused: bool,
    _start: Task<()>,
    _events: Option<Task<()>>,
    pending_input: VecDeque<Input>,
    paste_task: Option<Task<()>>,
}

impl EventEmitter<SessionEvent> for TerminalSession {}

impl TerminalSession {
    pub(crate) fn new(cwd: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let starting = AppState::get(cx).terminals.begin();
        let start = cx.background_executor().spawn(async move {
            let Some(starting) = starting else {
                return Err("Hive is shutting down".to_owned());
            };
            let result = Terminal::spawn(cwd.as_deref(), None, TerminalSize::new(80, 24));
            if let Ok(terminal) = &result {
                starting.register(terminal);
            }
            result.map_err(|error| {
                tracing::error!(?cwd, %error, "failed to start terminal");
                error.to_string()
            })
        });
        let start = cx.spawn(async move |this, cx| {
            let result = start.await;
            // If the view closed while spawning, dropping the result closes the late PTY.
            let _ = this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(mut terminal) => {
                        terminal.set_focused(this.focused);
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
                    Err(error) => SessionStatus::Failed(error),
                };
                cx.notify();
            });
        });
        Self {
            status: SessionStatus::Starting,
            title: None,
            exited: false,
            focused: false,
            _start: start,
            _events: None,
            pending_input: VecDeque::new(),
            paste_task: None,
        }
    }

    pub(crate) fn terminal(&self) -> Option<&Terminal> {
        match &self.status {
            SessionStatus::Ready(terminal) => Some(terminal),
            _ => None,
        }
    }

    pub(crate) fn set_focused(&mut self, focused: bool) {
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
}
