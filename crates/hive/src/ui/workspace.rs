//! Main window layout: TitleBar | Sidebar + center | StatusBar.
//! The center shows the selected project's own `DockArea` of terminal and diff tabs
//! (`ProjectCenter`); every project keeps its tabs while another one is shown.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{DockArea, DockPlacement, PanelId, panel_handle};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, InteractiveElementExt as _, Sizable as _, TitleBar, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::settings::clamp_sidebar_width;

use crate::app::AppState;
use crate::state::projects::Projects;
use crate::state::store::Store;
use crate::terminal::TerminalPanel;
use crate::ui::sidebar::Sidebar;
use crate::ui::status_bar::StatusBarView;
use crate::ui::{diff_panel::DiffPanel, git_panel::GitPanel};
use crate::ui::{keymap, theme};

mod center;
mod center_dock;
mod git;
mod project_center;

pub(crate) use git::right_sidebar_toggle;
use project_center::ProjectCenter;

// Bound in `ui::keymap`, where the user can reassign them.
actions!(workspace, [NewTerminal, ToggleSidebar, ToggleGitPanel]);

pub struct Workspace {
    focus: FocusHandle,
    sidebar: Entity<Sidebar>,
    sidebar_open: bool,
    /// Set by the first toggle, so the sidebar doesn't slide in on launch.
    sidebar_animate: bool,
    /// Live width while dragging; written to the `Store` once the drag ends.
    sidebar_width: Pixels,
    /// The resize handle is held down; the next mouse-up persists `sidebar_width`.
    resizing_sidebar: bool,
    /// Mirrors `window.is_fullscreen()`; the title slides into the traffic lights' space.
    fullscreen: bool,
    /// Set by the first fullscreen change, so the title doesn't slide on launch.
    fullscreen_animate: bool,
    status_bar: Entity<StatusBarView>,
    projects: Entity<Projects>,
    /// One center per project path, created on first use and dropped with the project.
    centers: HashMap<PathBuf, ProjectCenter>,
    /// The project whose center fills the window; `None` shows the welcome screen.
    shown: Option<PathBuf>,
    git_panel: Entity<GitPanel>,
    git_open: bool,
    /// Set by the first toggle, so the right sidebar doesn't slide in on launch.
    git_animate: bool,
    git_width: Pixels,
    resizing_git: bool,
    last_selection: Option<PathBuf>,
    _observers: Vec<Subscription>,
    /// Left button went down on the tab-bar band; the next mouse move drags the window.
    should_move: bool,
}

