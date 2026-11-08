//! The names an open file contains, and the spans where the menu stays quiet.
use super::ranked;
use std::collections::HashSet;
use std::ops::Range;
use tree_sitter::{Node, Tree};

/// Bounds memory and per-keystroke filtering on huge generated files.
const MAX_NAMES: usize = 20_000;
/// Longer "names" come from minified bundles or encoded data, not from code anyone types.
const MAX_NAME_BYTES: usize = 128;
/// Tokens after which a name is a member of what precedes it. `->` is not one: Rust and Python
/// write a return type after it, and C's `ptr->field` is a member through `field_identifier`.
const MEMBER_ACCESS: &[&str] = &[".", "?.", "::", "optional_chain"];
/// Ends of comments that cannot grow by typing after them.
const COMMENT_CLOSERS: &[&str] = &["*/", "-->", "]]"];

/// Names of one file for the completion menu, built off the main thread from a parse.
#[derive(Debug, Default)]
pub struct Index {
    /// Sorted, distinct names written as plain identifiers.
    names: Vec<String>,
    /// Sorted, distinct names written after member access or as properties and fields.
    members: Vec<String>,
    /// Comments and strings in text order; they do not overlap.
    quiet: Vec<Quiet>,
}

#[derive(Debug)]
struct Quiet {
    span: Range<usize>,
    /// A line comment: text typed right at its end still belongs to it.
    open_end: bool,
}

enum Visit {
    Quiet,
    Name,
    Descend,
}

impl Index {
    /// Collects the identifiers and the comment and string spans of `text`, which `tree` parses.
    /// The identifier that touches `editing` is left out: it is the word being typed, not one
    /// to suggest back, unless the file uses it somewhere else too.
    pub fn build(tree: &Tree, text: &str, editing: Option<usize>) -> Self {
        let mut names = HashSet::new();
        let mut members = HashSet::new();
        let mut quiet = Vec::new();
        let mut cursor = tree.walk();
        'walk: loop {
            let node = cursor.node();
            let descend = match visit(node) {
                Visit::Quiet => {
                    quiet.push(Quiet::new(node, text));
                    false
                }
                Visit::Name => {
                    let touches_cursor = editing.is_some_and(|offset| {
                        node.start_byte() <= offset && offset <= node.end_byte()
                    });
                    if !touches_cursor && let Some(name) = text.get(node.byte_range()) {
                        let set = if is_member(node) {
                            &mut members
                        } else {
                            &mut names
                        };
                        if set.len() < MAX_NAMES && is_name(name) && !set.contains(name) {
                            set.insert(name.to_owned());
                        }
                    }
                    false
                }
                Visit::Descend => true,
            };
            if descend && cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    continue 'walk;
                }
                if !cursor.goto_parent() {
                    break 'walk;
                }
            }
        }
        Self {
            names: sorted(names),
            members: sorted(members),
            quiet,
        }
    }

    /// Names that start with `prefix`, at most [`crate::completion::MAX_ITEMS`]. After member
    /// access members come first, otherwise plain names; within each, exact-case matches come
    /// before the rest. `prefix` itself is never offered.
    pub fn matches(&self, prefix: &str, member: bool) -> Vec<&str> {
        if member {
            ranked(&[&self.members, &self.names], prefix)
        } else {
            ranked(&[&self.names, &self.members], prefix)
        }
    }

    /// Whether `offset` is inside a comment or a string, where typing is prose, not code.
    pub fn is_quiet(&self, offset: usize) -> bool {
        let before = self.quiet.partition_point(|q| q.span.start < offset);
        before.checked_sub(1).is_some_and(|ix| {
            let quiet = &self.quiet[ix];
            offset < quiet.span.end || (offset == quiet.span.end && quiet.open_end)
        })
    }

    /// Keeps the quiet spans on the same text after an edit replaced `start..old_end` with text
    /// ending at `new_end`, so typing into a comment stays quiet until the next build.
    pub fn edit(&mut self, start: usize, old_end: usize, new_end: usize) {
        for quiet in &mut self.quiet {
            quiet.span.start = shift(quiet.span.start, start, old_end, new_end, true);
            quiet.span.end = shift(quiet.span.end, start, old_end, new_end, quiet.open_end);
        }
        self.quiet.retain(|quiet| !quiet.span.is_empty());
    }
}

