use super::*;
use gpui_kit::component::input::{EditorState, InputEvent, Position};
use hive_core::files::recovery::Draft;
use hive_core::files::{self, Snapshot};
use std::time::Duration;

mod io;

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
    pub base: Option<Snapshot>,
    pub conflict: Option<Snapshot>,
    pub error: Option<DocumentError>,
    pub missing: bool,
    pub dirty: bool,
    pub loading: bool,
    pub saving: bool,
    pub visible: bool,
    pub pinned: bool,
    store: Entity<Store>,
    recovery_key: String,
    pending_line: Option<usize>,
    position: Option<([u32; 2], [f32; 2])>,
    _read: Option<Task<()>>,
    _save: Option<Task<()>>,
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
        let language = request
            .path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let editor = cx.new(|cx| EditorState::new(window, cx).language(language));
        let subscriptions = vec![
            cx.subscribe_in(&editor, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.pinned = true;
                    this.update_dirty(cx);
                    this.schedule_recovery(cx);
                    cx.notify();
                }
            }),
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
        let mut document = Self {
            target: request.target,
            path: request.path,
            editor,
            base: None,
            conflict: None,
            error: None,
            missing: false,
            dirty: false,
            loading: false,
            saving: false,
            visible: false,
            pinned: !request.preview || recovered.is_some(),
            store,
            recovery_key,
            pending_line: request.line,
            position: None,
            _read: None,
            _save: None,
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
        self.position = Some((cursor, scroll));
        if self.base.is_some() {
            self.apply_position(window, cx);
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
        }
    }

    pub fn goto(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.base.is_none() {
            self.pending_line = Some(line);
            return;
        }
        self.editor.update(cx, |editor, cx| {
            let position = Position::new(line.saturating_sub(1) as u32, 0);
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
        cx.notify();
    }

    fn apply_position(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(line) = self.pending_line.take() {
            self.goto(line, window, cx);
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
