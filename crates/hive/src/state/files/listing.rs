use super::*;

/// What one read found: the listed directories, the ignored paths when Git was asked, and
/// why some directories could not be read.
pub(super) type Listing = (
    BTreeMap<PathBuf, Vec<files::Entry>>,
    Option<HashSet<PathBuf>>,
    Option<String>,
);

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
        let ignored_paths =
            tracking::IgnoredPaths::of(&AppState::get(cx).git.read(cx).tracking(Some(&target)));
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
                    let ignored = match ignored_paths {
                        tracking::IgnoredPaths::Unchanged => None,
                        tracking::IgnoredPaths::Nothing => Some(HashSet::new()),
                        tracking::IgnoredPaths::FromGit => Some(
                            files::decorations::ignored(
                                &read_target.host,
                                &read_target.path,
                                &paths,
                            )
                            .unwrap_or_else(|error| {
                                errors.push(error.to_string());
                                HashSet::new()
                            }),
                        ),
                    };
                    Ok((
                        directories,
                        ignored,
                        (!errors.is_empty()).then(|| errors.join("\n")),
                    ))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                this.apply_listing(result, cx);
                cx.notify();
                if std::mem::take(&mut this.pending_refresh) {
                    this.refresh(cx);
                }
            });
        }));
        cx.notify();
    }

    /// Applies a read of the listed directories. The rows are rebuilt only when it found
    /// something new: a reveal still waiting for its row gets it from the rebuild that the
    /// listing of its folders brings, and one whose row cannot appear must not rebuild the
    /// tree on every poll.
    pub(super) fn apply_listing(
        &mut self,
        result: Result<Listing, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok((directories, ignored, error)) => {
                let changed = self.entries != directories
                    || ignored
                        .as_ref()
                        .is_some_and(|ignored| self.ignored != *ignored);
                if self.error != error
                    && let Some(error) = &error
                {
                    tracing::warn!(%error, "file tree directory unavailable");
                }
                self.entries = directories;
                if let Some(ignored) = ignored {
                    self.ignored = ignored;
                }
                self.error = error;
                if changed {
                    self.rebuild(cx);
                }
            }
            Err(error) => {
                if self.error.as_ref() != Some(&error) {
                    tracing::warn!(%error, "file tree failed");
                }
                self.error = Some(error);
            }
        }
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
