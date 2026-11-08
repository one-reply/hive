//! Workspace integration for source control and diff tabs.
use super::*;
use crate::state::git::{DiffRequest, GitEvent};
use gpui_kit::base::motion::{Transition, transition};
use hive_core::projects::Target;
use hive_core::settings::clamp_git_panel_width;

const RIGHT_TAB_WIDTH: Rems = rems(2.);
const RIGHT_TAB_TRANSITION: Duration = Duration::from_millis(180);

impl Workspace {
    pub(super) fn watch_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected_files = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .files_selected;
        let git = AppState::get(cx).git.clone();
        git.update(cx, |g, _| {
            g.panel_visible = self.git_open && !selected_files;
            g.files_visible = self.git_open && selected_files;
            g.window_active = window.is_window_active();
        });
        self._observers.push(cx.subscribe_in(
            &git,
            window,
            |this, _, event, window, cx| match event {
                GitEvent::Refreshed { .. } => {}
                GitEvent::OpenDiff(request) => this.open_diff(request.clone(), window, cx),
                GitEvent::Activate(target) => this.activate_target(target, cx),
                GitEvent::Terminal(target) => {
                    this.activate_target(target, cx);
                    this.center(&(target.host.clone(), target.project.clone()), window, cx).show_terminal(
                        &target.path,
                        window,
                        cx,
                    );
                    this.sync_active_worktrees(cx);
                    cx.notify();
                }
                GitEvent::AgentDraft(target, agent, prompt) => {
                    if target.host != hive_core::host::Host::Local {
                        window.push_notification(Notification::error("Sending drafts to SSH agents is not supported yet. Copy the error instead."), cx);
                        return;
                    }
                    this.activate_target(target, cx);
                    this.center(&(target.host.clone(), target.project.clone()), window, cx).prepare_agent_draft(&target.path, *agent, prompt.clone(), window, cx);
                    this.sync_active_worktrees(cx);
                    this.save_center(&(target.host.clone(), target.project.clone()), window, cx);
                    cx.notify();
                }
            },
        ));
        self._observers
            .push(cx.observe_window_activation(window, move |_, window, cx| {
                git.update(cx, |g, cx| {
                    g.window_active = window.is_window_active();
                    if g.window_active {
                        g.refresh(false, cx);
                    }
                });
            }));
    }

    /// Selects the target in the sidebar and shows its project's center. The caller decides
    /// which tab to show, so `sync_selection` must not open a terminal on its own.
    pub(super) fn activate_target(&mut self, target: &Target, cx: &mut Context<Self>) {
        let hidden = self
            .projects
            .read(cx)
            .items
            .iter()
            .find(|p| p.path == target.project && p.host == target.host)
            .is_some_and(|p| {
                p.worktrees.iter().any(|w| w.path == target.path)
                    && !p.visible_worktrees().any(|(_, w)| w.path == target.path)
            });
        if hidden {
            AppState::get(cx).store.clone().update(cx, |s, cx| {
                s.toggle_worktree_filter(&target.host, &target.project, cx)
            });
        }
        self.last_selection = Some(target.path.clone());
        self.shown = Some((target.host.clone(), target.project.clone()));
        self.projects.update(cx, |p, cx| {
            p.select(&target.host, &target.project, Some(&target.path), cx)
        });
    }

    fn open_diff(&mut self, request: DiffRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_target(&request.target, cx);
        let project = (request.target.host.clone(), request.target.project.clone());
        self.center(&project, window, cx)
            .show_diff(request, window, cx);
        cx.notify();
    }

    pub(super) fn toggle_git(&mut self, cx: &mut Context<Self>) {
        self.git_open = !self.git_open;
        let selected_files = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .files_selected;
        let files_visible = self.git_open && selected_files;
        self.files_panel
            .read(cx)
            .files
            .clone()
            .update(cx, |files, cx| files.set_visible(files_visible, cx));
        self.git_animate = true;
        self.git_animation = Some(cx.spawn(async |this, cx| {
            cx.background_executor().timer(SIDEBAR_SLIDE_DURATION).await;
            let _ = this.update(cx, |this, cx| {
                // A closed sidebar must leave the layout tree once its slide finishes.
                this.git_animate = false;
                cx.notify();
            });
        }));
        let app = AppState::get(cx);
        let (store, git) = (app.store.clone(), app.git.clone());
        store.update(cx, |s, cx| s.set_git_panel_open(self.git_open, cx));
        git.update(cx, |g, cx| {
            g.panel_visible = self.git_open && !selected_files;
            g.files_visible = self.git_open && selected_files;
            if g.panel_visible {
                g.refresh(true, cx);
            } else if g.files_visible {
                g.refresh(false, cx);
            }
        });
        cx.notify();
    }

    /// The right sidebar: a tab bar for Git and Files with the sidebar toggle at
    /// its right end, then the selected section. Toggling slides it in from or out past the
    /// window's right edge: the content keeps its width and is clipped by an outer column
    /// whose width animates, the same way the left sidebar collapses.
    pub(super) fn render_right_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toggle_hover = Hover::new("toggle-right-sidebar", window, cx);
        let border = cx.theme().border;
        let open = self.git_open;
        let selected_files = AppState::get(cx)
            .store
            .read(cx)
            .state
            .settings
            .files_selected;
        let width = self.git_width;
        let indicator_position = transition(
            "right-sidebar-tab-indicator",
            if selected_files { 1.0 } else { 0.0 },
            Transition::new(RIGHT_TAB_TRANSITION).ease(ease_in_out),
            window,
            cx,
        );
        // The header sits where the macOS title bar would be, so it drags the window too.
        let header = h_flex()
            .id("right-sidebar-header")
            .h(TAB_BAR_HEIGHT)
            .flex_none()
            .px_2()
            .justify_between()
            .items_center()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .relative()
                    .h_full()
                    .flex_none()
                    .child(
                        render_right_tab("choose-git", Lucide::GitBranch, !selected_files, cx)
                            .aria_label("Git")
                            .on_click(cx.listener(|this, _, _, cx| this.choose_files(false, cx))),
                    )
                    .child(
                        render_right_tab("choose-files", Lucide::Folder, selected_files, cx)
                            .aria_label("Files")
                            .on_click(cx.listener(|this, _, _, cx| this.choose_files(true, cx))),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom_0()
                            .left(RIGHT_TAB_WIDTH * indicator_position)
                            .w(RIGHT_TAB_WIDTH)
                            .h(RIGHT_TAB_UNDERLINE)
                            .bg(cx.theme().primary),
                    ),
            )
            .child(right_sidebar_toggle().small().smooth_hover(
                ButtonLook::Ghost,
                &toggle_hover,
                cx,
            ));
        let header = self.window_drag_band(header, |_| true, cx);
        let content = v_flex()
            .h_full()
            .w(width)
            .flex_none()
            .border_l_1()
            .border_color(border)
            .child(header)
            .child(div().flex_1().min_h_0().child(if selected_files {
                super::cached(&self.files_panel).into_any_element()
            } else {
                super::cached(&self.git_panel).into_any_element()
            }));
        // Right-aligned so the content's left edge moves while its right edge stays put.
        let outer = v_flex()
            .h_full()
            .flex_shrink_0()
            .items_end()
            .overflow_hidden()
            .child(content);
        if !self.git_animate {
            return outer.w(width).into_any_element();
        }
        // 1.0 = fully open; `t` is the eased animation progress.
        let amount = move |t: f32| if open { t } else { 1. - t };
        outer
            .with_animation(
                ("right-sidebar-w", open as usize),
                Animation::new(SIDEBAR_SLIDE_DURATION).with_easing(ease_in_out),
                move |el, t| el.w(width * amount(t)),
            )
            .into_any_element()
    }

    fn finish_git_resize(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.resizing_git) {
            AppState::get(cx).store.clone().update(cx, |s, cx| {
                s.set_git_panel_width(f32::from(self.git_width), cx)
            });
        }
    }

    pub(super) fn render_git_resize_handle(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("git-resize")
            .absolute()
            .top_0()
            .bottom_0()
            .right(self.git_width - RESIZE_HANDLE_WIDTH / 2.)
            .w(RESIZE_HANDLE_WIDTH)
            .occlude()
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.resizing_git = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_git_resize(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_git_resize(cx)),
            )
            .on_drag(GitResize, |_, _, _, cx| cx.new(|_| GitResize))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<GitResize>, window, cx| {
                    let width = px(clamp_git_panel_width(f32::from(
                        window.viewport_size().width - event.event.position.x,
                    )));
                    if width != this.git_width {
                        this.git_width = width;
                        cx.notify();
                    }
                }),
            )
    }
}

/// Full-height tabs share one sliding indicator in their parent row.
fn render_right_tab(id: &'static str, icon: Lucide, active: bool, cx: &App) -> Stateful<Div> {
    let color = if active {
        cx.theme().foreground
    } else {
        cx.theme().muted_foreground
    };
    div()
        .id(id)
        .h_full()
        .w(RIGHT_TAB_WIDTH)
        .flex()
        .justify_center()
        .items_center()
        .cursor_pointer()
        .child(Icon::new(icon).small().text_color(color))
}

/// The button that shows or hides the right sidebar. It stays in the window's top-right
/// corner: in the sidebar's header while it is open, in the center's tab bar (or over the
/// welcome screen) while it is closed. Callers pick the size.
pub(crate) fn right_sidebar_toggle() -> Button {
    Button::new("toggle-right-sidebar")
        .cursor_pointer()
        .icon(Lucide::PanelRight)
        .tooltip_with_action(
            "Toggle right sidebar",
            &ToggleGitPanel,
            Some(keymap::CONTEXT),
        )
        .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleGitPanel), cx))
}

struct GitResize;
impl Render for GitResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}
