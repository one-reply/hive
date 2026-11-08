//! What the cursor completes, read from the text around it without parsing, so half-typed
//! code such as `import x from './co` still answers on every keystroke.
use super::Family;
use std::ops::Range;

/// How far back an import's `{` may be from the cursor; plenty for one statement.
const IMPORT_LIST_REACH: usize = 2048;

/// What the cursor completes and which bytes an accepted item replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// What kind of item fits here.
    pub request: Request,
    /// Bytes an accepted item replaces: the whole word around the cursor. For
    /// [`Request::ImportPath`] it starts at the module specifier and ends with the path segment
    /// under the cursor.
    pub replace: Range<usize>,
    /// Text typed before the cursor that items are matched against. For
    /// [`Request::ImportPath`] it is the specifier typed so far.
    pub prefix: String,
}

/// What kind of item fits at the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// A name from the open file; `member` right after `.`, `?.` or `::`.
    Name {
        /// Whether the name follows member access.
        member: bool,
    },
    /// A module specifier inside the quotes of an import, re-export or `require`.
    ImportPath,
    /// A name inside `import { … } from 'specifier'` or `export { … } from 'specifier'`.
    ImportName {
        /// The module the names come from, as written.
        specifier: String,
    },
}

/// Whether typing `typed` may open the menu. Spaces, operators and pasted text do not.
pub fn is_trigger(typed: &str, family: Family) -> bool {
    let mut chars = typed.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return false;
    };
    family.is_name_char(c)
        || matches!(c, '.' | ':')
        || (family == Family::Script
            && matches!(c, '/' | '\'' | '"' | '`' | '{' | ',' | '#' | '@' | '-'))
}

/// What `cursor` (a byte offset into `text`) completes, or `None` where nothing should open:
/// in a JavaScript comment or plain string, after a number, or with nothing typed and no
/// member access. Other languages rely on [`crate::completion::Index::is_quiet`] for comments
/// and strings.
pub fn site_at(text: &str, cursor: usize, family: Family) -> Option<Site> {
    if !text.is_char_boundary(cursor) {
        return None;
    }
    if family == Family::Script {
        let line_start = text[..cursor].rfind('\n').map_or(0, |ix| ix + 1);
        match scan_line(&text[line_start..cursor]) {
            Line::Code => {}
            Line::Comment => return None,
            Line::String { content } => return import_path(text, line_start + content, cursor),
        }
        if let Some(site) = import_name(text, cursor) {
            return Some(site);
        }
    }
    name(text, cursor, family)
}

/// Where a line of JavaScript ends up after reading it from its start to the cursor.
#[derive(Debug, PartialEq, Eq)]
enum Line {
    Code,
    Comment,
    /// Inside a string whose text starts at byte `content` of the line.
    String {
        content: usize,
    },
}

