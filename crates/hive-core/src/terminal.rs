//! A PTY-backed terminal emulator: `alacritty_terminal`'s `Term` plus its I/O `EventLoop`
//! on a std thread. UI-agnostic — the caller reads the grid through [`Terminal::term`]
//! and gets change notifications through [`Terminal::events`].

use async_channel::Receiver;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Instant;

mod activity;
mod directory;
mod events;
pub mod paste;
mod process;
mod pty;
mod scroll;
mod selection;
pub use activity::Activity;
pub use directory::working_directory;
use events::Publisher;
pub use events::TerminalUpdate;
use pty::{Completion, ManagedPty};
pub use scroll::ScrollModifiers;

use alacritty_terminal::event::{
    Event as AlacEvent, EventListener, Notify as _, OnResize as _, WindowSize,
};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};

pub use alacritty_terminal::index::{Column, Line, Point, Side};
pub use alacritty_terminal::selection::SelectionType;
pub use alacritty_terminal::term::cell::{Cell, Flags};
pub use alacritty_terminal::term::{RenderableContent, TermMode, point_to_viewport};
pub use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// The live shell's directory could not be read.
    #[error("could not read working directory of shell {pid}: {source}")]
    Directory {
        /// Shell process id.
        pid: u32,
        /// Native process query failure.
        #[source]
        source: std::io::Error,
    },
    #[error("failed to open pty: {0}")]
    Pty(#[source] std::io::Error),
    #[error("failed to start terminal event loop: {0}")]
    EventLoop(#[source] std::io::Error),
}

/// Grid dimensions in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl TerminalSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: cols.max(2),
            rows: rows.max(1),
            cell_width: 8,
            cell_height: 16,
        }
    }

    fn window_size(self) -> WindowSize {
        WindowSize {
            num_cols: self.cols,
            num_lines: self.rows,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

impl Dimensions for TerminalSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

/// Alacritty's listener: forwards the events the UI cares about over a channel, and answers
/// the ones that only need a PTY write (clipboard/color/size requests) right here.
#[derive(Clone)]
pub struct Listener {
    events: Publisher,
    activity: Activity,
    /// Filled in once the event loop exists; requests before that are dropped.
    pty: Arc<std::sync::Mutex<Option<EventLoopSender>>>,
    size: Arc<std::sync::Mutex<WindowSize>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: AlacEvent) {
        match event {
            AlacEvent::PtyWrite(text) => {
                self.write(text.into_bytes());
            }
            AlacEvent::TextAreaSizeRequest(format) => {
                let size = *self.size.lock().unwrap_or_else(|e| e.into_inner());
                self.write(format(size).into_bytes());
            }
            event => {
                // The event loop wakes the UI after every PTY read.
                if matches!(event, AlacEvent::Wakeup) {
                    self.activity.record_output();
                }
                self.events.send(event)
            }
        }
    }
}

impl Listener {
    fn write(&self, bytes: Vec<u8>) {
        if let Some(pty) = self.pty.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            Notifier(pty.clone()).notify(bytes);
        }
    }
}

/// One shell session. Dropping it shuts the event loop down and hangs up the PTY.
pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: Arc<Notifier>,
    events: Option<Receiver<TerminalUpdate>>,
    size: Arc<std::sync::Mutex<WindowSize>>,
    cwd: Option<PathBuf>,
    shell_pid: u32,
    activity: Activity,
    control: TerminalControl,
}

impl Terminal {
    /// Spawn `shell` (the user's `$SHELL` as a login shell when `None`) in `cwd`.
    pub fn spawn(
        cwd: Option<&Path>,
        shell: Option<&str>,
        size: TerminalSize,
    ) -> Result<Terminal, TerminalError> {
        let command = process::command(cwd, shell).map_err(TerminalError::Pty)?;
        Self::spawn_command(command, cwd, size)
    }

