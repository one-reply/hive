//! Save layout changes through Store and restore each project's dock on first use.
use super::*;
use hive_core::state::workspace::ProjectWorkspace;

impl Workspace {
    pub(super) fn saved_center(
        &self,
        project: &(Host, PathBuf),
        cx: &App,
    ) -> Option<ProjectWorkspace> {
        let host = &project.0;
        AppState::get(cx)
            .store
            .read(cx)
            .state
            .workspace
            .projects
            .iter()
            .find(|saved| saved.project == project.1 && &saved.host == host)
            .cloned()
    }

    pub(super) fn save_center(
        &mut self,
        project: &(Host, PathBuf),
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let mut saved = self
            .saved_center(project, cx)
            .unwrap_or_else(|| ProjectWorkspace {
                project: project.1.clone(),
                host: project.0.clone(),
                worktree: None,
                focused: 0,
                layout: None,
            });
        if let Some((selected, cwd)) = self.selected_target(cx)
            && &selected == project
        {
            saved.worktree = Some(cwd);
        }
        let Some(center) = self.centers.get_mut(project) else {
            return;
        };
        center.snapshot(&mut saved, window, cx);
        AppState::get(cx)
            .store
            .clone()
            .update(cx, |store, cx| store.set_project_workspace(saved, cx));
    }
}
