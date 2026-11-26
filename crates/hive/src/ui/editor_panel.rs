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
use gpui_kit::component::input::{Editor, EditorState, GutterDecorationCollection, RopeExt as _};
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

mod actions;
mod agent_context;
mod blame;
pub(crate) mod close;
mod editing;
mod image_preview;
mod line;
pub(crate) use line::goto_line;
pub(crate) mod import_menu;
mod language_assist;
mod language_tools;
mod navigation;
mod packages;
mod peek;
mod rename;
mod render;
pub(crate) mod source_preview;
mod sticky;
pub(crate) mod suggestions;
pub(super) mod workspace_symbols;

actions!(
    editor,
    [
        SelectNextOccurrence,
        ToggleComment,
        MoveLinesUp,
        MoveLinesDown,
        DuplicateLinesUp,
        DuplicateLinesDown,
        SaveFile,
        CloseFile,
        OpenFileSearch,
        SearchInFiles,
        GoToSymbol,
        GoToLine,
        SwitchRecentTab,
        ReopenClosedFile,
        LastEditLocation,
        NavigateBack,
        NavigateForward,
        FormatDocument,
        SelectionToAgent,
        PeekDefinition,
        WorkspaceSymbols,
        NextChange,
        PreviousChange,
        RenameSymbol,
        FindReferences,
        ShowProblems,
        NextDiagnostic,
        PreviousDiagnostic,
        PackageVersions,
        ShowSignature
    ]
);

/// The key context of a file tab, where its keymap bindings apply.
pub(crate) const EDITOR_PANEL_CONTEXT: &str = "EditorPanel";

/// What the saved workspace keeps of a file tab besides its cursor.
#[derive(Debug, PartialEq)]
struct SavedTab {
    pinned: bool,
    path: std::path::PathBuf,
}

impl SavedTab {
    /// Records the tab as it is now and says whether that differs from what was recorded.
    fn replace(&mut self, pinned: bool, path: &std::path::Path) -> bool {
        if self.pinned == pinned && self.path == path {
            return false;
        }
        *self = Self {
            pinned,
            path: path.to_owned(),
        };
        true
    }
}

