use super::*;

impl Document {
    pub fn changes(&self, cx: &App) -> Task<Result<Vec<crate::state::git::DiffRequest>, String>> {
        let (target, path) = (
            self.target.clone(),
            self.path.to_string_lossy().into_owned(),
        );
        cx.background_executor().spawn(async move {
            use hive_core::git::{diff::DiffSource, status};
            let status = status::read(&target.host, &target.path).map_err(|e| e.to_string())?;
            let mut requests = Vec::new();
            if let Some(file) = status.files.into_iter().find(|file| file.path == path) {
                let mut sources = Vec::new();
                if let Some(change) = file.unstaged {
                    sources.push(if change == status::Change::Untracked {
                        DiffSource::Untracked
                    } else {
                        DiffSource::WorkingTree
                    });
                }
                if file.staged.is_some() {
                    sources.push(DiffSource::Staged);
                }
                for source in sources {
                    requests.push(crate::state::git::DiffRequest {
                        target: target.clone(),
                        path: file.path.clone(),
                        old_path: file.old_path.clone(),
                        source,
                        whole_file_only: file.submodule,
                    });
                }
            }
            Ok(requests)
        })
    }

    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.saving || self.path_busy {
            return;
        }
        self.loading = true;
        let (target, path) = (self.target.clone(), self.path.clone());
        self._read = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    files::read(&target.host, &target.path, &path)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match result {
                    Ok(snapshot) => {
                        this.missing = false;
                        if this.base.as_ref() != Some(&snapshot) {
                            if this.dirty {
                                if snapshot.matches(&this.text(cx)) {
                                    this.base = Some(snapshot);
                                    this.dirty = false;
                                    this.conflict = None;
                                    this.flush_recovery(cx);
                                    this.emit(DocumentEvent::Changed(this.target.clone()), cx);
                                    cx.notify();
                                } else if this.conflict.as_ref() != Some(&snapshot) {
                                    this.conflict = Some(snapshot);
                                    cx.notify();
                                }
                            } else {
                                this.install(snapshot, window, cx);
                            }
                        } else if this.conflict.take().is_some() || matches!(this.error, Some(DocumentError::Read(_))) {
                            if matches!(this.error, Some(DocumentError::Read(_))) { this.error = None; }
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        this.missing = matches!(&error, files::FileError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound);
                        let error = DocumentError::Read(error.to_string());
                        if this.error.as_ref() != Some(&error) {
                            tracing::warn!(error = error.message(), "file refresh failed");
                            this.error = Some(error);
                            cx.notify();
                        }
                    }
                }
            });
        }));
    }

    /// Completion is true only if this exact buffer is clean after the serialized write.
    pub fn save(
        &mut self,
        overwrite: Option<Snapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<bool> {
        let (done, receiver) = async_channel::bounded(1);
        if self.path_busy
            || self.saving
            || self.base.is_none()
            || (self.conflict.is_some() && overwrite.is_none())
        {
            let _ = done.try_send(false);
            return receiver;
        }
        let Some(expected) = overwrite.or_else(|| self.base.clone()) else {
            return receiver;
        };
        // A pending read must not install an old version after this write completes.
        self._read = None;
        self.loading = false;
        self.saving = true;
        self.error = None;
        let (target, path, text) = (self.target.clone(), self.path.clone(), self.text(cx));
        self._save = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    files::save(&target.host, &target.path, &path, &expected, &text)
                        .map_err(|e| e.to_string())
                })
                .await;
            let success = this
                .update_in(cx, |this, window, cx| {
                    this.saving = false;
                    match result {
                        Ok(snapshot) => {
                            this.base = Some(snapshot);
                            this.conflict = None;
                            this.update_dirty(cx);
                            this._backup = None;
                            this.flush_recovery(cx);
                            this.emit(DocumentEvent::Saved(this.target.clone()), cx);
                        }
                        Err(error) => {
                            tracing::warn!(%error, "file save failed");
                            this.error = Some(DocumentError::Save(error));
                            this.refresh(window, cx);
                        }
                    }
                    cx.notify();
                    !this.dirty && this.error.is_none()
                })
                .unwrap_or(false);
            let _ = done.try_send(success);
        }));
        cx.notify();
        receiver
    }
}
