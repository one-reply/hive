use super::*;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileOperation {
    CreateFile(PathBuf),
    CreateDirectory(PathBuf),
    Move {
        from: PathBuf,
        to: PathBuf,
    },
    Delete(PathBuf),
    /// Copy `sources`, absolute paths on this Mac, into the worktree folder `directory`.
    Paste {
        directory: PathBuf,
        sources: Arc<[PathBuf]>,
    },
}

impl FileOperation {
    pub fn path(&self) -> &Path {
        match self {
            Self::CreateFile(path) | Self::CreateDirectory(path) | Self::Delete(path) => path,
            Self::Move { from, .. } => from,
            Self::Paste { directory, .. } => directory,
        }
    }

    /// The entry whose open documents must hold still while this runs. A paste only adds
    /// entries under names nothing uses yet, so the files of its folder stay editable.
    fn changed_path(&self) -> Option<&Path> {
        match self {
            Self::Paste { .. } => None,
            _ => Some(self.path()),
        }
    }

    /// Paths that must not be opened or closed while the operation runs.
    fn locked_paths(&self) -> impl Iterator<Item = &Path> {
        let destination = match self {
            Self::Move { to, .. } => Some(to.as_path()),
            _ => None,
        };
        self.changed_path().into_iter().chain(destination)
    }

    /// Runs the operation and returns the entries it created.
    fn run(
        &self,
        target: &Target,
        cancellation: &hive_core::command::Cancellation,
    ) -> Result<Vec<PathBuf>, files::FileError> {
        let access = files::Access::new(&target.host, cancellation);
        match self {
            Self::CreateFile(path) => access
                .create_file(&target.path, path)
                .map(|()| vec![path.clone()]),
            Self::CreateDirectory(path) => access
                .create_directory(&target.path, path)
                .map(|()| vec![path.clone()]),
            Self::Move { from, to } => access
                .move_entry(&target.path, from, to)
                .map(|()| vec![to.clone()]),
            Self::Delete(path) => access.delete_entry(&target.path, path).map(|()| Vec::new()),
            Self::Paste { directory, sources } => access.paste(&target.path, directory, sources),
        }
    }
}

impl Files {
    pub(crate) fn perform(
        &mut self,
        operation: FileOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.target.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        let registry = AppState::get(cx).documents.clone();
        let documents = operation
            .changed_path()
            .map(|path| registry.read(cx).in_path(&target, path, cx))
            .unwrap_or_default();
        let conflict = documents
            .iter()
            .any(|d| d.read(cx).saving || d.read(cx).path_busy);
        let destination_open = if let FileOperation::Move { to, .. } = &operation {
            !registry.read(cx).in_path(&target, to, cx).is_empty()
        } else {
            false
        };
        if conflict || destination_open {
            cx.emit(FilesEvent {
                target,
                operation,
                result: Err(if destination_open {
                    "The destination has an open document. Close it before moving a file here."
                } else {
                    "A document is saving or moving. Try again when it finishes."
                }
                .into()),
                warning: None,
            });
            return;
        }
        for (index, document) in documents.iter().enumerate() {
            if let Err(error) = document.update(cx, |d, cx| d.lock_path(cx)) {
                tracing::error!(%error, "file operation could not lock document");
                for locked in &documents[..index] {
                    locked.update(cx, |d, cx| d.unlock_path(cx));
                }
                cx.emit(FilesEvent {
                    target,
                    operation,
                    result: Err(error),
                    warning: None,
                });
                return;
            }
        }
        self._read = None;
        self.generation += 1;
        self.loading = false;
        self.busy = true;
        registry.update(cx, |registry, _| {
            lock_paths(&mut registry.mutating_paths, &target, &operation)
        });
        self._operation = Some(cx.spawn_in(window, async move |this, cx| {
            let (background_operation, background_target) = (operation.clone(), target.clone());
            let result = crate::state::file_io::run(cx.background_executor().clone(), move |cancellation| {
                background_operation.run(&background_target, cancellation).map_err(|e| e.to_string())
            }).await;
            let mut warning = None;
            if result.is_ok() && let FileOperation::Move { from, to } = &operation {
                    for document in &documents {
                        let completion = document.update(cx, |d, cx| {
                            let path = files::relocated_path(&d.path, from, to).expect("documents were selected inside the source path");
                            d.relocate(path, cx)
                        });
                        let outcome = completion.recv().await;
                        if !matches!(outcome, Ok(Ok(()))) {
                            tracing::error!(?outcome, "file recovery relocation failed");
                            warning = Some("File moved, but recovery could not be relocated. The editor text and previous recovery record have been retained.".to_owned());
                        }
                    }
                }
            let _ = this.update_in(cx, |this, window, cx| {
                for document in &documents {
                    document.update(cx, |document, cx| {
                        if result.is_ok() && matches!(operation, FileOperation::Delete(_)) {
                            document.deleted(window, cx);
                        } else { document.unlock_path(cx); }
                    });
                }
                this.busy = false;
                registry.update(cx, |registry, _| {
                    unlock_paths(&mut registry.mutating_paths, &target, &operation)
                });
                if result.is_ok() {
                    update_saved_paths(&target, &operation, cx);
                    if let FileOperation::Delete(path) = &operation {
                        let registry = registry.clone();
                        let (target, path) = (target.clone(), path.clone());
                        cx.defer(move |cx| registry.update(cx, |registry, cx| registry.emit(DocumentEvent::Deleted(target, path), cx)));
                    }
                    match &operation {
                        FileOperation::CreateFile(path) => { this.reveal(path.clone(), cx); this.open(path.clone(), false, cx); }
                        FileOperation::CreateDirectory(path) => { this.expand(path.clone(), cx); this.reveal(path.clone(), cx); }
                        FileOperation::Move { from, to } => {
                            this.expanded = this.expanded.iter().map(|path| files::relocated_path(path, from, to).unwrap_or_else(|| path.clone())).collect();
                            this.reveal(to.clone(), cx);
                        }
                        FileOperation::Delete(path) => this.expanded.retain(|p| !p.starts_with(path)),
                        FileOperation::Paste { directory, .. } => {
                            if !directory.as_os_str().is_empty() {
                                this.expand(directory.clone(), cx);
                            }
                            if let Some(first) = result.as_ref().ok().and_then(|created| created.first()) {
                                this.reveal(first.clone(), cx);
                            }
                        }
                    }
                } else if let Err(error) = &result {
                    tracing::warn!(%error, path = ?target.path, "file operation failed");
                }
                this.sync_documents(false, cx);
                this.refresh_all(cx);
                // A query typed during the mutation has been waiting for it.
                this.search(cx);
                let result = result.map(|_| ());
                cx.emit(FilesEvent { target, operation, result, warning });
                cx.notify();
            });
        }));
        cx.notify();
    }
}

