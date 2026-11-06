//! Where a module declares, or forwards, one of its exports.
use super::imports::{self, Imported};
use super::syntax::{has_child_kind, name_value, named_children, text};
use tree_sitter::{Node, Tree};

/// The answer for one module, before following other modules.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Export {
    /// Declared here; byte offset of the declaration's name.
    Declared(usize),
    /// Re-exported from another module (`export { a } from`, or an imported binding exported
    /// again); `at` is the forwarding statement, used if the other module cannot be followed.
    Forwarded {
        specifier: String,
        imported: Imported,
        at: usize,
    },
    /// Not exported by name; `star_sources` may still provide it.
    Missing,
}

#[derive(Debug)]
pub(super) struct Lookup {
    pub export: Export,
    /// `export * from '…'` specifiers, tried in order when the name is missing here.
    pub star_sources: Vec<String>,
}

pub(super) fn lookup(tree: &Tree, source: &str, imported: &Imported) -> Lookup {
    let program = tree.root_node();
    let mut star_sources = Vec::new();
    if *imported == Imported::Module {
        return Lookup {
            export: Export::Declared(0),
            star_sources,
        };
    }
    for statement in named_children(program) {
        let export = match statement.kind() {
            "export_statement" => {
                export_statement(program, statement, source, imported, &mut star_sources)
            }
            "expression_statement" => commonjs_export(program, statement, source, imported),
            _ => Export::Missing,
        };
        if export != Export::Missing {
            return Lookup {
                export,
                star_sources,
            };
        }
    }
    Lookup {
        export: Export::Missing,
        star_sources,
    }
}

fn export_statement(
    program: Node<'_>,
    statement: Node<'_>,
    source: &str,
    imported: &Imported,
    star_sources: &mut Vec<String>,
) -> Export {
    let specifier = statement
        .child_by_field_name("source")
        .and_then(|s| super::syntax::string_value(s, source));
    if let Some(specifier) = specifier {
        return forwarding_export(statement, specifier, source, imported, star_sources);
    }
    if has_child_kind(statement, "default") {
        if *imported != Imported::Default {
            return Export::Missing;
        }
        if let Some(declaration) = statement.child_by_field_name("declaration") {
            let name = declaration
                .child_by_field_name("name")
                .unwrap_or(declaration);
            return Export::Declared(name.start_byte());
        }
        return match statement.child_by_field_name("value") {
            Some(value) => expression(program, value, source),
            None => Export::Declared(statement.start_byte()),
        };
    }
    let Imported::Named(name) = imported else {
        return clause_export(program, statement, source, "default");
    };
    if let Some(declaration) = statement.child_by_field_name("declaration") {
        return declared_names(declaration)
            .into_iter()
            .find(|node| text(*node, source) == name)
            .map_or(Export::Missing, |node| Export::Declared(node.start_byte()));
    }
    clause_export(program, statement, source, name)
}

/// `export { a as b } from`, `export * as ns from`, `export * from`.
fn forwarding_export(
    statement: Node<'_>,
    specifier: String,
    source: &str,
    imported: &Imported,
    star_sources: &mut Vec<String>,
) -> Export {
    let wanted = exported_name(imported);
    if let Some(clause) = named_children(statement).find(|n| n.kind() == "export_clause") {
        for part in named_children(clause).filter(|n| n.kind() == "export_specifier") {
            let Some(name) = part.child_by_field_name("name") else {
                continue;
            };
            let exported = part.child_by_field_name("alias").unwrap_or(name);
            if name_value(exported, source) == wanted {
                return Export::Forwarded {
                    specifier,
                    imported: Imported::from_name(name_value(name, source)),
                    at: exported.start_byte(),
                };
            }
        }
        return Export::Missing;
    }
    if let Some(namespace) = named_children(statement).find(|n| n.kind() == "namespace_export") {
        let exported = named_children(namespace).next();
        if exported.is_some_and(|n| name_value(n, source) == wanted) {
            return Export::Forwarded {
                specifier,
                imported: Imported::Module,
                at: namespace.start_byte(),
            };
        }
        return Export::Missing;
    }
    // `export *` never forwards the default export.
    if matches!(imported, Imported::Named(_)) {
        star_sources.push(specifier);
    }
    Export::Missing
}

/// `export { local as wanted }` without a source.
fn clause_export(program: Node<'_>, statement: Node<'_>, source: &str, wanted: &str) -> Export {
    let Some(clause) = named_children(statement).find(|n| n.kind() == "export_clause") else {
        return Export::Missing;
    };
    for part in named_children(clause).filter(|n| n.kind() == "export_specifier") {
        let Some(name) = part.child_by_field_name("name") else {
            continue;
        };
        let exported = part.child_by_field_name("alias").unwrap_or(name);
        if name_value(exported, source) == wanted {
            return local(program, text(name, source), source)
                .unwrap_or(Export::Declared(name.start_byte()));
        }
    }
    Export::Missing
}

