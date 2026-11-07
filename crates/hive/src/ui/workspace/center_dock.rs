//! The center dock's appearance: gpui-component's `DockSkin` for everything except the tab bar,
//! which Hive draws itself. The library's bar always ends in an "…" menu whose only item is
//! "Zoom In", and the center has no side docks to zoom away from, so our bar has no menu.
//! Owning the bar also lets us place things (status badges, "+") where the library cannot.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::ResizeHandleContext;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::dock::{
    AnyDrag, BasePanelView, DockArea, DockAreaRenderer, DockContext, DockSkin, DragPanel,
    DropIndicator, NodeId, PanelHandle, PanelId, PanelState, TabGroupContext, TabGroupRenderer,
};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, IconName, Selectable as _, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// The size the drag preview occupies (`w_24` by the tab bar height), reported to the dock so
/// the drop placeholder knows where to fly in from.
const DRAG_PREVIEW_SIZE: Size<Pixels> = size(px(96.), px(30.));
/// The 20px close button has a 12px icon: pulling it 8px left keeps the label-to-icon gap at 12px.
const CLOSE_BUTTON_PULL: Pixels = px(8.);
/// Width of the left border gpui-component draws on every `Tab`, the first one included.
const FIRST_TAB_BORDER: Pixels = px(1.);

/// `DockSkin` with Hive's own tab bar. Build it with [`CenterDock::dock_area`].
pub(super) struct CenterDock {
    skin: Rc<DockSkin>,
    tab_inset: Rc<Cell<Pixels>>,
}

impl CenterDock {
    /// A center `DockArea` wearing this appearance. `tab_inset` is the space the tab bar
    /// leaves free at its left end; the workspace writes it when the title bar sits there.
    pub(super) fn dock_area(
        id: impl Into<SharedString>,
        tab_inset: Rc<Cell<Pixels>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<DockArea> {
        cx.new(|cx| {
            let skin = DockSkin::new(cx);
            DockArea::new(id, None, window, cx).with_renderer(Rc::new(Self { skin, tab_inset }))
        })
    }
}

impl DockAreaRenderer for CenterDock {
    fn frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.frame(window, cx)
    }

    fn split_frame(
        &self,
        node: NodeId,
        axis: Axis,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        self.skin.split_frame(node, axis, window, cx)
    }

    fn center_frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.center_frame(window, cx)
    }

    fn render_split_handle(
        &self,
        handle: &ResizeHandleContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.skin.render_split_handle(handle, window, cx)
    }

    fn render_dock(
        &self,
        dock: &DockContext,
        content: AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.skin.render_dock(dock, content, window, cx)
    }

    fn build_placeholder(
        &self,
        state: &PanelState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<dyn BasePanelView>> {
        self.skin.build_placeholder(state, window, cx)
    }

    fn tab_group_renderer(&self) -> Rc<dyn TabGroupRenderer> {
        Rc::new(CenterTabs {
            skin: self.skin.tab_group_renderer(),
            scroll_handle: ScrollHandle::default(),
            last_active_ix: Rc::new(Cell::new(None)),
            tab_inset: self.tab_inset.clone(),
        })
    }
}

/// One tab group's appearance: the library's for the frame, content and drop indicator, ours
/// for the tab bar.
struct CenterTabs {
    skin: Rc<dyn TabGroupRenderer>,
    scroll_handle: ScrollHandle,
    /// The tab the last frame displayed, so a selection made elsewhere (⌘T, the sidebar)
    /// scrolls the new tab into view. A clicked tab reveals itself, so a click records its
    /// index first and skips the snap.
    last_active_ix: Rc<Cell<Option<usize>>>,
    /// Width kept free before the first tab; see [`CenterDock::dock_area`].
    tab_inset: Rc<Cell<Pixels>>,
}

impl TabGroupRenderer for CenterTabs {
    fn frame(&self, group: &TabGroupContext, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        self.skin.frame(group, window, cx)
    }

    fn content_frame(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        self.skin.content_frame(group, window, cx)
    }

    fn render_tab_bar(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let visible: Vec<usize> = group
            .panels()
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.visible(cx))
            .map(|(ix, _)| ix)
            .collect();
        if visible.is_empty() {
            return Empty.into_any_element();
        }

        let active_ix = group.active_ix();
        if self.last_active_ix.replace(Some(active_ix)) != Some(active_ix)
            && let Some(visible_ix) = visible.iter().position(|ix| *ix == active_ix)
        {
            self.scroll_handle.scroll_to_item(visible_ix);
        }

        // The displayed panel, not the stored index: a hidden displayed tab falls back to the
        // first visible one.
        let displayed = group.active_panel().map(|panel| panel.panel_id(cx));
        let displayed_ix = displayed.and_then(|displayed| {
            group
                .panels()
                .iter()
                .position(|panel| panel.panel_id(cx) == displayed)
        });
        let droppable = group.is_droppable();

