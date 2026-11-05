//! Which import binds the symbol under the cursor.
use super::syntax::{name_value, named_children, require_specifier, string_value, text};
use std::ops::Range;
use tree_sitter::{Node, Tree};

/// What a binding takes from its module.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Imported {
    /// The module itself: `import * as ns`, `const m = require()`, the specifier string.
    Module,
    /// `import x from`.
    Default,
    /// `import { x }`, `const { x } = require()`, `ns.x`.
    Named(String),
}

impl Imported {
    pub(super) fn from_name(name: String) -> Self {
        if name == "default" {
            Self::Default
        } else {
            Self::Named(name)
        }
    }
}

/// A local name introduced by an import, `require` or a re-export of the current file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Binding {
    pub local: String,
    /// Byte offset of the local name.
    pub at: usize,
    pub specifier: String,
    pub imported: Imported,
}

/// The imported symbol at a cursor offset.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Reference {
    pub specifier: String,
    pub imported: Imported,
    pub origin: Range<usize>,
}

/// Identifier node kinds that can name an imported binding.
const NAME_KINDS: &[&str] = &[
    "identifier",
    "type_identifier",
    "shorthand_property_identifier",
    "shorthand_property_identifier_pattern",
];

pub(super) fn reference_at(tree: &Tree, source: &str, offset: usize) -> Option<Reference> {
    let node = node_at(tree, offset)?;
    let origin = node.byte_range();
    if node.kind() == "string_fragment" {
        let specifier = module_string(node, source)?;
        return Some(Reference {
            specifier,
            imported: Imported::Module,
            origin,
        });
    }
    if let Some(reference) = specifier_reference(node, source) {
        return Some(reference);
    }
    let bindings = bindings(tree.root_node(), source);
    if let Some((module, name)) = qualified_name(node, source) {
        let binding = bindings
            .iter()
            .find(|b| b.local == module && b.imported == Imported::Module)?;
        return Some(Reference {
            specifier: binding.specifier.clone(),
            imported: Imported::from_name(name),
            origin,
        });
    }
    if !NAME_KINDS.contains(&node.kind()) {
        return None;
    }
    let name = text(node, source);
    let binding = bindings.into_iter().find(|b| b.local == name)?;
    Some(Reference {
        specifier: binding.specifier,
        imported: binding.imported,
        origin,
    })
}

/// The smallest name-like node at `offset`, also accepting a cursor just past a word's end.
fn node_at(tree: &Tree, offset: usize) -> Option<Node<'_>> {
    let accept = |node: &Node| {
        NAME_KINDS.contains(&node.kind())
            || matches!(node.kind(), "property_identifier" | "string_fragment")
    };
    let root = tree.root_node();
    root.descendant_for_byte_range(offset, offset)
        .filter(accept)
        .or_else(|| {
            let before = offset.checked_sub(1)?;
            root.descendant_for_byte_range(before, before)
                .filter(accept)
        })
}

/// The specifier when `fragment` is the module string of an import, re-export or `require`.
fn module_string(fragment: Node<'_>, source: &str) -> Option<String> {
    let string = fragment.parent()?;
    let owner = string.parent()?;
    let is_source = match owner.kind() {
        "import_statement" | "export_statement" | "import_require_clause" => {
            owner.child_by_field_name("source") == Some(string)
        }
        "arguments" => owner
            .parent()
            .is_some_and(|call| require_specifier(call, source).is_some()),
        _ => false,
    };
    is_source.then(|| string_value(string, source)).flatten()
}

/// A name inside `import { a as b } from 'x'` or `export { a } from 'x'`: both `a` and `b`
/// lead to `x`'s `a`.
fn specifier_reference(node: Node<'_>, source: &str) -> Option<Reference> {
    let specifier = node.parent()?;
    let statement = match specifier.kind() {
        // import_specifier → named_imports → import_clause → import_statement
        "import_specifier" => specifier.parent()?.parent()?.parent()?,
        // export_specifier → export_clause → export_statement
        "export_specifier" => specifier.parent()?.parent()?,
        _ => return None,
    };
    let module = statement
        .child_by_field_name("source")
        .and_then(|s| string_value(s, source))?;
    let name = specifier.child_by_field_name("name")?;
    Some(Reference {
        specifier: module,
        imported: Imported::from_name(name_value(name, source)),
        origin: node.byte_range(),
    })
}

/// `ns.member` (value or type position): the namespace and member names when `node` is the member.
fn qualified_name(node: Node<'_>, source: &str) -> Option<(String, String)> {
    let parent = node.parent()?;
    let (object, member) = match (node.kind(), parent.kind()) {
        ("property_identifier", "member_expression") => (
            parent.child_by_field_name("object")?,
            parent.child_by_field_name("property")?,
        ),
        ("type_identifier", "nested_type_identifier") => (
            parent.child_by_field_name("module")?,
            parent.child_by_field_name("name")?,
        ),
        _ => return None,
    };
    if member != node || object.kind() != "identifier" {
        return None;
    }
    Some((
        text(object, source).to_owned(),
        text(node, source).to_owned(),
    ))
}

