use super::*;
use hive_core::package_lens::Workspace;

/// A running installation owns its directory until it exits. Keep only the newest queued save.
pub(super) type Installs =
    std::collections::HashMap<(hive_core::host::Host, PathBuf), Option<(Target, Workspace)>>;

impl Documents {
    pub(super) fn install_status(
        &self,
        target: &Target,
        folder: &std::path::Path,
        status: String,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        for doc in self.open.iter().filter_map(WeakEntity::upgrade) {
            let d = doc.read(cx);
            if d.target != *target
                || !d.is_package_manifest()
                || !d.target.path.join(&d.path).starts_with(folder)
            {
                continue;
            }
            doc.update(cx, |d, cx| {
                d.packages.install_status = Some(status.clone());
                if refresh {
                    d.refresh_packages(cx)
                }
                cx.notify()
            });
        }
    }

    pub(super) fn install_packages(
        &mut self,
        target: Target,
        ws: Workspace,
        cx: &mut Context<Self>,
    ) {
        if !self
            .store
            .read(cx)
            .state
            .projects
            .iter()
            .any(|p| p.host == target.host && p.path == target.project && p.install_on_save)
        {
            return;
        }
        let key = ws.installation_key();
        if let Some(pending) = self.installs.get_mut(&key) {
            *pending = Some((target, ws));
            return;
        }
        self.installs.insert(key.clone(), None);
        self.install_status(&target, &key.1, "Installing packages…".into(), false, cx);
        cx.spawn(async move |this, cx| {
            let install = ws.clone();
            let result =
                crate::state::file_io::run(cx.background_executor().clone(), move |cancel| {
                    install.install(cancel)
                })
                .await;
            this.update(cx, |docs, cx| {
                let pending = docs.installs.remove(&key).flatten();
                let ok = result.is_ok();
                let status = match result {
                    Ok(()) => "Packages installed".into(),
                    Err(e) => {
                        tracing::warn!(error=%e,"package install failed");
                        format!("Install failed · {e}")
                    }
                };
                docs.install_status(&target, &key.1, status, ok, cx);
                if let Some((target, ws)) = pending {
                    docs.install_packages(target, ws, cx)
                }
            })
        })
        .detach_and_log_err(cx);
    }
}
