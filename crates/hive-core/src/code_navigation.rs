//! Go to definition for JavaScript and TypeScript imports, resolved on demand.
//!
//! There is no language server and no project-wide index: a request parses the open text,
//! finds the import that binds the symbol under the cursor, resolves its module specifier
//! the way Node and TypeScript do (relative paths, `package.json` `imports`, `tsconfig`
//! `paths`, `node_modules`) and follows re-exports to the declaration. Only the few files on
//! that chain are read, through the same bounded local IO as the editor. Blocking: call it
//! from a background executor.
use crate::files::{self, FileError};
use crate::host::Host;
use std::collections::HashSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

mod config;
mod exports;
mod imports;
mod resolve;
mod syntax;
#[cfg(test)]
mod tests;

use exports::Export;
use imports::Imported;
use syntax::Dialect;

/// Re-export chains longer than this are treated as unresolved rather than walked further.
const MAX_HOPS: usize = 8;

/// Where an imported symbol is declared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// Declaring file, relative to the worktree root.
    pub path: PathBuf,
    /// Zero-based line of the declaration's name.
    pub line: u32,
    /// Zero-based column of the declaration's name, in characters.
    pub column: u32,
    /// Byte range of the clicked symbol in the requesting text.
    pub origin: Range<usize>,
}

/// A navigation request that could not be answered.
#[derive(Debug, thiserror::Error)]
pub enum NavigationError {
    /// The requesting text could not be parsed.
    #[error("Cannot parse {path} for navigation")]
    Parse {
        /// Requesting file, relative to the worktree root.
        path: PathBuf,
    },
    /// A module on the import chain resolved but could not be read.
    #[error(transparent)]
    Read(#[from] FileError),
}

/// Whether `path` is a JavaScript or TypeScript source this module understands.
pub fn is_supported(path: &Path) -> bool {
    Dialect::for_path(path).is_some()
}

/// Resolves import definitions; keeps parsed `package.json`/`tsconfig.json` between requests.
#[derive(Default)]
pub struct Navigator {
    configs: config::Configs,
}

impl Navigator {
    /// Declaration of the imported symbol at byte `offset` of `text`, the unsaved contents of
    /// `file` (relative to the absolute worktree `root`). `Ok(None)` when the symbol is not an
    /// import or its module is outside the worktree.
    pub fn definition(
        &self,
        root: &Path,
        file: &Path,
        text: &str,
        offset: usize,
    ) -> Result<Option<Definition>, NavigationError> {
        let Some(dialect) = Dialect::for_path(file) else {
            return Ok(None);
        };
        let tree = syntax::parse(text, dialect)
            .ok_or_else(|| NavigationError::Parse { path: file.into() })?;
        let Some(reference) = imports::reference_at(&tree, text, offset) else {
            return Ok(None);
        };
        let resolver = resolve::Resolver::new(root, &self.configs);
        let Some(module) = resolver.resolve(file, &reference.specifier) else {
            return Ok(None);
        };
        let mut visited = HashSet::new();
        let target = self
            .follow(
                &resolver,
                module.clone(),
                reference.imported,
                &mut visited,
                0,
            )?
            .unwrap_or(Target {
                path: module,
                text: String::new(),
                offset: 0,
            });
        let (line, column) = line_column(&target.text, target.offset);
        Ok(Some(Definition {
            path: target.path,
            line,
            column,
            origin: reference.origin,
        }))
    }

    /// Finds `imported` in `module`, walking re-exports; `None` when the chain ends without it.
    fn follow(
        &self,
        resolver: &resolve::Resolver,
        module: PathBuf,
        imported: Imported,
        visited: &mut HashSet<(PathBuf, Imported)>,
        hops: usize,
    ) -> Result<Option<Target>, NavigationError> {
        if hops > MAX_HOPS || !visited.insert((module.clone(), imported.clone())) {
            return Ok(None);
        }
        let text = files::read(&Host::Local, resolver.root(), &module)?.text();
        let found = |offset| Target {
            path: module.clone(),
            text: text.clone(),
            offset,
        };
        let Some(dialect) = Dialect::for_path(&module) else {
            return Ok(Some(found(0)));
        };
        let Some(tree) = syntax::parse(&text, dialect) else {
            return Ok(Some(found(0)));
        };
        let lookup = exports::lookup(&tree, &text, &imported);
        match lookup.export {
            Export::Declared(offset) => return Ok(Some(found(offset))),
            Export::Forwarded {
                specifier,
                imported,
                at,
            } => {
                let followed = match resolver.resolve(&module, &specifier) {
                    Some(next) => self.follow(resolver, next, imported, visited, hops + 1)?,
                    None => None,
                };
                return Ok(followed.or_else(|| Some(found(at))));
            }
            Export::Missing => {}
        }
        for specifier in lookup.star_sources {
            let Some(next) = resolver.resolve(&module, &specifier) else {
                continue;
            };
            if let Some(target) =
                self.follow(resolver, next, imported.clone(), visited, hops + 1)?
            {
                return Ok(Some(target));
            }
        }
        Ok(None)
    }
}

struct Target {
    path: PathBuf,
    text: String,
    offset: usize,
}

/// Zero-based line and character column of byte `offset`, clamped to `text`.
fn line_column(text: &str, offset: usize) -> (u32, u32) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |ix| ix + 1);
    let line = before.matches('\n').count();
    let column = before[line_start..].chars().count();
    (
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(column).unwrap_or(u32::MAX),
    )
}
