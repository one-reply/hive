//! The terminal entity: owns the PTY session, pumps its events into `cx.notify()`,
//! turns keyboard/scroll input into PTY writes.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use hive_core::terminal::{TermMode, Terminal, TerminalEvent, TerminalSize};

use super::element::TerminalElement;
use super::keys;

pub enum TerminalViewEvent {
    TitleChanged,
    Exited,
}

pub struct TerminalView {
    terminal: Terminal,
    focus_handle: FocusHandle,
    cwd: Option<PathBuf>,
    title: Option<String>,
    exited: bool,
    /// Fractional scroll-wheel lines not yet applied.
    scroll_remainder: f32,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl TerminalView {
    /// Wrap an already spawned [`Terminal`] (spawning can fail; entities can't).
    pub fn new(
        mut terminal: Terminal,
        cwd: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _, cx| {
                this.terminal.set_focused(true);
                cx.notify();
            }),
            cx.on_blur(&focus_handle, window, |this, _, cx| {
                this.terminal.set_focused(false);
                cx.notify();
            }),
        ];
        if let Some(events) = terminal.events() {
            Self::pump_events(events, cx);
        }
        Self {
            terminal,
            focus_handle,
            cwd: cwd.map(Path::to_path_buf),
            title: None,
            exited: false,
            scroll_remainder: 0.,
            _subscriptions: subscriptions,
        }
    }

    /// Block on the channel in the background; coalesce bursts of wakeups into one notify.
    fn pump_events(events: Receiver<TerminalEvent>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut events = Some(events);
            loop {
                let rx = events.take().expect("receiver is always put back");
                let (rx, batch) = cx
                    .background_executor()
                    .spawn(async move {
                        let mut batch = Vec::new();
                        match rx.recv() {
                            Ok(event) => batch.push(event),
                            Err(_) => return (rx, batch),
                        }
                        // Let the shell finish its burst of output before we repaint.
                        std::thread::sleep(Duration::from_millis(4));
                        while let Ok(event) = rx.try_recv() {
                            batch.push(event);
                        }
                        (rx, batch)
                    })
                    .await;
                events = Some(rx);
                if batch.is_empty() {
                    break; // sender gone: the event loop thread ended
                }
                let alive = this.update(cx, |this, cx| {
                    for event in batch {
                        this.handle_event(event, cx);
                    }
                    cx.notify();
                    !this.exited
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn handle_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::Wakeup => {}
            TerminalEvent::Title(title) => {
                self.title = title;
                cx.emit(TerminalViewEvent::TitleChanged);
            }
            TerminalEvent::Bell => {}
            TerminalEvent::Exit => {
                self.exited = true;
                cx.emit(TerminalViewEvent::Exited);
            }
        }
    }

    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    /// Shell-set title (without a `user@host:` prefix, capped so tabs stay narrow), else the
    /// cwd's last component, else "Terminal".
    pub fn title(&self) -> String {
        if let Some(title) = self
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            return shorten_title(title, 28);
        }
        self.cwd
            .as_deref()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Terminal".into())
    }

    /// Called by the element once it knows how many cells fit.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.terminal.resize(TerminalSize::new(cols, rows));
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.exited {
            return;
        }
        let mode = self.terminal.mode();
        if let Some(bytes) = keys::to_bytes(&event.keystroke, mode) {
            self.terminal.scroll_to_bottom();
            self.terminal.write(bytes);
            cx.stop_propagation();
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let mode = self.terminal.mode();
        let mut bytes = Vec::with_capacity(text.len() + 12);
        if mode.contains(TermMode::BRACKETED_PASTE) {
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            bytes.extend_from_slice(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        }
        self.terminal.scroll_to_bottom();
        self.terminal.write(bytes);
    }

    fn scroll_wheel(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let line_height = window.line_height();
        let delta = event.delta.pixel_delta(line_height).y / line_height;
        self.scroll_remainder += delta;
        let lines = self.scroll_remainder.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;
        let mode = self.terminal.mode();
        if mode.contains(TermMode::ALT_SCREEN) {
            // Alternate screen (less, vim…): translate the wheel into arrow keys.
            if mode.contains(TermMode::ALTERNATE_SCROLL) {
                let key: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
                let bytes = key.repeat(lines.unsigned_abs() as usize);
                self.terminal.write(bytes);
            }
        } else {
            self.terminal.scroll(lines);
        }
        cx.notify();
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let bg = cx.theme().background;
        div()
            .id("terminal")
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .size_full()
            .bg(bg)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
            .on_scroll(cx.listener(Self::scroll_wheel))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus_handle.focus(window, cx)),
            )
            .child(TerminalElement::new(cx.entity(), focused))
    }
}

/// `user@host:~/dir` → `~/dir`; long titles keep their tail, where the path is.
fn shorten_title(title: &str, max_chars: usize) -> String {
    let title = match title.split_once(':') {
        Some((prefix, rest)) if prefix.contains('@') && !prefix.contains(' ') => rest.trim(),
        _ => title,
    };
    let count = title.chars().count();
    if count <= max_chars {
        return title.to_string();
    }
    let tail: String = title.chars().skip(count - (max_chars - 1)).collect();
    format!("…{tail}")
}

actions!(terminal, [Paste]);

/// Keymap entries the terminal needs. Call once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-v", Paste, Some("Terminal"))]);
}

#[cfg(test)]
mod tests {
    use super::shorten_title;

    #[test]
    fn shortens_shell_titles() {
        assert_eq!(
            shorten_title("vitalii@MacBook-Pro:~/projects/hive", 28),
            "~/projects/hive"
        );
        assert_eq!(
            shorten_title("vim src/terminal/element.rs", 28),
            "vim src/terminal/element.rs"
        );
        assert_eq!(
            shorten_title("a/very/long/path/that/keeps/going/and/going", 12),
            "…g/and/going"
        );
    }
}
