//! Main window layout: TitleBar | Sidebar + center | StatusBar.
//! The center shows the selected project's own `DockArea` of terminal and diff tabs
//! (`ProjectCenter`); every project keeps its tabs while another one is shown.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{DockArea, DockPlacement, PanelId, panel_handle};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, InteractiveElementExt as _, Sizable as _, TitleBar, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use hive_core::host::Host;
use hive_core::settings::clamp_sidebar_width;

use crate::app::AppState;
use crate::state::agents::AgentsEvent;
use crate::state::projects::Projects;
use crate::terminal::TerminalPanel;
use crate::ui::hover::{ButtonLook, Hover, SmoothButton as _};
use crate::ui::sidebar::Sidebar;
use crate::ui::status_bar::StatusBarView;
use crate::ui::{diff_panel::DiffPanel, git_panel::GitPanel};
use crate::ui::{keymap, theme};

mod center;
mod center_dock;
mod files;
mod git;
mod persistence;
mod project_center;
pub(crate) mod switcher;

use crate::state::store::SaveError;
pub(crate) use git::right_sidebar_toggle;
use gpui_kit::component::{WindowExt as _, notification::Notification};
use project_center::ProjectCenter;

// Bound in `ui::keymap`, where the user can reassign them.
actions!(
    workspace,
    [QuickSwitch, NewTerminal, ToggleSidebar, ToggleGitPanel]
);

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
    centers: HashMap<(Host, PathBuf), ProjectCenter>,
    /// The project whose center fills the window; `None` shows the welcome screen.
    shown: Option<(Host, PathBuf)>,
    git_panel: Entity<GitPanel>,
    files_panel: Entity<crate::ui::files_panel::FilesPanel>,
    git_open: bool,
    /// Set by the first toggle, so the right sidebar doesn't slide in on launch.
    git_animate: bool,
    git_animation: Option<Task<()>>,
    git_width: Pixels,
    resizing_git: bool,
    last_selection: Option<PathBuf>,
    _observers: Vec<Subscription>,
    /// Left button went down on the tab-bar band; the next mouse move drags the window.
    should_move: bool,
}

