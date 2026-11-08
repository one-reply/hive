//! What an import can name: module specifiers while typing one, and the names a module
//! exports while filling `import { … }`. Same resolution rules as Go to Definition.
use super::config::PackageJson;
use super::resolve::{self, Resolver};
use super::syntax::{self, Dialect};
use super::{MAX_HOPS, Navigator, exports};
use crate::completion::{MAX_ITEMS, ranked, starts_with_ignoring_case};
use crate::files;
use crate::host::Host;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Extensions dropped from file names, longest first: bundlers and TypeScript resolve them.
const RESOLVED_EXTENSIONS: &[&str] = &[
    ".d.ts", ".d.mts", ".d.cts", ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx",
];
/// What Node needs written out in an ES module package (`"type": "module"`).
const NODE_ESM_EXTENSIONS: &[&str] = &[".js", ".jsx"];

/// Module specifiers that continue what was typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPaths {
    /// Byte offset in the typed specifier where an accepted item's text starts: after the
    /// last `/` when completing one path segment, `0` when replacing the whole specifier.
    pub from: usize,
    /// Candidates, folders before files, then aliases before packages.
    pub items: Vec<ImportPath>,
}

/// One candidate module specifier, or its next path segment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportPath {
    /// Text that replaces the typed specifier from [`ImportPaths::from`].
    pub label: String,
    /// What the label names.
    pub kind: ImportPathKind,
}

/// What an import path candidate names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportPathKind {
    /// A folder, or `./` and `../`.
    Directory,
    /// A source file, its extension dropped where resolution adds it back.
    File,
    /// A `tsconfig` `paths` pattern or a `package.json` `imports` key.
    Alias,
    /// A dependency named in a `package.json`.
    Package,
}

impl Navigator {
    /// Specifiers that continue `typed`, the text between the opening quote and the cursor in
    /// `file` (relative to the absolute worktree `root`). Relative paths list the folder on
    /// `host`; aliases and packages come from local configs only. Blocking.
    pub fn import_paths(&self, host: &Host, root: &Path, file: &Path, typed: &str) -> ImportPaths {
        let directory = file.parent().unwrap_or(Path::new(""));
        let local = host.is_local().then(|| Resolver::new(root, &self.configs));
        if let Some(slash) = typed.rfind('/') {
            let (folder, segment) = typed.split_at(slash + 1);
            let listed = if resolve::is_relative(folder) {
                resolve::join(directory, folder)
            } else {
                local
                    .as_ref()
                    .and_then(|resolver| alias_folder(resolver, directory, folder))
            };
            if let Some(listed) = listed {
                let keep_js = local
                    .as_ref()
                    .is_some_and(|r| is_es_module_package(r, directory));
                return ImportPaths {
                    from: slash + 1,
                    items: folder_items(host, root, file, &listed, segment, keep_js),
                };
            }
        }
        ImportPaths {
            from: 0,
            items: specifiers(local.as_ref(), directory, typed),
        }
    }

    /// Names `import { … } from 'specifier'` can take in `file`, following `export * from`,
    /// that start with `prefix`. Local worktrees only; unreadable modules add nothing. Blocking.
    pub fn export_names(
        &self,
        root: &Path,
        file: &Path,
        specifier: &str,
        prefix: &str,
    ) -> Vec<String> {
        let resolver = Resolver::new(root, &self.configs);
        let mut names = BTreeSet::new();
        let mut visited = HashSet::new();
        let mut pending = vec![(file.to_path_buf(), specifier.to_owned(), 0)];
        while let Some((from, specifier, hops)) = pending.pop() {
            let Some(module) = resolver.resolve(&from, &specifier) else {
                continue;
            };
            if hops > MAX_HOPS || !visited.insert(module.clone()) {
                continue;
            }
            let text = match files::read(&Host::Local, root, &module) {
                Ok(snapshot) => snapshot.text(),
                Err(error) => {
                    tracing::debug!(%error, "cannot read module for import names");
                    continue;
                }
            };
            let Some(tree) = Dialect::for_path(&module).and_then(|d| syntax::parse(&text, d))
            else {
                continue;
            };
            let exported = exports::names(&tree, &text);
            names.extend(exported.names);
            let next = exported.star_sources.into_iter();
            pending.extend(next.map(|specifier| (module.clone(), specifier, hops + 1)));
        }
        let names: Vec<String> = names.into_iter().collect();
        ranked(&[&names], prefix)
            .into_iter()
            .map(str::to_owned)
            .collect()
    }
}

