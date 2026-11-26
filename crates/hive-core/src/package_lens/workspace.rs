//! Locate the package manager on the manifest's host, stopping at the worktree boundary.
use super::{Error, lock::Lock};
use crate::{
    command::{self, Cancellation},
    files::Access,
    host::{Host, preamble, quote},
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

const LOCKS: &[(&str, &str)] = &[
    ("pnpm-lock.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("bun.lock", "bun"),
    ("bun.lockb", "bun"),
    ("npm-shrinkwrap.json", "npm"),
    ("package-lock.json", "npm"),
];

/// Select the declared package manager, a lockfile's manager, or npm by default.
pub fn detect_manager<'a>(declared: Option<&'a str>, files: &[&str]) -> &'a str {
    declared
        .and_then(|s| s.split('@').next())
        .filter(|s| matches!(*s, "npm" | "pnpm" | "yarn" | "bun"))
        .or_else(|| {
            LOCKS
                .iter()
                .find(|(file, _)| files.contains(file))
                .map(|(_, m)| *m)
        })
        .unwrap_or("npm")
}

/// Which host and package workspace own a manifest.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Workspace {
    host: Host,
    root: PathBuf,
    pub(super) importer: PathBuf,
    pub(super) manager: String,
    lock: Option<String>,
}

impl Workspace {
    /// Resolve configuration for a worktree-relative manifest without traversing its parent worktrees.
    pub fn discover(
        host: &Host,
        root: &Path,
        path: &Path,
        cancel: &Cancellation,
    ) -> Result<Self, Error> {
        host.validate().map_err(|e| Error::Tool(e.to_string()))?;
        let folder = path.parent().ok_or(Error::Manifest)?;
        let access = Access::new(host, cancel);
        let mut fallback = None;
        for parent in folder.ancestors() {
            let entries = access.list_directory(root, parent)?;
            let names = entries
                .iter()
                .filter(|e| !e.directory)
                .filter_map(|e| e.path.file_name()?.to_str())
                .collect::<Vec<_>>();
            if !names.contains(&"package.json") {
                continue;
            }
            let manifest = access.read(root, &parent.join("package.json"))?;
            let manifest: serde_json::Value =
                serde_json::from_str(&manifest.text()).map_err(|_| Error::Manifest)?;
            let importer = folder.strip_prefix(parent).map_err(|_| Error::Manifest)?;
            if parent != folder {
                let pnpm = names.contains(&"pnpm-workspace.yaml");
                let config = if pnpm {
                    let yaml = access.read(root, &parent.join("pnpm-workspace.yaml"))?;
                    let v: serde_json::Value = serde_saphyr::from_str(&yaml.text())
                        .map_err(|_| Error::Tool("Invalid pnpm workspace configuration".into()))?;
                    v.get("packages")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!(["**"]))
                } else {
                    let v = &manifest["workspaces"];
                    v.get("packages").unwrap_or(v).clone()
                };
                if !member(&config, importer)? {
                    continue;
                }
            }
            let declared = manifest.get("packageManager").and_then(|v| v.as_str());
            if declared.is_some_and(|s| {
                !matches!(s.split('@').next(), Some("npm" | "pnpm" | "yarn" | "bun"))
            }) {
                return Err(Error::Tool("Unsupported package manager".into()));
            }
            let manager = detect_manager(declared, &names);
            let lock = LOCKS
                .iter()
                .find(|(file, m)| *m == manager && names.contains(file))
                .map(|(file, _)| file.to_string());
            if let Some(file) = &lock
                && entries
                    .iter()
                    .any(|e| e.unsupported && e.path.file_name() == Some(file.as_ref()))
            {
                return Err(Error::Tool("Unsupported lockfile entry".into()));
            }
            let ws = Self {
                host: host.clone(),
                root: root.join(parent),
                importer: importer.into(),
                manager: manager.into(),
                lock,
            };
            if ws.lock.is_some() || parent != folder {
                return Ok(ws);
            }
            fallback = Some(ws);
        }
        fallback.ok_or(Error::Manifest)
    }

    /// Host and installation directory shared by manifests in the same workspace.
    pub fn installation_key(&self) -> (Host, PathBuf) {
        (self.host.clone(), self.root.clone())
    }

    pub(super) fn run(
        &self,
        args: &[&str],
        cwd: &Path,
        cancel: &Cancellation,
        seconds: u64,
    ) -> Result<Vec<u8>, Error> {
        run(&self.host, &self.manager, args, cwd, cancel, seconds)
    }

    pub(super) fn folder(&self) -> PathBuf {
        self.root.join(&self.importer)
    }

    pub(super) fn saved_dependencies(&self, cancel: &Cancellation) -> Option<serde_json::Value> {
        let snapshot = Access::new(&self.host, cancel)
            .read(&self.root, &self.importer.join("package.json"))
            .ok()?;
        serde_json::from_str(&snapshot.text()).ok()
    }

    pub(super) fn read_lock(&self, cancel: &Cancellation) -> Result<Option<Lock>, Error> {
        let Some(name) = self.lock.as_deref() else {
            return Ok(None);
        };
        let (tool, args) = if name == "bun.lockb" {
            ("bun", vec!["./bun.lockb"])
        } else {
            ("cat", vec![name])
        };
        let bytes = run(&self.host, tool, &args, &self.root, cancel, 20)?;
        Lock::parse(name, &bytes).map(Some)
    }

    /// Install with the selected package manager in its workspace root.
    pub fn install(&self, cancel: &Cancellation) -> Result<(), Error> {
        self.run(&["install"], &self.root, cancel, 600).map(drop)
    }
}

