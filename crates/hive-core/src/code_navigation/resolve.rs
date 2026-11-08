//! Module specifier → file, following Node's and TypeScript's lookup rules closely enough to
//! navigate. All paths are relative to the worktree root and never leave it.
use super::config::{Configs, TsConfig};
use crate::files;
use crate::host::Host;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// Extensions tried for an extensionless specifier, sources before declarations.
const EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "d.ts"];
/// TypeScript sources may import `./x.js` while the file on disk is `./x.ts`.
const SOURCE_FOR_OUTPUT: &[(&str, &[&str])] = &[
    ("js", &["ts", "tsx"]),
    ("jsx", &["tsx"]),
    ("mjs", &["mts"]),
    ("cjs", &["cts"]),
];
/// Conditions of `exports`/`imports` targets, in preference order; declarations first, as in
/// editors, because published packages often ship only compiled JavaScript besides them.
const CONDITIONS: &[&str] = &["types", "import", "module", "node", "require", "default"];
const CONFIG_NAMES: &[&str] = &["tsconfig.json", "jsconfig.json"];
/// `extends` chains and package `main` → directory → `main` loops stop here.
const MAX_DEPTH: usize = 4;

pub(super) struct Resolver<'a> {
    root: &'a Path,
    configs: &'a Configs,
}

impl<'a> Resolver<'a> {
    pub fn new(root: &'a Path, configs: &'a Configs) -> Self {
        Self { root, configs }
    }

    pub fn root(&self) -> &Path {
        self.root
    }

    /// The file `specifier` refers to when imported from `from`.
    pub fn resolve(&self, from: &Path, specifier: &str) -> Option<PathBuf> {
        let directory = from.parent().unwrap_or(Path::new(""));
        if is_relative(specifier) {
            return self.module(&join(directory, specifier)?, 0);
        }
        if specifier.is_empty() || specifier.starts_with('/') || specifier.contains(':') {
            return None;
        }
        if let Some(path) = self.tsconfig_paths(directory, specifier) {
            return Some(path);
        }
        if specifier.starts_with('#') {
            return self.package_imports(directory, specifier);
        }
        self.node_modules(directory, specifier)
    }

    /// `compilerOptions.paths` and `baseUrl` that apply in `directory`, from the nearest
    /// `tsconfig.json`/`jsconfig.json` and its `extends`.
    pub fn options(&self, directory: &Path) -> Option<Options> {
        let (config, config_dir) = directory.ancestors().find_map(|ancestor| {
            CONFIG_NAMES.iter().find_map(|name| {
                let config = self.configs.tsconfig(self.root, &ancestor.join(name))?;
                Some((config, ancestor.to_path_buf()))
            })
        })?;
        Some(self.effective_options(config, config_dir))
    }

    fn tsconfig_paths(&self, directory: &Path, specifier: &str) -> Option<PathBuf> {
        let options = self.options(directory)?;
        if let Some((paths, paths_dir)) = &options.paths {
            let base = options.base_url.as_ref().unwrap_or(paths_dir);
            if let Some((pattern, captured)) =
                best_match(paths.keys().map(String::as_str), specifier)
            {
                for target in paths.get(pattern).into_iter().flatten() {
                    let target = target.replacen('*', &captured, 1);
                    if let Some(path) = join(base, &target).and_then(|p| self.module(&p, 0)) {
                        return Some(path);
                    }
                }
            }
        }
        let base = options.base_url?;
        self.module(&join(&base, specifier)?, 0)
    }

    /// `paths` and `baseUrl` after `extends`; the nearest config that sets each one wins.
    fn effective_options(&self, config: std::sync::Arc<TsConfig>, dir: PathBuf) -> Options {
        let mut options = Options::default();
        let mut pending = std::collections::VecDeque::from([(config, dir)]);
        let mut depth = 0;
        while let Some((config, dir)) = pending.pop_front() {
            let compiler = &config.compiler_options;
            if options.base_url.is_none() {
                options.base_url = compiler.base_url.as_deref().and_then(|b| join(&dir, b));
            }
            if options.paths.is_none() {
                options.paths = compiler.paths.clone().map(|paths| (paths, dir.clone()));
            }
            depth += 1;
            if depth > MAX_DEPTH {
                break;
            }
            // Later `extends` entries override earlier ones, so they are examined first.
            for parent in config.extends.iter().flat_map(|e| e.parents()).rev() {
                if !is_relative(parent) {
                    continue;
                }
                let mut parent = parent.clone();
                if !parent.ends_with(".json") {
                    parent.push_str(".json");
                }
                let Some(path) = join(&dir, &parent) else {
                    continue;
                };
                let parent_dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
                if let Some(config) = self.configs.tsconfig(self.root, &path) {
                    pending.push_back((config, parent_dir));
                }
            }
        }
        options
    }