/// The folder an aliased specifier such as `@/components/` points to, through `tsconfig`
/// `paths` (`"@/*": ["src/*"]`) or `baseUrl`.
fn alias_folder(resolver: &Resolver, directory: &Path, folder: &str) -> Option<PathBuf> {
    let options = resolver.options(directory)?;
    if let Some((paths, paths_dir)) = &options.paths {
        let base = options.base_url.as_ref().unwrap_or(paths_dir);
        if let Some((pattern, captured)) =
            resolve::best_match(paths.keys().map(String::as_str), folder)
        {
            let found = paths[pattern]
                .iter()
                .filter_map(|target| resolve::join(base, &target.replacen('*', &captured, 1)))
                .find(|path| resolver.is_directory(path));
            if found.is_some() {
                return found;
            }
        }
    }
    let base = options.base_url?;
    resolve::join(&base, folder).filter(|path| resolver.is_directory(path))
}

fn folder_items(
    host: &Host,
    root: &Path,
    file: &Path,
    folder: &Path,
    segment: &str,
    keep_js: bool,
) -> Vec<ImportPath> {
    let entries = match files::list_directory(host, root, folder) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::debug!(%error, "cannot list folder for import paths");
            return Vec::new();
        }
    };
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for entry in entries {
        let Some(name) = entry.path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if entry.unsupported
            || entry.path == file
            || name.starts_with('.')
            || name == "node_modules"
        {
            continue;
        }
        let (label, kind) = if entry.directory {
            (name, ImportPathKind::Directory)
        } else {
            (module_name(name, keep_js), ImportPathKind::File)
        };
        if label != segment
            && starts_with_ignoring_case(label, segment)
            && seen.insert(label.to_owned())
        {
            items.push(ImportPath {
                label: label.to_owned(),
                kind,
            });
        }
        if items.len() == MAX_ITEMS {
            break;
        }
    }
    items
}

/// `button.tsx` → `button`; `.js` stays in ES module packages, where Node requires it.
fn module_name(name: &str, keep_js: bool) -> &str {
    RESOLVED_EXTENSIONS
        .iter()
        .filter(|&&extension| !(keep_js && NODE_ESM_EXTENSIONS.contains(&extension)))
        .find_map(|extension| name.strip_suffix(extension).filter(|stem| !stem.is_empty()))
        .unwrap_or(name)
}

/// Whole specifiers: `./`, `../`, aliases, `#imports` keys and dependencies.
fn specifiers(resolver: Option<&Resolver>, directory: &Path, typed: &str) -> Vec<ImportPath> {
    let mut candidates: Vec<ImportPath> = ["./", "../"]
        .into_iter()
        .map(|label| ImportPath {
            label: label.to_owned(),
            kind: ImportPathKind::Directory,
        })
        .collect();
    if let Some(resolver) = resolver {
        let packages = packages(resolver, directory);
        if typed.is_empty() || typed.starts_with('#') {
            let imports = packages
                .iter()
                .find_map(|p| p.imports.as_ref()?.as_object());
            candidates.extend(aliases(imports.into_iter().flat_map(|map| map.keys())));
        }
        if !typed.starts_with('#') {
            let options = resolver.options(directory);
            let paths = options.as_ref().and_then(|o| o.paths.as_ref());
            candidates.extend(aliases(
                paths.into_iter().flat_map(|(paths, _)| paths.keys()),
            ));
            let dependencies: BTreeSet<&String> = packages
                .iter()
                .flat_map(|p| [&p.dependencies, &p.dev_dependencies])
                .filter_map(|names| names.as_ref()?.as_object())
                .flat_map(|names| names.keys())
                .collect();
            candidates.extend(dependencies.into_iter().map(|name| ImportPath {
                label: name.clone(),
                kind: ImportPathKind::Package,
            }));
        }
    }
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|c| c.label != typed && starts_with_ignoring_case(&c.label, typed))
        .filter(|c| seen.insert(c.label.clone()))
        .take(MAX_ITEMS)
        .collect()
}

/// Pattern keys as typed text: `@/*` → `@/`, `#utils` stays. Sorted here because JSON maps keep
/// file order whenever another crate in the build enables `serde_json/preserve_order`.
fn aliases<'a>(keys: impl Iterator<Item = &'a String>) -> impl Iterator<Item = ImportPath> {
    let keys: BTreeSet<&String> = keys.collect();
    keys.into_iter()
        .map(|key| ImportPath {
            label: key.split('*').next().unwrap_or_default().to_owned(),
            kind: ImportPathKind::Alias,
        })
        .filter(|alias| !alias.label.is_empty())
}

/// Every `package.json` from `directory` up to the worktree root, nearest first.
fn packages(resolver: &Resolver, directory: &Path) -> Vec<Arc<PackageJson>> {
    directory
        .ancestors()
        .filter_map(|ancestor| resolver.package(&ancestor.join("package.json")))
        .collect()
}

fn is_es_module_package(resolver: &Resolver, directory: &Path) -> bool {
    packages(resolver, directory)
        .first()
        .is_some_and(|package| {
            package.module_type.as_ref().and_then(|t| t.as_str()) == Some("module")
        })
}
