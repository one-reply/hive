//! Versions for the dependencies of an open package.json: entries still waiting for the
//! registry, its answers, and keeping both on their entries while the text is edited.
use super::*;
use gpui_kit::component::input::{InputEdit, Rope};
use hive_core::package_lens::{
    self, Cache, Dependency, Lens, Manifest, TextEdit, Versions, Workspace,
};

/// Typing pauses this long before the registry is asked, so a burst of keys asks once.
const LOOKUP_DELAY: Duration = Duration::from_millis(400);

/// The lens after one dependency of the manifest.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PackageRow {
    /// The registry has not answered for this entry yet.
    Checking(Dependency),
    /// The registry answered.
    Ready(Lens),
}

impl PackageRow {
    pub(crate) fn dependency(&self) -> &Dependency {
        match self {
            Self::Checking(dependency) => dependency,
            Self::Ready(lens) => &lens.dependency,
        }
    }

    fn dependency_mut(&mut self) -> &mut Dependency {
        match self {
            Self::Checking(dependency) => dependency,
            Self::Ready(lens) => &mut lens.dependency,
        }
    }
}

#[derive(Default)]
pub(crate) struct PackageLens {
    /// In source order. Every position is valid for the editor's current text: edits move
    /// them, and an edit inside an entry drops its row until the next lookup.
    rows: Vec<PackageRow>,
    /// Why versions are missing, or why a package action failed.
    status: Option<String>,
    pub(crate) install_status: Option<String>,
    /// The switch the rows were last refreshed under, so only its changes refresh them.
    enabled: bool,
    cache: Arc<Cache>,
    task: Option<Task<()>>,
}

impl PackageLens {
    pub(crate) fn rows(&self) -> &[PackageRow] {
        &self.rows
    }

    pub(crate) fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Move the rows past `edits`, applied in order; an entry an edit touched waits for the
    /// next lookup.
    fn follow(&mut self, edits: impl IntoIterator<Item = TextEdit>) {
        for edit in edits {
            self.rows
                .retain_mut(|row| row.dependency_mut().follow(&edit));
        }
    }

    /// The entries of the current text: an entry that did not change keeps its answer, the
    /// others wait for the registry.
    fn expect(&mut self, entries: &[Dependency]) {
        let mut previous = std::mem::take(&mut self.rows);
        self.rows = entries
            .iter()
            .filter(|entry| entry.is_searchable())
            .map(
                |entry| match previous.iter().position(|row| row.dependency() == entry) {
                    Some(index) => previous.swap_remove(index),
                    None => PackageRow::Checking(entry.clone()),
                },
            )
            .collect();
    }

    /// One answer of a lookup of the current text.
    pub(super) fn answer(&mut self, lens: Lens) {
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|row| row.dependency() == &lens.dependency)
        {
            *row = PackageRow::Ready(lens);
            return;
        }
        let at = self
            .rows
            .partition_point(|row| row.dependency().pair.start < lens.dependency.pair.start);
        self.rows.insert(at, PackageRow::Ready(lens));
    }

    /// The lookup ended. Its answers replace the rows; after a failure an entry that was still
    /// waiting stops waiting, and the earlier answers stay.
    fn finish(&mut self, result: Result<Vec<Lens>, package_lens::Error>) {
        match result {
            Ok(lenses) => {
                self.rows = lenses.into_iter().map(PackageRow::Ready).collect();
                self.status = None;
            }
            Err(error) => {
                self.rows.retain(|row| matches!(row, PackageRow::Ready(_)));
                self.status = Some(error.to_string());
            }
        }
    }

    fn clear(&mut self) {
        self.task = None;
        self.rows.clear();
        self.status = None;
    }
}

fn text_edit(edit: &InputEdit) -> TextEdit {
    TextEdit {
        start: edit.start_byte,
        old_end: edit.old_end_byte,
        new_end: edit.new_end_byte,
        old_end_line: edit.old_end_position.row,
        new_end_line: edit.new_end_position.row,
    }
}

impl Document {
    pub(crate) fn is_package_manifest(&self) -> bool {
        self.path.file_name().is_some_and(|n| n == "package.json")
    }

    /// The switch for version lookups (Settings → Files), the same for every manifest.
    pub(crate) fn shows_package_versions(&self, cx: &App) -> bool {
        self.store.read(cx).state.settings.files.package_versions
    }

    pub(crate) fn install_on_save(&self, cx: &App) -> bool {
        self.store.read(cx).state.projects.iter().any(|p| {
            p.path == self.target.project && p.host == self.target.host && p.install_on_save
        })
    }

    pub(crate) fn toggle_package_install(&self, cx: &mut Context<Self>) {
        self.store.update(cx, |s, cx| {
            s.toggle_install_on_save(&self.target.host, &self.target.project, cx)
        });
        cx.notify();
    }

    /// Keep the version rows on their entries while the text changes.
    pub(super) fn follow_package_edits(&mut self, edits: &[(InputEdit, Rope)]) {
        self.packages
            .follow(edits.iter().map(|(edit, _)| text_edit(edit)));
    }

    /// Settings changed: follow the version lookups switch.
    pub(super) fn apply_package_setting(&mut self, cx: &mut Context<Self>) {
        if self.packages.enabled != self.shows_package_versions(cx) {
            self.refresh_packages(cx);
            cx.notify();
        }
    }

