//! Terminal presentation and keyboard/scroll dispatch to its session entity.

use std::path::{Path, PathBuf};

use crate::state::terminal_session::{SessionEvent, SessionStatus, TerminalSession};
use crate::ui::files_panel::DraggedFiles;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, notification::Notification};
use gpui_kit::component::{Sizable as _, button::Button, menu::ContextMenuExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::agent::AgentKind;
use hive_core::agent::launch::{Launch, ResumeSession};
use hive_core::host::Host;
use hive_core::terminal::{ScrollModifiers, SelectionType};

use super::element::TerminalElement;
use super::glyph_cache::GlyphCache;
use super::keys;
use super::mouse::GridGeometry;
use crate::ui::keymap;

mod draft;
mod menu;

pub enum TerminalViewEvent {
    TitleChanged,
    DirectoryChanged,
    Exited,
    /// Open this agent in a new pane beside this one, in the same worktree.
    StartAgent(AgentKind),
}

pub struct TerminalView {
    session: Entity<TerminalSession>,
    focus_handle: FocusHandle,
    cwd: Option<PathBuf>,
    /// A conversation resumed on restore hands its tile back to a shell when the agent ends,
    /// including a resume the CLI rejects, instead of closing the tile.
    shell_after_exit: bool,
    /// Fractional scroll-wheel lines not yet applied.
    scroll_remainder: f32,
    grid: Option<GridGeometry>,
    glyph_cache: GlyphCache,
    selecting: bool,
    _subscriptions: Vec<Subscription>,
    _session_subscriptions: Vec<Subscription>,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

impl TerminalView {
    pub(crate) fn in_directory(
        cwd: Option<&Path>,
        directory: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_launch(cwd, directory, None, window, cx)
    }

    /// A restored tile that continues the conversation it showed when Hive quit.
    pub(crate) fn resuming(
        cwd: Option<&Path>,
        directory: Option<&Path>,
        resume: ResumeSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::with_launch(cwd, directory, Some(Launch::Resume(resume)), window, cx);
        view.shell_after_exit = true;
        view
    }

    pub(crate) fn with_launch(
        cwd: Option<&Path>,
        directory: Option<&Path>,
        launch: Option<Launch>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::on_host(&Host::Local, (cwd, directory), launch, true, window, cx)
    }

    pub(crate) fn on_host(
        host: &Host,
        paths: (Option<&Path>, Option<&Path>),
        launch: Option<Launch>,
        connect: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (cwd, directory) = paths;
        let session = cx.new(|cx| {
            TerminalSession::new(
                host.clone(),
                directory.map(Path::to_path_buf),
                cwd.map(Path::to_path_buf),
                launch,
                connect,
                cx,
            )
        });
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
        ];
        Self {
            _session_subscriptions: Self::subscribe_session(&session, window, cx),
            session,
            focus_handle,
            cwd: cwd.map(Path::to_path_buf),
            shell_after_exit: false,
            scroll_remainder: 0.,
            grid: None,
            glyph_cache: GlyphCache::default(),
            selecting: false,
            _subscriptions: subscriptions,
        }
    }

    fn subscribe_session(
        session: &Entity<TerminalSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Subscription> {
        vec![
            cx.observe(session, |_, _, cx| cx.notify()),
            cx.subscribe_in(session, window, |this, _, event, window, cx| match event {
                SessionEvent::TitleChanged => cx.emit(TerminalViewEvent::TitleChanged),
                SessionEvent::DirectoryChanged => cx.emit(TerminalViewEvent::DirectoryChanged),
                SessionEvent::Exited if this.shell_after_exit => this.continue_in_shell(window, cx),
                SessionEvent::Exited => cx.emit(TerminalViewEvent::Exited),
                SessionEvent::PasteFailed(error) => window.push_notification(
                    Notification::error(error.clone()).title("Could not paste image"),
                    cx,
                ),
            }),
        ]
    }

    /// Replaces the ended agent with a shell in its last directory, keeping the tile in place.
    fn continue_in_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shell_after_exit = false;
        let directory = self.restore_directory(cx);
        let worktree = self.cwd.clone();
        let focused = self.focus_handle.is_focused(window);
        let host = self.session.read(cx).host.clone();
        let session = cx.new(|cx| {
            let mut session = TerminalSession::new(host, Some(directory), worktree, None, true, cx);
            session.set_focused(focused);
            session
        });
        self._session_subscriptions = Self::subscribe_session(&session, window, cx);
        self.session = session;
        self.selecting = false;
        // The saved tile no longer holds a conversation to resume.
        cx.emit(TerminalViewEvent::DirectoryChanged);
        cx.notify();
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

    pub(super) fn glyph_cache(&mut self) -> &mut GlyphCache {
        &mut self.glyph_cache
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

    fn drop_file(&mut self, drag: &DraggedFiles, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.session.update(cx, |session, cx| {
            session.insert_file_references(&drag.target, &drag.paths, cx)
        });
        match result {
            Ok(()) => self.focus_handle.focus(window, cx),
            Err(error) => {
                tracing::warn!(%error, "could not insert dropped file reference");
                window.push_notification(
                    Notification::error(error).title("Could not insert file path"),
                    cx,
                );
            }
        }
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
        let host = self.session.read(cx).host.clone();
        let bg = cx.theme().background;
        let draft = self.render_agent_draft(window, cx);
        let drop_session = self.session.clone();
        let disconnected = !host.is_local()
            && (self.session.read(cx).exited
                || matches!(self.session.read(cx).status, SessionStatus::Failed(_)));
        let reconnect_hover = Hover::new("ssh-reconnect", window, cx);
        v_flex()
            .size_full()
            .children(draft)
            .when(disconnected, |body| {
                body.child(
                    Button::new("ssh-reconnect")
                        .cursor_pointer()
                        .label("New SSH session")
                        .small()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.continue_in_shell(window, cx)),
                        )
                        .smooth_hover(ButtonLook::Outline, &reconnect_hover, cx),
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
                    .border_1()
                    .border_color(bg)
                    .cursor(CursorStyle::IBeam)
                    .can_drop(move |value, _, cx| {
                        value.downcast_ref::<DraggedFiles>().is_some_and(|drag| {
                            drop_session.read(cx).accepts_file_drop(&drag.target)
                        })
                    })
                    .drag_over::<DraggedFiles>(|style, _, _, cx| {
                        style.border_color(cx.theme().drag_border)
                    })
                    .on_drop(cx.listener(Self::drop_file))
                    .on_key_down(cx.listener(Self::key_down))
                    .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
                    .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy(cx)))
                    .on_action(cx.listener(|this, _: &component::input::Copy, _, cx| this.copy(cx)))
                    .on_scroll(cx.listener(Self::scroll_wheel))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::context_mouse_down))
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
                    })
                    .context_menu(self.context_menu(cx)),
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