    /// `#name` through the `imports` field of the nearest `package.json`.
    fn package_imports(&self, directory: &Path, specifier: &str) -> Option<PathBuf> {
        let (package, package_dir) = directory.ancestors().find_map(|ancestor| {
            let package = self
                .configs
                .package(self.root, &ancestor.join("package.json"))?;
            Some((package, ancestor))
        })?;
        let imports = package.imports.as_ref()?.as_object()?;
        let (pattern, captured) = best_match(imports.keys().map(String::as_str), specifier)?;
        targets(&imports[pattern])
            .into_iter()
            .map(|target| target.replace('*', &captured))
            .find_map(|target| {
                if is_relative(&target) {
                    self.module(&join(package_dir, &target)?, 0)
                } else {
                    self.node_modules(package_dir, &target)
                }
            })
    }

    /// A package installed in a `node_modules` directory inside the worktree.
    fn node_modules(&self, directory: &Path, specifier: &str) -> Option<PathBuf> {
        let (name, subpath) = split_package(specifier)?;
        directory.ancestors().find_map(|ancestor| {
            let package_dir = join(ancestor, "node_modules")?.join(name);
            self.package_entry(&package_dir, subpath)
        })
    }

    fn package_entry(&self, package_dir: &Path, subpath: &str) -> Option<PathBuf> {
        let package = self
            .configs
            .package(self.root, &package_dir.join("package.json"));
        if let Some(exports) = package.as_ref().and_then(|p| p.exports.as_ref()) {
            let key = if subpath.is_empty() {
                ".".to_owned()
            } else {
                format!("./{subpath}")
            };
            let found = export_targets(exports, &key)
                .into_iter()
                .find_map(|target| self.module(&join(package_dir, &target)?, 0));
            if found.is_some() {
                return found;
            }
        }
        if !subpath.is_empty() {
            return self.module(&join(package_dir, subpath)?, 0);
        }
        let package = package?;
        let fields = [
            &package.types,
            &package.typings,
            &package.module,
            &package.main,
        ];
        fields
            .into_iter()
            .flatten()
            .find_map(|entry| self.module(&join(package_dir, entry)?, 0))
            .or_else(|| self.module(package_dir, 0))
    }

    /// The file for `path`: exact, with a source extension, or a directory's entry point.
    fn module(&self, path: &Path, depth: usize) -> Option<PathBuf> {
        if self.is_file(path) {
            return Some(path.to_path_buf());
        }
        if let Some(extension) = path.extension().and_then(|e| e.to_str())
            && let Some((_, sources)) = SOURCE_FOR_OUTPUT.iter().find(|(e, _)| *e == extension)
            && let Some(found) = sources
                .iter()
                .map(|source| path.with_extension(source))
                .find(|candidate| self.is_file(candidate))
        {
            return Some(found);
        }
        if let Some(found) = EXTENSIONS
            .iter()
            .map(|extension| with_added_extension(path, extension))
            .find(|candidate| self.is_file(candidate))
        {
            return Some(found);
        }
        if depth < MAX_DEPTH
            && let Some(package) = self.configs.package(self.root, &path.join("package.json"))
            && let Some(found) = [&package.types, &package.module, &package.main]
                .into_iter()
                .flatten()
                .find_map(|entry| self.module(&join(path, entry)?, depth + 1))
        {
            return Some(found);
        }
        EXTENSIONS
            .iter()
            .map(|extension| path.join(format!("index.{extension}")))
            .find(|candidate| self.is_file(candidate))
    }

    fn is_file(&self, path: &Path) -> bool {
        !path.as_os_str().is_empty() && self.metadata(path).is_some_and(|m| m.is_file())
    }

