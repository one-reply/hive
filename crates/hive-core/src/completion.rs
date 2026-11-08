//! Code completion from the open file alone: no language server and no project-wide index.
//!
//! [`Index::build`] runs on a background thread over a tree-sitter parse of the buffer. It keeps
//! every identifier the file contains, split into plain names and members (names written after
//! `.`, `?.` or `::`), and the comment and string spans where the menu stays quiet. On each
//! keystroke [`site_at`] reads the bytes around the cursor to decide what is being typed and
//! [`Index::matches`] filters the cached names; neither parses nor touches the disk. Module
//! specifiers and the names a module exports come from [`crate::code_navigation::Navigator`].
use std::path::Path;
use tree_sitter::{Language, Parser, Tree};

mod index;
mod site;

pub use index::Index;
pub use site::{Request, Site, is_trigger, site_at};

/// The menu never shows more items than this; the closest matches come first.
pub const MAX_ITEMS: usize = 50;

/// Language rules that change what the cursor completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// JavaScript and TypeScript: `$` in names, module specifiers and named imports.
    Script,
    /// Every other language: names and members only.
    Other,
}

impl Family {
    /// The family of a source file, by its extension.
    pub fn for_path(path: &Path) -> Self {
        if crate::code_navigation::is_supported(path) {
            Self::Script
        } else {
            Self::Other
        }
    }

    fn is_name_char(self, c: char) -> bool {
        c.is_alphanumeric() || c == '_' || (c == '$' && self == Self::Script)
    }
}

/// `None` only when `grammar` was built for an incompatible tree-sitter version.
pub fn parse(text: &str, grammar: &Language) -> Option<Tree> {
    let mut parser = Parser::new();
    if let Err(error) = parser.set_language(grammar) {
        tracing::warn!(%error, "tree-sitter grammar is incompatible");
        return None;
    }
    parser.parse(text, None)
}

/// Names from `lists` that start with `prefix`, at most [`MAX_ITEMS`]: list by list, and in each
/// exact-case matches before the rest. `prefix` itself is never offered.
pub(crate) fn ranked<'a>(lists: &[&'a [String]], prefix: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for names in lists {
        for exact_case in [true, false] {
            for name in names.iter() {
                if found.len() == MAX_ITEMS {
                    return found;
                }
                let matched = if exact_case {
                    name.starts_with(prefix)
                } else {
                    !name.starts_with(prefix) && starts_with_ignoring_case(name, prefix)
                };
                if matched && name != prefix && seen.insert(name.as_str()) {
                    found.push(name.as_str());
                }
            }
        }
    }
    found
}

/// Prefix comparison that folds case the Unicode way, so `ПРИ` matches `привіт`.
pub(crate) fn starts_with_ignoring_case(text: &str, prefix: &str) -> bool {
    let mut text = text.chars().flat_map(char::to_lowercase);
    prefix
        .chars()
        .flat_map(char::to_lowercase)
        .all(|expected| text.next() == Some(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_ignores_case_in_any_script() {
        assert!(starts_with_ignoring_case("useState", "uses"));
        assert!(starts_with_ignoring_case("Привіт", "пРИ"));
        assert!(starts_with_ignoring_case("anything", ""));
        assert!(!starts_with_ignoring_case("use", "used"));
    }

    #[test]
    fn only_javascript_and_typescript_get_script_rules() {
        assert_eq!(Family::for_path(Path::new("a/b.tsx")), Family::Script);
        assert_eq!(Family::for_path(Path::new("b.MJS")), Family::Script);
        assert_eq!(Family::for_path(Path::new("main.rs")), Family::Other);
        assert_eq!(Family::for_path(Path::new("Makefile")), Family::Other);
    }
}