/// Height of the band at the top of the center column that behaves like a title bar.
const TAB_BAR_HEIGHT: Pixels = px(34.);
/// Grab area straddling the sidebar's right border.
const RESIZE_HANDLE_WIDTH: Pixels = px(6.);
/// Marker under the active tab of the right sidebar's header.
const RIGHT_TAB_UNDERLINE: Pixels = px(2.);
const HIVE_MARK: &[u8] = include_bytes!("../../assets/icons/Hive.icon/Assets/mark.svg");
const WELCOME_LOGO_SIZE: Pixels = px(96.);
/// `TitleBar`'s left padding that clears the macOS traffic lights.
const TRAFFIC_LIGHTS_INSET: Pixels = px(80.);
/// Width of the name and the sidebar toggle in the collapsed title bar, after the inset.
const COLLAPSED_TITLE_CONTENT_WIDTH: Pixels = px(72.);
/// Left padding `TitleBar` adds on its own in fullscreen, where the traffic lights are hidden.
const FULLSCREEN_INSET: Pixels = px(12.);
const TITLE_INSET_DURATION: Duration = Duration::from_millis(200);
/// Both sidebars slide open and closed at this pace.
pub(super) const SIDEBAR_SLIDE_DURATION: Duration = Duration::from_millis(200);

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = AppState::get(cx);
        let (store, projects) = (app.store.clone(), app.projects.clone());
        let sidebar_width = px(store.read(cx).state.settings.sidebar_width());
        let git_width = px(store.read(cx).state.settings.git_panel_width());
        let git_open = store.read(cx).state.settings.git_panel_open;

        let observers = vec![
            cx.observe_in(&projects, window, |this, _, window, cx| {
                this.sync_selection(window, cx)
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                theme::sync_system_appearance(window, cx)
            }),
            // Entering or leaving fullscreen resizes the window; there is no dedicated event.
            cx.observe_window_bounds(window, |this, window, cx| {
                let fullscreen = window.is_fullscreen();
                if this.fullscreen != fullscreen {
                    this.fullscreen = fullscreen;
                    this.fullscreen_animate = true;
                    cx.notify();
                }
            }),
        ];

        let mut this = Self {
            focus: cx.focus_handle(),
            sidebar: cx.new(Sidebar::new),
            sidebar_open: true,
            sidebar_animate: false,
            sidebar_width,
            resizing_sidebar: false,
            fullscreen: window.is_fullscreen(),
            fullscreen_animate: false,
            status_bar: cx.new(StatusBarView::new),
            projects,
            centers: HashMap::new(),
            shown: None,
            git_panel: cx.new(|cx| GitPanel::new(window, cx)),
            git_open,
            git_animate: false,
            git_width,
            resizing_git: false,
            last_selection: None,
            _observers: observers,
            should_move: false,
        };
        this.watch_git(window, cx);
        this.sync_selection(window, cx);
        this
    }

    /// The selected project and the folder terminals open in: its selected worktree, or the
    /// project folder itself when it is not a git repository.
    fn selected_target(&self, cx: &App) -> Option<(PathBuf, PathBuf)> {
        let (project, worktree) = self.projects.read(cx).selection()?;
        let cwd = worktree
            .map(|w| w.path.clone())
            .unwrap_or_else(|| project.path.clone());
        Some((project.path.clone(), cwd))
    }

    /// Show the selected project's center with a terminal for its selected worktree: focus an
    /// existing tab for that path, or open one.
    fn sync_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.drop_removed_centers(cx);
        let Some((project, cwd)) = self.selected_target(cx) else {
            self.last_selection = None;
            self.shown = None;
            cx.notify();
            return;
        };
        if self.last_selection.as_ref() == Some(&cwd) && self.shown.as_ref() == Some(&project) {
            return;
        }
        self.last_selection = Some(cwd.clone());
        self.shown = Some(project.clone());
        self.center(&project, window, cx)
            .show_terminal(&cwd, window, cx);
        self.sync_active_worktrees(cx);
        cx.notify();
    }

    /// A project removed from the sidebar takes its tabs (and their shells) with it.
    fn drop_removed_centers(&mut self, cx: &App) {
        let projects = self.projects.read(cx);
        self.centers
            .retain(|path, _| projects.project(path).is_some());
    }

    /// The project's center, created on first use.
    fn center(
        &mut self,
        project: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> &mut ProjectCenter {
        self.centers
            .entry(project.to_path_buf())
            .or_insert_with(|| ProjectCenter::new(project, window, cx))
    }

    fn shown_center(&self) -> Option<&ProjectCenter> {
        self.shown.as_ref().and_then(|path| self.centers.get(path))
    }

    /// Worktrees with an open terminal in any project get a lit status dot in the sidebar.
    fn sync_active_worktrees(&self, cx: &mut Context<Self>) {
        let paths: HashSet<PathBuf> = self
            .centers
            .values()
            .flat_map(|center| center.terminal_paths(cx))
            .collect();
        self.projects
            .update(cx, |projects, cx| projects.set_active(paths, cx));
    }

    /// Opens another terminal tab in the selected worktree. Terminals belong to a project,
    /// so this does nothing while no project is selected.
    fn new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let Some((project, cwd)) = self.selected_target(cx) else {
            return;
        };
        self.shown = Some(project.clone());
        self.center(&project, window, cx)
            .open_terminal(Some(&cwd), window, cx);
        self.sync_active_worktrees(cx);
        cx.notify();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        self.sidebar_animate = true;
        cx.notify();
    }

    /// Width of the title bar once the sidebar is collapsed: the traffic lights' inset (gone
    /// in fullscreen, where `TitleBar` pads by `FULLSCREEN_INSET` instead) plus its content.
    fn collapsed_title_width(&self) -> Pixels {
        let inset = if self.fullscreen {
            FULLSCREEN_INSET
        } else {
            TRAFFIC_LIGHTS_INSET
        };
        inset + COLLAPSED_TITLE_CONTENT_WIDTH
    }

    /// Slides the title from the traffic lights' inset to the left edge on entering fullscreen;
    /// `TitleBar` alone would jump. Leaving fullscreen snaps it back at once: macOS draws no
    /// frames during its exit transition, so a slide would freeze under the traffic lights.
    fn animate_title_inset(&self, title_bar: TitleBar) -> AnyElement {
        if !(self.fullscreen && self.fullscreen_animate) {
            return title_bar.into_any_element();
        }
        // In fullscreen `TitleBar` pads its content by `FULLSCREEN_INSET` itself.
        let from = TRAFFIC_LIGHTS_INSET - FULLSCREEN_INSET;
        title_bar
            .with_animation(
                "title-inset",
                Animation::new(TITLE_INSET_DURATION).with_easing(ease_in_out),
                move |bar, t| bar.pl(from * (1. - t)),
            )
            .into_any_element()
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
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The welcome screen has no focused input or panel to route workspace actions through.
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        let bg = cx.theme().background;
        let open = self.sidebar_open;
        // Open, the toggle sits at the sidebar's right border; collapsed, the title bar shrinks
        // to the name and the toggle, and the center's tab bar starts right after it.
        let title_bar = TitleBar::new().bg(bg).child(
            h_flex()
                .justify_between()
                .pr_2()
                .map(|el| {
                    if open {
                        el.flex_1()
                    } else {
                        el.w(COLLAPSED_TITLE_CONTENT_WIDTH).flex_none()
                    }
                })
                .child(div().text_sm().child("Hive"))
                .child(
                    Button::new("toggle-sidebar")
                        .cursor_pointer()
                        .ghost()
                        .small()
                        .icon(Lucide::PanelLeft)
                        .tooltip_with_action(
                            "Toggle sidebar",
                            &ToggleSidebar,
                            Some(keymap::CONTEXT),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_sidebar(&ToggleSidebar, window, cx)
                        })),
                ),
        );
        let title_bar = self.animate_title_inset(title_bar);
        // Orca-style: the title bar (traffic lights + name) sits above the sidebar only and the
        // center dock with its tab bar runs to the very top. Collapsing slides the sidebar to
        // zero width while the title bar shrinks into the tab bar's row, whose tabs move left
        // up to the title bar's edge. 1.0 = fully open; `t` is the eased animation progress.
        let amount = move |t: f32| if open { t } else { 1. - t };
        let border = cx.theme().sidebar_border;
        let width = self.sidebar_width;
        let left = move |el: Div, t: f32| {
            let a = amount(t);
            el.w(width * a)
                .when(a > 0., |el| el.border_r_1().border_color(border))
        };
        // The dock draws its own tab bar in its own view, so the inset reaches it through the
        // center's cell; the animated element itself does not change.
        let title_width = self.collapsed_title_width();
        let tab_inset: Option<Rc<Cell<Pixels>>> = self.shown_center().map(ProjectCenter::tab_inset);
        let center = move |el: Div, t: f32| {
            if let Some(cell) = &tab_inset {
                cell.set(title_width * (1. - amount(t)));
            }
            el
        };
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
            .child(self.render_center(window, cx));
        let (left_el, center_el) = if self.sidebar_animate {
            let anim = Animation::new(SIDEBAR_SLIDE_DURATION).with_easing(ease_in_out);
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
            .when(self.git_open || self.git_animate, |el| {
                el.child(self.render_right_sidebar(cx))
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .map(|el| {
                        if open {
                            el.w(width - px(1.)) /* leave the sidebar border visible */
                        } else {
                            el.w(title_width)
                        }
                    })
                    .child(title_bar),
            )
            .when(open, |el| el.child(self.render_sidebar_resize_handle(cx)))
            .when(self.git_open, |el| {
                el.child(self.render_git_resize_handle(cx))
            });
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .key_context("Workspace")
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(|this, _: &ToggleGitPanel, _, cx| this.toggle_git(cx)))
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
