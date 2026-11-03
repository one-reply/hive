//! A PTY-backed terminal emulator: `alacritty_terminal`'s `Term` plus its I/O `EventLoop`
//! on a std thread. UI-agnostic — the caller reads the grid through [`Terminal::term`]
//! and gets change notifications through [`Terminal::events`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::event::{
    Event as AlacEvent, EventListener, Notify as _, OnResize as _, WindowSize,
};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::tty;

pub use alacritty_terminal::index::{Column, Line, Point};
pub use alacritty_terminal::term::cell::{Cell, Flags};
pub use alacritty_terminal::term::{RenderableContent, TermMode, point_to_viewport};
pub use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    #[error("failed to open pty: {0}")]
    Pty(#[source] std::io::Error),
    #[error("failed to start terminal event loop: {0}")]
    EventLoop(#[source] std::io::Error),
}

/// What the UI needs to know; everything else alacritty handles internally.
#[derive(Clone, Debug)]
pub enum TerminalEvent {
    /// Grid content changed: repaint.
    Wakeup,
    /// The shell set a window title (`None` = reset to default).
    Title(Option<String>),
    /// Terminal rang the bell.
    Bell,
    /// Child process exited; the terminal is dead.
    Exit,
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
    tx: Sender<TerminalEvent>,
    /// Filled in once the event loop exists; requests before that are dropped.
    pty: Arc<std::sync::Mutex<Option<EventLoopSender>>>,
    size: Arc<std::sync::Mutex<WindowSize>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: AlacEvent) {
        let forwarded = match event {
            AlacEvent::Wakeup | AlacEvent::CursorBlinkingChange | AlacEvent::MouseCursorDirty => {
                Some(TerminalEvent::Wakeup)
            }
            AlacEvent::Title(title) => Some(TerminalEvent::Title(Some(title))),
            AlacEvent::ResetTitle => Some(TerminalEvent::Title(None)),
            AlacEvent::Bell => Some(TerminalEvent::Bell),
            AlacEvent::Exit | AlacEvent::ChildExit(_) => Some(TerminalEvent::Exit),
            AlacEvent::PtyWrite(text) => {
                self.write(text.into_bytes());
                None
            }
            AlacEvent::TextAreaSizeRequest(format) => {
                let size = *self.size.lock().unwrap();
                self.write(format(size).into_bytes());
                None
            }
            AlacEvent::ColorRequest(_, _)
            | AlacEvent::ClipboardStore(_, _)
            | AlacEvent::ClipboardLoad(_, _) => None,
        };
        if let Some(event) = forwarded {
            let _ = self.tx.send(event);
        }
    }
}

impl Listener {
    fn write(&self, bytes: Vec<u8>) {
        if let Some(pty) = self.pty.lock().unwrap().as_ref() {
            Notifier(pty.clone()).notify(bytes);
        }
    }
}

/// One shell session. Dropping it shuts the event loop down and hangs up the PTY.
pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    pty: Notifier,
    events: Option<Receiver<TerminalEvent>>,
    size: Arc<std::sync::Mutex<WindowSize>>,
    cwd: Option<PathBuf>,
    _loop: std::thread::JoinHandle<()>,
}

impl Terminal {
    /// Spawn `shell` (the user's `$SHELL` as a login shell when `None`) in `cwd`.
    pub fn spawn(
        cwd: Option<&Path>,
        shell: Option<&str>,
        size: TerminalSize,
    ) -> Result<Terminal, TerminalError> {
        let (tx, rx) = mpsc::channel();
        let window_size = size.window_size();
        let listener = Listener {
            tx,
            pty: Arc::new(std::sync::Mutex::new(None)),
            size: Arc::new(std::sync::Mutex::new(window_size)),
        };

        let term = Term::new(Config::default(), &size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        let mut env = std::collections::HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "hive".to_string());
        let options = tty::Options {
            shell: shell.map(|s| tty::Shell::new(s.to_string(), vec![])),
            working_directory: cwd.map(Path::to_path_buf),
            drain_on_exit: false,
            env,
        };
        let pty = tty::new(&options, window_size, 0).map_err(TerminalError::Pty)?;

        let event_loop = EventLoop::new(term.clone(), listener.clone(), pty, false, false)
            .map_err(TerminalError::EventLoop)?;
        let sender = event_loop.channel();
        *listener.pty.lock().unwrap() = Some(sender.clone());
        let handle = event_loop.spawn();
        let join = std::thread::Builder::new()
            .name("hive-terminal-join".into())
            .spawn(move || {
                let _ = handle.join();
            })
            .map_err(TerminalError::EventLoop)?;

        Ok(Terminal {
            term,
            pty: Notifier(sender),
            events: Some(rx),
            size: listener.size,
            cwd: cwd.map(Path::to_path_buf),
            _loop: join,
        })
    }

    /// The shared grid. Lock briefly; the reader thread needs it too.
    pub fn term(&self) -> &Arc<FairMutex<Term<Listener>>> {
        &self.term
    }

    /// Take the event receiver. Only the first call returns `Some`.
    pub fn events(&mut self) -> Option<Receiver<TerminalEvent>> {
        self.events.take()
    }

    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Send bytes to the shell's stdin.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        self.pty.notify(bytes.into());
    }

    pub fn resize(&mut self, size: TerminalSize) {
        let window_size = size.window_size();
        {
            let current = *self.size.lock().unwrap();
            if current.num_cols == window_size.num_cols
                && current.num_lines == window_size.num_lines
            {
                return;
            }
        }
        *self.size.lock().unwrap() = window_size;
        self.term.lock().resize(size);
        self.pty.on_resize(window_size);
    }

    pub fn size(&self) -> TerminalSize {
        let s = *self.size.lock().unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn echo_round_trip() {
        let mut term =
            Terminal::spawn(None, Some("/bin/sh"), TerminalSize::new(40, 5)).expect("spawn");
        let rx = term.events().unwrap();
        term.write(b"printf 'hive-ok\\n'\n".to_vec());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = {
                let t = term.term().lock();
                let content = t.renderable_content();
                content.display_iter.map(|c| c.c).collect::<String>()
            };
            if text.contains("hive-ok") {
                break;
            }
            assert!(Instant::now() < deadline, "shell never echoed: {text:?}");
            let _ = rx.recv_timeout(Duration::from_millis(200));
        }
    }
}