fn run(
    host: &Host,
    tool: &str,
    args: &[&str],
    cwd: &Path,
    cancel: &Cancellation,
    seconds: u64,
) -> Result<Vec<u8>, Error> {
    let cmd = if host.is_local() {
        host.command(tool, args, Some(cwd))
    } else {
        let script = std::iter::once(tool)
            .chain(args.iter().copied())
            .map(quote)
            .collect::<Vec<_>>()
            .join(" ");
        host.login_command(&format!("exec {script}"), Some(cwd))
    };
    let output = command::run(
        cmd,
        &[],
        cancel,
        Duration::from_secs(seconds),
        16 * 1024 * 1024,
    )
    .map_err(|e| Error::Tool(format!("{tool}: {e}")))?;
    if output.code != Some(0) {
        return Err(Error::Tool(format!(
            "{tool} {} failed (exit {:?})",
            args.first().unwrap_or(&""),
            output.code
        )));
    }
    if host.is_local() {
        Ok(output.stdout)
    } else {
        preamble::strip(&output.stdout)
            .map(|s| s.to_owned())
            .map_err(|e| Error::Tool(e.to_string()))
    }
}

fn member(patterns: &serde_json::Value, path: &Path) -> Result<bool, Error> {
    let Some(patterns) = patterns.as_array() else {
        return Ok(false);
    };
    let mut included = false;
    for raw in patterns {
        let raw = raw.as_str().ok_or(Error::Manifest)?;
        let pattern = raw
            .strip_prefix('!')
            .unwrap_or(raw)
            .trim_start_matches("./")
            .trim_end_matches('/');
        let glob = globset::GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|_| Error::Tool("Invalid workspace pattern".into()))?;
        if glob.compile_matcher().is_match(path) {
            if raw.starts_with('!') {
                return Ok(false);
            }
            included = true;
        }
    }
    Ok(included)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_globs_do_not_claim_nested_standalone_or_excluded_packages() {
        let globs = serde_json::json!(["packages/*", "!packages/excluded"]);
        assert!(member(&globs, Path::new("packages/web")).unwrap());
        for outside in ["packages/web/tools", "packages/excluded", "vendor/pkg"] {
            assert!(!member(&globs, Path::new(outside)).unwrap())
        }
    }
}
