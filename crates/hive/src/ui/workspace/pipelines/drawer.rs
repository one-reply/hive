//! Project-local visibility, animated geometry and keyboard focus.
use super::*;
use gpui_kit::base::motion::{Transition, transition};

pub(in crate::ui::workspace) struct PanelSlot {
    pub(super) view: Entity<PipelinePanel>,
    pub(super) open: bool,
    pub(super) previous: Option<(PathBuf, Option<FocusHandle>)>,
}

impl PanelSlot {
    fn show(&mut self, cwd: &Path, window: &mut Window, cx: &mut App) {
        if !self.open {
            self.previous = Some((cwd.to_owned(), window.focused(cx)));
            self.open = true;
        }
        self.view.read(cx).focus.clone().focus(window, cx);
    }
    /// Returns true when the newly exposed dock needs a different terminal.
    pub(super) fn toggle(
        &mut self,
        cwd: &Path,
        fallback: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        if !self.open {
            self.show(cwd, window, cx);
            return false;
        }
        self.open = false;
        if let Some((previous_cwd, focus)) = self.previous.take() {
            if previous_cwd != cwd {
                return true;
            }
            focus.as_ref().unwrap_or(fallback).focus(window, cx);
        } else {
            fallback.focus(window, cx);
        }
        false
    }
    pub(in crate::ui::workspace) fn hide_for_reveal(&mut self) {
        self.previous = None;
        self.open = false;
    }
}
impl Workspace {
    pub(crate) fn pipelines_open(&self) -> bool {
        self.open_pipeline_panel().is_some()
    }

    pub(in crate::ui::workspace) fn open_pipeline_panel(&self) -> Option<Entity<PipelinePanel>> {
        let (Host::Local, project) = self.shown.as_ref()? else {
            return None;
        };
        let slot = self.pipeline_panels.get(project)?;
        slot.open.then(|| slot.view.clone())
    }

    fn pipeline_slot(&mut self, project: PathBuf, cx: &mut Context<Self>) -> &mut PanelSlot {
        self.pipeline_panels
            .entry(project.clone())
            .or_insert_with(|| PanelSlot {
                view: cx.new(|cx| PipelinePanel::new(project, cx)),
                open: false,
                previous: None,
            })
    }

    /// Workspace-wide shortcuts must expose the destination they focus.
    pub(in crate::ui::workspace) fn restore_dock(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pipelines_open() {
            self.toggle_pipelines(&TogglePipelines, window, cx);
        }
    }

    pub(in crate::ui::workspace) fn toggle_pipelines(
        &mut self,
        _: &TogglePipelines,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((Host::Local, project)) = self.shown.clone() else {
            window.push_notification(
                Notification::info("Select a local project to open Pipelines."),
                cx,
            );
            return;
        };
        let focus = self.focus.clone();
        // An agent reveal can show a project before the sidebar follows it.
        let selected = self
            .selected_target(cx)
            .filter(|(target, _)| self.shown.as_ref() == Some(target));
        let slot = self.pipeline_slot(project.clone(), cx);
        let cwd = selected
            .map(|(_, cwd)| cwd)
            .or_else(|| slot.previous.as_ref().map(|(cwd, _)| cwd.clone()))
            .unwrap_or_else(|| project.clone());
        if slot.toggle(&cwd, &focus, window, cx) {
            let project = (Host::Local, project);
            self.center(&project, window, cx)
                .show_terminal(&cwd, window, cx);
            self.sync_active_worktrees(cx);
            self.save_center(&project, window, cx);
        }
        cx.notify();
    }

    pub(in crate::ui::workspace) fn show_pipeline_run(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = AppState::get(cx)
            .store
            .read(cx)
            .state
            .pipelines
            .runs
            .iter()
            .find(|run| run.id() == id)
            .map(|r| r.owner().to_path_buf())
        else {
            return;
        };
        let registered = self
            .projects
            .read(cx)
            .items
            .iter()
            .any(|project| project.host == Host::Local && project.path == owner);
        if !registered {
            // Showing it anyway would leave the current project's panel on screen.
            tracing::warn!(
                run = id,
                ?owner,
                "pipeline run belongs to a removed project"
            );
            return;
        }
        self.projects.update(cx, |projects, cx| {
            projects.select_project(&Host::Local, &owner, cx)
        });
        self.sync_selection(window, cx);
        let cwd = self.last_selection.clone().unwrap_or_else(|| owner.clone());
        let slot = self.pipeline_slot(owner, cx);
        slot.show(&cwd, window, cx);
        slot.view
            .update(cx, |view, cx| view.show_run(id, window, cx));
        cx.notify();
    }

    pub(in crate::ui::workspace) fn pipeline_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(bool, AnyElement)> {
        let (Host::Local, project) = self.shown.as_ref()? else {
            return None;
        };
        let slot = self.pipeline_panels.get(project);
        let open = slot.is_some_and(|slot| slot.open);
        let key = ElementId::from(("pipeline-drawer", self.shown_center()?.dock().entity_id()));
        let t = transition(
            key,
            if open { 1.0_f32 } else { 0. },
            Transition::new(SIDEBAR_SLIDE_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let slot = slot?;
        if t <= 0. && !open {
            return None;
        }
        let header_inset = if self.sidebar_open {
            px(0.)
        } else {
            TAB_BAR_HEIGHT
        };
        let overlay = div()
            .id("pipeline-drawer")
            .absolute()
            .left_0()
            .top(relative(1. - t))
            .size_full()
            .occlude()
            .bg(cx.theme().background)
            .pt(header_inset)
            .child(slot.view.clone());
        Some((
            open && t >= 1.,
            self.window_drag_band(
                overlay,
                move |position| in_pipeline_title_band(position.y, header_inset),
                cx,
            )
            .into_any_element(),
        ))
    }
}

fn in_pipeline_title_band(y: Pixels, header_inset: Pixels) -> bool {
    y < header_inset + TAB_BAR_HEIGHT
}

#[cfg(test)]
mod tests {
    use super::{TAB_BAR_HEIGHT, in_pipeline_title_band, px};

    #[test]
    fn pipeline_title_band_reaches_the_breadcrumb_but_excludes_the_body() {
        for header_inset in [px(0.), TAB_BAR_HEIGHT] {
            assert!(in_pipeline_title_band(px(0.), header_inset));
            let body_top = header_inset + TAB_BAR_HEIGHT;
            assert!(in_pipeline_title_band(body_top - px(1.), header_inset));
            assert!(!in_pipeline_title_band(body_top, header_inset));
        }
    }
}
