//! Main window layout: TitleBar | Sidebar + center dock | StatusBar.
//! The center is a `DockArea` of terminal tabs, one group per selected project/worktree.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{
    DockArea, DockPlacement, DockSkin, PanelId, PanelStyle, panel_handle,
};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, InteractiveElementExt as _, Sizable as _, TitleBar, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::settings::{ThemeChoice, clamp_sidebar_width};

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
    /// Set by the first toggle, so the sidebar doesn't slide in on launch.
    sidebar_animate: bool,
    /// Live width while dragging; written to the `Store` once the drag ends.
    sidebar_width: Pixels,
    /// The resize handle is held down; the next mouse-up persists `sidebar_width`.
    resizing_sidebar: bool,
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
/// Grab area straddling the sidebar's right border.
const RESIZE_HANDLE_WIDTH: Pixels = px(6.);
const HIVE_MARK: &[u8] = include_bytes!("../../assets/icons/Hive.icon/Assets/mark.svg");
const WELCOME_LOGO_SIZE: Pixels = px(96.);

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::get(cx);
        let (store, projects) = (app.store.clone(), app.projects.clone());
        let sidebar_width = px(store.read(cx).state.settings.sidebar_width());

        apply_theme(store.read(cx).state.settings.theme, Some(window), cx);

        let (dock, skin) = DockSkin::dock_area("center", None, window, cx);
        skin.set_panel_style(PanelStyle::TabBar, cx);
        skin.set_close_button_visible(true, cx);
        skin.set_toggle_button_visible(false, cx);

        let observers = vec![
            cx.observe(&store, |_, store, cx| {
                apply_theme(store.read(cx).state.settings.theme, None, cx)
            }),
            cx.observe_in(&projects, window, |this, _, window, cx| {
                this.sync_selection(window, cx)
            }),
            // Tabs closed from the dock (× button, exited shell) must drop out of `terminals`
            // so the empty state with its "+" strip comes back.
            cx.observe(&dock, |this, dock, cx| {
                this.terminals.retain(|panel| {
                    dock.read(cx)
                        .panel(PanelId::from(panel.entity_id()))
                        .is_some()
                });
                this.sync_alone(cx);
                this.sync_active_worktrees(cx);
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
            sidebar_animate: false,
            sidebar_width,
            resizing_sidebar: false,
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
        Some(
            worktree
                .map(|w| w.path.clone())
                .unwrap_or_else(|| project.path.clone()),
        )
    }

    /// Show a terminal for the selected worktree: focus an existing tab for that path, or open one.
    fn sync_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selected_path(cx) else {
            cx.notify();
            return;
        };
        let dock = self.dock.clone();
        self.terminals.retain(|panel| {
            dock.read(cx)
                .panel(PanelId::from(panel.entity_id()))
                .is_some()
        });

        let existing = self
            .terminals
            .iter()
            .find(|panel| panel.read(cx).cwd(cx).as_deref() == Some(path.as_path()))
            .cloned();
        match existing {
            Some(panel) => dock.update(cx, |dock, cx| {
                dock.select_panel(PanelId::from(panel.entity_id()), window, cx)
            }),
            None => self.open_terminal(Some(path), window, cx),
        }
        cx.notify();
    }

    /// Open a new terminal tab in the center dock (`None` = the user's home directory).
    fn open_terminal(&mut self, cwd: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let dock = self.dock.clone();
        if let Some(panel) = TerminalPanel::open(cwd.as_deref(), dock.downgrade(), window, cx) {
            dock.update(cx, |dock, cx| {
                dock.add_panel_view(
                    panel_handle(panel.clone()),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
            self.terminals.push(panel);
            self.sync_alone(cx);
            self.sync_active_worktrees(cx);
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

    /// Worktrees with an open terminal get a lit status dot in the sidebar.
    fn sync_active_worktrees(&self, cx: &mut Context<Self>) {
        let paths: HashSet<PathBuf> = self
            .terminals
            .iter()
            .filter_map(|panel| panel.read(cx).cwd(cx))
            .collect();
        self.projects
            .update(cx, |projects, cx| projects.set_active(paths, cx));
    }

    fn new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.selected_path(cx);
        self.open_terminal(cwd, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        self.sidebar_animate = true;
        cx.notify();
    }

    fn finish_sidebar_resize(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.resizing_sidebar) {
            return;
        }
        let width = f32::from(self.sidebar_width);
        AppState::get(cx).store.clone().update(cx, |store, cx| {
            if store.state.settings.sidebar_width() != width {
                store.set_sidebar_width(width, cx);
            }
        });
    }

    /// Invisible band over the sidebar's right border that drags its width. Mouse-up is caught
    /// both on and off the band because the pointer usually leaves it while dragging.
    fn render_sidebar_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-resize")
            .absolute()
            .top_0()
            .bottom_0()
            .left(self.sidebar_width - RESIZE_HANDLE_WIDTH / 2.)
            .w(RESIZE_HANDLE_WIDTH)
            .occlude()
            .cursor_col_resize()
            .on_hover(|hovered, _, _| tracing::debug!(hovered, "DBG-RESIZE hover"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    tracing::debug!(?event.position, "DBG-RESIZE down");
                    this.resizing_sidebar = true
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_sidebar_resize(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_sidebar_resize(cx)),
            )
            .on_drag(SidebarResize, |_, _, _, cx| {
                tracing::debug!("DBG-RESIZE drag start");
                cx.new(|_| SidebarResize)
            })
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<SidebarResize>, _, cx| {
                    // The sidebar starts at the window's left edge, so the pointer's x is its width.
                    let width = px(clamp_sidebar_width(f32::from(event.event.position.x)));
                    tracing::debug!(?width, "DBG-RESIZE move");
                    if width != this.sidebar_width {
                        this.sidebar_width = width;
                        cx.notify();
                    }
                }),
            )
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
                cx.listener(|this, event: &MouseDownEvent, _, _| {
                    this.should_move = event.position.y < TAB_BAR_HEIGHT
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.should_move = false),
            )
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
            .child(
                Icon::default()
                    .data(HIVE_MARK)
                    .size(WELCOME_LOGO_SIZE)
                    .text_color(cx.theme().foreground),
            )
            .child(div().text_3xl().font_weight(FontWeight::BOLD).child("HIVE"))
            .child(div().text_sm().text_color(muted).child(hint))
            .child(
                h_flex()
                    .gap_2()
                    .mt_2()
                    .child(
                        Button::new("welcome-add-project")
                            .cursor_pointer()
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Add project")
                            .on_click(|_, _, cx| {
                                Store::prompt_add_projects(AppState::get(cx).store.clone(), cx)
                            }),
                    )
                    .child(
                        Button::new("welcome-new-terminal")
                            .cursor_pointer()
                            .outline()
                            .icon(Lucide::SquareTerminal)
                            .label("New terminal")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_terminal(&NewTerminal, window, cx)
                            })),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .mt_6()
                    .child(shortcut("New terminal", "cmd-t")),
            )
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bg = cx.theme().background;
        let open = self.sidebar_open;
        // Open, the toggle sits at the sidebar's right border; collapsed, the title bar spans the
        // window, so it stays next to the name instead of jumping to the far edge.
        let title_bar = TitleBar::new().bg(bg).child(
            h_flex()
                .flex_1()
                .gap_2()
                .pr_2()
                .when(open, |el| el.justify_between())
                .child(div().text_sm().child("Hive"))
                .child(
                    Button::new("toggle-sidebar")
                        .cursor_pointer()
                        .ghost()
                        .small()
                        .icon(Lucide::PanelLeft)
                        .tooltip_with_action("Toggle sidebar", &ToggleSidebar, Some("Workspace"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_sidebar(&ToggleSidebar, window, cx)
                        })),
                ),
        );
        // Orca-style: the title bar (traffic lights + name) sits above the sidebar only and the
        // center dock with its tab bar runs to the very top. Collapsing slides the sidebar to
        // zero width while the dock drops below the title bar, which then spans the window.
        // 1.0 = fully open; `t` is the eased animation progress.
        let amount = move |t: f32| if open { t } else { 1. - t };
        let border = cx.theme().sidebar_border;
        let width = self.sidebar_width;
        let left = move |el: Div, t: f32| {
            let a = amount(t);
            el.w(width * a)
                .when(a > 0., |el| el.border_r_1().border_color(border))
        };
        let center = move |el: Div, t: f32| el.pt(TAB_BAR_HEIGHT * (1. - amount(t)));
        let left_el = v_flex().h_full().flex_shrink_0().overflow_hidden().child(
            v_flex()
                .w(width)
                .h_full()
                .child(div().h(TAB_BAR_HEIGHT).flex_none())
                .child(div().flex_1().min_h_0().child(self.sidebar.clone())),
        );
        let center_el = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_center(cx));
        let (left_el, center_el) = if self.sidebar_animate {
            let anim = Animation::new(Duration::from_millis(200)).with_easing(ease_in_out);
            (
                left_el
                    .with_animation(("sidebar-w", open as usize), anim.clone(), left)
                    .into_any_element(),
                center_el
                    .with_animation(("center-pt", open as usize), anim, center)
                    .into_any_element(),
            )
        } else {
            (
                left(left_el, 1.).into_any_element(),
                center(center_el, 1.).into_any_element(),
            )
        };
        let body = h_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .child(left_el)
            .child(center_el)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .map(|el| {
                        if open {
                            el.w(width - px(1.)) /* leave the sidebar border visible */
                        } else {
                            el.w_full()
                        }
                    })
                    .child(title_bar),
            )
            .when(open, |el| el.child(self.render_sidebar_resize_handle(cx)));
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

/// Drag payload of the sidebar resize handle; renders nothing under the pointer.
struct SidebarResize;

impl Render for SidebarResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}
