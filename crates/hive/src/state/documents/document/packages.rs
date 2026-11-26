use super::*;
use hive_core::package_lens::{self, Cache, Lens, Manifest, Workspace};

#[derive(Default)]
pub(crate) struct PackageLens {
    pub(crate) rows: Vec<Lens>,
    pub(crate) status: Option<String>,
    pub(crate) install_status: Option<String>,
    cache: Arc<Cache>,
    task: Option<Task<()>>,
}

impl Document {
    pub(crate) fn is_package_manifest(&self) -> bool {
        self.path.file_name().is_some_and(|n| n == "package.json")
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

    pub(crate) fn refresh_packages(&mut self, cx: &mut Context<Self>) {
        self.packages.task = None;
        self.packages.rows.clear();
        self.packages.status = None;
        if !self.visible || !self.is_package_manifest() || self.base.is_none() {
            return;
        }
        if !self.runs_language_tools(cx) {
            self.packages.status =
                Some("Registry lookup is disabled in Settings → Files → Language tools".into());
            return;
        }
        self.packages.status = Some("Checking package versions…".into());
        let target = self.target.clone();
        let path = self.path.clone();
        let text = self.editor.read(cx).text().clone();
        let cache = self.packages.cache.clone();
        self.packages.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let (sender, receiver) = async_channel::bounded(16);
            let executor = cx.background_executor().clone();
            let work = cx.spawn(async move |_| {
                crate::state::file_io::run(executor, move |cancel| {
                    let manifest = Manifest::parse(&text.to_string())?;
                    if manifest.entries.is_empty() {
                        return Ok(Vec::new());
                    }
                    let workspace = Workspace::discover(&target.host, &target.path, &path, cancel)?;
                    package_lens::inspect(
                        &workspace,
                        manifest,
                        &cache,
                        |row| {
                            let _ = sender.send_blocking(row.clone());
                        },
                        cancel,
                    )
                })
                .await
            });
            while let Ok(row) = receiver.recv().await {
                if this
                    .update(cx, |d, cx| {
                        d.packages.rows.push(row);
                        while let Ok(row) = receiver.try_recv() {
                            d.packages.rows.push(row);
                        }
                        cx.notify()
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = work.await;
            let _ = this.update(cx, |d, cx| {
                match result {
                    Ok(rows) => {
                        d.packages.status = None;
                        d.packages.rows = rows
                    }
                    Err(e) => d.packages.status = Some(e.to_string()),
                }
                cx.notify()
            });
        }));
    }

    pub(crate) fn recheck_packages(&mut self, cx: &mut Context<Self>) {
        self.packages.cache = Arc::default();
        self.refresh_packages(cx);
        cx.notify()
    }

    pub(crate) fn apply_package(
        &mut self,
        revision: u64,
        dep: &package_lens::Dependency,
        spec: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.revision != revision || self.path_busy || self.base.is_none() {
            return;
        }
        let Some(row) = self.packages.rows.iter().find(|r| {
            r.dependency.pair == dep.pair
                && r.dependency.name == dep.name
                && r.dependency.spec == dep.spec
                && r.dependency.group == dep.group
                && r.choices.iter().any(|(_, s)| s == spec)
        }) else {
            return;
        };
        let range = row.dependency.value.clone();
        let editor = self.editor.read(cx);
        if range.end > editor.text().len()
            || !editor.text().is_char_boundary(range.start)
            || !editor.text().is_char_boundary(range.end)
            || serde_json::from_str::<String>(&editor.text().slice(range.clone()).to_string())
                .ok()
                .as_ref()
                != Some(&row.dependency.spec)
        {
            return;
        }
        let value = format!("\"{spec}\"");
        self.editor.update(cx, |e, cx| {
            e.set_selected_range(range, cx);
            e.replace(value, window, cx)
        });
    }

    pub(crate) fn package_revision(&self) -> u64 {
        self.revision
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
