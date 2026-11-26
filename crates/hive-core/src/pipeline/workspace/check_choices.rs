//! Offer local QA commands from tracked manifests at the approved base commit.
use crate::{
    git::{self, GitError},
    host::Host,
};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};

const MANIFEST_LIMIT: usize = 256 * 1024;

pub(super) fn discover(project: &Path, commit: &str) -> Result<Vec<String>, GitError> {
    let paths = git::run(
        &Host::Local,
        project,
        &[
            "ls-tree",
            commit,
            "--",
            "Cargo.toml",
            "go.mod",
            "package.json",
            "pnpm-lock.yaml",
            "yarn.lock",
            "bun.lock",
            "bun.lockb",
            "package-lock.json",
        ],
    )?;
    let files: Vec<_> = paths
        .lines()
        .filter_map(|line| {
            let (object, name) = line.split_once('\t')?;
            (object.starts_with("100644 blob ") || object.starts_with("100755 blob "))
                .then_some(name)
        })
        .collect();
    let mut choices = Vec::new();
    if files.contains(&"Cargo.toml") {
        choices.extend(["cargo test".into(), "cargo clippy --all-targets".into()]);
    }
    if files.contains(&"go.mod") {
        choices.push("go test ./...".into());
    }
    if files.contains(&"package.json") {
        let blob = git::bytes(
            &Host::Local,
            project,
            &["cat-file", "blob", &format!("{commit}:package.json")],
            None,
            MANIFEST_LIMIT,
            false,
        )?;
        choices.extend(package_scripts(&files, &blob));
    }
    Ok(choices)
}

fn package_scripts(files: &[&str], manifest: &[u8]) -> Vec<String> {
    #[derive(Deserialize)]
    struct Package {
        #[serde(default)]
        scripts: BTreeMap<String, String>,
        #[serde(rename = "packageManager")]
        manager: Option<String>,
    }
    let Ok(package) = serde_json::from_slice::<Package>(manifest) else {
        return Vec::new();
    };
    let manager = crate::package_lens::detect_manager(package.manager.as_deref(), files);
    ["test", "lint", "typecheck", "type-check"]
        .into_iter()
        .filter(|name| {
            package
                .scripts
                .get(*name)
                .is_some_and(|s| !s.trim().is_empty() && !s.contains("no test specified"))
        })
        .map(|name| format!("{manager} run {name}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn suggests_existing_named_scripts_and_uses_declared_manager_before_lockfile() {
        let manifest = br#"{"scripts":{"test":"vitest","lint":"eslint .","anything":"foo"},"packageManager":"yarn@4.3.0"}"#;
        assert_eq!(
            package_scripts(&["pnpm-lock.yaml"], manifest),
            ["yarn run test", "yarn run lint"]
        );
        assert_eq!(
            package_scripts(
                &["bun.lock"],
                br#"{"scripts":{"test":"echo no test specified","typecheck":"tsc"}}"#
            ),
            ["bun run typecheck"]
        );
        assert!(package_scripts(&[], b"{").is_empty());
    }
}
