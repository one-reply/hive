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
        let previous = self.patch.clone();
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
                    let files = changed_display(&patch, previous.as_deref())?;
                    Ok::<_, String>((patch, files))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok((patch, files)) => {
                        this.error = None;
                        if let Some(files) = files {
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

fn changed_display(
    patch: &FileDiff,
    previous: Option<&FileDiff>,
) -> Result<Option<Vec<DiffFile>>, String> {
    if previous == Some(patch) {
        return Ok(None);
    }
    let files = DiffFile::parse(patch.patch())
        .map_err(|e| format!("Cannot display this patch: {e}. Open it in the terminal."))?;
    Ok(Some(files))
}

#[cfg(test)]
mod tests {
    use super::changed_display;
    use hive_core::{
        git::diff::{self, DiffSource},
        host::Host,
    };
    use std::{fs, path::PathBuf};

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn unchanged_patch_skips_display_but_status_does_not_hide_content_changes() {
        let fixture =
            Fixture(std::env::temp_dir().join(format!("hive-diff-display-{}", std::process::id())));
        fs::create_dir(&fixture.0).unwrap();
        assert!(
            Host::Local
                .command(
                    "git",
                    &["init", "-q", "-b", "main", "--template="],
                    Some(&fixture.0)
                )
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap()
                .status
                .success()
        );
        let read = || {
            diff::read(
                &Host::Local,
                &fixture.0,
                "sample.txt",
                &DiffSource::Untracked,
                None,
            )
            .unwrap()
        };
        fs::write(fixture.0.join("sample.txt"), "first\n").unwrap();
        let initial = read();
        let status = hive_core::git::status::read(&Host::Local, &fixture.0).unwrap();
        assert!(changed_display(&initial, None).unwrap().is_some());
        assert!(changed_display(&read(), Some(&initial)).unwrap().is_none());

        fs::write(fixture.0.join("sample.txt"), "second\n").unwrap();
        assert_eq!(
            status,
            hive_core::git::status::read(&Host::Local, &fixture.0).unwrap()
        );
        let updated = read();
        assert!(changed_display(&updated, Some(&initial)).unwrap().is_some());
        assert!(changed_display(&read(), Some(&updated)).unwrap().is_none());
        assert!(
            diff::read(
                &Host::Local,
                &fixture.0.join("missing"),
                "sample.txt",
                &DiffSource::Untracked,
                None
            )
            .is_err()
        );
        // Refresh clears the previous patch on I/O failure, so recovery must parse again.
        assert!(changed_display(&read(), None).unwrap().is_some());
    }
}