    /// The text was replaced without reporting edits (a read from disk): no position holds.
    pub(super) fn reset_packages(&mut self, cx: &mut Context<Self>) {
        self.packages.clear();
        self.refresh_packages(cx);
    }

    pub(crate) fn refresh_packages(&mut self, cx: &mut Context<Self>) {
        self.packages.task = None;
        self.packages.enabled = self.shows_package_versions(cx);
        if !self.packages.enabled || !self.is_package_manifest() {
            self.packages.clear();
            return;
        }
        // A hidden tab keeps its rows, which its edits still move, and looks again when shown.
        if !self.visible || self.base.is_none() {
            return;
        }
        if !self.runs_language_tools(cx) {
            self.packages.clear();
            self.packages.status =
                Some("Version lookups are off with language tools (Settings → Files)".into());
            return;
        }
        let (target, path) = (self.target.clone(), self.path.clone());
        let text = self.editor.read(cx).text().clone();
        let cache = self.packages.cache.clone();
        self.packages.task = Some(cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let parsed = crate::state::file_io::run(executor.clone(), move |_| {
                Manifest::parse(&text.to_string())
            })
            .await;
            let manifest = match parsed {
                Ok(manifest) => manifest,
                Err(error) => {
                    let _ = this.update(cx, |d, cx| {
                        d.packages.finish(Err(error));
                        cx.notify()
                    });
                    return;
                }
            };
            let expected = this.update(cx, |d, cx| {
                d.packages.status = None;
                d.packages.expect(&manifest.entries);
                cx.notify()
            });
            if expected.is_err() || !manifest.entries.iter().any(Dependency::is_searchable) {
                return;
            }
            executor.timer(LOOKUP_DELAY).await;
            let (sender, receiver) = async_channel::bounded(16);
            let work = cx.spawn(async move |_| {
                crate::state::file_io::run(executor, move |cancel| {
                    let workspace = Workspace::discover(&target.host, &target.path, &path, cancel)?;
                    package_lens::inspect(
                        &workspace,
                        manifest,
                        &cache,
                        |lens| {
                            let _ = sender.send_blocking(lens.clone());
                        },
                        cancel,
                    )
                })
                .await
            });
            while let Ok(lens) = receiver.recv().await {
                let answered = this.update(cx, |d, cx| {
                    d.packages.answer(lens);
                    while let Ok(lens) = receiver.try_recv() {
                        d.packages.answer(lens);
                    }
                    cx.notify()
                });
                if answered.is_err() {
                    return;
                }
            }
            let result = work.await;
            let _ = this.update(cx, |d, cx| {
                d.packages.finish(result);
                cx.notify()
            });
        }));
    }

    pub(crate) fn recheck_packages(&mut self, cx: &mut Context<Self>) {
        self.packages.cache = Arc::default();
        self.refresh_packages(cx);
        cx.notify()
    }

    /// Replace an entry's requirement with one of the upgrades offered for it, as one
    /// undoable edit. Nothing happens when the row no longer stands for that entry.
    pub(crate) fn apply_upgrade(
        &mut self,
        dependency: &Dependency,
        spec: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.path_busy || self.base.is_none() {
            return;
        }
        let offered = self.packages.rows.iter().any(|row| match row {
            PackageRow::Ready(lens) => {
                lens.dependency == *dependency
                    && matches!(&lens.versions, Versions::Found(upgrades)
                        if upgrades.iter().any(|upgrade| upgrade.spec == spec))
            }
            PackageRow::Checking(_) => false,
        });
        if !offered {
            return;
        }
        let range = dependency.value.clone();
        let text = self.editor.read(cx).text();
        if range.end > text.len()
            || !text.is_char_boundary(range.start)
            || !text.is_char_boundary(range.end)
            || serde_json::from_str::<String>(&text.slice(range.clone()).to_string())
                .ok()
                .as_ref()
                != Some(&dependency.spec)
        {
            return;
        }
        let value = format!("\"{spec}\"");
        self.editor.update(cx, |e, cx| {
            e.set_selected_range(range, cx);
            e.replace(value, window, cx)
        });
    }

    pub(crate) fn sort_packages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.path_busy || self.base.is_none() || !self.is_package_manifest() {
            return;
        }
        let source = self.text(cx);
        match Manifest::sorted(&source) {
            Ok(sorted) if sorted != source => self.replace_preserving_view(sorted, window, cx),
            Err(e) => {
                self.packages.status = Some(e.to_string());
                cx.notify()
            }
            _ => {}
        }
    }

    pub(super) fn queue_package_install(&self, text: String, cx: &mut Context<Self>) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let (target, path) = (self.target.clone(), self.path.clone());
        cx.spawn(async move |_, cx| {
            let work = target.clone();
            let document_path = target.path.join(&path);
            let result =
                crate::state::file_io::run(cx.background_executor().clone(), move |cancel| {
                    Manifest::parse(&text)?;
                    Workspace::discover(&work.host, &work.path, &path, cancel)
                })
                .await;
            registry.update(cx, |docs, cx| match result {
                Ok(ws) => docs.install_packages(target, ws, cx),
                Err(e) => docs.install_status(
                    &target,
                    &document_path,
                    format!("Install skipped · {e}"),
                    false,
                    cx,
                ),
            })
        })
        .detach();
    }
}

#[cfg(test)]
mod tests;
