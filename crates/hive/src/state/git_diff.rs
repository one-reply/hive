//! Loads review data off-thread and holds what a diff tab shows: the raw patch and its
//! parsed files. The widget that displays them belongs to the panel.
use super::git::{DiffRequest, Git, GitEvent};
use gpui_kit::component::diff::DiffFile;
use gpui_kit::*;
use hive_core::git::diff::{self, FileDiff};
use std::sync::Arc;

pub(crate) struct GitDiff {
    pub request: DiffRequest,
    pub patch: Option<Arc<FileDiff>>,
    /// The patch as parsed for display; `files_generation` changes with it.
    files: Vec<DiffFile>,
    files_generation: u64,
    pub error: Option<String>,
    pub loading: bool,
    /// Whether a panel shows this diff; only an active diff follows Git refreshes.
    active: bool,
    /// A refresh requested while one was loading; served when the load completes.
    pending: bool,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl GitDiff {
    pub fn new(request: DiffRequest, git: Entity<Git>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.subscribe(&git, |this, git, event, cx| {
            if let GitEvent::Refreshed(target) = event
                && this.active
                && git.read(cx).window_active
                && target == &this.request.target
            {
                this.refresh(cx);
            }
        })];
        let mut this = Self {
            request,
            patch: None,
            files: Vec::new(),
            files_generation: 0,
            error: None,
            loading: false,
            active: false,
            pending: false,
            _task: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    /// The parsed patch the panel should display.
    pub fn files(&self) -> &[DiffFile] {
        &self.files
    }

    /// Changes whenever [`GitDiff::files`] does, so a panel can skip reloading its widget.
    pub fn files_generation(&self) -> u64 {
        self.files_generation
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.pending = true;
            return;
        }
        self.loading = true;
        let request = self.request.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let patch = diff::read(
                        &request.target.host,
                        &request.target.path,
                        &request.path,
                        &request.source,
                        request.old_path.as_deref(),
                    )
                    .map_err(|e| e.to_string())?;
                    let files = DiffFile::parse(patch.patch()).map_err(|e| {
                        format!("Cannot display this patch: {e}. Open it in the terminal.")
                    })?;
                    Ok::<_, String>((patch, files))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((patch, files)) => {
                        let changed = this.patch.as_deref() != Some(&patch) || this.error.is_some();
                        this.error = None;
                        if changed {
                            this.patch = Some(Arc::new(patch));
                            this.files = files;
                            this.files_generation += 1;
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        tracing::warn!(path = %this.request.path, %error, "diff preview failed");
                        this.patch = None;
                        if this.error.as_ref() != Some(&error) {
                            this.error = Some(error);
                            cx.notify();
                        }
                    }
                }
                if std::mem::take(&mut this.pending) {
                    this.refresh(cx);
                }
            });
        }));
    }
}
