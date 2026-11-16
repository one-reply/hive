//! Declaration outlines over the same syntax tree used for local completion.
use std::ops::Range;
use tree_sitter::{Node, Tree};

/// A named declaration and its extent in the current UTF-8 buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// Display name, including the receiver for Rust impl blocks.
    pub name: String,
    /// Language syntax kind, useful in an outline without a second classification.
    pub kind: String,
    /// Full declaration extent, in bytes.
    pub range: Range<usize>,
    /// Byte offset of its name.
    pub offset: usize,
    /// Nesting depth among named declarations.
    pub depth: usize,
}

/// Whether the initial outline has declaration rules for this editor language.
pub fn supports(language: &str) -> bool {
    matches!(language, "javascript" | "typescript" | "tsx" | "rust")
}

/// Collect declarations in source order, tolerating incomplete trees and UTF-8 names.
pub fn collect(tree: &Tree, text: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let mut parents: Vec<usize> = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        while parents.last().is_some_and(|end| *end <= node.start_byte()) {
            parents.pop();
        }
        if let Some(name) = declaration_name(node)
            && let Some(value) = text.get(name.byte_range())
            && !value.is_empty()
        {
            symbols.push(Symbol {
                name: value.to_owned(),
                kind: node.kind().into(),
                range: node.byte_range(),
                offset: name.start_byte(),
                depth: parents.len(),
            });
            parents.push(node.end_byte());
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return symbols;
            }
        }
    }
}

fn declaration_name(node: Node<'_>) -> Option<Node<'_>> {
    match node.kind() {
        "function_declaration"
        | "generator_function_declaration"
        | "class_declaration"
        | "method_definition"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration"
        | "internal_module"
        | "function_item"
        | "struct_item"
        | "enum_item"
        | "trait_item"
        | "mod_item"
        | "type_item" => node.child_by_field_name("name"),
        "impl_item" => node.child_by_field_name("type"),
        "variable_declarator" => {
            let value = node.child_by_field_name("value")?;
            matches!(
                value.kind(),
                "arrow_function" | "function_expression" | "generator_function"
            )
            .then(|| node.child_by_field_name("name"))
            .flatten()
        }
        _ => None,
    }
}

/// The innermost declaration containing the byte offset.
pub fn containing(symbols: &[Symbol], offset: usize) -> Option<&Symbol> {
    symbols
        .iter()
        .rev()
        .find(|symbol| symbol.range.contains(&offset))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outlines_nested_declarations_and_arrow_functions_without_calls() {
        let text = "export class Café { run() { function inner() {} inner(); } } const f = () => 1; const x = 2;";
        let tree =
            crate::completion::parse(text, &tree_sitter_javascript::LANGUAGE.into()).unwrap();
        let symbols = collect(&tree, text);
        assert_eq!(
            symbols
                .iter()
                .map(|s| (s.name.as_str(), s.depth))
                .collect::<Vec<_>>(),
            [("Café", 0), ("run", 1), ("inner", 2), ("f", 0)]
        );
        let inner = &symbols[2];
        assert_eq!(containing(&symbols, inner.offset), Some(inner));
        assert_eq!(
            &text[inner.offset..inner.offset + inner.name.len()],
            "inner"
        );
    }
    #[test]
    fn incomplete_typescript_and_empty_documents_are_tolerated() {
        let language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let text = "interface User { name: string }\nfunction unfinished(";
        let symbols = collect(&crate::completion::parse(text, &language).unwrap(), text);
        assert!(symbols.iter().any(|s| s.name == "User"));
        assert!(collect(&crate::completion::parse("", &language).unwrap(), "").is_empty());
    }
}
