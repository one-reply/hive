use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use crate::{
    app::AppState,
    state::documents::{Document, OpenFile},
    state::git::{FileChanges, Git},
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{BasePanel, DockArea, DockEvent, Panel, PanelEvent, TabGroup};
use gpui_kit::component::input::Editor;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::*;

mod actions;
pub(crate) mod close;

actions!(editor, [SaveFile, CloseFile, OpenFileSearch]);

pub(crate) struct EditorPanel {
    pub document: Entity<Document>,
    area: WeakEntity<DockArea>,
    pub group: Option<WeakEntity<TabGroup>>,
    alone: bool,
    pub restoring: bool,
    last_position: (u32, u32, Point<Pixels>),
    changes: FileChanges,
    _position: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EditorPanel {
    pub fn new(
        request: OpenFile,
        area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let documents = AppState::get(cx).documents.clone();
        let document = documents.update(cx, |documents, cx| documents.get(&request, window, cx));
        let editor = document.read(cx).editor.clone();
        let git = AppState::get(cx).git.clone();
        let changes = file_changes(&git, &document, cx);
        let subscriptions = vec![
            cx.observe(&git, |this, git, cx| {
                let changes = file_changes(&git, &this.document, cx);
                if this.changes != changes {
                    this.changes = changes;
                    cx.notify();
                }
            }),
            cx.observe_in(&document, window, |this, document, window, cx| {
                if this.restoring && document.read(cx).missing && document.read(cx).base.is_none() {
                    this.request_close(window, cx);
                    return;
                }
                if let Some(group) = this.group.as_ref().and_then(WeakEntity::upgrade) {
                    group.update(cx, |_, cx| cx.notify());
                }
                if let Some(area) = this.area.upgrade() {
                    cx.defer(move |cx| area.update(cx, |_, cx| cx.emit(DockEvent::LayoutChanged)));
                }
                cx.notify();
            }),
            cx.observe(&editor, |this, editor, cx| {
                let editor = editor.read(cx);
                let cursor = editor.cursor_position();
                let position = (cursor.line, cursor.character, editor.scroll_offset());
                if this.last_position != position {
                    this.last_position = position;
                    this._position = Some(cx.spawn(async |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(500))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            if let Some(area) = this.area.upgrade() {
                                cx.defer(move |cx| {
                                    area.update(cx, |_, cx| cx.emit(DockEvent::LayoutChanged))
                                });
                            }
                        });
                    }));
                    cx.notify();
                }
            }),
        ];
        Self {
            document,
            area,
            group: None,
            alone: false,
            restoring: false,
            last_position: (0, 0, Point::default()),
            changes,
            _position: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn label(&self, cx: &App) -> SharedString {
        let document = self.document.read(cx);
        format!(
            "{}{}",
            document
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            if document.dirty { " •" } else { "" }
        )
        .into()
    }

    pub fn set_alone(&mut self, alone: bool, cx: &mut Context<Self>) {
        if self.alone != alone {
            self.alone = alone;
            cx.notify();
        }
    }

    pub fn request_close(&self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = cx.entity();
        let area = self.area.clone();
        close::guard(
            close::Scope::Document(self.document.clone()),
            window,
            cx,
            move |window, cx| {
                if let Some(area) = area.upgrade() {
                    area.update(cx, |area, cx| area.remove_panel(panel, window, cx));
                }
            },
        );
    }
}

