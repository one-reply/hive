use super::*;

impl Files {
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.busy {
            self.pending_refresh = true;
            return;
        }
        let Some(target) = self.target.clone() else {
            return;
        };
        if target.host != hive_core::host::Host::Local {
            self.error = Some(
                "Files is available for local worktrees. SSH file browsing is not supported yet."
                    .into(),
            );
            cx.notify();
            return;
        }
        self.loading = true;
        let generation = self.generation;
        let paths: Vec<_> = std::iter::once(PathBuf::new())
            .chain(self.expanded.iter().cloned())
            .collect();
        let git = AppState::get(cx).git.read(cx);
        let is_git = git.target.as_ref() == Some(&target) && git.status.is_some();
        self._read = Some(cx.spawn(async move |this, cx| {
            let read_target = target.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut directories = BTreeMap::new();
                    let mut errors = Vec::new();
                    for path in paths {
                        match files::list_directory(&read_target.host, &read_target.path, &path) {
                            Ok(entries) => {
                                directories.insert(path, entries);
                            }
                            Err(error) if path.as_os_str().is_empty() => {
                                return Err(error.to_string());
                            }
                            Err(error) => {
                                directories.insert(path, Vec::new());
                                errors.push(error.to_string());
                            }
                        }
                    }
                    let paths = directories
                        .values()
                        .flatten()
                        .map(|entry| entry.path.clone())
                        .collect::<Vec<_>>();
                    let ignored = if is_git {
                        match files::decorations::ignored(
                            &read_target.host,
                            &read_target.path,
                            &paths,
                        ) {
                            Ok(ignored) => ignored,
                            Err(error) => {
                                errors.push(error.to_string());
                                HashSet::new()
                            }
                        }
                    } else {
                        HashSet::new()
                    };
                    Ok((
                        directories,
                        ignored,
                        (!errors.is_empty()).then(|| errors.join("\n")),
                    ))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.target.as_ref() != Some(&target) || this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((directories, ignored, error)) => {
                        let changed = this.entries != directories || this.ignored != ignored;
                        if this.error != error
                            && let Some(error) = &error
                        {
                            tracing::warn!(%error, "file tree directory unavailable");
                        }
                        this.entries = directories;
                        this.ignored = ignored;
                        this.error = error;
                        if changed || this.pending_reveal.is_some() {
                            this.rebuild(cx);
                        }
                    }
                    Err(error) => {
                        if this.error.as_ref() != Some(&error) {
                            tracing::warn!(%error, "file tree failed");
                        }
                        this.error = Some(error);
                    }
                }
                cx.notify();
                if std::mem::take(&mut this.pending_refresh) {
                    this.refresh(cx);
                }
            });
        }));
        cx.notify();
    }

    pub(crate) fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.refresh_search(cx);
        self.refresh(cx);
        let git = AppState::get(cx).git.clone();
        let target = self.target.clone();
        cx.defer(move |cx| {
            git.update(cx, |git, cx| {
                if git.target == target {
                    git.refresh(false, cx);
                }
            })
        });
    }
}
