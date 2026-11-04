//! A dock tab holding a tree of terminal panes (Orca-style tiles), with "+" (new tab) and
//! "split" toolbar buttons and ⌘D / ⌘⇧D / ⌘W for splitting and closing the active pane.
//! All three are reassignable, so their bindings live in `ui::keymap`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{BasePanel, DockArea, Panel, PanelEvent, TabGroup};
use gpui_kit::component::resizable::{h_resizable, v_resizable};
use gpui_kit::component::{IconName, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::pane_tree::PaneTree;
use super::view::{TerminalView, TerminalViewEvent};
use crate::app::AppState;
use crate::ui::keymap;
use crate::ui::workspace::{NewTerminal, right_sidebar_toggle};

actions!(terminal_panel, [SplitRight, SplitDown, ClosePane]);

pub struct TerminalPanel {
    panes: PaneTree<Entity<TerminalView>>,
    /// The pane that last had focus: it is split and closed by the shortcuts and names the tab.
    active: Entity<TerminalView>,
    /// Per-pane subscriptions, dropped together with the pane.
    pane_subscriptions: HashMap<EntityId, Vec<Subscription>>,
    area: WeakEntity<DockArea>,
    group: Option<WeakEntity<TabGroup>>,
    alone: bool,
}

impl TerminalPanel {
    /// Open a panel immediately; its session starts the shell in the background.
    pub fn open(
        cwd: Option<&Path>,
        area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let view = cx.new(|cx| TerminalView::new(cwd, window, cx));
            let mut this = Self {
                panes: PaneTree::Leaf(view.clone()),
                active: view.clone(),
                pane_subscriptions: HashMap::new(),
                area,
                group: None,
                alone: false,
            };
            this.watch_pane(&view, window, cx);
            this
        })
    }

    fn watch_pane(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus_handle = view.read(cx).focus_handle().clone();
        let weak_view = view.downgrade();
        let subscriptions = vec![
            // The tab group renders this panel through `AnyView::cached`, which only re-renders when the
            // panel itself is dirty; forward the view's notifications so grid updates reach the screen.
            cx.observe(view, |_, _, cx| cx.notify()),
            cx.subscribe_in(view, window, |this, view, event, window, cx| match event {
                TerminalViewEvent::TitleChanged => this.notify_group(cx),
                TerminalViewEvent::Exited => this.close_pane(view, window, cx),
            }),
            cx.on_focus(&focus_handle, window, move |this, _, cx| {
                if let Some(view) = weak_view.upgrade() {
                    this.active = view;
                    this.select_workspace(cx);
                    this.notify_group(cx);
                }
            }),
        ];
        self.pane_subscriptions
            .insert(view.entity_id(), subscriptions);
    }

    pub fn cwd(&self, cx: &App) -> Option<PathBuf> {
        self.active.read(cx).cwd().map(Path::to_path_buf)
    }

    /// The tab bar reads titles on render; make sure it renders.
    fn notify_group(&self, cx: &mut Context<Self>) {
        if let Some(group) = self.group.as_ref().and_then(|g| g.upgrade()) {
            group.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.active.read(cx).focus_handle().clone();
        handle.focus(window, cx);
    }

    /// Split the active pane: a new shell in the same cwd lands after it along `axis`.
    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.cwd(cx);
        let view = cx.new(|cx| TerminalView::new(cwd.as_deref(), window, cx));
        self.watch_pane(&view, window, cx);
        self.panes.split(&self.active, view.clone(), axis);
        self.active = view;
        self.focus_active(window, cx);
        self.notify_group(cx);
    }

    /// Close one pane; closing the last one closes the tab.
    fn close_pane(
        &mut self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.panes, PaneTree::Leaf(_)) {
            self.close(window, cx);
            return;
        }
        let Some(neighbor) = self.panes.remove(view) else {
            return;
        };
        self.pane_subscriptions.remove(&view.entity_id());
        if self.active == *view {
            self.active = neighbor;
            self.focus_active(window, cx);
        }
        self.notify_group(cx);
    }

    /// Remove this tab. Goes through the dock, not the tab group, so the last
    /// terminal can be closed too (the group refuses to empty itself).
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(area) = self.area.upgrade() else {
            return;
        };
        let me = cx.entity();
        // We are inside our own `update` (click handler / event subscription); the dock
        // reads the panel while removing it, so run the removal after this update ends.
        window.defer(cx, move |window, cx| {
            area.update(cx, |area, cx| area.remove_panel(me, window, cx));
        });
    }

    /// The workspace tells us when we are the only terminal in the dock: the tab bar then
    /// hides its own close button, so we draw one. (Reading the tab group here is not an
    /// option: `title_suffix` runs inside the group's render, while it is leased.)
    pub fn set_alone(&mut self, alone: bool, cx: &mut Context<Self>) {
        if self.alone != alone {
            self.alone = alone;
            self.notify_group(cx);
        }
    }

    fn select_workspace(&self, cx: &mut Context<Self>) {
        let Some(path) = self.cwd(cx) else {
            return;
        };
        let git = AppState::get(cx).git.clone();
        cx.defer(move |cx| git.update(cx, |g, cx| g.activate_path(&path, cx)));
    }

    /// Let Workspace own new tabs so close tracking includes every terminal.
    fn open_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.dispatch_action(Box::new(NewTerminal), cx);
    }
}

