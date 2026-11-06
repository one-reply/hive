use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FileOperation {
    CreateFile(PathBuf),
    CreateDirectory(PathBuf),
    Move { from: PathBuf, to: PathBuf },
    Delete(PathBuf),
}

impl FileOperation {
    pub fn path(&self) -> &Path {
        match self {
            Self::CreateFile(path) | Self::CreateDirectory(path) | Self::Delete(path) => path,
            Self::Move { from, .. } => from,
        }
    }

    fn run(&self, target: &Target) -> Result<(), files::FileError> {
        match self {
            Self::CreateFile(path) => files::create_file(&target.host, &target.path, path),
            Self::CreateDirectory(path) => {
                files::create_directory(&target.host, &target.path, path)
            }
            Self::Move { from, to } => files::move_entry(&target.host, &target.path, from, to),
            Self::Delete(path) => files::delete_entry(&target.host, &target.path, path),
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
        let documents = registry.read(cx).in_path(&target, operation.path(), cx);
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
            registry
                .mutating_paths
                .push((target.clone(), operation.path().to_owned()));
            if let FileOperation::Move { to, .. } = &operation {
                registry.mutating_paths.push((target.clone(), to.clone()));
            }
        });
        self._operation = Some(cx.spawn_in(window, async move |this, cx| {
            let (background_operation, background_target) = (operation.clone(), target.clone());
            let result = cx.background_executor().spawn(async move {
                background_operation.run(&background_target).map_err(|e| e.to_string())
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
                registry.update(cx, |registry, _| registry.mutating_paths.clear());
                if result.is_ok() {
                    update_saved_paths(&target, &operation, cx);
                    if let FileOperation::Delete(path) = &operation {
                        let registry = registry.clone();
                        let (target, path) = (target.clone(), path.clone());
                        cx.defer(move |cx| registry.update(cx, |registry, cx| registry.emit(DocumentEvent::Deleted(target, path), cx)));
                    }
                    if this.target.as_ref() == Some(&target) {
                        match &operation {
                            FileOperation::CreateFile(path) => { this.reveal(path.clone(), cx); this.open(path.clone(), false, cx); }
                            FileOperation::CreateDirectory(path) => { this.expand(path.clone(), cx); this.reveal(path.clone(), cx); }
                            FileOperation::Move { from, to } => {
                                this.expanded = this.expanded.iter().map(|path| files::relocated_path(path, from, to).unwrap_or_else(|| path.clone())).collect();
                                this.reveal(to.clone(), cx);
                            }
                            FileOperation::Delete(path) => this.expanded.retain(|p| !p.starts_with(path)),
                        }
                    }
                } else if let Err(error) = &result {
                    tracing::warn!(%error, path = ?target.path, "file operation failed");
                }
                if this.target.as_ref() == Some(&target) {
                    this.sync_documents(false, cx);
                    this.refresh_all(cx);
                }
                // A query in another worktree may have been waiting for this mutation.
                this.search(cx);
                cx.emit(FilesEvent { target, operation, result, warning });
                cx.notify();
            });
        }));
        cx.notify();
    }
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
