//! Dependency versions and source-preserving edits for an open npm manifest.
use node_semver::Range as VersionRange;
pub use node_semver::Version;
use std::{collections::HashSet, ops::Range};
use tree_sitter::Node;

mod lock;
mod registry;
mod workspace;
pub use registry::{Cache, inspect};

/// One replacement in the manifest text, as byte offsets and zero-based lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEdit {
    /// First replaced byte.
    pub start: usize,
    /// End of the replaced bytes, in the text before the edit.
    pub old_end: usize,
    /// End of the inserted bytes, in the text after the edit.
    pub new_end: usize,
    /// Line of `old_end` before the edit.
    pub old_end_line: usize,
    /// Line of `new_end` after the edit.
    pub new_end_line: usize,
}
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Keep the positions on the same entry across `edit`. False when the edit touched the entry
    /// itself: its name or requirement may no longer be what was looked up.
    pub fn follow(&mut self, edit: &TextEdit) -> bool {
        if edit.start >= self.pair.end {
            return true;
        }
        if edit.old_end > self.pair.start {
            return false;
        }
        // Everything from `old_end` on moved by the same number of bytes and lines.
        let Some(line) = (self.line + edit.new_end_line).checked_sub(edit.old_end_line) else {
            return false;
        };
        let shift = |offset: usize| offset - edit.old_end + edit.new_end;
        self.pair = shift(self.pair.start)..shift(self.pair.end);
        self.value = shift(self.value.start)..shift(self.value.end);
        self.line = line;
        true
    }

    /// A registry package with a version range or a dist-tag, not a path, URL or workspace link.
    pub fn is_searchable(&self) -> bool {
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

/// How far an upgrade moves from the version the requirement names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// A new major version, which may break callers.
    Major,
    /// A new minor version.
    Minor,
    /// A new patch version.
    Patch,
}

impl Change {
    /// The step from `from` to a newer `to`; `None` when `to` is not newer in its numbers.
    pub fn between(from: &Version, to: &Version) -> Option<Self> {
        use std::cmp::Ordering::{Equal, Greater};
        match (
            to.major.cmp(&from.major),
            to.minor.cmp(&from.minor),
            to.patch.cmp(&from.patch),
        ) {
            (Greater, _, _) => Some(Self::Major),
            (Equal, Greater, _) => Some(Self::Minor),
            (Equal, Equal, Greater) => Some(Self::Patch),
            _ => None,
        }
    }
}

/// Which registry answer an upgrade follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// The newest release the current requirement already accepts.
    Wanted,
    /// The release the registry tags `latest`.
    Latest,
}

/// One replacement of a requirement that moves it to a newer release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upgrade {
    /// Where the release comes from.
    pub channel: Channel,
    /// The release the new requirement names.
    pub version: Version,
    /// The step from the current requirement, when it names an exact version to compare with.
    pub change: Option<Change>,
    /// The new requirement, keeping a `^` or `~` prefix of the old one.
    pub spec: String,
}

/// What the registry said about one requirement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Versions {
    /// The workspace's registry client failed for this package; the text is safe to display.
    Unavailable(String),
    /// Upgrades in the order offered, the one inside the current range first. Empty when the
    /// requirement already names the newest release.
    Found(Vec<Upgrade>),
}

/// Registry suggestions and OSV result for a single manifest entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lens {
    /// Source entry to which this result belongs.
    pub dependency: Dependency,
    /// The exact release the entry resolves to: the lockfile's, or the version the requirement
    /// names exactly. `None` when neither says, or the lockfile is older than the manifest.
    pub resolved: Option<Version>,
    /// Newer releases, or why they are unknown.
    pub versions: Versions,
    /// OSV advisory IDs for the resolved version. Empty when none are known, and also when the
    /// version could not be checked (a private package, no lockfile entry, OSV unavailable).
    pub advisories: Vec<String>,
    /// OSV knows more advisories than `advisories` lists.
    pub more_advisories: bool,
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
                !Manifest::parse(&text).unwrap().entries[0].is_searchable(),
                "{name}@{spec}"
            );
        }
    }

    /// The edit that turns `before` into `after` by replacing `before[start..old_end]`.
    fn edit(before: &str, after: &str, start: usize, old_end: usize) -> TextEdit {
        let new_end = old_end + after.len() - before.len();
        let line = |text: &str, offset: usize| text[..offset].matches('\n').count();
        TextEdit {
            start,
            old_end,
            new_end,
            old_end_line: line(before, old_end),
            new_end_line: line(after, new_end),
        }
    }

    #[test]
    fn entries_follow_edits_around_them_and_drop_out_when_edited() {
        let before = "{\"dependencies\": {\n  \"a\": \"^1.0.0\",\n  \"b\": \"~2.0.0\"\n}}";
        let entries = Manifest::parse(before).unwrap().entries;
        let follow = |edit: TextEdit| {
            entries
                .iter()
                .cloned()
                .filter_map(|mut d| d.follow(&edit).then_some(d))
                .collect::<Vec<_>>()
        };
        // A line inserted before both entries: both move down by a line and its bytes.
        let at = before.find("  \"a\"").unwrap();
        let after = format!("{}  \"new\": \"1\",\n{}", &before[..at], &before[at..]);
        let moved = follow(edit(before, &after, at, at));
        assert_eq!(moved, Manifest::parse(&after).unwrap().entries[1..]);
        // Typing right before an entry's key moves it on the same line.
        let key = before.find("\"b\"").unwrap();
        let after = format!("{}x{}", &before[..key], &before[key..]);
        let moved = follow(edit(before, &after, key, key));
        assert_eq!(moved[0], entries[0]);
        assert_eq!(
            (moved[1].line, &after[moved[1].value.clone()]),
            (2, "\"~2.0.0\"")
        );
        // Editing inside a requirement drops that entry and leaves the earlier one alone.
        let digit = before.find("2.0.0").unwrap();
        let after = before.replacen("2.0.0", "3.0.0", 1);
        assert_eq!(follow(edit(before, &after, digit, digit + 1)), entries[..1]);
        // Text added after the closing quote does not touch the entry.
        let end = entries[0].pair.end;
        let after = format!("{} {}", &before[..end], &before[end..]);
        let kept = follow(edit(before, &after, end, end));
        assert_eq!(kept[0], entries[0]);
        // Deleting the whole first line, newline included, moves the second one up.
        let line = before.find("  \"a\"").unwrap()..before.find("  \"b\"").unwrap();
        let after = format!("{}{}", &before[..line.start], &before[line.end..]);
        let moved = follow(edit(before, &after, line.start, line.end));
        assert_eq!(moved, Manifest::parse(&after).unwrap().entries);
    }

    #[test]
    fn an_upgrade_step_is_the_first_number_that_grows() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert_eq!(
            Change::between(&v("1.2.3"), &v("2.0.0")),
            Some(Change::Major)
        );
        assert_eq!(
            Change::between(&v("1.2.3"), &v("1.3.0")),
            Some(Change::Minor)
        );
        assert_eq!(
            Change::between(&v("1.2.3"), &v("1.2.4")),
            Some(Change::Patch)
        );
        // A lower major with a higher minor is not an upgrade at all.
        assert_eq!(Change::between(&v("2.5.0"), &v("1.9.0")), None);
        assert_eq!(Change::between(&v("1.2.3"), &v("1.2.3")), None);
        assert_eq!(Change::between(&v("1.2.3-beta.1"), &v("1.2.3")), None);
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
