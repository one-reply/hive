//! Terminal presentation and keyboard/scroll dispatch to its session entity.

use std::path::{Path, PathBuf};

use crate::state::terminal_session::{
    AgentPlacement, SessionEvent, SessionStatus, TerminalSession,
};
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

mod cursor;
mod draft;
mod menu;
mod selection;
mod session;

pub(crate) enum TerminalViewEvent {
    TitleChanged,
    DirectoryChanged,
    Exited,
    StartAgent(AgentKind, AgentPlacement),
}

pub struct TerminalView {
    session: Entity<TerminalSession>,
    focus_handle: FocusHandle,
    cwd: Option<PathBuf>,
    /// Restored and in-place agents hand their tile back to a shell when they exit.
    shell_after_exit: bool,
    /// Keep the original shell and its scrollback alive during an in-place agent launch.
    shell: Option<Entity<TerminalSession>>,
    /// Fractional scroll-wheel lines not yet applied.
    scroll_remainder: f32,
    grid: Option<GridGeometry>,
    glyph_cache: GlyphCache,
    /// Blink steps since the cursor was last lit by input or a focus change.
    cursor_step: usize,
    cursor_tick: Option<Task<()>>,
    selecting: bool,
    selection_scroll: gpui_kit::base::AutoScroll,
    selection_scroll_remainder: f32,
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
        Self::with_session(session, cwd, window, cx)
    }

    fn with_session(
        session: Entity<TerminalSession>,
        cwd: Option<&Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _, cx| {
                this.restart_cursor_blink(cx);
                this.session
                    .update(cx, |session, _| session.set_focused(true));
                cx.notify();
            }),
            cx.on_blur(&focus_handle, window, |this, _, cx| {
                this.restart_cursor_blink(cx);
                this.stop_selection();
                this.session
                    .update(cx, |session, _| session.set_focused(false));
                cx.notify();
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                this.restart_cursor_blink(cx);
                if !window.is_window_active() {
                    this.stop_selection();
                }
            }),
        ];
        Self {
            _session_subscriptions: Self::subscribe_session(&session, window, cx),
            session,
            focus_handle,
            cwd: cwd.map(Path::to_path_buf),
            shell_after_exit: false,
            shell: None,
            scroll_remainder: 0.,
            grid: None,
            glyph_cache: GlyphCache::default(),
            cursor_step: 0,
            cursor_tick: None,
            selecting: false,
            selection_scroll: gpui_kit::base::AutoScroll::default(),
            selection_scroll_remainder: 0.,
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

    pub(super) fn glyph_cache(&mut self) -> &mut GlyphCache {
        &mut self.glyph_cache
    }

    /// Called by the element once it knows how many cells fit.
    pub(super) fn set_grid(&mut self, grid: GridGeometry, cx: &mut Context<Self>) {
        self.grid = Some(grid);
        self.session
            .update(cx, |session, _| session.resize(grid.cols, grid.rows));
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
            self.restart_cursor_blink(cx);
            self.stop_selection();
            cx.stop_propagation();
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        self.restart_cursor_blink(cx);
        self.stop_selection();
        self.session.update(cx, |session, cx| session.paste(cx));
    }

    fn drop_file(&mut self, drag: &DraggedFiles, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.session.update(cx, |session, cx| {
            session.insert_file_references(&drag.target, &drag.paths, cx)
        });
        match result {
            Ok(()) => {
                self.restart_cursor_blink(cx);
                self.focus_handle.focus(window, cx);
            }
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
        self.sync_cursor_blink(focused && window.is_window_active(), cx);
        let host = self.session.read(cx).host.clone();
        let bg = cx.theme().background;
        let draft = self.render_agent_draft(window, cx);
        let drop_session = self.session.clone();
        let disconnected = !host.is_local()
            && (self.session.read(cx).exited
                || matches!(self.session.read(cx).status, SessionStatus::Failed(_)));
        let launch_failed = self.shell.is_some()
            && matches!(self.session.read(cx).status, SessionStatus::Failed(_));
        let reconnect_hover = Hover::new("ssh-reconnect", window, cx);
        v_flex()
            .size_full()
            .children(draft)
            .when(disconnected || launch_failed, |body| {
                body.child(
                    Button::new("ssh-reconnect")
                        .cursor_pointer()
                        .label(if launch_failed {
                            "Return to shell"
                        } else {
                            "New SSH session"
                        })
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