pub(crate) struct EditorPanel {
    pub document: Entity<Document>,
    _gutter: Entity<crate::state::documents::Gutter>,
    gutter_marks: GutterDecorationCollection,
    saved_tab: SavedTab,
    area: WeakEntity<DockArea>,
    pub group: Option<WeakEntity<TabGroup>>,
    alone: bool,
    pub restoring: bool,
    last_position: (u32, u32, Point<Pixels>),
    image_focus: FocusHandle,
    changes: FileChanges,
    blame_card: blame::BlameCard,
    blame_fade: blame::LabelFade,
    peek: Option<peek::Peek>,
    _peek: Option<Task<()>>,
    import_scroll: ScrollHandle,
    signature: Option<crate::state::documents::CallSignature>,
    _signature: Option<Task<()>>,
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
        let gutter =
            cx.new(|cx| crate::state::documents::Gutter::new(document.clone(), git.clone(), cx));
        let markers = editor.update(cx, |editor, cx| {
            editor.create_gutter_decorations_collection(Vec::new(), cx)
        });
        let image_focus = cx.focus_handle();
        let mut subscriptions = vec![
            cx.on_release(|this: &mut Self, cx| this.gutter_marks.dispose(cx)),
            cx.observe(&gutter, |this, gutter, cx| {
                use gpui_kit::component::input::GutterDecoration;
                use hive_core::git::gutter::Kind;
                let marks = gutter
                    .read(cx)
                    .marks
                    .iter()
                    .map(|m| {
                        let color = match m.kind {
                            Kind::Added => cx.theme().success,
                            Kind::Deleted => cx.theme().danger,
                            Kind::Modified => cx.theme().blue,
                        };
                        GutterDecoration::new(m.range.clone()).with_color(color)
                    })
                    .collect();
                this.gutter_marks.set(marks, cx);
            }),
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
                let document = document.read(cx);
                // A document notifies on every keystroke; only what the saved workspace keeps
                // of this tab is worth a `state.json` write. The cursor is saved after a pause.
                if this.saved_tab.replace(document.pinned, &document.path)
                    && let Some(area) = this.area.upgrade()
                {
                    cx.defer(move |cx| area.update(cx, |_, cx| cx.emit(DockEvent::LayoutChanged)));
                }
                cx.notify();
            }),
            cx.observe_in(&editor, window, |this, editor, window, cx| {
                let editor = editor.read(cx);
                let cursor = editor.cursor_position();
                let position = (cursor.line, cursor.character, editor.scroll_offset());
                let caret = editor.cursor();
                if this.last_position != position {
                    if (this.last_position.0, this.last_position.1)
                        != (cursor.line, cursor.character)
                    {
                        let offset = editor.cursor();
                        let trigger = offset
                            .checked_sub(1)
                            .and_then(|start| {
                                editor
                                    .text()
                                    .is_char_boundary(start)
                                    .then(|| editor.text().slice(start..offset).to_string())
                            })
                            .is_some_and(|last| matches!(last.as_str(), "(" | ","));
                        let refresh =
                            trigger || this.signature.is_some() || this._signature.is_some();
                        this.signature = None;
                        this._signature = None;
                        if refresh && editor.focus_handle(cx).is_focused(window) {
                            this.signature_help(false, window, cx);
                        }
                    }
                    if this
                        .document
                        .read(cx)
                        .import_menu
                        .as_ref()
                        .is_some_and(|menu| menu.cursor != caret)
                    {
                        this.document.update(cx, |doc, cx| {
                            doc.import_menu = None;
                            cx.notify();
                        });
                    }
                    this.peek = None;
                    this._peek = None;
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
        for focus in [&editor.read(cx).focus_handle(cx), &image_focus] {
            subscriptions.push(cx.on_focus(focus, window, |this, _, cx| {
                if let Some(area) = this.area.upgrade() {
                    cx.defer(move |cx| area.update(cx, |_, cx| cx.emit(DockEvent::LayoutChanged)));
                }
            }));
        }
        let saved_tab = {
            let document = document.read(cx);
            SavedTab {
                pinned: document.pinned,
                path: document.path.clone(),
            }
        };
        Self {
            document,
            _gutter: gutter,
            gutter_marks: markers,
            saved_tab,
            area,
            group: None,
            alone: false,
            restoring: false,
            last_position: (0, 0, Point::default()),
            image_focus,
            changes,
            blame_card: blame::BlameCard::default(),
            blame_fade: blame::LabelFade::default(),
            peek: None,
            _peek: None,
            import_scroll: ScrollHandle::new(),
            signature: None,
            _signature: None,
            _position: None,
            _subscriptions: subscriptions,
        }
    }

    fn jump_change(&self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let editor = doc.editor.read(cx);
        if let Some(offset) = hive_core::git::gutter::destination(
            &self.gutter_marks.get_ranges(cx),
            editor.cursor(),
            forward,
        ) {
            let position = editor.text().offset_to_position(offset);
            self.document
                .update(cx, |doc, cx| doc.goto(position, window, cx));
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
                    let d = panel.read(cx).document.read(cx);
                    if !d.missing && d.base.is_some() {
                        let closed = d.at_cursor(cx);
                        AppState::get(cx)
                            .documents
                            .clone()
                            .update(cx, |documents, cx| {
                                documents.emit(
                                    crate::state::documents::DocumentEvent::Closed(closed),
                                    cx,
                                )
                            });
                    }
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

#[cfg(test)]
mod tests {
    use super::SavedTab;
    use std::path::Path;

    #[test]
    fn typing_leaves_the_saved_tab_unchanged_while_pinning_and_renaming_change_it() {
        let mut tab = SavedTab {
            pinned: false,
            path: "src/main.rs".into(),
        };
        assert!(!tab.replace(false, Path::new("src/main.rs")));
        assert!(tab.replace(true, Path::new("src/main.rs")));
        assert!(!tab.replace(true, Path::new("src/main.rs")));
        assert!(tab.replace(true, Path::new("src/app.rs")));
        assert_eq!(
            tab,
            SavedTab {
                pinned: true,
                path: "src/app.rs".into()
            }
        );
    }
}
