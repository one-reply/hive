//! Dependency versions and source-preserving edits for an open npm manifest.
use node_semver::{Range as VersionRange, Version};
use std::{collections::HashSet, ops::Range};
use tree_sitter::Node;

mod lock;
mod registry;
mod workspace;
pub use registry::{Cache, inspect};
pub use workspace::{Workspace, detect_manager};

/// Errors safe to display without exposing registry credentials or command output.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An incomplete or ambiguous manifest cannot be edited automatically.
    #[error("Invalid or ambiguous package.json")]
    Manifest,
    /// A project file could not be inspected.
    #[error(transparent)]
    File(#[from] crate::files::FileError),
    /// A tool failed or its response was unreadable.
    #[error("{0}")]
    Tool(String),
}

/// A direct dependency and its byte positions in the parsed source.
#[derive(Clone, Debug)]
pub struct Dependency {
    /// Registry package name.
    pub name: String,
    /// Dependency group in the manifest.
    pub group: String,
    /// Original requirement.
    pub spec: String,
    /// Full key/value pair.
    pub pair: Range<usize>,
    /// Quoted version value.
    pub value: Range<usize>,
    /// Zero-based line containing the version.
    pub line: usize,
}

impl Dependency {
    fn searchable(&self) -> bool {
        fn segment(s: &str) -> bool {
            !s.is_empty()
                && !s.starts_with(['-', '.', '_'])
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
        }
        let valid = self
            .name
            .strip_prefix('@')
            .and_then(|s| s.split_once('/'))
            .map_or_else(
                || segment(&self.name),
                |(scope, name)| segment(scope) && segment(name),
            );
        valid
            && self.name.len() <= 214
            && (VersionRange::parse(&self.spec).is_ok() || segment(&self.spec))
    }

    fn replacement(&self, version: &Version) -> String {
        let prefix = self
            .spec
            .get(..1)
            .filter(|s| matches!(*s, "^" | "~") && Version::parse(&self.spec[1..]).is_ok())
            .unwrap_or("");
        format!("{prefix}{version}")
    }
}

/// Parsed direct dependencies, in source order.
#[derive(Debug)]
pub struct Manifest {
    /// Dependencies from the four standard groups.
    pub entries: Vec<Dependency>,
}

impl Manifest {
    /// Validate strict JSON and retain the positions of unambiguous dependency entries.
    pub fn parse(text: &str) -> Result<Self, Error> {
        serde_json::from_str::<serde::de::IgnoredAny>(text).map_err(|_| Error::Manifest)?;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_json::LANGUAGE.into())
            .map_err(|_| Error::Manifest)?;
        let tree = parser.parse(text, None).ok_or(Error::Manifest)?;
        let root = tree
            .root_node()
            .named_child(0)
            .filter(|n| n.kind() == "object" && !n.has_error())
            .ok_or(Error::Manifest)?;
        let mut entries = Vec::new();
        let mut groups = HashSet::new();
        for pair in root.named_children(&mut root.walk()) {
            let (group, object) = fields(pair, text)?;
            if !matches!(
                group.as_str(),
                "dependencies" | "devDependencies" | "peerDependencies" | "optionalDependencies"
            ) {
                continue;
            }
            if !groups.insert(group.clone()) || object.kind() != "object" {
                return Err(Error::Manifest);
            }
            let mut names = HashSet::new();
            for pair in object.named_children(&mut object.walk()) {
                let (name, value) = fields(pair, text)?;
                let spec =
                    serde_json::from_str(&text[value.byte_range()]).map_err(|_| Error::Manifest)?;
                if !names.insert(name.clone()) {
                    return Err(Error::Manifest);
                }
                entries.push(Dependency {
                    name,
                    group: group.clone(),
                    spec,
                    line: value.end_position().row,
                    pair: pair.byte_range(),
                    value: value.byte_range(),
                });
            }
        }
        Ok(Self { entries })
    }

    /// Sort complete pairs within each dependency group, preserving separators and other bytes.
    pub fn sorted(text: &str) -> Result<String, Error> {
        let manifest = Self::parse(text)?;
        let mut result = String::with_capacity(text.len());
        let mut offset = 0;
        for group in manifest.entries.chunk_by(|a, b| a.group == b.group) {
            let mut ordered = group.iter().collect::<Vec<_>>();
            ordered.sort_by(|a, b| a.name.cmp(&b.name));
            for (old, new) in group.iter().zip(ordered) {
                result.push_str(&text[offset..old.pair.start]);
                result.push_str(&text[new.pair.clone()]);
                offset = old.pair.end;
            }
        }
        result.push_str(&text[offset..]);
        Ok(result)
    }
}

fn fields<'a>(pair: Node<'a>, text: &str) -> Result<(String, Node<'a>), Error> {
    let key = pair.child_by_field_name("key").ok_or(Error::Manifest)?;
    let name = serde_json::from_str(&text[key.byte_range()]).map_err(|_| Error::Manifest)?;
    Ok((
        name,
        pair.child_by_field_name("value").ok_or(Error::Manifest)?,
    ))
}