        let inset = self.tab_inset.get();
        TabBar::new("tab-bar")
            .track_scroll(&self.scroll_handle)
            .map(|bar| {
                if inset > px(0.) {
                    bar.prefix(div().w(inset).flex_none())
                } else {
                    // gpui-component keeps the first tab's left border even without a prefix,
                    // doubling the divider on the bar's left. Tucking the bar under that divider
                    // hides the border; the workspace title bar repaints the divider on top.
                    bar.ml(-FIRST_TAB_BORDER)
                }
            })
            .children(visible.into_iter().map(|ix| {
                self.render_tab(group, ix, displayed_ix == Some(ix), droppable, window, cx)
            }))
            .last_empty_space(render_empty_space(group, droppable))
            .suffix(render_suffix(group, window, cx))
            .into_any_element()
    }

    fn render_active_panel(
        &self,
        panel: AnyView,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.skin.render_active_panel(panel, group, window, cx)
    }

    fn render_drop_indicator(
        &self,
        indicator: DropIndicator,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.skin.render_drop_indicator(indicator, window, cx)
    }

    fn render_empty(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.skin.render_empty(group, window, cx)
    }
}

impl CenterTabs {
    fn render_tab(
        &self,
        group: &TabGroupContext,
        ix: usize,
        selected: bool,
        droppable: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Tab {
        let panel = &group.panels()[ix];
        let panel_id = panel.panel_id(cx);
        let accessible_name = PanelHandle::of(panel)
            .and_then(|handle| handle.tab_name(cx))
            .or_else(|| {
                panel
                    .view()
                    .downcast::<crate::terminal::panel::TerminalPanel>()
                    .ok()
                    .map(|p| p.read(cx).tab_title(cx).into())
            })
            .or_else(|| {
                panel
                    .view()
                    .downcast::<crate::ui::diff_panel::DiffPanel>()
                    .ok()
                    .map(|p| p.read(cx).label(cx))
            })
            .unwrap_or_else(|| "Workspace tab".into());
        let label = match PanelHandle::of(panel).and_then(|handle| handle.tab_name(cx)) {
            Some(name) => name.into_any_element(),
            None => panel_title(panel, window, cx),
        };
        let drag = group
            .is_draggable()
            .then(|| group.drag_panel(ix, cx))
            .flatten();

        let label = if let Ok(editor) = panel
            .view()
            .downcast::<crate::ui::editor_panel::EditorPanel>()
        {
            let document = editor.read(cx).document.read(cx);
            let tooltip = format!(
                "{} · {}",
                document.path.display(),
                document.target.path.display()
            );
            div()
                .id(("editor-tab-label", ix))
                .child(label)
                .when(!document.pinned, |label| label.italic())
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
                .into_any_element()
        } else {
            label
        };

        Tab::new()
            .aria_label(accessible_name)
            .child(label)
            // Mirrors `TabGroup::close_panel`, which refuses the last tab: that one closes
            // itself from inside `Panel::title` through `DockArea::remove_panel`.
            .when(group.is_panel_closable(panel_id, cx), |this| {
                this.suffix(render_close_button(group, ix, panel_id, cx))
            })
            .selected(selected)
            .on_click({
                let group = group.clone();
                let last_active_ix = self.last_active_ix.clone();
                move |event, window, cx| {
                    last_active_ix.set(Some(ix));
                    if event.click_count() >= 2
                        && let Some(editor) = group.panels().get(ix).and_then(|p| {
                            p.view()
                                .downcast::<crate::ui::editor_panel::EditorPanel>()
                                .ok()
                        })
                    {
                        editor.read(cx).document.clone().update(cx, |d, cx| {
                            d.pinned = true;
                            cx.notify();
                        });
                    }
                    group.select_tab(ix, window, cx);
                }
            })
            .when_some(drag, |this, drag| {
                this.on_drag(drag, {
                    let panel = panel.clone();
                    move |drag, offset, _, cx| {
                        cx.stop_propagation();
                        drag.set_drag_offset(offset);
                        drag.set_preview_size(DRAG_PREVIEW_SIZE);
                        cx.new(|_| TabDragPreview {
                            panel: panel.clone(),
                        })
                    }
                })
            })
            .when(droppable, |this| {
                this.drag_over::<DragPanel>(|this, _, _, cx| drop_edge(this, cx))
                    .on_drop({
                        let group = group.clone();
                        move |drag: &DragPanel, window, cx| {
                            group.drop_panel(drag.clone(), Some(ix), true, window, cx);
                        }
                    })
                    .drag_over::<AnyDrag>(|this, _, _, cx| drop_edge(this, cx))
                    .on_drop({
                        let group = group.clone();
                        move |item: &AnyDrag, window, cx| {
                            group.drop_item(item.clone(), None, window, cx);
                        }
                    })
            })
    }
}

/// The highlight of a tab a panel is about to be dropped before.
fn drop_edge(style: StyleRefinement, cx: &mut App) -> StyleRefinement {
    style
        .rounded_l_none()
        .border_l_2()
        .border_r_0()
        .border_color(cx.theme().drag_border)
}

fn render_close_button(
    group: &TabGroupContext,
    ix: usize,
    panel_id: PanelId,
    cx: &mut App,
) -> Button {
    Button::new(("close-tab", ix))
        .accessibility_label("Close tab")
        .tooltip("Close tab")
        .cursor_pointer()
        .icon(IconName::Close)
        .xsmall()
        // The ghost hover matches the inactive tab background; use a stronger surface while
        // keeping a transparent idle background.
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(cx.theme().secondary_foreground)
                .hover(*cx.theme().tokens.secondary_hover)
                .active(*cx.theme().tokens.secondary_active),
        )
        .ml(-CLOSE_BUTTON_PULL)
        .mr_2()
        .tab_stop(false)
        .on_click({
            let group = group.clone();
            move |_, window, cx| {
                // Close by id without also selecting the tab underneath.
                cx.stop_propagation();
                let panel = group
                    .panels()
                    .iter()
                    .find(|panel| panel.panel_id(cx) == panel_id)
                    .map(|panel| panel.view());
                if let Some(editor) = panel
                    .clone()
                    .and_then(|view| view.downcast::<crate::ui::editor_panel::EditorPanel>().ok())
                {
                    editor.update(cx, |panel, cx| panel.request_close(window, cx));
                    return;
                }
                if let Some(terminal) = panel.and_then(|view| {
                    view.downcast::<crate::terminal::panel::TerminalPanel>()
                        .ok()
                }) {
                    terminal.update(cx, |panel, cx| panel.request_close_tab(window, cx));
                    return;
                }
                group.close(panel_id, window, cx);
            }
        })
}

