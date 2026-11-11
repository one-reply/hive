use super::*;

impl Document {
    pub(crate) fn lock_path(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        if self.saving || self.path_busy {
            return Err("A file operation is already running. Try again when it finishes.".into());
        }
        self.flush_recovery(cx);
        self._read = None;
        self._backup = None;
        self.invalidate_image(cx);
        self.loading = false;
        self.path_busy = true;
        self.editor
            .update(cx, |editor, cx| editor.set_disabled(true, cx));
        cx.notify();
        Ok(())
    }

    pub(crate) fn unlock_path(&mut self, cx: &mut Context<Self>) {
        self.path_busy = false;
        self.editor
            .update(cx, |editor, cx| editor.set_disabled(false, cx));
        self.flush_recovery(cx);
        self.render_image(cx);
        cx.notify();
    }

    pub(crate) fn relocate(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> async_channel::Receiver<Result<(), String>> {
        self.path = path;
        let content = Content::new(&self.path, self.dirty);
        if std::mem::discriminant(&self.content) != std::mem::discriminant(&content) {
            self.invalidate_image(cx);
            if self.content.is_raster() || content.is_raster() {
                self.base = None;
                self.conflict = None;
            }
            self.content = content;
            self.error = None;
        }
        let old_key = std::mem::replace(
            &mut self.recovery_key,
            recovery::key(&self.target.host, &self.target.path, &self.path),
        );
        let language = files::editor_language(&self.path);
        self.editor
            .update(cx, |editor, cx| editor.set_highlighter(language, cx));
        self.completions.schedule_refresh(cx);
        self.invalidate_blame(cx);
        let draft = self.base.as_ref().filter(|_| self.dirty).map(|base| Draft {
            host: self.target.host.clone(),
            project: self.target.project.clone(),
            root: self.target.path.clone(),
            path: self.path.clone(),
            base: base.clone(),
            text: self.text(cx),
        });
        let done =
            self.store
                .read(cx)
                .replace_recovery(self.recovery_key.clone(), draft, vec![old_key]);
        self.emit(DocumentEvent::Changed(self.target.clone()), cx);
        cx.notify();
        done
    }

    pub(crate) fn deleted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.discard(window, cx);
        self.invalidate_image(cx);
        self.missing = true;
        self.path_busy = true;
        cx.notify();
    }

    pub(super) fn emit(&self, event: DocumentEvent, cx: &mut Context<Self>) {
        let documents = crate::app::AppState::get(cx).documents.clone();
        cx.defer(move |cx| documents.update(cx, |documents, cx| documents.emit(event, cx)));
    }
}