/// `module.exports = …`, `module.exports.name = …`, `exports.name = …`.
fn commonjs_export(
    program: Node<'_>,
    statement: Node<'_>,
    source: &str,
    imported: &Imported,
) -> Export {
    let Some(assignment) = named_children(statement).find(|n| n.kind() == "assignment_expression")
    else {
        return Export::Missing;
    };
    let (Some(left), Some(right)) = (
        assignment.child_by_field_name("left"),
        assignment.child_by_field_name("right"),
    ) else {
        return Export::Missing;
    };
    let target = text(left, source);
    if target == "module.exports" {
        return match imported {
            Imported::Default => expression(program, right, source),
            Imported::Named(name) if right.kind() == "object" => {
                object_property(program, right, source, name)
            }
            _ => Export::Missing,
        };
    }
    let Imported::Named(name) = imported else {
        return Export::Missing;
    };
    let property = target
        .strip_prefix("module.exports.")
        .or_else(|| target.strip_prefix("exports."));
    if property != Some(name.as_str()) {
        return Export::Missing;
    }
    match right.kind() {
        "identifier" => local(program, text(right, source), source)
            .unwrap_or(Export::Declared(left.start_byte())),
        _ => Export::Declared(
            left.child_by_field_name("property")
                .map_or(left.start_byte(), |p| p.start_byte()),
        ),
    }
}

/// `module.exports = { name, name: value, name() {} }`.
fn object_property(program: Node<'_>, object: Node<'_>, source: &str, name: &str) -> Export {
    for property in named_children(object) {
        let (key, value) = match property.kind() {
            "shorthand_property_identifier" => (property, Some(property)),
            "pair" => match property.child_by_field_name("key") {
                Some(key) => (key, property.child_by_field_name("value")),
                None => continue,
            },
            "method_definition" => match property.child_by_field_name("name") {
                Some(key) => (key, None),
                None => continue,
            },
            _ => continue,
        };
        if name_value(key, source) != name {
            continue;
        }
        return value
            .filter(|v| matches!(v.kind(), "identifier" | "shorthand_property_identifier"))
            .and_then(|v| local(program, text(v, source), source))
            .unwrap_or(Export::Declared(key.start_byte()));
    }
    Export::Missing
}

/// An exported expression: an identifier points at its declaration, anything else at itself.
fn expression(program: Node<'_>, value: Node<'_>, source: &str) -> Export {
    if value.kind() == "identifier"
        && let Some(export) = local(program, text(value, source), source)
    {
        return export;
    }
    let named = value.child_by_field_name("name").unwrap_or(value);
    Export::Declared(named.start_byte())
}

/// A top-level declaration or import of `name` in this module.
fn local(program: Node<'_>, name: &str, source: &str) -> Option<Export> {
    for statement in named_children(program) {
        let declaration = match statement.kind() {
            "export_statement" => statement.child_by_field_name("declaration"),
            _ => Some(statement),
        };
        let Some(declaration) = declaration else {
            continue;
        };
        if let Some(node) = declared_names(declaration)
            .into_iter()
            .find(|node| text(*node, source) == name)
        {
            return Some(Export::Declared(node.start_byte()));
        }
    }
    imports::bindings(program, source)
        .into_iter()
        .find(|binding| binding.local == name)
        .map(|binding| Export::Forwarded {
            specifier: binding.specifier,
            imported: binding.imported,
            at: binding.at,
        })
}

/// Name nodes a declaration introduces: functions, classes, types, enums, `const a, b`.
fn declared_names(declaration: Node<'_>) -> Vec<Node<'_>> {
    match declaration.kind() {
        "lexical_declaration" | "variable_declaration" => named_children(declaration)
            .filter(|n| n.kind() == "variable_declarator")
            .filter_map(|d| d.child_by_field_name("name"))
            .filter(|n| n.kind() == "identifier")
            .collect(),
        "ambient_declaration" => named_children(declaration)
            .flat_map(declared_names)
            .collect(),
        "function_declaration"
        | "generator_function_declaration"
        | "function_signature"
        | "class_declaration"
        | "abstract_class_declaration"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration"
        | "module"
        | "internal_module" => declaration
            .child_by_field_name("name")
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn exported_name(imported: &Imported) -> &str {
    match imported {
        Imported::Named(name) => name,
        Imported::Default | Imported::Module => "default",
    }
}