fn scan_line(line: &str) -> Line {
    let trimmed = line.trim_start();
    if trimmed == "*" || trimmed.starts_with("* ") {
        return Line::Comment;
    }
    let mut string: Option<(char, usize)> = None;
    let mut block_comment = false;
    // Brace depth inside a template literal's `${ … }`.
    let mut substitution: Option<usize> = None;
    let mut chars = line.char_indices().peekable();
    while let Some((ix, c)) = chars.next() {
        let next = chars.peek().map(|&(_, next)| next);
        if block_comment {
            if c == '*' && next == Some('/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }
        if let Some((quote, _)) = string {
            match c {
                '\\' => {
                    chars.next();
                }
                '$' if quote == '`' && next == Some('{') => {
                    chars.next();
                    string = None;
                    substitution = Some(0);
                }
                c if c == quote => string = None,
                _ => {}
            }
            continue;
        }
        match (c, next) {
            ('/', Some('/')) => return Line::Comment,
            ('/', Some('*')) => {
                chars.next();
                block_comment = true;
            }
            ('\'' | '"' | '`', _) => string = Some((c, ix + 1)),
            ('{', _) => substitution = substitution.map(|depth| depth + 1),
            ('}', _) => match substitution {
                Some(0) => {
                    substitution = None;
                    string = Some(('`', ix + 1));
                }
                Some(depth) => substitution = Some(depth - 1),
                None => {}
            },
            _ => {}
        }
    }
    match (block_comment, string) {
        (true, _) => Line::Comment,
        (false, Some((_, content))) => Line::String { content },
        (false, None) => Line::Code,
    }
}

/// A specifier being typed after `from`, `import`, `require(` or `import(`; any other string is
/// prose and gets nothing.
fn import_path(text: &str, content: usize, cursor: usize) -> Option<Site> {
    // The opening quote is one byte, right before `content`.
    let head = text[..content - 1].trim_end();
    let call = head.strip_suffix('(').map(str::trim_end);
    let imports = ends_with_word(head, "from")
        || ends_with_word(head, "import")
        || call
            .is_some_and(|call| ends_with_word(call, "require") || ends_with_word(call, "import"));
    if !imports {
        return None;
    }
    let segment_end = text[cursor..]
        .find(|c: char| matches!(c, '/' | '\'' | '"' | '`') || c.is_whitespace())
        .map_or(text.len(), |ix| cursor + ix);
    Some(Site {
        request: Request::ImportPath,
        replace: content..segment_end,
        prefix: text[content..cursor].to_owned(),
    })
}

/// A name inside the braces of `import { … } from '…'`, once the statement names its module.
fn import_name(text: &str, cursor: usize) -> Option<Site> {
    let word = word_around(text, cursor, Family::Script);
    let reach = floor_char_boundary(text, word.start.saturating_sub(IMPORT_LIST_REACH));
    let before = &text[reach..word.start];
    let open = before.rfind(|c: char| !is_import_list_char(c))?;
    if !before[open..].starts_with('{') || ends_with_word(before[open + 1..].trim_end(), "as") {
        return None;
    }
    let head = before[..open].trim_end();
    let head = head
        .strip_suffix("type")
        .filter(|rest| !rest.ends_with(|c: char| Family::Script.is_name_char(c)))
        .map_or(head, str::trim_end);
    if !ends_with_word(head, "import") && !ends_with_word(head, "export") {
        return None;
    }
    let after = &text[word.end..];
    let close = after.find(|c: char| !is_import_list_char(c))?;
    let rest = after[close..].strip_prefix('}')?.trim_start();
    let rest = rest.strip_prefix("from")?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
    let quoted = &rest[1..];
    let specifier = &quoted[..quoted.find(quote)?];
    if specifier.contains('\n') {
        return None;
    }
    Some(Site {
        request: Request::ImportName {
            specifier: specifier.to_owned(),
        },
        prefix: text[word.start..cursor].to_owned(),
        replace: word,
    })
}

fn name(text: &str, cursor: usize, family: Family) -> Option<Site> {
    let word = word_around(text, cursor, family);
    let prefix = &text[word.start..cursor];
    if prefix.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let member = is_member_access(&text[..word.start]);
    if prefix.is_empty() && !member {
        return None;
    }
    Some(Site {
        request: Request::Name { member },
        prefix: prefix.to_owned(),
        replace: word,
    })
}

/// `a.`, `a?.`, `A::`, but not `...rest`, `0..n` or the decimal point of `1.5`.
fn is_member_access(before: &str) -> bool {
    if before.ends_with("::") {
        return true;
    }
    let Some(head) = before.strip_suffix('.') else {
        return false;
    };
    match head.chars().next_back() {
        None | Some('.') => false,
        Some(c) if c.is_alphanumeric() || c == '_' => !head
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .last()
            .is_some_and(|first| first.is_ascii_digit()),
        Some(_) => true,
    }
}

fn word_around(text: &str, cursor: usize, family: Family) -> Range<usize> {
    let start = text[..cursor]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| family.is_name_char(c))
        .last()
        .map_or(cursor, |(ix, _)| ix);
    let end = text[cursor..]
        .char_indices()
        .take_while(|&(_, c)| family.is_name_char(c))
        .last()
        .map_or(cursor, |(ix, c)| cursor + ix + c.len_utf8());
    start..end
}

fn is_import_list_char(c: char) -> bool {
    Family::Script.is_name_char(c) || c == ',' || c.is_whitespace()
}

fn ends_with_word(text: &str, word: &str) -> bool {
    text.strip_suffix(word)
        .is_some_and(|rest| !rest.ends_with(|c: char| Family::Script.is_name_char(c)))
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The site at `|` in `marked`, a JavaScript source.
    fn site(marked: &str) -> Option<Site> {
        site_in(Family::Script, marked)
    }

    fn site_in(family: Family, marked: &str) -> Option<Site> {
        let cursor = marked.find('|').expect("a cursor mark");
        let text = marked.replacen('|', "", 1);
        site_at(&text, cursor, family)
    }

    /// The request, the replaced text and the prefix, for compact assertions.
    fn parts(marked: &str) -> Option<(Request, String, String)> {
        let cursor = marked.find('|').expect("a cursor mark");
        let text = marked.replacen('|', "", 1);
        let site = site_at(&text, cursor, Family::Script)?;
        Some((site.request, text[site.replace].to_owned(), site.prefix))
    }

    fn name(member: bool) -> Request {
        Request::Name { member }
    }

    #[test]
    fn plain_names_and_members_replace_the_whole_word() {
        assert_eq!(
            parts("const value = va|"),
            Some((name(false), "va".into(), "va".into()))
        );
        assert_eq!(
            parts("foo.ba|r()"),
            Some((name(true), "bar".into(), "ba".into()))
        );
        assert_eq!(parts("a?.b|"), Some((name(true), "b".into(), "b".into())));
        assert_eq!(
            parts("obj.|"),
            Some((name(true), String::new(), String::new()))
        );
        assert_eq!(
            parts("x1.|"),
            Some((name(true), String::new(), String::new()))
        );
        assert_eq!(
            parts("f(...ar|)"),
            Some((name(false), "ar".into(), "ar".into()))
        );
        assert_eq!(
            parts("$sco|pe"),
            Some((name(false), "$scope".into(), "$sco".into()))
        );
        assert_eq!(
            parts("const при|віт"),
            Some((name(false), "привіт".into(), "при".into()))
        );
        let rust = site_in(Family::Other, "Foo::ne|").expect("a site");
        assert_eq!(rust.request, name(true));
        assert_eq!(
            site_in(Family::Other, "$x|").map(|s| s.prefix),
            Some("x".into()),
            "`$` is part of a name only in JavaScript"
        );
    }

    #[test]
    fn nothing_opens_without_a_word_numbers_or_ranges() {
        for marked in ["x = |", "1.|", "1.5|", "0..|", "a: |", "a -|", "f(1|"] {
            assert_eq!(site(marked), None, "{marked}");
        }
    }

    #[test]
    fn javascript_comments_and_strings_stay_quiet() {
        for marked in [
            "// call fo|",
            "x; // fo|",
            "const s = \"hel|",
            "const s = 'it\\'s fo|",
            "/* fo|",
            "  * the fo|",
            "foo('./x|')",
        ] {
            assert_eq!(site(marked), None, "{marked}");
        }
        assert_eq!(site("/* a */ fo|").map(|s| s.request), Some(name(false)));
        assert_eq!(site("`${us|}`").map(|s| s.request), Some(name(false)));
        assert_eq!(site("`${ {a: b|} }`").map(|s| s.request), Some(name(false)));
        assert_eq!(site("`${a} te|`"), None);
        assert_eq!(
            site_in(Family::Other, "// call fo|").map(|s| s.request),
            Some(name(false)),
            "other languages leave comments to the index"
        );
    }

    #[test]
    fn module_specifiers_complete_the_path_segment_under_the_cursor() {
        let path = |marked| {
            let cursor = str::find(marked, '|').unwrap();
            let text = marked.replacen('|', "", 1);
            let site = site_at(&text, cursor, Family::Script)?;
            assert_eq!(site.request, Request::ImportPath);
            Some((text[site.replace].to_owned(), site.prefix))
        };
        assert_eq!(
            path("import x from './co|'"),
            Some(("./co".into(), "./co".into()))
        );
        assert_eq!(
            path("import x from './co|mp/x'"),
            Some(("./comp".into(), "./co".into()))
        );
        assert_eq!(
            path("const r = require('rea|ct')"),
            Some(("react".into(), "rea".into()))
        );
        assert_eq!(
            path("await import( \"./|"),
            Some(("./".into(), "./".into()))
        );
        assert_eq!(
            path("export * from '#ut|'"),
            Some(("#ut".into(), "#ut".into()))
        );
        assert_eq!(
            path("import './side|'"),
            Some(("./side".into(), "./side".into()))
        );
        assert_eq!(
            path("import {\n  a,\n} from '|'"),
            Some((String::new(), String::new()))
        );
        assert_eq!(path("transform('./x|')"), None);
        assert_eq!(path("reimport('./x|')"), None);
    }

    #[test]
    fn named_imports_know_their_module() {
        let names = |marked| {
            let (request, replaced, prefix) = parts(marked)?;
            let Request::ImportName { specifier } = request else {
                return None;
            };
            Some((specifier, replaced, prefix))
        };
        assert_eq!(
            names("import { us| } from 'react'"),
            Some(("react".into(), "us".into(), "us".into()))
        );
        assert_eq!(
            names("import {\n  a,\n  b|ee\n} from \"./x\";"),
            Some(("./x".into(), "bee".into(), "b".into()))
        );
        assert_eq!(
            names("import type { Fo| } from './types'").map(|n| n.0),
            Some("./types".into())
        );
        assert_eq!(
            names("import { |} from './x'").map(|n| n.0),
            Some("./x".into())
        );
        assert_eq!(
            names("export { a, b| } from '#lib'").map(|n| n.0),
            Some("#lib".into())
        );
        assert_eq!(names("import { a as b| } from 'x'"), None);
        assert_eq!(names("import { a| }"), None);
        assert_eq!(names("const { a| } = obj"), None);
        assert_eq!(names("reimport { a| } from 'x'"), None);
    }

    #[test]
    fn cursor_inside_a_character_gives_nothing() {
        assert_eq!(site_at("привіт", 1, Family::Script), None);
    }

    #[test]
    fn triggers_are_single_characters_that_can_start_or_continue_an_item() {
        for typed in ["a", "Я", "_", "1", ".", ":"] {
            assert!(is_trigger(typed, Family::Other), "{typed}");
        }
        for typed in ["$", "/", "'", "\"", "`", "{", ",", "#", "@", "-"] {
            assert!(is_trigger(typed, Family::Script), "{typed}");
            assert!(!is_trigger(typed, Family::Other), "{typed}");
        }
        for typed in [" ", "\n", "(", "=", "ab", ""] {
            assert!(!is_trigger(typed, Family::Script), "{typed:?}");
        }
    }
}
