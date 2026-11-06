//! `package.json` and `tsconfig.json`/`jsconfig.json`, parsed once per file version.
use crate::files;
use crate::host::Host;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

/// Beyond this many cached files the cache starts over instead of growing without bound.
const MAX_CACHED: usize = 512;

/// The `package.json` fields module resolution reads. Condition maps stay JSON values:
/// they are recursive and keyed by arbitrary condition names.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct PackageJson {
    pub imports: Option<Value>,
    pub exports: Option<Value>,
    pub types: Option<String>,
    pub typings: Option<String>,
    pub module: Option<String>,
    pub main: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct TsConfig {
    pub extends: Option<Extends>,
    #[serde(rename = "compilerOptions")]
    pub compiler_options: CompilerOptions,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum Extends {
    One(String),
    Many(Vec<String>),
}

impl Extends {
    pub fn parents(&self) -> &[String] {
        match self {
            Self::One(parent) => std::slice::from_ref(parent),
            Self::Many(parents) => parents,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct CompilerOptions {
    #[serde(rename = "baseUrl")]
    pub base_url: Option<String>,
    pub paths: Option<BTreeMap<String, Vec<String>>>,
}

struct Entry<T> {
    modified: SystemTime,
    /// `None` when the file is not valid JSON; kept so a broken file is not re-parsed each time.
    value: Option<Arc<T>>,
}

type Cache<T> = Mutex<HashMap<PathBuf, Entry<T>>>;

#[derive(Default)]
pub(super) struct Configs {
    packages: Cache<PackageJson>,
    tsconfigs: Cache<TsConfig>,
}

impl Configs {
    pub fn package(&self, root: &Path, relative: &Path) -> Option<Arc<PackageJson>> {
        load(&self.packages, root, relative, |text| {
            serde_json::from_str(text)
        })
    }

    pub fn tsconfig(&self, root: &Path, relative: &Path) -> Option<Arc<TsConfig>> {
        load(&self.tsconfigs, root, relative, |text| {
            serde_json::from_str(&without_jsonc_syntax(text))
        })
    }
}

/// The parsed file, re-read only when its modification time changes. Missing files are not
/// cached: probing for them is a single `stat`.
fn load<T: DeserializeOwned>(
    cache: &Cache<T>,
    root: &Path,
    relative: &Path,
    parse: impl Fn(&str) -> serde_json::Result<T>,
) -> Option<Arc<T>> {
    let path = files::resolve(&Host::Local, root, relative).ok()?;
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified = metadata.modified().ok()?;
    let mut entries = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(entry) = entries.get(&path).filter(|e| e.modified == modified) {
        return entry.value.clone();
    }
    let value = match files::read(&Host::Local, root, relative) {
        Ok(snapshot) => match parse(&snapshot.text()) {
            Ok(value) => Some(Arc::new(value)),
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "ignoring malformed config");
                None
            }
        },
        Err(error) => {
            tracing::debug!(%error, "cannot read config");
            return None;
        }
    };
    if entries.len() >= MAX_CACHED {
        entries.clear();
    }
    entries.insert(
        path,
        Entry {
            modified,
            value: value.clone(),
        },
    );
    value
}

/// `tsconfig.json` is JSONC: drop comments and trailing commas outside strings.
pub(super) fn without_jsonc_syntax(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            output.push(c);
            match c {
                '\\' => output.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        let next = chars.peek().copied();
        match (c, next) {
            ('"', _) => {
                in_string = true;
                output.push(c);
            }
            ('/', Some('/')) => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        output.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
                output.push(' ');
            }
            (',', _) if closes_next(chars.clone()) => {}
            _ => output.push(c),
        }
    }
    output
}

/// Whether the next significant character closes an object or array, ignoring comments.
fn closes_next(mut chars: std::iter::Peekable<std::str::Chars<'_>>) -> bool {
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        match (c, next) {
            (c, _) if c.is_whitespace() => {}
            ('/', Some('/')) => {
                chars.by_ref().find(|&next| next == '\n');
            }
            ('/', Some('*')) => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            (c, _) => return matches!(c, '}' | ']'),
        }
    }
    false
}