impl Quiet {
    fn new(node: Node<'_>, text: &str) -> Self {
        let span = node.byte_range();
        let body = text
            .get(span.clone())
            .unwrap_or_default()
            .trim_end_matches(['\n', '\r']);
        let open_end = node.kind().contains("comment")
            && !body.contains('\n')
            && !COMMENT_CLOSERS.iter().any(|closer| body.ends_with(closer));
        Self { span, open_end }
    }
}

fn visit(node: Node<'_>) -> Visit {
    let kind = node.kind();
    if kind.contains("comment") || (kind.contains("string") && !has_embedded_code(node)) {
        return Visit::Quiet;
    }
    let is_name_kind = kind.contains("identifier") || matches!(kind, "name" | "constant");
    if node.is_named() && node.child_count() == 0 && is_name_kind {
        return Visit::Name;
    }
    Visit::Descend
}

/// Template literals, f-strings and the like hold code between their text parts.
fn has_embedded_code(node: Node<'_>) -> bool {
    (0..node.named_child_count()).any(|ix| {
        node.named_child(ix as u32).is_some_and(|child| {
            let kind = child.kind();
            kind.contains("substitution") || kind.contains("interpolation")
        })
    })
}

fn is_member(node: Node<'_>) -> bool {
    let kind = node.kind();
    kind.contains("property")
        || kind.contains("field")
        || node
            .prev_sibling()
            .is_some_and(|previous| MEMBER_ACCESS.contains(&previous.kind()))
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    text.len() <= MAX_NAME_BYTES
        && text != "_"
        && chars
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn sorted(names: HashSet<String>) -> Vec<String> {
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort_unstable();
    names
}

/// Where `offset` lands after an edit. With `sticky`, text inserted exactly at `offset` goes
/// before it, so `offset` moves past the insertion.
fn shift(offset: usize, start: usize, old_end: usize, new_end: usize, sticky: bool) -> usize {
    if offset < start || (offset == start && !sticky) {
        offset
    } else if offset < old_end {
        new_end
    } else {
        offset - old_end + new_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::completion::MAX_ITEMS;

    fn index(source: &str) -> Index {
        index_at(source, None)
    }

    fn index_at(source: &str, editing: Option<usize>) -> Index {
        let grammar = tree_sitter_typescript::LANGUAGE_TSX.into();
        let tree = crate::completion::parse(source, &grammar).expect("tsx parses");
        Index::build(&tree, source, editing)
    }

    /// The React Native sample from the TSX highlighting regression.
    const TSX: &str = "import { AppHeader } from '@components/header';\n\
        const loadMore = () => {\n  if (hasNextPage && !isFetching) {\n    return <View style={styles.row}><div /></View>;\n  }\n};\n";

    #[test]
    fn names_and_members_come_from_identifiers_only() {
        let index = index(TSX);
        for name in [
            "AppHeader",
            "loadMore",
            "hasNextPage",
            "isFetching",
            "View",
            "styles",
        ] {
            assert!(
                index.names.contains(&name.to_owned()),
                "{name} in {:?}",
                index.names
            );
        }
        assert_eq!(index.members, ["row", "style"]);
        for word in ["import", "const", "return", "if", "components", "header"] {
            assert!(!index.names.contains(&word.to_owned()), "{word}");
        }
    }

    #[test]
    fn comments_and_strings_add_no_names_and_are_quiet() {
        let source = "// todo makeThing\nconst a = \"hello world\";\n/* block */ b;\n";
        let index = index(source);
        assert_eq!(index.names, ["a", "b"]);
        let in_comment = source.find("makeThing").unwrap();
        let line_end = source.find('\n').unwrap();
        let in_string = source.find("world").unwrap();
        let after_string = source.find(';').unwrap();
        let after_block = source.find(" b;").unwrap();
        assert!(index.is_quiet(in_comment));
        assert!(
            index.is_quiet(line_end),
            "the end of a line comment is still the comment"
        );
        assert!(index.is_quiet(in_string));
        assert!(
            !index.is_quiet(after_string),
            "right after the closing quote is code"
        );
        assert!(index.is_quiet(source.find("block").unwrap()));
        assert!(!index.is_quiet(after_block), "right after `*/` is code");
        assert!(!index.is_quiet(0));
    }

    #[test]
    fn template_substitutions_are_code() {
        let source = "const s = `Hi ${user.name}!`;\n";
        let index = index(source);
        assert!(index.names.contains(&"user".to_owned()));
        assert_eq!(index.members, ["name"]);
        assert!(!index.is_quiet(source.find("user").unwrap()));
    }

    #[test]
    fn optional_chaining_and_methods_are_members() {
        let index = index("class A { run() {} }\nconfig?.port;\nserver.listen();\n");
        assert_eq!(index.members, ["listen", "port", "run"]);
        assert_eq!(index.names, ["A", "config", "server"]);
    }

    #[test]
    fn the_word_being_typed_is_not_suggested_back() {
        let source = "const value = 1;\nval";
        let index = index_at(source, Some(source.len()));
        assert_eq!(index.names, ["value"]);
        let repeated = "val;\nval";
        assert_eq!(index_at(repeated, Some(repeated.len())).names, ["val"]);
    }

    #[test]
    fn matches_rank_role_then_case_and_skip_the_prefix_itself() {
        let index = index("const apple = 1, Apply = 2, app = 3;\nobj.append;\nobj.Apart;\n");
        assert_eq!(
            index.matches("ap", false),
            ["app", "apple", "Apply", "append", "Apart"]
        );
        assert_eq!(
            index.matches("ap", true),
            ["append", "Apart", "app", "apple", "Apply"]
        );
        assert_eq!(index.matches("app", false), ["apple", "Apply", "append"]);
        assert!(index.matches("zzz", false).is_empty());
    }

    #[test]
    fn matches_are_capped() {
        let source: String = (0..80).map(|ix| format!("let n{ix} = {ix};\n")).collect();
        assert_eq!(index(&source).matches("n", false).len(), MAX_ITEMS);
    }

    #[test]
    fn empty_and_broken_text_degrade_to_what_parses() {
        let empty = index("");
        assert!(empty.names.is_empty() && empty.matches("", false).is_empty());
        let broken = index("const = ;; {{ foo. (bar ``` 'x");
        assert!(broken.names.contains(&"foo".to_owned()));
    }

    #[test]
    fn quiet_spans_follow_edits_until_the_next_build() {
        let mut index = index("// note\nx;\n\"ab\" + y;\n");
        index.edit(7, 7, 10);
        assert!(
            index.is_quiet(10),
            "typing at the end of a line comment extends it"
        );
        index.edit(0, 0, 2);
        assert!(
            !index.is_quiet(1),
            "text inserted before a comment is not in it"
        );
        assert!(index.is_quiet(5));
        let string_end = "  // note   \nx;\n\"ab\"".len();
        index.edit(string_end, string_end, string_end + 1);
        assert!(
            !index.is_quiet(string_end + 1),
            "after a closing quote stays code"
        );
        index.edit(0, 13, 0);
        assert!(
            !index.is_quiet(0),
            "a deleted comment leaves nothing quiet behind"
        );
        assert!(index.is_quiet(4), "the string moved to the start");
    }

    #[test]
    fn shift_maps_offsets_around_an_edit() {
        assert_eq!(shift(3, 5, 5, 8, true), 3);
        assert_eq!(shift(5, 5, 5, 8, true), 8);
        assert_eq!(shift(5, 5, 5, 8, false), 5);
        assert_eq!(shift(6, 5, 9, 5, false), 5, "inside deleted text");
        assert_eq!(shift(12, 5, 9, 6, false), 9);
    }
}