impl EventEmitter<PanelEvent> for EditorPanel {}
impl Focusable for EditorPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.document.read(cx).editor.read(cx).focus_handle(cx)
    }
}
impl BasePanel for EditorPanel {
    fn panel_name(&self) -> &'static str {
        "EditorPanel"
    }
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.document
            .update(cx, |d, cx| d.activate(active, window, cx));
        if active {
            let target = self.document.read(cx).target.clone();
            let git = AppState::get(cx).git.clone();
            cx.defer(move |cx| git.update(cx, |git, cx| git.activate(target, cx)));
        }
    }
    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, _: &mut Window, _: &mut Context<Self>) {
        self.group = Some(group);
    }
    fn on_removed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.document
            .update(cx, |d, cx| d.activate(false, window, cx));
    }
}
impl Panel for EditorPanel {
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        (!AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .git_panel_open)
            .then(|| vec![crate::ui::workspace::right_sidebar_toggle()])
    }
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        (!self.alone).then(|| self.label(cx))
    }
    fn title(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = Hover::new("editor-close", window, cx);
        h_flex().gap_2().child(self.label(cx)).child(
            Button::new("editor-close")
                .small()
                .cursor_pointer()
                .label("×")
                .tooltip("Close file")
                .on_click(cx.listener(|this, _, window, cx| this.request_close(window, cx)))
                .smooth_hover(ButtonLook::Ghost, &hover, cx),
        )
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}
fn file_changes(git: &Entity<Git>, document: &Entity<Document>, cx: &App) -> FileChanges {
    let document = document.read(cx);
    git.read(cx).file_changes(&document.target, &document.path)
}

impl Render for EditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let [save_hover, changes_hover, conflict_hover, sources_hover] =
            ["file-save", "file-changes", "file-conflict", "file-sources"]
                .map(|id| Hover::new(id, window, cx));
        let document = self.document.read(cx);
        let editor = document.editor.clone();
        let header = format!(
            "{} · {}",
            document.path.display(),
            document.target.path.display()
        );
        let (loading, saving, dirty, ready) = (
            document.loading,
            document.saving,
            document.dirty,
            document.base.is_some() && !document.path_busy,
        );
        let error = document
            .error
            .as_ref()
            .map(|error| error.message().to_owned());
        let conflict = document.conflict.is_some();
        let mut body = v_flex()
            .key_context("EditorPanel")
            .size_full()
            .min_w_0()
            .min_h_0()
            .on_action(cx.listener(|this, _: &SaveFile, window, cx| {
                this.document.update(cx, |d, cx| {
                    d.save(None, window, cx);
                });
            }))
            .on_action(
                cx.listener(|this, _: &CloseFile, window, cx| this.request_close(window, cx)),
            )
            .child(
                h_flex()
                    .p_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(header))
                    .child(
                        Button::new("file-changes")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::GitCompareArrows)
                            .tooltip("Show changes")
                            .disabled(self.changes == FileChanges::Clean)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_changes(false, window, cx)
                            }))
                            .smooth_hover(ButtonLook::Ghost, &changes_hover, cx),
                    )
                    .child(
                        Button::new("file-sources")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::ChevronDown)
                            .tooltip("Choose unstaged or staged changes")
                            .disabled(!matches!(
                                self.changes,
                                FileChanges::StagedAndUnstaged | FileChanges::Unknown
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_changes(true, window, cx)
                            }))
                            .smooth_hover(ButtonLook::Ghost, &sources_hover, cx),
                    )
                    .child(
                        Button::new("file-save")
                            .small()
                            .cursor_pointer()
                            .icon(Lucide::Save)
                            .tooltip("Save")
                            .loading(saving)
                            .disabled(!ready || !dirty || saving || conflict)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.document.update(cx, |d, cx| {
                                    d.save(None, window, cx);
                                });
                            }))
                            .smooth_hover(ButtonLook::Ghost, &save_hover, cx),
                    ),
            );
        if dirty {
            body = body.child(
                div()
                    .px_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Unsaved buffer · Git diff shows the disk version"),
            );
        }
        if let Some(error) = error {
            body = body.child(div().p_2().text_color(cx.theme().danger).child(error));
        }
        if conflict {
            body = body.child(
                Button::new("file-conflict")
                    .cursor_pointer()
                    .label("File changed on disk · Resolve conflict")
                    .on_click(cx.listener(|this, _, window, cx| this.resolve_conflict(window, cx)))
                    .smooth_hover(ButtonLook::Outline, &conflict_hover, cx),
            );
        }
        if loading && !ready {
            body = body.child(gpui_kit::component::spinner::Spinner::new());
        }
        body.child(
            Editor::new(&editor)
                .readonly(!ready)
                .bordered(false)
                .size_full(),
        )
    }
}
