use super::*;
use gpui_kit::component::input::{EditorState, InputEvent, Position};
use hive_core::files::recovery::Draft;
use hive_core::files::{self, Snapshot};
use std::time::Duration;

mod image;
mod io;
mod relocation;
pub(crate) use image::{Content, PreviewState, SvgMode};

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum DocumentError {
    Read(String),
    Save(String),
}
impl DocumentError {
    pub fn message(&self) -> &str {
        match self {
            Self::Read(message) | Self::Save(message) => message,
        }
    }
}

pub(crate) struct Document {
    pub target: Target,
    pub path: PathBuf,
    pub editor: Entity<EditorState>,
    pub content: Content,
    pub base: Option<Snapshot>,
    pub conflict: Option<Snapshot>,
    pub error: Option<DocumentError>,
    pub missing: bool,
    pub dirty: bool,
    pub loading: bool,
    pub saving: bool,
    pub visible: bool,
    pub pinned: bool,
    pub path_busy: bool,
    store: Entity<Store>,
    recovery_key: String,
    pending_position: Option<Position>,
    position: Option<([u32; 2], [f32; 2])>,
    _read: Option<Task<()>>,
    _save: Option<Task<()>>,
    /// Callers that asked to save while a write was already running.
    save_waiters: Vec<async_channel::Sender<bool>>,
    _backup: Option<Task<()>>,
    _poll: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Document {
    pub fn new(
        request: OpenFile,
        recovered: Option<Record>,
        store: Entity<Store>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = files::editor_language(&request.path);
        let editor = cx.new(|cx| EditorState::new(window, cx).language(language));
        let editor_focus = editor.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(&editor, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.invalidate_image(cx);
                    this.pinned = true;
                    this.update_dirty(cx);
                    this.schedule_recovery(cx);
                    this.emit(DocumentEvent::Changed(this.target.clone()), cx);
                    cx.notify();
                }
            }),
            cx.on_focus_out(&editor_focus, window, |this, _, window, cx| {
                if this.dirty && this.saves_on_focus_change(cx) {
                    this.save(None, window, cx);
                }
            }),
            cx.on_release(|this, cx| this.invalidate_image(cx)),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() && this.should_poll(cx) {
                    this.refresh(window, cx);
                }
            }),
        ];
        let poll = cx.spawn_in(window, async |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                if this
                    .update_in(cx, |this, window, cx| {
                        if window.is_window_active() && this.should_poll(cx) {
                            this.refresh(window, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let recovery_key = recovered
            .as_ref()
            .map(|r| r.key.clone())
            .unwrap_or_else(|| {
                recovery::key(&request.target.host, &request.target.path, &request.path)
            });
        let content = Content::new(&request.path, recovered.is_some());
        let mut document = Self {
            target: request.target,
            path: request.path,
            editor,
            content,
            base: None,
            conflict: None,
            error: None,
            missing: false,
            dirty: false,
            loading: false,
            saving: false,
            visible: false,
            pinned: !request.preview || recovered.is_some(),
            path_busy: false,
            store,
            recovery_key,
            pending_position: request.position,
            position: None,
            _read: None,
            _save: None,
            save_waiters: Vec::new(),
            _backup: None,
            _poll: poll,
            _subscriptions: subscriptions,
        };
        if let Some(record) = recovered {
            document.editor.update(cx, |editor, cx| {
                editor.set_value(record.draft.text, window, cx)
            });
            document.base = Some(record.draft.base);
            document.update_dirty(cx);
            if !document.dirty {
                document.flush_recovery(cx);
            }
        }
        document.refresh(window, cx);
        document
    }

    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).text().to_string()
    }

    pub fn saved_position(&self, cx: &App) -> ([u32; 2], [f32; 2]) {
        if self.content.is_raster() {
            return ([0; 2], [0.; 2]);
        }
        let editor = self.editor.read(cx);
        let position = editor.cursor_position();
        let offset = editor.scroll_offset();
        (
            [position.line, position.character],
            [offset.x.into(), offset.y.into()],
        )
    }

    pub fn saves_on_focus_change(&self, cx: &App) -> bool {
        self.store
            .read(cx)
            .state
            .settings
            .files
            .save_on_focus_change
    }

    fn should_poll(&self, cx: &App) -> bool {
        self.dirty
            || (self.visible
                && crate::app::AppState::get(cx)
                    .projects
                    .read(cx)
                    .selection()
                    .is_some_and(|(project, _)| {
                        project.path == self.target.project && project.host == self.target.host
                    }))
    }

    pub fn restore_position(
        &mut self,
        cursor: [u32; 2],
        scroll: [f32; 2],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.content.is_raster() {
            return;
        }
        self.position = Some((cursor, scroll));
        if self.base.is_some() {
            let focus = window.focused(cx);
            self.apply_position(window, cx);
            if let Some(focus) = focus {
                focus.focus(window, cx);
            } else {
                window.blur(cx);
            }
        }
    }

    fn update_dirty(&mut self, cx: &App) {
        self.dirty = self
            .base
            .as_ref()
            .is_some_and(|base| !base.matches(&self.text(cx)));
    }

    pub fn activate(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = active;
        if active {
            self.refresh(window, cx);
            self.emit(
                DocumentEvent::Activated(self.target.clone(), self.path.clone()),
                cx,
            );
        }
    }

    pub fn goto(&mut self, position: Position, window: &mut Window, cx: &mut Context<Self>) {
        if self.content.is_raster() {
            return;
        }
        self.set_svg_mode(SvgMode::Code, window, cx);
        if self.base.is_none() {
            self.pending_position = Some(position);
            return;
        }
        self.editor.update(cx, |editor, cx| {
            editor.unfold_at(position, cx);
            editor.set_cursor_position(position, window, cx);
        });
    }

    fn schedule_recovery(&mut self, cx: &mut Context<Self>) {
        self._backup = Some(cx.spawn(async |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = this.update(cx, |this, cx| this.flush_recovery(cx));
        }));
    }

    pub fn flush_recovery(&mut self, cx: &App) {
        if self.path_busy || self.content.is_raster() {
            return;
        }
        let draft = self.base.as_ref().filter(|_| self.dirty).map(|base| Draft {
            host: self.target.host.clone(),
            project: self.target.project.clone(),
            root: self.target.path.clone(),
            path: self.path.clone(),
            base: base.clone(),
            text: self.text(cx),
        });
        self.store
            .read(cx)
            .save_recovery(self.recovery_key.clone(), draft);
    }

    pub fn discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._backup = None;
        if let Some(base) = self.base.clone() {
            self.install(base, window, cx);
        }
        self.store
            .read(cx)
            .save_recovery(self.recovery_key.clone(), None);
    }

    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.conflict.take() {
            self.install(snapshot, window, cx);
            self._backup = None;
            self.flush_recovery(cx);
        }
    }

    fn install(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        // Restoring a cursor focuses gpui-component's editor; background reads must not steal focus.
        let focus = window.focused(cx);
        self.editor.update(cx, |editor, cx| {
            let position = editor.cursor_position();
            let scroll = editor.scroll_offset();
            editor.set_value(snapshot.text(), window, cx);
            editor.set_cursor_position(position, window, cx);
            editor.set_scroll_offset(scroll, cx);
        });
        self.base = Some(snapshot);
        self.conflict = None;
        self.error = None;
        self.dirty = false;
        self.apply_position(window, cx);
        if let Some(focus) = focus {
            focus.focus(window, cx);
        } else {
            window.blur(cx);
        }
        self.emit(DocumentEvent::Changed(self.target.clone()), cx);
        self.render_image(cx);
        cx.notify();
    }

    fn apply_position(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(position) = self.pending_position.take() {
            self.goto(position, window, cx);
        } else if let Some((cursor, scroll)) = self.position.take() {
            self.editor.update(cx, |editor, cx| {
                editor.set_cursor_position(Position::new(cursor[0], cursor[1]), window, cx);
                if scroll.iter().all(|v| v.is_finite()) {
                    editor.set_scroll_offset(point(px(scroll[0]), px(scroll[1])), cx);
                }
            });
        }
    }
}