/// Room past the last tab so a panel can be dropped at the end.
fn render_empty_space(group: &TabGroupContext, droppable: bool) -> impl IntoElement {
    let tabs_count = group.panels().len();
    div()
        .id("tab-bar-empty-space")
        .h_full()
        .flex_grow_1()
        .min_w_16()
        .when(droppable, |this| {
            this.drag_over::<DragPanel>(|this, _, _, cx| this.bg(cx.theme().tokens.drop_target))
                .on_drop({
                    let group = group.clone();
                    let node = group.node();
                    move |drag: &DragPanel, window, cx| {
                        // A panel dropped past its own last tab lands in the final slot; one
                        // from elsewhere is appended in the background.
                        let ix = (drag.source() == node).then(|| tabs_count - 1);
                        group.drop_panel(drag.clone(), ix, false, window, cx);
                    }
                })
                .drag_over::<AnyDrag>(|this, _, _, cx| this.bg(cx.theme().tokens.drop_target))
                .on_drop({
                    let group = group.clone();
                    move |item: &AnyDrag, window, cx| {
                        group.drop_item(item.clone(), None, window, cx);
                    }
                })
        })
}

/// The displayed panel's `title_suffix` and `toolbar_buttons`, pinned to the bar's right edge.
fn render_suffix(group: &TabGroupContext, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let handle = group.active_panel().and_then(PanelHandle::of);
    let suffix = handle.and_then(|handle| handle.title_suffix(window, cx));
    let buttons = handle
        .and_then(|handle| handle.toolbar_buttons(window, cx))
        .unwrap_or_default();

    h_flex()
        .items_center()
        .top_0()
        .right_0()
        .border_l_1()
        .border_b_1()
        .h_full()
        .border_color(cx.theme().border)
        .bg(cx.theme().tokens.tab_bar)
        .px_2()
        .gap_1()
        .children(suffix)
        .child(
            h_flex()
                .gap_1()
                // The strip above the center drags the window; the buttons must win.
                .occlude()
                .children(
                    buttons
                        .into_iter()
                        .map(|button| button.xsmall().ghost().tab_stop(false)),
                ),
        )
}

/// A panel's title, or its registered name when it was added without `panel_handle`.
fn panel_title(panel: &Arc<dyn BasePanelView>, window: &mut Window, cx: &mut App) -> AnyElement {
    match PanelHandle::of(panel) {
        Some(handle) => handle.title(window, cx),
        None => SharedString::from(panel.panel_name(cx)).into_any_element(),
    }
}

/// The preview that follows the cursor while a tab is dragged.
struct TabDragPreview {
    panel: Arc<dyn BasePanelView>,
}

impl Render for TabDragPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("drag-panel")
            .cursor_grab()
            .py_1()
            .px_3()
            .w_24()
            .overflow_hidden()
            .whitespace_nowrap()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .text_color(cx.theme().tab_foreground)
            .bg(cx.theme().tokens.tab_active)
            .opacity(0.75)
            .child(panel_title(&self.panel, window, cx))
    }
}