    /// Spawn a prepared Host command as the foreground process of a fresh PTY.
    pub fn spawn_command(
        command: std::process::Command,
        cwd: Option<&Path>,
        size: TerminalSize,
    ) -> Result<Terminal, TerminalError> {
        let (events, rx) = Publisher::new();
        let completion = Arc::new(Completion::default());
        let window_size = size.window_size();
        let activity = Activity::default();
        let listener = Listener {
            events: events.clone(),
            activity: activity.clone(),
            pty: Arc::new(std::sync::Mutex::new(None)),
            size: Arc::new(std::sync::Mutex::new(window_size)),
        };

        let term = Term::new(Config::default(), &size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        let (child, file) = process::spawn(command, window_size).map_err(TerminalError::Pty)?;
        let shell_pid = child.id();
        let pty = ManagedPty::new(child, file, completion.clone(), events);
        let event_loop = EventLoop::new(term.clone(), listener.clone(), pty, false, false)
            .map_err(TerminalError::EventLoop)?;
        let sender = event_loop.channel();
        *listener.pty.lock().unwrap_or_else(|e| e.into_inner()) = Some(sender.clone());
        drop(event_loop.spawn());
        let pty = Arc::new(Notifier(sender));
        let control = TerminalControl {
            sender: Arc::downgrade(&pty),
            completion,
        };

        Ok(Terminal {
            term,
            pty,
            events: Some(rx),
            size: listener.size,
            cwd: cwd.map(Path::to_path_buf),
            shell_pid,
            activity,
            control,
        })
    }

    /// The shared grid. Lock briefly; the reader thread needs it too.
    pub fn term(&self) -> &Arc<FairMutex<Term<Listener>>> {
        &self.term
    }

    /// Take the event receiver. Only the first call returns `Some`.
    pub fn events(&mut self) -> Option<Receiver<TerminalUpdate>> {
        self.events.take()
    }

    /// Shutdown handle that does not keep the terminal grid alive.
    pub fn control(&self) -> TerminalControl {
        self.control.clone()
    }

    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Process id of the shell the PTY runs; its terminal's foreground job is what runs now.
    pub fn shell_pid(&self) -> u32 {
        self.shell_pid
    }

    /// Output/input timing, for telling a busy program from an idle one.
    pub fn activity(&self) -> Activity {
        self.activity.clone()
    }

    /// Send bytes to the shell's stdin.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        self.activity.record_input();
        self.pty.notify(bytes.into());
    }

    pub fn resize(&mut self, size: TerminalSize) {
        let window_size = size.window_size();
        {
            let current = *self.size.lock().unwrap_or_else(|e| e.into_inner());
            if current.num_cols == window_size.num_cols
                && current.num_lines == window_size.num_lines
            {
                return;
            }
        }
        *self.size.lock().unwrap_or_else(|e| e.into_inner()) = window_size;
        // The program redraws to the new size; that output is not work.
        self.activity.record_input();
        self.term.lock().resize(size);
        Notifier(self.pty.0.clone()).on_resize(window_size);
    }

    pub fn size(&self) -> TerminalSize {
        let s = *self.size.lock().unwrap_or_else(|e| e.into_inner());
        TerminalSize {
            cols: s.num_cols,
            rows: s.num_lines,
            cell_width: s.cell_width,
            cell_height: s.cell_height,
        }
    }

    /// Scroll the viewport through the scrollback by `lines` (positive = up / older).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    pub fn set_focused(&self, focused: bool) {
        self.term.lock().is_focused = focused;
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.pty.0.send(Msg::Shutdown);
    }
}

/// Shutdown control independent of views and foreground task delivery.
#[derive(Clone)]
pub struct TerminalControl {
    sender: Weak<Notifier>,
    completion: Arc<Completion>,
}
impl TerminalControl {
    /// Request asynchronous teardown of this terminal's session.
    pub fn shutdown(&self) {
        if let Some(sender) = self.sender.upgrade() {
            let _ = sender.0.send(Msg::Shutdown);
        }
    }
    /// Whether the PTY and its child have been cleaned up.
    pub fn is_closed(&self) -> bool {
        self.completion.is_closed()
    }
    /// Blocking wait with an absolute deadline, used only during application shutdown or tests.
    pub fn wait_until(&self, deadline: Instant) -> bool {
        self.completion.wait_until(deadline)
    }
}

#[cfg(test)]
mod tests;