/// Top-level bindings, in source order. Shadowing by local declarations is ignored.
pub(super) fn bindings(program: Node<'_>, source: &str) -> Vec<Binding> {
    let mut bindings = Vec::new();
    for statement in named_children(program) {
        match statement.kind() {
            "import_statement" => import_bindings(statement, source, &mut bindings),
            "export_statement" => reexport_bindings(statement, source, &mut bindings),
            "lexical_declaration" | "variable_declaration" => {
                require_bindings(statement, source, &mut bindings)
            }
            _ => {}
        }
    }
    bindings
}

fn import_bindings(statement: Node<'_>, source: &str, bindings: &mut Vec<Binding>) {
    for child in named_children(statement) {
        match child.kind() {
            "import_clause" => {
                let Some(specifier) = statement
                    .child_by_field_name("source")
                    .and_then(|s| string_value(s, source))
                else {
                    return;
                };
                clause_bindings(child, &specifier, source, bindings);
            }
            "import_require_clause" => {
                let specifier = child
                    .child_by_field_name("source")
                    .and_then(|s| string_value(s, source));
                let local = named_children(child).find(|n| n.kind() == "identifier");
                if let (Some(specifier), Some(local)) = (specifier, local) {
                    bindings.push(Binding {
                        local: text(local, source).to_owned(),
                        at: local.start_byte(),
                        specifier,
                        imported: Imported::Module,
                    });
                }
            }
            _ => {}
        }
    }
}

fn clause_bindings(clause: Node<'_>, specifier: &str, source: &str, bindings: &mut Vec<Binding>) {
    let mut push = |local: Node<'_>, imported| {
        bindings.push(Binding {
            local: text(local, source).to_owned(),
            at: local.start_byte(),
            specifier: specifier.to_owned(),
            imported,
        })
    };
    for part in named_children(clause) {
        match part.kind() {
            "identifier" => push(part, Imported::Default),
            "namespace_import" => {
                if let Some(local) = named_children(part).find(|n| n.kind() == "identifier") {
                    push(local, Imported::Module);
                }
            }
            "named_imports" => {
                for specifier in named_children(part).filter(|n| n.kind() == "import_specifier") {
                    let Some(name) = specifier.child_by_field_name("name") else {
                        continue;
                    };
                    let local = specifier.child_by_field_name("alias").unwrap_or(name);
                    push(local, Imported::from_name(name_value(name, source)));
                }
            }
            _ => {}
        }
    }
}

/// `export { a, b as c } from 'x'`: clicking `a` goes to `x`'s `a`.
fn reexport_bindings(statement: Node<'_>, source: &str, bindings: &mut Vec<Binding>) {
    let Some(specifier) = statement
        .child_by_field_name("source")
        .and_then(|s| string_value(s, source))
    else {
        return;
    };
    let Some(clause) = named_children(statement).find(|n| n.kind() == "export_clause") else {
        return;
    };
    for part in named_children(clause).filter(|n| n.kind() == "export_specifier") {
        let Some(name) = part.child_by_field_name("name") else {
            continue;
        };
        let at = name.start_byte();
        let name = name_value(name, source);
        bindings.push(Binding {
            local: name.clone(),
            at,
            specifier: specifier.clone(),
            imported: Imported::from_name(name),
        });
    }
}

/// `const m = require('x')` and `const { a, b: c } = require('x')`.
fn require_bindings(declaration: Node<'_>, source: &str, bindings: &mut Vec<Binding>) {
    for declarator in named_children(declaration).filter(|n| n.kind() == "variable_declarator") {
        let (Some(pattern), Some(value)) = (
            declarator.child_by_field_name("name"),
            declarator.child_by_field_name("value"),
        ) else {
            continue;
        };
        let Some(specifier) = require_specifier(value, source) else {
            continue;
        };
        let mut push = |local: Node<'_>, imported| {
            bindings.push(Binding {
                local: text(local, source).to_owned(),
                at: local.start_byte(),
                specifier: specifier.clone(),
                imported,
            })
        };
        match pattern.kind() {
            "identifier" => push(pattern, Imported::Module),
            "object_pattern" => {
                for property in named_children(pattern) {
                    match property.kind() {
                        "shorthand_property_identifier_pattern" => {
                            push(property, Imported::Named(text(property, source).to_owned()))
                        }
                        "pair_pattern" => {
                            let key = property.child_by_field_name("key");
                            let value = property.child_by_field_name("value");
                            if let (Some(key), Some(value)) = (key, value)
                                && value.kind() == "identifier"
                            {
                                push(value, Imported::from_name(name_value(key, source)));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}
