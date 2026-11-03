//! Main window layout: TitleBar | Sidebar + center dock | StatusBar.
//! The center is a `DockArea` of terminal tabs, one group per selected project/worktree.

use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::dock::{DockArea, DockPlacement, DockSkin, PanelId, PanelStyle, panel_handle};
use gpui_kit::component::{
    ActiveTheme as _, IconName, InteractiveElementExt as _, Sizable as _, TitleBar, h_flex, v_flex,
};
use gpui_kit::*;
use hive_core::settings::ThemeChoice;

use crate::app::AppState;
use crate::state::projects::Projects;
use crate::state::store::Store;
use crate::terminal::TerminalPanel;
use crate::ui::sidebar::Sidebar;
use crate::ui::status_bar::StatusBarView;
use crate::ui::theme::apply_theme;

actions!(workspace, [NewTerminal, ToggleSidebar]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-t", NewTerminal, Some("Workspace")),
        KeyBinding::new("cmd-shift-\\", ToggleSidebar, Some("Workspace")),
    ]);
}

pub struct Workspace {
    sidebar: Entity<Sidebar>,
    sidebar_open: bool,
    status_bar: Entity<StatusBarView>,
    projects: Entity<Projects>,
    dock: Entity<DockArea>,
    _skin: Rc<DockSkin>,
    /// Terminal tabs we opened; pruned when the dock no longer holds them.
    terminals: Vec<Entity<TerminalPanel>>,
    _observers: Vec<Subscription>,
    /// Left button went down on the tab-bar band; the next mouse move drags the window.
    should_move: bool,
}

