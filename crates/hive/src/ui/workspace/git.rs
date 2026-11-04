//! Workspace integration for source control and diff tabs.
use super::*;
use crate::state::git::{DiffRequest, GitEvent, Target};
use hive_core::settings::clamp_git_panel_width;

impl Workspace {
    pub(super) fn watch_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let git = AppState::get(cx).git.clone();
        git.update(cx, |g, _| {
            g.panel_visible = self.git_open;
            g.window_active = window.is_window_active();
        });
        self._observers.push(cx.subscribe_in(
            &git,
            window,
            |this, _, event, window, cx| match event {
                GitEvent::Refreshed(_) => {}
                GitEvent::OpenDiff(request) => this.open_diff(request.clone(), window, cx),
                GitEvent::Activate(target) => this.activate_target(target, cx),
                GitEvent::Terminal(target) => {
                    this.activate_target(target, cx);
                    this.center(&target.project, window, cx).show_terminal(
                        &target.path,
                        window,
                        cx,
                    );
                    this.sync_active_worktrees(cx);
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
    fn activate_target(&mut self, target: &Target, cx: &mut Context<Self>) {
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
            AppState::get(cx)
                .store
                .clone()
                .update(cx, |s, cx| s.toggle_worktree_filter(&target.project, cx));
        }
        self.last_selection = Some(target.path.clone());
        self.shown = Some(target.project.clone());
        self.projects.update(cx, |p, cx| {
            p.select(&target.project, Some(&target.path), cx)
        });
    }

    fn open_diff(&mut self, request: DiffRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_target(&request.target, cx);
        let project = request.target.project.clone();
        self.center(&project, window, cx)
            .show_diff(request, window, cx);
        cx.notify();
    }

    pub(super) fn toggle_git(&mut self, cx: &mut Context<Self>) {
        self.git_open = !self.git_open;
        self.git_animate = true;
        let app = AppState::get(cx);
        let (store, git) = (app.store.clone(), app.git.clone());
        store.update(cx, |s, cx| s.set_git_panel_open(self.git_open, cx));
        git.update(cx, |g, cx| {
            g.panel_visible = self.git_open;
            if self.git_open {
                g.refresh(true, cx);
            }
        });
        cx.notify();
    }

    /// The right sidebar: a tab bar of sections (only Git today) with the sidebar toggle at
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
        let width = self.git_width;
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
            .child(h_flex().h_full().child(render_right_tab(
                "right-tab-git",
                Lucide::GitBranch,
                true,
                cx,
            )))
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
            .child(div().flex_1().min_h_0().child(self.git_panel.clone()));
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

/// An icon tab of the right sidebar's header; the active one is underlined. There is one
/// section so far, so a tab only shows where it is.
fn render_right_tab(id: &'static str, icon: Lucide, active: bool, cx: &App) -> impl IntoElement {
    let color = if active {
        cx.theme().foreground
    } else {
        cx.theme().muted_foreground
    };
    div()
        .id(id)
        .relative()
        .h_full()
        .px_2()
        .flex()
        .items_center()
        .cursor_pointer()
        .child(Icon::new(icon).small().text_color(color))
        .when(active, |el| {
            el.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(RIGHT_TAB_UNDERLINE)
                    .bg(cx.theme().primary),
            )
        })
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
