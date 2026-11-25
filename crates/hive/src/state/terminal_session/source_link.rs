//! Resolve compiler output on the terminal's host, independently of the selected worktree.
use super::*;
use crate::{app::AppState, state::file_io};
use hive_core::{
    files::{self, location},
    projects::Target,
};
use std::path::Path;

type SourceMatches = Task<Result<Vec<PathBuf>, String>>;

impl TerminalSession {
    pub(crate) fn source_files(&self, name: &Path, cx: &App) -> Option<(Target, SourceMatches)> {
        let worktree = self.worktree.as_ref()?;
        let project = AppState::get(cx).projects.read(cx).items.iter().find(|p| {
            p.host == self.host
                && (&p.path == worktree || p.worktrees.iter().any(|w| &w.path == worktree))
        })?;
        let target = Target {
            host: self.host.clone(),
            project: project.path.clone(),
            path: worktree.clone(),
        };
        let (host, root, name) = (target.host.clone(), target.path.clone(), name.to_owned());
        let directory = self.directory.clone();
        let executor = cx.background_executor().clone();
        let task = cx
            .background_executor()
            .spawn(file_io::run(executor, move |cancel| {
                if name.is_absolute() {
                    return Ok(location::candidates(&[], &root, &name));
                }
                let access = files::Access::new(&host, cancel);
                let index = access
                    .list_files(&root)
                    .map_err(|error| error.to_string())?;
                let mut paths = location::candidates(&index.paths, &root, &name);
                // Ignored source files at a known directory can still be opened, like in Files.
                for dir in std::iter::once(root.as_path()).chain(directory.as_deref()) {
                    let direct = dir.join(&name);
                    let Some(relative) = location::candidates(&[], &root, &direct).pop() else {
                        continue;
                    };
                    if paths.contains(&relative) {
                        continue;
                    }
                    let parent = relative.parent().unwrap_or(Path::new(""));
                    if access.list_directory(&root, parent).is_ok_and(|entries| {
                        entries.iter().any(|entry| {
                            entry.path == relative && !entry.directory && !entry.unsupported
                        })
                    }) {
                        paths.push(relative);
                    }
                }
                if index.truncated && paths.len() < 2 {
                    return Err(
                        "The worktree file index is incomplete. Open the source by its full path."
                            .into(),
                    );
                }
                paths.sort();
                Ok(paths)
            }));
        Some((target, task))
    }
}
