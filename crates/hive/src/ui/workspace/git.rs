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

    pub(super) fn render_git_column(&self) -> impl IntoElement {
        v_flex()
            .h_full()
            .w(self.git_width)
            .flex_shrink_0()
            .border_l_1()
            .child(self.git_panel.clone())
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

struct GitResize;
impl Render for GitResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}
