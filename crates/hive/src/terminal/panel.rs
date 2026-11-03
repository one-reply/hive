//! A dock tab holding one [`TerminalView`], with "+" (new tab) and "split" toolbar buttons.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{
    BasePanel, DockArea, DockPlacement, InsertTarget, Panel, PanelControl, PanelEvent, PanelId, TabGroup, panel_handle,
};
use gpui_kit::component::{IconName, Placement, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::terminal::{Terminal, TerminalSize};

use super::view::{TerminalView, TerminalViewEvent};

pub struct TerminalPanel {
    view: Entity<TerminalView>,
    area: WeakEntity<DockArea>,
    group: Option<WeakEntity<TabGroup>>,
    alone: bool,
    _subscriptions: Vec<Subscription>,
}

impl TerminalPanel {
    /// Spawn a shell in `cwd` and wrap it in a panel. `None` when the PTY could not be opened.
    pub fn open(cwd: Option<&Path>, area: WeakEntity<DockArea>, window: &mut Window, cx: &mut App) -> Option<Entity<Self>> {
        let terminal = match Terminal::spawn(cwd, None, TerminalSize::new(80, 24)) {
            Ok(terminal) => terminal,
            Err(err) => {
                tracing::error!(?cwd, %err, "failed to spawn terminal");
                return None;
            }
        };
        Some(cx.new(|cx| {
            let view = cx.new(|cx| TerminalView::new(terminal, cwd, window, cx));
            Self::new(view, area, window, cx)
        }))
    }

    fn new(view: Entity<TerminalView>, area: WeakEntity<DockArea>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            // The tab group renders this panel through `AnyView::cached`, which only re-renders when the
            // panel itself is dirty; forward the view's notifications so grid updates reach the screen.
            cx.observe(&view, |_, _, cx| cx.notify()),
            cx.subscribe_in(&view, window, |this, _, event, window, cx| match event {
                TerminalViewEvent::TitleChanged => this.notify_group(cx),
                TerminalViewEvent::Exited => this.close(window, cx),
            }),
        ];
        Self { view, area, group: None, alone: false, _subscriptions: subscriptions }
    }

    pub fn cwd(&self, cx: &App) -> Option<PathBuf> {
        self.view.read(cx).cwd().map(Path::to_path_buf)
    }

    /// The tab bar reads titles on render; make sure it renders.
    fn notify_group(&self, cx: &mut Context<Self>) {
        if let Some(group) = self.group.as_ref().and_then(|g| g.upgrade()) {
            group.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// Remove this tab. Goes through the dock, not the tab group, so the last
    /// terminal can be closed too (the group refuses to empty itself).
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(area) = self.area.upgrade() else { return };
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

    /// Open another terminal in the same cwd: as a new tab of this group, or split beside it.
    pub fn open_sibling(&mut self, split: Option<Placement>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(area) = self.area.upgrade() else { return };
        let cwd = self.cwd(cx);
        let Some(panel) = Self::open(cwd.as_deref(), self.area.clone(), window, cx) else { return };
        let id: PanelId = panel.entity_id().into();
        let node = self.group.as_ref().and_then(|g| g.upgrade()).map(|g| g.read(cx).node());
        area.update(cx, |area, cx| {
            area.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx);
            if let Some(node) = node {
                let target = match split {
                    Some(placement) => InsertTarget::Split { node, placement, size: None },
                    None => InsertTarget::Tabs { node, ix: None, activate: true },
                };
                area.move_panel(id, target, window, cx);
            }
        });
    }
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).focus_handle().clone()
    }
}

impl BasePanel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "TerminalPanel"
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            let handle = self.view.read(cx).focus_handle().clone();
            handle.focus(window, cx);
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
        (!self.alone).then(|| self.view.read(cx).title().into())
    }

    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.view.read(cx);
        let title = view.title();
        h_flex()
            .gap_2()
            .items_center()
            .when(view.exited(), |d| d.opacity(0.6))
            .child(title)
            .when(self.alone, |this| {
                this.child(
                    Button::new("terminal-close")
                        .icon(IconName::Close)
                        .xsmall()
                        .ghost()
                        .tooltip("Close terminal")
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                )
            })
    }

    fn toolbar_buttons(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("terminal-new")
                .icon(IconName::Plus)
                .tooltip("New terminal tab")
                .on_click(cx.listener(|this, _, window, cx| this.open_sibling(None, window, cx))),
            Button::new("terminal-split")
                .icon(Lucide::SquareSplitHorizontal)
                .tooltip("Split right")
                .on_click(cx.listener(|this, _, window, cx| this.open_sibling(Some(Placement::Right), window, cx))),
        ])
    }

    fn zoom_control(&self, _: &App) -> Option<PanelControl> {
        Some(PanelControl::Menu)
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.view.clone())
    }
}
