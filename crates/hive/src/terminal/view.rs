//! Terminal presentation and keyboard/scroll dispatch to its session entity.

use std::path::{Path, PathBuf};

use crate::state::terminal_session::{SessionEvent, SessionStatus, TerminalSession};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;
use hive_core::terminal::TermMode;

use super::element::TerminalElement;
use super::keys;
use crate::ui::keymap;

pub enum TerminalViewEvent {
    TitleChanged,
    Exited,
}

pub struct TerminalView {
    session: Entity<TerminalSession>,
    focus_handle: FocusHandle,
    cwd: Option<PathBuf>,
    /// Fractional scroll-wheel lines not yet applied.
    scroll_remainder: f32,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl TerminalView {
    pub fn new(cwd: Option<&Path>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let session = cx.new(|cx| TerminalSession::new(cwd.map(Path::to_path_buf), cx));
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _, cx| {
                this.session
                    .update(cx, |session, _| session.set_focused(true));
                cx.notify();
            }),
            cx.on_blur(&focus_handle, window, |this, _, cx| {
                this.session
                    .update(cx, |session, _| session.set_focused(false));
                cx.notify();
            }),
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.subscribe(&session, |_, _, event, cx| {
                cx.emit(match event {
                    SessionEvent::TitleChanged => TerminalViewEvent::TitleChanged,
                    SessionEvent::Exited => TerminalViewEvent::Exited,
                });
            }),
        ];
        Self {
            session,
            focus_handle,
            cwd: cwd.map(Path::to_path_buf),
            scroll_remainder: 0.,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn session(&self) -> &Entity<TerminalSession> {
        &self.session
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn cwd(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    pub fn exited(&self, cx: &App) -> bool {
        self.session.read(cx).exited
    }

    /// Shell-set title (without a `user@host:` prefix, capped so tabs stay narrow), else the
    /// cwd's last component, else "Terminal".
    pub fn title(&self, cx: &App) -> String {
        if let Some(title) = self
            .session
            .read(cx)
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
    pub fn resize(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, _| session.resize(cols, rows));
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let handled = self.session.update(cx, |session, _| {
            let Some(terminal) = session.terminal().filter(|_| !session.exited) else {
                return false;
            };
            if let Some(bytes) = keys::to_bytes(&event.keystroke, terminal.mode()) {
                terminal.scroll_to_bottom();
                terminal.write(bytes);
                return true;
            }
            false
        });
        if handled {
            cx.stop_propagation();
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.session.update(cx, |session, _| {
            let Some(terminal) = session.terminal().filter(|_| !session.exited) else {
                return;
            };
            let mut bytes = Vec::with_capacity(text.len() + 12);
            if terminal.mode().contains(TermMode::BRACKETED_PASTE) {
                bytes.extend_from_slice(b"\x1b[200~");
                bytes.extend_from_slice(text.as_bytes());
                bytes.extend_from_slice(b"\x1b[201~");
            } else {
                bytes.extend_from_slice(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
            }
            terminal.scroll_to_bottom();
            terminal.write(bytes);
        });
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
        self.session.update(cx, |session, _| {
            let Some(terminal) = session.terminal() else {
                return;
            };
            let mode = terminal.mode();
            if mode.contains(TermMode::ALT_SCREEN) {
                // Alternate screen (less, vim…): translate the wheel into arrow keys.
                if mode.contains(TermMode::ALTERNATE_SCROLL) {
                    let key: &[u8] = if lines > 0 { b"\x1b[A" } else { b"\x1b[B" };
                    let bytes = key.repeat(lines.unsigned_abs() as usize);
                    terminal.write(bytes);
                }
            } else {
                terminal.scroll(lines);
            }
        });
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
            .key_context(keymap::TERMINAL_VIEW_CONTEXT)
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
            .child(match &self.session.read(cx).status {
                SessionStatus::Starting => {
                    div().p_4().child("Starting terminal…").into_any_element()
                }
                SessionStatus::Failed(error) => div()
                    .p_4()
                    .child(format!("Could not start terminal: {error}"))
                    .into_any_element(),
                SessionStatus::Ready(_) => {
                    TerminalElement::new(cx.entity(), focused).into_any_element()
                }
            })
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
