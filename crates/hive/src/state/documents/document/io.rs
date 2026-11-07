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
        if self.content.preview().is_some() {
            self.refresh_image(window, cx);
            return;
        }
        self.loading = true;
        let (target, path) = (self.target.clone(), self.path.clone());
        self._read = Some(cx.spawn_in(window, async move |this, cx| {
            let result =
                crate::state::file_io::run(cx.background_executor().clone(), move |cancellation| {
                    files::Access::new(&target.host, cancellation).read(&target.path, &path)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match result {
                    Ok(snapshot) => this.accept_snapshot(snapshot, window, cx),
                    Err(error) => this.read_failed(error, cx),
                }
            });
        }));
    }

    pub(super) fn accept_snapshot(
        &mut self,
        snapshot: Snapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.missing = false;
        let text = self.dirty.then(|| self.text(cx));
        match disk_update(self.base.as_ref(), text.as_deref(), &snapshot) {
            DiskUpdate::Reload => self.install(snapshot, window, cx),
            DiskUpdate::MatchesBuffer => {
                self.error = None;
                self.base = Some(snapshot);
                self.dirty = false;
                self.conflict = None;
                self.flush_recovery(cx);
                self.emit(DocumentEvent::Changed(self.target.clone()), cx);
                cx.notify();
            }
            DiskUpdate::Conflict => {
                self.error = None;
                if self.conflict.as_ref() != Some(&snapshot) {
                    self.conflict = Some(snapshot);
                    cx.notify();
                }
            }
            DiskUpdate::Unchanged => {
                let changed = self.conflict.take().is_some();
                let read_error = matches!(
                    self.error,
                    Some(DocumentError::Read(_) | DocumentError::Save(_))
                );
                if read_error {
                    self.error = None;
                }
                if changed || read_error {
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn read_failed(&mut self, error: files::FileError, cx: &mut Context<Self>) {
        self.missing = matches!(&error, files::FileError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound);
        let error = DocumentError::Read(error.to_string());
        if self.error.as_ref() != Some(&error) {
            tracing::warn!(error = error.message(), "file refresh failed");
            self.error = Some(error);
            cx.notify();
        }
    }

    /// Completion is true only if this exact buffer is clean after the serialized write.
    pub fn save(
        &mut self,
        overwrite: Option<Snapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<bool> {
        let (done, receiver) = async_channel::bounded(1);
        // A focus-out save may already be writing when the user saves or closes.
        if self.saving {
            self.save_waiters.push(done);
            return receiver;
        }
        if self.content.is_raster()
            || (!self.target.host.is_local()
                && matches!(
                    self.error,
                    Some(DocumentError::Read(_) | DocumentError::Save(_))
                ))
            || self.path_busy
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
            let result =
                crate::state::file_io::run(cx.background_executor().clone(), move |cancellation| {
                    files::Access::new(&target.host, cancellation)
                        .save(&target.path, &path, &expected, &text)
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
                    let success = !this.dirty && this.error.is_none();
                    for waiter in this.save_waiters.drain(..) {
                        let _ = waiter.try_send(success);
                    }
                    success
                })
                .unwrap_or(false);
            let _ = done.try_send(success);
        }));
        cx.notify();
        receiver
    }
}

#[derive(Debug, PartialEq, Eq)]
enum DiskUpdate {
    Unchanged,
    Reload,
    MatchesBuffer,
    Conflict,
}

fn disk_update(base: Option<&Snapshot>, dirty_text: Option<&str>, disk: &Snapshot) -> DiskUpdate {
    if base == Some(disk) {
        return DiskUpdate::Unchanged;
    }
    match dirty_text {
        None => DiskUpdate::Reload,
        Some(text) if disk.matches(text) => DiskUpdate::MatchesBuffer,
        Some(_) => DiskUpdate::Conflict,
    }
}

#[cfg(test)]
mod tests {
    use super::{DiskUpdate, Snapshot, disk_update};
    use std::path::Path;

    #[test]
    fn disk_refresh_preserves_unsaved_svg_and_recognizes_external_save() {
        let old = Snapshot::parse(Path::new("a.svg"), b"<svg/>").unwrap();
        let external = Snapshot::parse(Path::new("a.svg"), b"<svg width=\"10\"/>").unwrap();
        assert_eq!(disk_update(None, None, &old), DiskUpdate::Reload);
        assert_eq!(disk_update(Some(&old), None, &external), DiskUpdate::Reload);
        assert_eq!(
            disk_update(Some(&old), Some("mine"), &external),
            DiskUpdate::Conflict
        );
        assert_eq!(
            disk_update(Some(&old), Some("mine"), &old),
            DiskUpdate::Unchanged
        );
        assert_eq!(
            disk_update(Some(&old), Some(&external.text()), &external),
            DiskUpdate::MatchesBuffer
        );
    }
}