/// Splits become resizable groups. The group id encodes the node's structure (axis, first pane,
/// child count), so its keyed size state resets to equal shares whenever the structure changes
/// and survives drags otherwise.
fn render_pane(pane: &PaneTree<Entity<TerminalView>>) -> AnyElement {
    match pane {
        PaneTree::Leaf(view) => div().size_full().child(view.clone()).into_any_element(),
        PaneTree::Split { axis, children } => {
            let id = SharedString::from(format!(
                "pane-split-{axis:?}-{}-{}",
                pane.first_leaf().entity_id(),
                children.len()
            ));
            let group = match axis {
                Axis::Horizontal => h_resizable(id),
                Axis::Vertical => v_resizable(id),
            };
            group
                .children(children.iter().map(render_pane))
                .into_any_element()
        }
    }
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.active.read(cx).focus_handle().clone()
    }
}

impl BasePanel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "TerminalPanel"
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            self.select_workspace(cx);
            self.focus_active(window, cx);
        }
    }

    fn on_added_to(&mut self, group: WeakEntity<TabGroup>, _: &mut Window, _: &mut Context<Self>) {
        self.group = Some(group);
    }
}

impl Panel for TerminalPanel {
    /// A plain name while the tab bar draws its own close button; `None` when we are the
    /// last tab, so the bar renders [`Panel::title`] with our own "×" inside the tab instead.
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        (!self.alone).then(|| self.active.read(cx).title(cx).into())
    }

    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.active.read(cx);
        let title = view.title(cx);
        h_flex()
            .gap_2()
            .items_center()
            .when(view.exited(cx), |d| d.opacity(0.6))
            .child(title)
            .when(self.alone, |this| {
                this.child(
                    Button::new("terminal-close")
                        .cursor_pointer()
                        .icon(IconName::Close)
                        .xsmall()
                        .ghost()
                        .tooltip("Close terminal")
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                )
            })
    }

    /// "+" and the split buttons; the right-sidebar toggle joins them while that sidebar is
    /// closed, so it keeps its place in the window's top-right corner.
    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        let mut buttons = vec![
            Button::new("terminal-new")
                .cursor_pointer()
                .icon(IconName::Plus)
                .tooltip("New terminal tab")
                .on_click(cx.listener(|this, _, window, cx| this.open_tab(window, cx))),
            Button::new("terminal-split-right")
                .cursor_pointer()
                .icon(Lucide::SquareSplitHorizontal)
                .tooltip_with_action("Split right", &SplitRight, Some(keymap::TERMINAL_CONTEXT))
                .on_click(
                    cx.listener(|this, _, window, cx| this.split(Axis::Horizontal, window, cx)),
                ),
            Button::new("terminal-split-down")
                .cursor_pointer()
                .icon(Lucide::SquareSplitVertical)
                .tooltip_with_action("Split down", &SplitDown, Some(keymap::TERMINAL_CONTEXT))
                .on_click(
                    cx.listener(|this, _, window, cx| this.split(Axis::Vertical, window, cx)),
                ),
        ];
        if !AppState::get(cx).git.read(cx).panel_visible {
            buttons.push(right_sidebar_toggle());
        }
        Some(buttons)
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context(keymap::TERMINAL_CONTEXT)
            .on_action(cx.listener(|this, _: &SplitRight, window, cx| {
                this.split(Axis::Horizontal, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SplitDown, window, cx| {
                    this.split(Axis::Vertical, window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ClosePane, window, cx| {
                let active = this.active.clone();
                this.close_pane(&active, window, cx)
            }))
            .child(render_pane(&self.panes))
    }
}
