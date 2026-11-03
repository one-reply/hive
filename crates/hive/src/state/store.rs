use gpui_kit::*;
use hive_core::host::Host;
use hive_core::settings::ThemeChoice;
use hive_core::state::{ProjectRef, State};
use std::path::{Path, PathBuf};

/// Persisted state. Every mutation saves to disk and notifies observers.
pub struct Store {
    pub state: State,
}

impl Store {
    pub fn load() -> Self {
        Self {
            state: State::load(),
        }
    }

    pub fn set_theme(&mut self, theme: ThemeChoice, cx: &mut Context<Self>) {
        self.state.settings.theme = theme;
        self.commit(cx);
    }

    pub fn set_sidebar_width(&mut self, width: f32, cx: &mut Context<Self>) {
        self.state.settings.set_sidebar_width(width);
        self.commit(cx);
    }

    pub fn add_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let project = ProjectRef::new(path, Host::Local);
        if !self
            .state
            .projects
            .iter()
            .any(|p| p.is_same_folder(&project))
        {
            self.state.projects.push(project);
            self.commit(cx);
        }
    }

    pub fn remove_project(&mut self, path: &Path, cx: &mut Context<Self>) {
        let before = self.state.projects.len();
        self.state.projects.retain(|p| p.path != path);
        if self.state.projects.len() != before {
            self.commit(cx);
        }
    }

    /// Switches a project between listing only Hive's worktrees and listing all of them.
    pub fn toggle_worktree_filter(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Some(project) = self.state.projects.iter_mut().find(|p| p.path == path) {
            project.worktrees = project.worktrees.toggled();
            self.commit(cx);
        }
    }

    /// Native folder picker; every chosen folder becomes a project.
    pub fn prompt_add_projects(store: Entity<Store>, cx: &mut App) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn(async move |cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = cx.update(|cx| {
                    store.update(cx, |store, cx| {
                        for path in paths {
                            store.add_project(path, cx);
                        }
                    })
                });
            }
        })
        .detach();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        self.state.save();
        cx.notify();
    }
}
