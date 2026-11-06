//! Terminal presentation and keyboard/scroll dispatch to its session entity.

use std::path::{Path, PathBuf};

use crate::state::terminal_session::{SessionEvent, SessionStatus, TerminalSession};
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, notification::Notification};
use gpui_kit::component::{Sizable as _, button::Button, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::agent::launch::{Launch, ResumeSession};
use hive_core::terminal::{ScrollModifiers, SelectionType};

use super::element::TerminalElement;
use super::keys;
use super::mouse::GridGeometry;
use crate::ui::keymap;

mod draft;

pub enum TerminalViewEvent {
    TitleChanged,
    DirectoryChanged,
    Exited,
}

pub struct TerminalView {
    session: Entity<TerminalSession>,
    focus_handle: FocusHandle,
    cwd: Option<PathBuf>,
    /// Fractional scroll-wheel lines not yet applied.
    scroll_remainder: f32,
    grid: Option<GridGeometry>,
    selecting: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl TerminalView {
    pub fn new(cwd: Option<&Path>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::in_directory(cwd, cwd, window, cx)
    }

    pub(crate) fn in_directory(
        cwd: Option<&Path>,
        directory: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_launch(cwd, directory, None, None, window, cx)
    }

    pub(crate) fn with_launch(
        cwd: Option<&Path>,
        directory: Option<&Path>,
        launch: Option<Launch>,
        resume: Option<ResumeSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = cx.new(|cx| {
            TerminalSession::new(
                directory.map(Path::to_path_buf),
                cwd.map(Path::to_path_buf),
                launch,
                cx,
            )
        });
        // A restored tile offers its saved conversation; a fresh launch may already own one.
        if let Some(resume) = resume {
            session.update(cx, |session, _| {
                session.resume_available = true;
                session.resume = Some(resume);
            });
        }
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _, cx| {
                this.session
                    .update(cx, |session, _| session.set_focused(true));
                cx.notify();
            }),
            cx.on_blur(&focus_handle, window, |this, _, cx| {
                this.selecting = false;
                this.session
                    .update(cx, |session, _| session.set_focused(false));
                cx.notify();
            }),
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.subscribe_in(&session, window, |_, _, event, window, cx| match event {
                SessionEvent::TitleChanged => cx.emit(TerminalViewEvent::TitleChanged),
                SessionEvent::DirectoryChanged => cx.emit(TerminalViewEvent::DirectoryChanged),
                SessionEvent::Exited => cx.emit(TerminalViewEvent::Exited),
                SessionEvent::PasteFailed(error) => window.push_notification(
                    Notification::error(error.clone()).title("Could not paste image"),
                    cx,
                ),
            }),
        ];
        Self {
            session,
            focus_handle,
            cwd: cwd.map(Path::to_path_buf),
            scroll_remainder: 0.,
            grid: None,
            selecting: false,
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

    pub(crate) fn restore_directory(&self, cx: &App) -> PathBuf {
        self.session
            .read(cx)
            .directory
            .clone()
            .or_else(|| self.cwd.clone())
            .unwrap_or_default()
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
    pub(super) fn set_grid(&mut self, grid: GridGeometry, cx: &mut Context<Self>) {
        self.grid = Some(grid);
        self.session
            .update(cx, |session, _| session.resize(grid.cols, grid.rows));
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        let Some(grid) = self.grid else { return };
        let (point, side) = grid.cell_at(event.position);
        let kind = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.selecting = true;
        self.session.update(cx, |session, cx| {
            session.start_selection(point, side, kind, cx)
        });
        cx.stop_propagation();
    }

    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        let Some(grid) = self.grid else { return };
        let (point, side) = grid.cell_at(position);
        self.session
            .update(cx, |session, cx| session.update_selection(point, side, cx));
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.dragging() {
            self.extend_selection(event.position, cx);
        } else {
            self.selecting = false;
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.extend_selection(event.position, cx);
        self.selecting = false;
    }

    fn copy(&self, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| session.copy(cx));
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let handled = self.session.update(cx, |session, cx| {
            let Some(terminal) = session.terminal().filter(|_| !session.exited) else {
                return false;
            };
            if let Some(bytes) = keys::to_bytes(&event.keystroke, terminal.mode()) {
                session.write_input(bytes, cx);
                return true;
            }
            false
        });
        if handled {
            cx.stop_propagation();
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        self.session.update(cx, |session, cx| session.paste(cx));
    }

    fn scroll_wheel(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(grid) = self.grid else { return };
        let line_height = grid.cell.height;
        let delta = event.delta.pixel_delta(line_height).y / line_height;
        cx.stop_propagation();
        self.scroll_remainder += delta;
        let lines = self.scroll_remainder.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;
        let (point, _) = grid.cell_at(event.position);
        let modifiers = ScrollModifiers {
            shift: event.modifiers.shift,
            alt: event.modifiers.alt,
            control: event.modifiers.control,
        };
        self.session.update(cx, |session, cx| {
            session.scroll_wheel(lines, point, modifiers, cx);
        });
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
        let resume = self
            .session
            .read(cx)
            .resume
            .clone()
            .filter(|_| self.session.read(cx).resume_available);
        let resume_hover = Hover::new("resume-agent", window, cx);
        let draft = self.render_agent_draft(window, cx);
        v_flex()
            .size_full()
            .children(draft)
            .when_some(resume, |view, resume| {
                view.child(
                    Button::new("resume-agent")
                        .cursor_pointer()
                        .small()
                        .label(format!("Resume {} session", resume.agent().name()))
                        .tooltip("Continue the saved conversation in a new terminal")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(path) = this.cwd.clone() {
                                this.session
                                    .update(cx, |session, cx| session.request_resume(path, cx));
                            }
                        }))
                        .smooth_hover(ButtonLook::Outline, &resume_hover, cx),
                )
            })
            .child(
                div()
                    .id("terminal")
                    .track_focus(&self.focus_handle)
                    .key_context(keymap::TERMINAL_VIEW_CONTEXT)
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .bg(bg)
                    .cursor(CursorStyle::IBeam)
                    .on_key_down(cx.listener(Self::key_down))
                    .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
                    .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
                    .on_action(cx.listener(|this, _: &component::input::Copy, _, cx| this.copy(cx)))
                    .on_scroll(cx.listener(Self::scroll_wheel))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .on_mouse_move(cx.listener(Self::mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
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
                    }),
            )
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

actions!(terminal, [Copy, Paste]);

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
