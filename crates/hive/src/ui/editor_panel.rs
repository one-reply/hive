use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use crate::{
    app::AppState,
    state::documents::{Content, Document, OpenFile, SvgMode},
    state::git::{FileChanges, Git},
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{BasePanel, DockArea, DockEvent, Panel, PanelEvent, TabGroup};
use gpui_kit::component::input::{Editor, EditorState, RopeExt as _};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

mod actions;
mod blame;
pub(crate) mod close;
mod image_preview;
mod render;
mod suggestions;

actions!(editor, [SaveFile, CloseFile, OpenFileSearch, SearchInFiles]);

pub(crate) struct EditorPanel {
    pub document: Entity<Document>,
    area: WeakEntity<DockArea>,
    pub group: Option<WeakEntity<TabGroup>>,
    alone: bool,
    pub restoring: bool,
    last_position: (u32, u32, Point<Pixels>),
    image_focus: FocusHandle,
    changes: FileChanges,
    blame_card: blame::BlameCard,
    blame_fade: blame::LabelFade,
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
                    // A commit or checkout changes what the lines are blamed on.
                    this.document.update(cx, |d, cx| d.schedule_blame(cx));
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
                    this.close_blame_card(cx);
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
            image_focus: cx.focus_handle(),
            changes,
            blame_card: blame::BlameCard::default(),
            blame_fade: blame::LabelFade::default(),
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
        if self.document.read(cx).content.shows_image() {
            return self.image_focus.clone();
        }
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