/// Height of the band at the top of the center column that behaves like a title bar.
const TAB_BAR_HEIGHT: Pixels = px(34.);

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::get(cx);
        let (store, projects) = (app.store.clone(), app.projects.clone());

        apply_theme(store.read(cx).state.settings.theme, Some(window), cx);

        let (dock, skin) = DockSkin::dock_area("center", None, window, cx);
        skin.set_panel_style(PanelStyle::TabBar, cx);
        skin.set_close_button_visible(true, cx);
        skin.set_toggle_button_visible(false, cx);

        let observers = vec![
            cx.observe(&store, |_, store, cx| apply_theme(store.read(cx).state.settings.theme, None, cx)),
            cx.observe_in(&projects, window, |this, _, window, cx| this.sync_selection(window, cx)),
            // Tabs closed from the dock (× button, exited shell) must drop out of `terminals`
            // so the empty state with its "+" strip comes back.
            cx.observe(&dock, |this, dock, cx| {
                this.terminals.retain(|panel| dock.read(cx).panel(PanelId::from(panel.entity_id())).is_some());
                this.sync_alone(cx);
                cx.notify();
            }),
            cx.observe_window_appearance(window, {
                let store = store.clone();
                move |_, window, cx| {
                    if store.read(cx).state.settings.theme == ThemeChoice::System {
                        apply_theme(ThemeChoice::System, Some(window), cx);
                    }
                }
            }),
        ];

        let mut this = Self {
            sidebar: cx.new(Sidebar::new),
            sidebar_open: true,
            status_bar: cx.new(StatusBarView::new),
            projects,
            dock,
            _skin: skin,
            terminals: Vec::new(),
            _observers: observers,
            should_move: false,
        };
        this.sync_selection(window, cx);
        this
    }

    fn selected_path(&self, cx: &App) -> Option<PathBuf> {
        let (project, worktree) = self.projects.read(cx).selection()?;
        Some(worktree.map(|w| w.path.clone()).unwrap_or_else(|| project.path.clone()))
    }

    /// Show a terminal for the selected worktree: focus an existing tab for that path, or open one.
    fn sync_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path(cx) else {
            cx.notify();
            return;
        };
        let dock = self.dock.clone();
        self.terminals.retain(|panel| dock.read(cx).panel(PanelId::from(panel.entity_id())).is_some());

        let existing = self.terminals.iter().find(|panel| panel.read(cx).cwd(cx).as_deref() == Some(path.as_path())).cloned();
        match existing {
            Some(panel) => dock.update(cx, |dock, cx| dock.select_panel(PanelId::from(panel.entity_id()), window, cx)),
            None => self.open_terminal(Some(path), window, cx),
        }
        cx.notify();
    }

    /// Open a new terminal tab in the center dock (`None` = the user's home directory).
    fn open_terminal(&mut self, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let dock = self.dock.clone();
        if let Some(panel) = TerminalPanel::open(cwd.as_deref(), dock.downgrade(), window, cx) {
            dock.update(cx, |dock, cx| {
                dock.add_panel_view(panel_handle(panel.clone()), DockPlacement::Center, None, window, cx);
            });
            self.terminals.push(panel);
            self.sync_alone(cx);
            cx.notify();
        }
    }

    /// Tell every terminal whether it is the only one, so the last tab can draw its own close button.
    fn sync_alone(&self, cx: &mut Context<Self>) {
        let alone = self.terminals.len() == 1;
        for panel in &self.terminals {
            panel.update(cx, |panel, cx| panel.set_alone(alone, cx));
        }
    }

    fn new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.selected_path(cx);
        self.open_terminal(cwd, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    fn render_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        // The center column starts at the window's top edge, so its top band is where the
        // macOS title bar would be: let it drag and double-click-zoom the window like `TitleBar`
        // does. Tabs stop mouse-down propagation, so dragging a tab still reorders it.
        let center = v_flex()
            .id("center")
            .flex_1()
            .h_full()
            .min_w_0()
            .min_h_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| this.should_move = event.position.y < TAB_BAR_HEIGHT),
            )
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.should_move = false))
            .on_mouse_move(cx.listener(|this, _, window, _| {
                if this.should_move {
                    this.should_move = false;
                    window.start_window_move();
                }
            }))
            .on_double_click(|event, window, _| {
                if event.position().y < TAB_BAR_HEIGHT {
                    window.titlebar_double_click();
                }
            });
        if self.terminals.is_empty() {
            return center.child(self.render_welcome(muted, cx));
        }
        center.child(div().size_full().child(self.dock.clone()))
    }

    /// Shown while the dock has no tabs (the dock itself draws nothing then).
    fn render_welcome(&self, muted: Hsla, cx: &mut Context<Self>) -> impl IntoElement {
        let hint = if self.projects.read(cx).selection().is_some() {
            "Open a terminal in the selected worktree to begin."
        } else {
            "Select a project from the sidebar to begin."
        };
        let shortcut = |label: &'static str, keys: &'static str| {
            h_flex()
                .w(px(320.))
                .justify_between()
                .items_center()
                .text_sm()
                .text_color(muted)
                .child(label)
                .child(Kbd::new(Keystroke::parse(keys).expect("static keystroke")))
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(div().text_3xl().font_weight(FontWeight::BOLD).child("HIVE"))
            .child(div().text_sm().text_color(muted).child(hint))
            .child(
                h_flex()
                    .gap_2()
                    .mt_2()
                    .child(
                        Button::new("welcome-add-project")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Add project")
                            .on_click(|_, _, cx| Store::prompt_add_projects(AppState::get(cx).store.clone(), cx)),
                    )
                    .child(
                        Button::new("welcome-new-terminal")
                            .outline()
                            .icon(Lucide::SquareTerminal)
                            .label("New terminal")
                            .on_click(cx.listener(|this, _, window, cx| this.new_terminal(&NewTerminal, window, cx))),
                    ),
            )
            .child(v_flex().gap_1().mt_6().child(shortcut("New terminal", "cmd-t")))
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = cx.theme().background;
        let title_bar = TitleBar::new().bg(bg).child(
            h_flex().gap_2().child(div().text_sm().child("Hive")).child(
                Button::new("toggle-sidebar")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::PanelLeft)
                    .tooltip_with_action("Toggle sidebar", &ToggleSidebar, Some("Workspace"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx))),
            ),
        );
        // Orca-style: the title bar (traffic lights + name) sits above the sidebar only;
        // the center dock with its tab bar runs to the very top of the window.
        // ponytail: collapsed, the title bar spans the window above the dock; inline it into the tab bar if that looks off.
        let body = if self.sidebar_open {
            let left = v_flex()
                .w(px(260.))
                .h_full()
                .flex_shrink_0()
                .border_r_1()
                .border_color(cx.theme().sidebar_border)
                .child(title_bar)
                .child(div().flex_1().min_h_0().child(self.sidebar.clone()));
            h_flex().flex_1().min_h_0().child(left).child(self.render_center(cx)).into_any_element()
        } else {
            v_flex().flex_1().min_h_0().child(title_bar).child(self.render_center(cx)).into_any_element()
        };
        v_flex()
            .size_full()
            .key_context("Workspace")
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::toggle_sidebar))
            .bg(bg)
            .text_color(cx.theme().foreground)
            .child(body)
            .child(self.status_bar.clone())
    }
}
