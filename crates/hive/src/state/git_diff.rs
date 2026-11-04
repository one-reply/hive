//! Loads review data off-thread and owns the presentation model consumed by a diff tab.
use super::{
    git::{DiffRequest, Git, GitEvent},
    store::Store,
};
use gpui_kit::component::diff::{DiffFile, DiffMode, DiffState};
use gpui_kit::*;
use hive_core::git::diff::{self, FileDiff};

pub(crate) struct GitDiff {
    pub request: DiffRequest,
    pub display: Entity<DiffState>,
    pub patch: Option<FileDiff>,
    pub error: Option<String>,
    pub loading: bool,
    pub active: bool,
    _task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl GitDiff {
    pub fn new(
        request: DiffRequest,
        git: Entity<Git>,
        store: Entity<Store>,
        cx: &mut Context<Self>,
    ) -> Self {
        let split = store.read(cx).state.settings.diff_split;
        let display = cx.new(|cx| {
            DiffState::new([], cx).with_mode(if split {
                DiffMode::Split
            } else {
                DiffMode::Unified
            })
        });
        let subscriptions = vec![
            cx.subscribe(&git, |this, git, event, cx| {
                if let GitEvent::Refreshed(target) = event
                    && this.active
                    && git.read(cx).window_active
                    && target == &this.request.target
                {
                    this.refresh(cx);
                }
            }),
            cx.observe(&store, |this, store, cx| {
                let mode = if store.read(cx).state.settings.diff_split {
                    DiffMode::Split
                } else {
                    DiffMode::Unified
                };
                this.display.update(cx, |d, cx| d.set_mode(mode, cx));
            }),
        ];
        let mut this = Self {
            request,
            display,
            patch: None,
            error: None,
            loading: false,
            active: false,
            _task: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
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
                        let changed = this.patch.as_ref() != Some(&patch) || this.error.is_some();
                        this.error = None;
                        if changed {
                            this.patch = Some(patch);
                            this.display.update(cx, |d, cx| d.set_files(files, cx));
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
            });
        }));
    }
}
