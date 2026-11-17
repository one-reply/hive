use super::*;
use gpui_kit::component::input::EditorState;
use std::path::Path;

pub(crate) struct SourcePreview {
    pub target: Target,
    pub path: PathBuf,
    pub position: Position,
    pub editor: Entity<EditorState>,
    pub loading: bool,
    pub error: Option<String>,
    _read: Option<Task<()>>,
}

impl Documents {
    pub(crate) fn buffer(
        &self,
        host: &hive_core::host::Host,
        path: &Path,
        cx: &App,
    ) -> Option<String> {
        self.open
            .iter()
            .filter_map(WeakEntity::upgrade)
            .find_map(|document| {
                let d = document.read(cx);
                (d.target.host == *host && d.target.path.join(&d.path) == path && d.base.is_some())
                    .then(|| d.text(cx))
            })
    }

    pub(crate) fn preview(
        &self,
        target: Target,
        path: PathBuf,
        position: Position,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<SourcePreview> {
        let buffer = self.buffer(&target.host, &path, cx);
        cx.new(|cx| SourcePreview::new(target, path, position, buffer, window, cx))
    }
}

impl SourcePreview {
    fn new(
        target: Target,
        path: PathBuf,
        position: Position,
        buffer: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = hive_core::files::editor_language(&path);
        let editor = cx.new(|cx| EditorState::new(window, cx).language(language));
        let (host, source) = (target.host.clone(), path.clone());
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = match buffer {
                Some(text) => Ok(text),
                None => {
                    crate::state::file_io::run(
                        cx.background_executor().clone(),
                        move |cancellation| {
                            let root = source
                                .parent()
                                .ok_or_else(|| "Source has no parent directory".to_owned())?;
                            let name = source
                                .file_name()
                                .ok_or_else(|| "Source has no filename".to_owned())?;
                            hive_core::files::Access::new(&host, cancellation)
                                .read(root, Path::new(name))
                                .map(|snapshot| snapshot.text())
                                .map_err(|e| e.to_string())
                        },
                    )
                    .await
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.loading = false;
                match result {
                    Ok(text) => {
                        let focus = window.focused(cx);
                        this.editor.update(cx, |editor, cx| {
                            editor.set_value(text, window, cx);
                            editor.set_cursor_position(this.position, window, cx);
                        });
                        if let Some(focus) = focus {
                            focus.focus(window, cx);
                        } else {
                            window.blur(cx);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error,"readonly source preview failed");
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        });
        Self {
            target,
            path,
            position,
            editor,
            loading: true,
            error: None,
            _read: Some(task),
        }
    }

    pub(crate) fn open_request(&self) -> Option<OpenFile> {
        Some(OpenFile {
            target: self.target.clone(),
            path: self.path.strip_prefix(&self.target.path).ok()?.into(),
            position: Some(self.position),
            preview: false,
            placement: Default::default(),
        })
    }
}