fn lock_paths(locked: &mut Vec<(Target, PathBuf)>, target: &Target, operation: &FileOperation) {
    locked.extend(
        operation
            .locked_paths()
            .map(|path| (target.clone(), path.to_owned())),
    );
}

/// Releases the paths `operation` locked and no others: every worktree has its own
/// navigator, so an operation in another worktree may still be running.
fn unlock_paths(locked: &mut Vec<(Target, PathBuf)>, target: &Target, operation: &FileOperation) {
    locked.retain(|(owner, path)| {
        owner != target || !operation.locked_paths().any(|own| own == path)
    });
}

fn update_saved_paths(target: &Target, operation: &FileOperation, cx: &mut App) {
    use hive_core::state::workspace::Tab;
    if !matches!(
        operation,
        FileOperation::Move { .. } | FileOperation::Delete(_)
    ) {
        return;
    }
    let store = AppState::get(cx).store.clone();
    let projects = store.read(cx).state.workspace.projects.clone();
    for mut project in projects.into_iter().filter(|p| p.host == target.host) {
        if let Some(layout) = &project.layout {
            project.layout = layout.filter_map(&mut |tabs| {
                let mut tabs = tabs.clone();
                let old_active = tabs.items.get(tabs.active).cloned();
                tabs.items.retain_mut(|tab| {
                    if let Tab::File { worktree, path, .. } = tab
                        && *worktree == target.path
                    {
                        match operation {
                            FileOperation::Move { from, to } => {
                                if let Some(relocated) = files::relocated_path(path, from, to) {
                                    *path = relocated;
                                }
                            }
                            FileOperation::Delete(deleted) if path.starts_with(deleted) => {
                                return false;
                            }
                            _ => {}
                        }
                    }
                    true
                });
                tabs.active = old_active
                    .and_then(|active| tabs.items.iter().position(|tab| *tab == active))
                    .unwrap_or(tabs.active.min(tabs.items.len().saturating_sub(1)));
                (!tabs.items.is_empty()).then_some(tabs)
            });
            store.update(cx, |store, cx| store.set_project_workspace(project, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileOperation, lock_paths, unlock_paths};
    use hive_core::{command::Cancellation, host::Host, projects::Target};
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::Arc,
    };

    struct TemporaryDirectory(PathBuf);
    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_paste_locks_no_open_documents_and_reports_the_entries_it_created() {
        let temporary = TemporaryDirectory(
            std::env::temp_dir().join(format!("hive-state-paste-{}", std::process::id())),
        );
        let worktree = temporary.0.join("worktree");
        fs::create_dir_all(worktree.join("src")).unwrap();
        fs::write(worktree.join("src/notes.md"), "mine").unwrap();
        fs::write(temporary.0.join("notes.md"), "copied in Finder").unwrap();
        let target = Target {
            project: worktree.clone(),
            path: worktree.clone(),
            host: Host::Local,
        };
        let paste = FileOperation::Paste {
            directory: "src".into(),
            sources: Arc::from([temporary.0.join("notes.md")]),
        };

        assert_eq!(paste.path(), Path::new("src"));
        assert_eq!(paste.changed_path(), None);
        let mut locked = Vec::new();
        lock_paths(&mut locked, &target, &paste);
        assert!(locked.is_empty(), "files of the folder stay open to edits");
        let created = paste.run(&target, &Cancellation::default()).unwrap();

        assert_eq!(created, [PathBuf::from("src/notes copy.md")]);
        assert_eq!(
            fs::read_to_string(worktree.join("src/notes.md")).unwrap(),
            "mine"
        );
        let delete = FileOperation::Delete("src".into());
        assert_eq!(delete.changed_path(), Some(Path::new("src")));
    }

    fn worktree(path: &str) -> Target {
        Target {
            host: Host::Local,
            project: "/project".into(),
            path: path.into(),
        }
    }

    #[test]
    fn a_finished_operation_releases_only_its_own_paths() {
        let (first, second) = (worktree("/project"), worktree("/worktree"));
        let moving = FileOperation::Move {
            from: "src/a.rs".into(),
            to: "src/b.rs".into(),
        };
        let deleting = FileOperation::Delete("src/a.rs".into());
        let mut locked = Vec::new();
        lock_paths(&mut locked, &first, &moving);
        lock_paths(&mut locked, &second, &deleting);
        assert_eq!(locked.len(), 3);

        unlock_paths(&mut locked, &first, &moving);
        // The same relative path is still being deleted in the other worktree.
        assert_eq!(locked, [(second.clone(), PathBuf::from("src/a.rs"))]);

        unlock_paths(&mut locked, &second, &deleting);
        assert!(locked.is_empty());
    }
}