/// Height of the band at the top of the center column that behaves like a title bar. Equals the
/// center tab bar, which is as tall as gpui-component's default `Tab`, so the bottom lines of
/// the title bar, the tab bar and the right sidebar's header form one line.
const TAB_BAR_HEIGHT: Pixels = px(32.);
/// Grab area straddling the sidebar's right border.
const RESIZE_HANDLE_WIDTH: Pixels = px(6.);
/// Marker under the active tab of the right sidebar's header.
const RIGHT_TAB_UNDERLINE: Pixels = px(2.);
const HIVE_MARK: &[u8] = include_bytes!("../../assets/icons/Hive.icon/Assets/mark.svg");
const WELCOME_LOGO_SIZE: Pixels = px(96.);
/// Width of the welcome screen's shortcut list.
const WELCOME_CONTENT_WIDTH: Pixels = px(360.);
/// `TitleBar`'s left padding that clears the macOS traffic lights.
const TRAFFIC_LIGHTS_INSET: Pixels = px(80.);
/// Width of the sidebar toggle in the collapsed title bar, after the inset.
const COLLAPSED_TITLE_CONTENT_WIDTH: Pixels = px(32.);
/// Left padding `TitleBar` adds on its own in fullscreen, where the traffic lights are hidden.
const FULLSCREEN_INSET: Pixels = px(12.);
const TITLE_INSET_DURATION: Duration = Duration::from_millis(200);
/// Both sidebars slide open and closed at this pace.
pub(super) const SIDEBAR_SLIDE_DURATION: Duration = Duration::from_millis(200);

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        window.on_window_should_close(cx, |window, cx| {
            if AppState::get(cx).documents.read(cx).dirty(cx).is_empty() {
                return true;
            }
            crate::ui::editor_panel::close::all(window, cx, |window, _| window.remove_window());
            false
        });
        let app = AppState::get(cx);
        let (store, projects, agents) =
            (app.store.clone(), app.projects.clone(), app.agents.clone());
        let updates = app.updates.clone();
        let sidebar_width = px(store.read(cx).state.settings.sidebar_width());
        let git_width = px(store.read(cx).state.settings.git_panel_width());
        let git_open = store.read(cx).state.settings.git_panel_open;

        let release_store = store.clone();
        window.defer(cx, move |window, cx| {
            crate::ui::update_available::sync(window, cx);
            if release_store.update(cx, |store, cx| store.take_release_notes(cx)) {
                crate::ui::release_notes::show(window, cx);
            }
        });

        if let Some(notice) = &store.read(cx).notice {
            let notice = notice.clone();
            window.defer(cx, move |window, cx| {
                window.push_notification(Notification::error(notice).title("State recovery"), cx);
            });
        }
        let observers = vec![
            cx.observe_in(&updates, window, |_, _, window, cx| {
                crate::ui::update_available::sync(window, cx);
            }),
            cx.subscribe_in(&store, window, |_, _, error: &SaveError, window, cx| {
                window.push_notification(
                    Notification::error(error.0.clone()).title("Could not save state"),
                    cx,
                );
            }),
            cx.observe_in(&projects, window, |this, _, window, cx| {
                this.sync_selection(window, cx)
            }),
            cx.subscribe_in(&agents, window, |this, _, event, window, cx| match event {
                AgentsEvent::Reveal(session) => this.reveal_session(*session, window, cx),
                AgentsEvent::Launch(host, path, launch) => {
                    let project = this
                        .projects
                        .read(cx)
                        .items
                        .iter()
                        .find(|project| {
                            project.host == *host
                                && (project.path == *path
                                    || project.worktrees.iter().any(|w| w.path == *path))
                        })
                        .map(|p| (p.host.clone(), p.path.clone()));
                    if let Some(project) = project {
                        this.shown = Some(project.clone());
                        this.center(&project, window, cx).open_agent(
                            path,
                            launch.clone(),
                            window,
                            cx,
                        );
                        this.sync_active_worktrees(cx);
                        this.save_center(&project, window, cx);
                        cx.notify();
                    }
                }
                AgentsEvent::Attention(session, status) => {
                    let session = *session;
                    let agents = AppState::get(cx).agents.clone();
                    window.push_notification(
                        Notification::info(status.label())
                            .title("Agent needs attention")
                            .id1::<AgentsEvent>(("agent", session.as_u64()))
                            .in_app_and_system()
                            .on_click(move |_, _, cx| {
                                agents.update(cx, |agents, cx| agents.reveal(session, cx))
                            }),
                        cx,
                    );
                }
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

        let files = cx.new(|cx| crate::state::files::Files::new(projects.clone(), None, cx));
        let files_panel = cx.new(|cx| crate::ui::files_panel::FilesPanel::new(files, window, cx));
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
            files_panel,
            git_open,
            git_animate: false,
            git_animation: None,
            git_width,
            resizing_git: false,
            last_selection: None,
            _observers: observers,
            should_move: false,
        };
        this.watch_git(window, cx);
        this.watch_files(window, cx);
        this.sync_selection(window, cx);
        this
    }

    /// The selected project and the folder terminals open in: its selected worktree, or the
    /// project folder itself when it is not a git repository.
    fn selected_target(&self, cx: &App) -> Option<((Host, PathBuf), PathBuf)> {
        let target = self.projects.read(cx).target()?;
        Some(((target.host, target.project), target.path))
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
        let restore = !self.centers.contains_key(&project)
            && self
                .saved_center(&project, cx)
                .is_some_and(|saved| saved.worktree.as_ref() == Some(&cwd));
        let center = self.center(&project, window, cx);
        if !restore {
            center.show_terminal(&cwd, window, cx);
        }
        self.sync_active_worktrees(cx);
        self.save_center(&project, window, cx);
        cx.notify();
    }

    /// Shows the project, tab and pane running terminal `session`, wherever it is.
    fn reveal_session(&mut self, session: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let project = self.centers.iter_mut().find_map(|(project, center)| {
            center
                .reveal_session(session, window, cx)
                .then(|| project.clone())
        });
        if let Some(project) = project {
            self.shown = Some(project);
            cx.notify();
        }
    }

    /// A project removed from the sidebar takes its tabs (and their shells) with it.
    fn drop_removed_centers(&mut self, cx: &App) {
        let projects = self.projects.read(cx);
        self.centers
            .retain(|(host, path), _| projects.project(host, path).is_some());
    }

    /// The project's center, created on first use.
    fn center(
        &mut self,
        project: &(Host, PathBuf),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> &mut ProjectCenter {
        let saved = (!self.centers.contains_key(project))
            .then(|| self.saved_center(project, cx))
            .flatten();
        self.centers.entry(project.clone()).or_insert_with(|| {
            let mut center = ProjectCenter::new(project, window, cx);
            if let Some(saved) = saved {
                center.restore(&saved, window, cx);
            }
            center
        })
    }

    /// Lets `band` drag and double-click-zoom the window the way `TitleBar` does, for the
    /// part of it where `in_band` holds: mouse-down only arms the move, the first mouse-move
    /// starts it, so clicks on buttons inside the band still work. Elements that must not drag
    /// the window (tabs) stop mouse-down propagation themselves.
    fn window_drag_band(
        &self,
        band: Stateful<Div>,
        in_band: impl Fn(Point<Pixels>) -> bool + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        band.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                this.should_move = in_band(event.position)
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
        .on_double_click(move |event, window, _| {
            if in_band(event.position()) {
                window.titlebar_double_click();
            }
        })
    }

    fn shown_center(&self) -> Option<&ProjectCenter> {
        self.shown.as_ref().and_then(|path| self.centers.get(path))
    }

    /// Worktrees with an open terminal in any project get a lit status dot in the sidebar.
    fn sync_active_worktrees(&self, cx: &mut Context<Self>) {
        let paths: HashSet<(Host, PathBuf)> = self
            .centers
            .values()
            .flat_map(|center| {
                center
                    .terminal_paths(cx)
                    .map(|path| (center.host.clone(), path))
            })
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
        let toggle_hover = Hover::new("toggle-sidebar", window, cx);
        let bg = cx.theme().background;
        let open = self.sidebar_open;
        // The toggle keeps a fixed slot in both states, so it does not jump when the sidebar
        // collapses and the center's tab bar starts right after it.
        let sidebar_border = cx.theme().sidebar_border;
        // Open, the title bar draws the sidebar border over the tab bar's first column, where
        // the center dock tucks the first tab's left border so the two don't double up.
        let title_bar = TitleBar::new()
            .h(TAB_BAR_HEIGHT)
            .bg(bg)
            .when(open, |bar| bar.border_r_1().border_color(sidebar_border))
            .child(
                h_flex()
                    .pr_2()
                    .w(COLLAPSED_TITLE_CONTENT_WIDTH)
                    .flex_none()
                    .child(
                        Button::new("toggle-sidebar")
                            .cursor_pointer()
                            .small()
                            .icon(Lucide::PanelLeft)
                            .tooltip_with_action(
                                "Toggle sidebar",
                                &ToggleSidebar,
                                Some(keymap::CONTEXT),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_sidebar(&ToggleSidebar, window, cx)
                            }))
                            .smooth_hover(ButtonLook::Ghost, &toggle_hover, cx),
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
                el.child(self.render_right_sidebar(window, cx))
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .map(|el| if open { el.w(width) } else { el.w(title_width) })
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
            .on_action(cx.listener(
                |this, _: &crate::ui::editor_panel::OpenFileSearch, window, cx| {
                    this.choose_files(true, cx);
                    if !this.git_open {
                        this.toggle_git(cx);
                    }
                    let panel = this.files_panel.clone();
                    window.defer(cx, move |window, cx| {
                        panel.update(cx, |panel, cx| panel.focus_search(window, cx))
                    });
                },
            ))
            .on_action(
                cx.listener(|_, _: &QuickSwitch, window, cx| switcher::open(false, window, cx)),
            )
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
