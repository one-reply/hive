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
        self.loading = true;
        let generation = self.generation;
        let paths: Vec<_> = std::iter::once(PathBuf::new())
            .chain(self.expanded.iter().cloned())
            .collect();
        let tracking = AppState::get(cx).git.read(cx).tracking(Some(&target));
        let is_git = matches!(tracking, Tracking::Status(_));
        // Without an answer from Git the paths ignored so far stay ignored.
        let ignored_known = tracking != Tracking::Unknown;
        self._read = Some(cx.spawn(async move |this, cx| {
            let read_target = target.clone();
            let result =
                crate::state::file_io::run(cx.background_executor().clone(), move |cancellation| {
                    let mut directories = BTreeMap::new();
                    let mut errors = Vec::new();
                    for path in paths {
                        match files::Access::new(&read_target.host, cancellation)
                            .list_directory(&read_target.path, &path)
                        {
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
                            Ok(ignored) => Some(ignored),
                            Err(error) => {
                                errors.push(error.to_string());
                                Some(HashSet::new())
                            }
                        }
                    } else {
                        ignored_known.then(HashSet::new)
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
                        let changed = this.entries != directories
                            || ignored
                                .as_ref()
                                .is_some_and(|ignored| this.ignored != *ignored);
                        if this.error != error
                            && let Some(error) = &error
                        {
                            tracing::warn!(%error, "file tree directory unavailable");
                        }
                        this.entries = directories;
                        if let Some(ignored) = ignored {
                            this.ignored = ignored;
                        }
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
