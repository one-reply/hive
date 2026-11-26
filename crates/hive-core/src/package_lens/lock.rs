//! Resolve a direct dependency in the lockfile for this specific importer.
use super::{Dependency, Error};
use node_semver::{Range, Version};
use serde_json::Value;
use std::{collections::HashMap, path::Path};

pub(super) enum Lock {
    Npm(Value),
    Pnpm(Value),
    Yarn(HashMap<(String, String), String>),
    Bun(Value),
}

impl Lock {
    pub(super) fn parse(name: &str, bytes: &[u8]) -> Result<Self, Error> {
        let invalid = || Error::Tool(format!("Cannot resolve versions from {name}"));
        match name {
            "pnpm-lock.yaml" => serde_yaml_ng::from_slice(bytes)
                .map(Self::Pnpm)
                .map_err(|_| invalid()),
            "bun.lock" => serde_json_lenient::from_slice(bytes)
                .map(Self::Bun)
                .map_err(|_| invalid()),
            "yarn.lock" | "bun.lockb" => {
                let lock =
                    yarn_lock_parser::parse_str(std::str::from_utf8(bytes).map_err(|_| invalid())?)
                        .map_err(|_| invalid())?;
                let mut resolutions = HashMap::new();
                for entry in lock.entries {
                    for (name, spec) in entry.descriptors {
                        let spec = spec.strip_prefix("npm:").unwrap_or(spec);
                        let key = (name.to_owned(), spec.to_owned());
                        if resolutions.insert(key, entry.version.into()).is_some() {
                            return Err(invalid());
                        }
                    }
                }
                Ok(Self::Yarn(resolutions))
            }
            _ => serde_json::from_slice(bytes)
                .map(Self::Npm)
                .map_err(|_| invalid()),
        }
    }

    pub(super) fn resolved(&self, importer: &Path, d: &Dependency) -> Option<Version> {
        let path = importer.to_str()?;
        let name = &d.name;
        let group = &d.group;
        let spec = &d.spec;
        let version = match self {
            Self::Npm(lock) => {
                if let Some(packages) = lock.get("packages") {
                    if packages.get(path)?.get(group)?.get(name)?.as_str()? != spec {
                        return None;
                    }
                    importer
                        .ancestors()
                        .map(|p| p.join("node_modules").join(name))
                        .find_map(|p| packages.get(p.to_str()?))?
                        .get("version")?
                        .as_str()?
                } else if path.is_empty() {
                    lock.get("dependencies")?
                        .get(name)?
                        .get("version")?
                        .as_str()?
                } else {
                    return None;
                }
            }
            Self::Pnpm(lock) => {
                let project = if let Some(importers) = lock.get("importers") {
                    importers.get(if path.is_empty() { "." } else { path })?
                } else if path.is_empty() {
                    lock
                } else {
                    return None;
                };
                let entry = project.get(group)?.get(name)?;
                let declared = entry
                    .get("specifier")
                    .or_else(|| project.get("specifiers")?.get(name))?
                    .as_str()?;
                if declared != spec {
                    return None;
                }
                entry
                    .get("version")
                    .unwrap_or(entry)
                    .as_str()?
                    .split(['(', '_'])
                    .next()?
            }
            Self::Yarn(entries) => entries.get(&(name.clone(), spec.clone()))?,
            Self::Bun(lock) => {
                let workspace = lock.get("workspaces")?.get(path)?;
                if workspace.get(group)?.get(name)?.as_str()? != spec {
                    return None;
                }
                let packages = lock.get("packages")?;
                let nested = workspace
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(|workspace| packages.get(format!("{workspace}/{name}")));
                nested
                    .or_else(|| packages.get(name))?
                    .get(0)?
                    .as_str()?
                    .strip_prefix(name)?
                    .strip_prefix('@')?
            }
        };
        let version = Version::parse(version).ok()?;
        if Range::parse(spec).is_ok_and(|range| !range.satisfies(&version)) {
            return None;
        }
        Some(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn berry_maps_the_npm_descriptor_to_its_pinned_version() {
        let lock = Lock::parse("yarn.lock", include_bytes!("fixtures/berry.lock")).unwrap();
        let manifest =
            super::super::Manifest::parse(r#"{"dependencies":{"is-number":"^6.0.0"}}"#).unwrap();
        assert_eq!(
            lock.resolved(Path::new(""), &manifest.entries[0])
                .unwrap()
                .to_string(),
            "6.0.0"
        );
    }
}