    /// The parsed `package.json` at `path`, relative to the worktree root.
    pub fn package(&self, path: &Path) -> Option<std::sync::Arc<super::config::PackageJson>> {
        self.configs.package(self.root, path)
    }

    /// A real directory inside the worktree; the empty path is the worktree itself.
    pub fn is_directory(&self, path: &Path) -> bool {
        self.metadata(path).is_some_and(|m| m.is_dir())
    }

    fn metadata(&self, path: &Path) -> Option<std::fs::Metadata> {
        files::resolve(&Host::Local, self.root, path)
            .ok()
            .and_then(|absolute| std::fs::symlink_metadata(absolute).ok())
    }
}

/// The module-resolution settings of a `tsconfig.json` that `paths` and `baseUrl` lookups use.
#[derive(Default)]
pub(super) struct Options {
    /// `baseUrl`, relative to the worktree root.
    pub base_url: Option<PathBuf>,
    /// `paths` patterns and the directory of the config that declares them.
    pub paths: Option<(std::collections::BTreeMap<String, Vec<String>>, PathBuf)>,
}

pub(super) fn is_relative(specifier: &str) -> bool {
    specifier == "."
        || specifier == ".."
        || specifier.starts_with("./")
        || specifier.starts_with("../")
}

/// `base/specifier` normalised lexically; `None` if it climbs above the worktree root.
pub(super) fn join(base: &Path, specifier: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for component in base.components().chain(Path::new(specifier).components()) {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !path.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(path)
}

fn with_added_extension(path: &Path, extension: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(extension);
    PathBuf::from(name)
}

/// `@scope/name/sub/path` → (`@scope/name`, `sub/path`).
fn split_package(specifier: &str) -> Option<(&str, &str)> {
    let mut slashes = specifier.match_indices('/').map(|(ix, _)| ix);
    let end = if specifier.starts_with('@') {
        slashes.nth(1)
    } else {
        slashes.next()
    };
    match end {
        Some(end) => Some((&specifier[..end], &specifier[end + 1..])),
        None if specifier.starts_with('@') && !specifier.contains('/') => None,
        None => Some((specifier, "")),
    }
}

/// The exact key, else the `*` pattern with the longest prefix, and the text `*` stands for.
pub(super) fn best_match<'k>(
    keys: impl Iterator<Item = &'k str>,
    specifier: &str,
) -> Option<(&'k str, String)> {
    let mut best: Option<(&str, String, usize)> = None;
    for key in keys {
        if key == specifier {
            return Some((key, String::new()));
        }
        let Some((prefix, suffix)) = key.split_once('*') else {
            continue;
        };
        if specifier.len() < prefix.len() + suffix.len()
            || !specifier.starts_with(prefix)
            || !specifier.ends_with(suffix)
        {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(_, _, length)| prefix.len() > *length)
        {
            let captured = specifier[prefix.len()..specifier.len() - suffix.len()].to_owned();
            best = Some((key, captured, prefix.len()));
        }
    }
    best.map(|(key, captured, _)| (key, captured))
}

/// `exports` of a package: a single target, a condition map, or a map of subpaths.
fn export_targets(exports: &Value, key: &str) -> Vec<String> {
    let subpaths = exports
        .as_object()
        .filter(|map| map.keys().any(|k| k.starts_with('.')));
    let Some(subpaths) = subpaths else {
        return if key == "." {
            targets(exports)
        } else {
            Vec::new()
        };
    };
    let Some((pattern, captured)) = best_match(subpaths.keys().map(String::as_str), key) else {
        return Vec::new();
    };
    targets(&subpaths[pattern])
        .into_iter()
        .map(|target| target.replace('*', &captured))
        .collect()
}

/// Candidate paths of one `exports`/`imports` value, in [`CONDITIONS`] order.
fn targets(value: &Value) -> Vec<String> {
    match value {
        Value::String(target) => vec![target.clone()],
        Value::Array(values) => values.iter().flat_map(targets).collect(),
        Value::Object(conditions) => CONDITIONS
            .iter()
            .filter_map(|condition| conditions.get(*condition))
            .flat_map(targets)
            .collect(),
        _ => Vec::new(),
    }
}
