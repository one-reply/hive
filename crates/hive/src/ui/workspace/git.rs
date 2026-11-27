//! Workspace integration for source control, diff and conflict tabs.
use super::*;
use crate::state::git::{ConflictRequest, DiffRequest, GitEvent};
use gpui_kit::component::tab::{Tab, TabBar};
use hive_core::projects::Target;
use hive_core::settings::clamp_git_panel_width;

impl Workspace {
    pub(super) fn sync_git_file(&self, cx: &mut Context<Self>) {
        let active = self
            .shown_center()
            .and_then(|center| center.active_git_file(cx));
        self.git_panel
            .update(cx, |panel, cx| panel.set_active_file(active, cx));
    }

    pub(super) fn watch_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = RightTab::selected(cx);
        let git = AppState::get(cx).git.clone();
        let mut absent = git.read(cx).has_no_repository();
        self._observers.push(cx.observe(&git, move |_, git, cx| {
            let current = git.read(cx).has_no_repository();
            if current != absent {
                absent = current;
                cx.notify();
            }
        }));
        git.update(cx, |g, _| {
            g.panel_visible = self.git_open && tab == RightTab::Git;
            g.files_visible = self.git_open && tab == RightTab::Files;
            g.window_active = window.is_window_active();
        });
        self._observers.push(cx.subscribe_in(
            &git,
            window,
            |this, _, event, window, cx| match event {
                GitEvent::Refreshed { .. } => {}
                GitEvent::OpenDiff(request) => this.open_diff(request.clone(), window, cx),
                GitEvent::OpenConflict(request) => {
                    this.open_conflict(request.clone(), window, cx)
                }
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
        if target.host.is_local()
            && let Some(slot) = self.pipeline_panels.get_mut(&target.project)
        {
            slot.hide_for_reveal();
        }
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

    fn open_conflict(
        &mut self,
        request: ConflictRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_target(&request.target, cx);
        let project = (request.target.host.clone(), request.target.project.clone());
        self.center(&project, window, cx)
            .show_conflict(request, window, cx);
        cx.notify();
    }

    pub(super) fn toggle_git(&mut self, cx: &mut Context<Self>) {
        self.git_open = !self.git_open;
        let store = AppState::get(cx).store.clone();
        store.update(cx, |s, cx| s.set_git_panel_open(self.git_open, cx));
        self.sync_right_tab_visibility(RightTab::selected(cx), cx);
        self.refresh_right_tab(cx);
        cx.notify();
    }

    /// The right sidebar: a tab bar for Git and Files with the sidebar toggle at
    /// its right end, then the selected section. Toggling slides it in from or out past the
    /// window's right edge: the content keeps its width and is clipped by an outer column
    /// whose width animates, the same way the left sidebar collapses.
    pub(super) fn render_right_sidebar(
        &self,
        openness: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toggle_hover = Hover::new("toggle-right-sidebar", window, cx);
        let border = cx.theme().border;
        let open = self.git_open;
        let selected_files = RightTab::selected(cx).is_files();
        let git_absent = AppState::get(cx).git.read(cx).has_no_repository();
        let width = self.git_width;
        let tab = |label: &'static str, icon: Lucide, choice, enabled: bool| {
            Tab::new()
                .prefix(Icon::new(icon).small())
                .label(label)
                .h(TAB_BAR_HEIGHT)
                .cursor_pointer()
                .disabled(!enabled)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| this.choose_right_tab(choice, cx)))
        };
        let header = h_flex()
            .id("right-sidebar-header")
            .h(TAB_BAR_HEIGHT)
            .flex_none()
            .child(
                TabBar::new("right-sidebar-tabs")
                    .underline()
                    .small()
                    .flex_1()
                    .h_full()
                    .px_2()
                    .selected_index(usize::from(selected_files))
                    .child(tab("Git", Lucide::GitBranch, RightTab::Git, !git_absent))
                    .child(tab("Files", Lucide::Folder, RightTab::Files, true))
                    .suffix(
                        right_sidebar_toggle()
                            .small()
                            .panel_toggle(open, &toggle_hover, cx)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
                    ),
            );
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
                self.sync_git_file(cx);
                super::cached(&self.git_panel).into_any_element()
            }));
        // Right-aligned so the content's left edge moves while its right edge stays put.
        v_flex()
            .h_full()
            .w(width * openness)
            .flex_shrink_0()
            .items_end()
            .overflow_hidden()
            .child(content)
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

/// The button that shows or hides the right sidebar. It stays in the window's top-right
/// corner: in the sidebar's header while it is open, in the center's tab bar (or over the
/// welcome screen) while it is closed. Callers pick the size.
pub(crate) fn right_sidebar_toggle() -> Button {
    Button::new("toggle-right-sidebar")
        .toggled(false)
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