/// Registry suggestions and OSV result for a single manifest entry.
#[derive(Clone, Debug)]
pub struct Lens {
    /// Source entry to which this result belongs.
    pub dependency: Dependency,
    /// Visible inline label.
    pub label: String,
    /// Menu label and exact replacement specifier for each upgrade.
    pub choices: Vec<(String, String)>,
    /// Audited version or reason the check was unavailable.
    pub audit: String,
    /// OSV advisory IDs for the audited version.
    pub advisories: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn captured_lockfiles_resolve_the_same_pinned_release() {
        let manifest = Manifest::parse(r#"{"dependencies":{"is-number":"^6.0.0"}}"#).unwrap();
        for (name, bytes) in [
            (
                "package-lock.json",
                include_bytes!("package_lens/fixtures/package-lock.json").as_slice(),
            ),
            ("bun.lock", include_bytes!("package_lens/fixtures/bun.lock")),
            (
                "pnpm-lock.yaml",
                include_bytes!("package_lens/fixtures/pnpm-lock.yaml"),
            ),
            (
                "yarn.lock",
                include_bytes!("package_lens/fixtures/yarn.lock"),
            ),
        ] {
            let lock = lock::Lock::parse(name, bytes).unwrap();
            assert_eq!(
                lock.resolved(Path::new(""), &manifest.entries[0])
                    .unwrap()
                    .to_string(),
                "6.0.0",
                "{name}"
            );
            let web = Manifest::parse(r#"{"dependencies":{"is-number":"^7.0.0"}}"#).unwrap();
            assert_eq!(
                lock.resolved(Path::new("packages/web"), &web.entries[0])
                    .unwrap()
                    .to_string(),
                "7.0.0",
                "{name}"
            );
            assert!(lock::Lock::parse(name, b"{invalid").is_err(), "{name}");
        }
    }

    #[test]
    fn nested_importers_resolve_local_before_hoisted_and_reject_stale_specs() {
        let lock=lock::Lock::parse("package-lock.json",br#"{"packages":{
            "":{"dependencies":{"pkg":"^1"}},"packages/web":{"dependencies":{"pkg":"^2"}},
            "node_modules/pkg":{"version":"1.5.0"},"packages/web/node_modules/pkg":{"version":"2.8.0"}
        }}"#).unwrap();
        let manifest = Manifest::parse(r#"{"dependencies":{"pkg":"^2"}}"#).unwrap();
        let d = &manifest.entries[0];
        assert_eq!(
            lock.resolved(Path::new("packages/web"), d)
                .unwrap()
                .to_string(),
            "2.8.0"
        );
        assert!(lock.resolved(Path::new(""), d).is_none());
        assert!(lock.resolved(Path::new("other"), d).is_none());
    }

    #[test]
    fn sorts_only_dependency_pairs_and_preserves_unicode_crlf_and_spacing() {
        let source = "{\r\n  \"name\":\"тест\", \"dependencies\": { \"z\" : \"~2.0.0\",\r\n \"a\":\"^1.0.0\" }, \"scripts\":{\"z\":1,\"a\":2}, \"devDependencies\":{\"x\":\"1\"}\r\n}";
        let expected = source.replace(
            "\"z\" : \"~2.0.0\",\r\n \"a\":\"^1.0.0\"",
            "\"a\":\"^1.0.0\",\r\n \"z\" : \"~2.0.0\"",
        );
        assert_eq!(Manifest::sorted(source).unwrap(), expected);
        let manifest = Manifest::parse(source).unwrap();
        assert_eq!(&source[manifest.entries[1].value.clone()], "\"^1.0.0\"");
        assert_eq!(manifest.entries[1].line, 2);
    }

    #[test]
    fn malformed_ambiguous_and_nonregistry_specs_cannot_be_updated() {
        for text in [
            "[",
            r#"{"dependencies":{"a":"1","a":"2"}}"#,
            r#"{"dependencies":null}"#,
            r#"{"dependencies":{"a":3}}"#,
        ] {
            assert!(Manifest::parse(text).is_err());
        }
        for (name, spec) in [
            ("a", "workspace:*"),
            ("a", "file:../a"),
            ("a", "github:user/repo"),
            ("--registry", "^1"),
            ("a", "https://example.org/file.tgz"),
            ("a", "org/project"),
        ] {
            let text = serde_json::json!({"dependencies":{name:spec}}).to_string();
            assert!(
                !Manifest::parse(&text).unwrap().entries[0].searchable(),
                "{name}@{spec}"
            );
        }
    }

    #[test]
    fn old_and_malformed_project_settings_never_opt_in_to_install() {
        for text in [
            r#"{"path":"/repo"}"#,
            r#"{"path":"/repo","install_on_save":"yes","future":true}"#,
        ] {
            assert!(
                !serde_json::from_str::<crate::state::ProjectRef>(text)
                    .unwrap()
                    .install_on_save
            );
        }
    }
}
