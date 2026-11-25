use super::completions::Completions;
use super::*;
use gpui_kit::component::input::{
    EditorState, InputEditsEvent, InputEvent, Position, RopeExt as _,
};
use hive_core::agent::reference::{self, CodeReference, Version};
use hive_core::code_navigation::Navigator;
use hive_core::files::recovery::Draft;
use hive_core::files::{self, Snapshot};
use std::rc::Rc;
use std::time::Duration;

mod blame;
mod editing;
pub(crate) use editing::Indentation;
mod formatting;
mod image;
mod io;
mod navigation;
mod relocation;
pub(crate) use blame::LineBlame;
pub(crate) use image::{Content, PreviewState, SvgMode};
pub(crate) use io::{Changes, SaveBlocker};
pub(crate) use navigation::{STICKY_BORDER_WIDTH, sticky_row_limit};

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
    pub(crate) indentation: Indentation,
    needs_indentation_detection: bool,
    selection_commands: editing::SelectionCommands,
    completions: Rc<Completions>,
    pub(crate) symbols: Arc<Vec<hive_core::symbols::Symbol>>,
    pub(crate) symbols_text: Option<gpui_kit::component::input::Rope>,
    pub(crate) language: Option<Entity<super::language::LanguageSession>>,
    pub(crate) diagnostics: Vec<lsp_types::Diagnostic>,
    pub(crate) import_menu: Option<super::language_features::ImportMenu>,
    pub(super) _language_subscription: Option<Subscription>,
    pub(super) _language_sync: Option<Task<()>>,
    blame: LineBlame,
    pub content: Content,
    pub base: Option<Snapshot>,
    pub conflict: Option<Snapshot>,
    pub error: Option<DocumentError>,
    pub missing: bool,
    pub dirty: bool,
    pub loading: bool,
    pub saving: bool,
    pub formatting: bool,
    pub format_error: Option<String>,
    pub(super) revision: u64,
    pub visible: bool,
    pub pinned: bool,
    pub path_busy: bool,
    store: Entity<Store>,
    /// Starts language tools for this document; it owns the shared servers.
    pub(super) registry: WeakEntity<Documents>,
    recovery_key: String,
    pending_position: Option<Position>,
    position: Option<([u32; 2], [f32; 2])>,
    _read: Option<Task<()>>,
    _save: Option<Task<()>>,
    _format: Option<Task<()>>,
    /// Callers that asked to save while a write was already running.
    save_waiters: Vec<async_channel::Sender<bool>>,
    _backup: Option<Task<()>>,
    _poll: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Document {
    #[cfg(test)]
    pub(super) fn isolated_for_test(
        request: OpenFile,
        store: Entity<Store>,
        registry: WeakEntity<Documents>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut document = Self::new(
            request,
            None,
            store,
            registry,
            Arc::new(Navigator::default()),
            window,
            cx,
        );
        document._read = None;
        document._poll = Task::ready(());
        document._subscriptions.clear();
        document.loading = false;
        document
    }

    /// A document already read as `base` and then edited to `text`, without its poll or
    /// its listeners: tests drive its saves and reads themselves.
    #[cfg(test)]
    pub(super) fn edited_for_test(
        request: OpenFile,
        base: Snapshot,
        text: &str,
        store: Entity<Store>,
        registry: WeakEntity<Documents>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut document = Self::isolated_for_test(request, store, registry, window, cx);
        document.base = Some(base);
        document
            .editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
        document.update_dirty(cx);
        document
    }

    pub fn new(
        request: OpenFile,
        recovered: Option<Record>,
        store: Entity<Store>,
        registry: WeakEntity<Documents>,
        navigator: Arc<Navigator>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = files::editor_language(&request.path);
        let completions = Completions::new(cx.weak_entity(), navigator);
        let indentation = Indentation::for_path(&request.path);
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language(language)
                .tab_size(indentation.tab_size());
            editor.lsp_mut().completion_provider = Some(completions.clone());
            editor
        });
        let editor_focus = editor.read(cx).focus_handle(cx);
        let blame = LineBlame::new(store.read(cx).state.settings.files.inline_blame);
        let subscriptions = vec![
            cx.on_release(|this, cx| this.close_language(cx)),
            cx.observe(&store, |this, _, cx| this.apply_blame_setting(cx)),
            cx.subscribe(&editor, |this, _, event: &InputEditsEvent, _| {
                this.selection_commands.edited(event);
                this.completions.edit(event.edits());
            }),
            cx.subscribe_in(&editor, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.import_menu = None;
                    this.revision = this.revision.wrapping_add(1);
                    this.symbols = Arc::default();
                    this.invalidate_image(cx);
                    this.pin(cx);
                    this.update_dirty(cx);
                    this.schedule_recovery(cx);
                    this.completions.schedule_refresh(cx);
                    this.schedule_language(cx);
                    this.invalidate_blame(cx);
                    this.emit(DocumentEvent::Edited(this.at_cursor(cx)), cx);
                    cx.notify();
                }
            }),
            cx.on_focus_out(&editor_focus, window, |this, _, window, cx| {
                if this.import_menu.take().is_some() {
                    cx.notify();
                }
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
            indentation,
            needs_indentation_detection: true,
            selection_commands: Default::default(),
            completions,
            symbols: Arc::default(),
            symbols_text: None,
            language: None,
            diagnostics: Vec::new(),
            import_menu: None,
            _language_subscription: None,
            _language_sync: None,
            blame,
            content,
            base: None,
            conflict: None,
            error: None,
            missing: false,
            dirty: false,
            loading: false,
            saving: false,
            formatting: false,
            format_error: None,
            revision: 0,
            visible: false,
            pinned: !request.preview || recovered.is_some(),
            path_busy: false,
            store,
            registry,
            recovery_key,
            pending_position: request.position,
            position: None,
            _read: None,
            _save: None,
            _format: None,
            save_waiters: Vec::new(),
            _backup: None,
            _poll: poll,
            _subscriptions: subscriptions,
        };
        if let Some(record) = recovered {
            document.infer_indentation(&record.draft.text, cx);
            document.editor.update(cx, |editor, cx| {
                editor.set_value(record.draft.text, window, cx)
            });
            document.base = Some(record.draft.base);
            document.update_dirty(cx);
            if !document.dirty {
                document.flush_recovery(cx);
            }
        }
        document.completions.schedule_refresh(cx);
        document.refresh(window, cx);
        document
    }

    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).text().to_string()
    }

    pub(crate) fn selection_draft(&self, cx: &App) -> Option<String> {
        let editor = self.editor.read(cx);
        hive_core::agent::draft::selection(
            &self.target.host,
            &self.target.path,
            &self.path,
            &editor.text().to_string(),
            editor.selected_range(),
            self.dirty,
        )
    }

    /// The selected lines, or the cursor's line, as a reference for an agent's prompt.
    pub(crate) fn code_reference(&self, cx: &App) -> Option<CodeReference> {
        if self.content.is_raster() {
            return None;
        }
        let editor = self.editor.read(cx);
        let lines = reference::selected_lines(&editor.text().to_string(), editor.selected_range())?;
        CodeReference::new(
            self.target.host.clone(),
            &self.target.path,
            &self.path,
            lines,
            Version::Worktree,
        )
        .inspect_err(|error| tracing::warn!(%error, "no code reference for this file"))
        .ok()
    }

    pub(crate) fn at_cursor(&self, cx: &App) -> OpenFile {
        OpenFile {
            target: self.target.clone(),
            path: self.path.clone(),
            position: Some(self.editor.read(cx).cursor_position()),
            preview: false,
            placement: Default::default(),
        }
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

    /// The user keeps this tab: the next opened file no longer replaces it, and it may
    /// start language tools, which a preview never does.
    pub(crate) fn pin(&mut self, cx: &mut Context<Self>) {
        if self.pinned {
            return;
        }
        self.pinned = true;
        self.request_language(cx);
        cx.notify();
    }

    pub fn saves_on_focus_change(&self, cx: &App) -> bool {
        self.store
            .read(cx)
            .state
            .settings
            .files
            .save_on_focus_change
    }

    pub fn completes_code(&self, cx: &App) -> bool {
        self.store.read(cx).state.settings.files.code_completion
    }

    /// Whether language servers and formatters may run programs from this worktree.
    pub fn runs_language_tools(&self, cx: &App) -> bool {
        self.store.read(cx).state.settings.files.language_tools
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
            self.schedule_blame(cx);
            self.emit(
                DocumentEvent::Activated(self.target.clone(), self.path.clone()),
                cx,
            );
        }
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

    pub(super) fn replace_preserving_view(
        &mut self,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            let selected = editor.selected_range();
            let start = editor.text().offset_to_position(selected.start);
            let end = editor.text().offset_to_position(selected.end);
            let reversed = editor.is_selection_reversed();
            let scroll = editor.scroll_offset();
            editor.replace_all(text, window, cx);
            let range =
                editor.text().position_to_offset(&start)..editor.text().position_to_offset(&end);
            editor.set_selected_range_with_direction(range, reversed, cx);
            editor.set_scroll_offset(scroll, cx);
        });
    }

    /// A just-created buffer already has an exact, rechecked rename baseline.
    pub(super) fn install_reviewed(
        &mut self,
        snapshot: Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._read = None;
        self.loading = false;
        self.install(snapshot, window, cx);
    }

    fn install(&mut self, snapshot: Snapshot, window: &mut Window, cx: &mut Context<Self>) {
        // Restoring a cursor focuses gpui-component's editor; background reads must not steal focus.
        let focus = window.focused(cx);
        let text = snapshot.text();
        self.infer_indentation(&text, cx);
        self.selection_commands = Default::default();
        self.editor.update(cx, |editor, cx| {
            let position = editor.cursor_position();
            let scroll = editor.scroll_offset();
            editor.set_value(text, window, cx);
            editor.set_cursor_position(position, window, cx);
            editor.set_scroll_offset(scroll, cx);
        });
        self.base = Some(snapshot);
        self.conflict = None;
        self.error = None;
        self.dirty = false;
        self.completions.schedule_refresh(cx);
        self.schedule_language(cx);
        self.invalidate_blame(cx);
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
