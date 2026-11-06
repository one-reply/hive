//! Tree-sitter parsing and the node helpers shared by import and export lookup.
use std::path::Path;
use tree_sitter::{Node, Parser, Tree};

/// Grammar used for a source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Dialect {
    /// `.js`, `.jsx`, `.mjs`, `.cjs` (the grammar includes JSX).
    JavaScript,
    /// `.ts`, `.mts`, `.cts`, `.d.ts`.
    TypeScript,
    /// `.tsx`.
    Tsx,
}

impl Dialect {
    pub(super) fn for_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            _ => None,
        }
    }
}

/// `None` only if tree-sitter gives up, which it does not for plain parses without a timeout.
pub(super) fn parse(text: &str, dialect: Dialect) -> Option<Tree> {
    let language = match dialect {
        Dialect::JavaScript => tree_sitter_javascript::LANGUAGE,
        Dialect::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        Dialect::Tsx => tree_sitter_typescript::LANGUAGE_TSX,
    };
    let mut parser = Parser::new();
    if let Err(error) = parser.set_language(&language.into()) {
        tracing::warn!(?dialect, %error, "tree-sitter grammar is incompatible");
        return None;
    }
    parser.parse(text, None)
}

pub(super) fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or_default()
}

/// Contents of a string literal without quotes; escape sequences are kept verbatim, which is
/// enough for module specifiers.
pub(super) fn string_value(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let raw = text(node, source);
    let inner = raw.get(1..raw.len().checked_sub(1)?)?;
    Some(inner.to_owned())
}

/// Name of an import/export specifier part: an identifier or a string such as `"a b"`.
pub(super) fn name_value(node: Node<'_>, source: &str) -> String {
    string_value(node, source).unwrap_or_else(|| text(node, source).to_owned())
}

pub(super) fn named_children(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    (0..node.named_child_count()).filter_map(move |ix| node.named_child(ix as u32))
}

pub(super) fn has_child_kind(node: Node<'_>, kind: &str) -> bool {
    (0..node.child_count()).any(|ix| node.child(ix as u32).is_some_and(|c| c.kind() == kind))
}

/// The module specifier of `require('x')`, if `node` is exactly that call.
pub(super) fn require_specifier(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "identifier" || text(function, source) != "require" {
        return None;
    }
    let arguments = node.child_by_field_name("arguments")?;
    let first = named_children(arguments).next()?;
    string_value(first, source)
}
