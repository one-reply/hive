//! Markdown a language server sends for display: hover, completion documentation and
//! diagnostic messages.
//!
//! gpui-kit renders it with a `TextView` that has no link handler, and the editor's popovers
//! offer no way to set one: a click passes the link's target to `App::open_url` whatever its
//! scheme (`NSWorkspace openURL` on macOS), so a `file://` link in a dependency's doc comment
//! would open, or run, a file. The text is therefore restricted before the editor gets it:
//! every construct the renderer's parser makes clickable keeps its target only when that
//! target starts with `http://` or `https://`; any other is turned into literal text by
//! escaping the character that opens it.

use ::markdown::{ParseOptions, mdast::Node};
use lsp_types::{Documentation, HoverContents, MarkedString, MarkupContent, MarkupKind};
use std::{borrow::Cow, collections::HashMap};

#[cfg(test)]
mod tests;

/// The most text one popover shows, in bytes; longer text is cut and ends with `…`.
/// rust-analyzer's hover for `Vec`, among the longest in `std`, is 11 KiB.
pub const MAX_LEN: usize = 16 * 1024;

/// Escaping one link can make another: the `]` of an escaped `[x](a)` may close an earlier `[`,
/// and an escaped `<mailto:a@b.co>` leaves a bare address that GFM links by itself. Each round
/// parses again; text that is still not settled after this many is shown as a code block.
const MAX_ROUNDS: usize = 6;

/// `contents` as the hover popover will render it, restricted to web links and at most
/// [`MAX_LEN`] bytes long.
///
/// The popover joins array items with a blank line and parses the result as one document, so
/// that joined text is what is restricted: items checked one by one can still form a link
/// together (an HTML comment opened in one item and closed in the next).
pub fn hover(contents: HoverContents) -> HoverContents {
    let text = match contents {
        HoverContents::Scalar(item) => marked_text(item),
        HoverContents::Array(items) => items
            .into_iter()
            .map(marked_text)
            .collect::<Vec<_>>()
            .join("\n\n"),
        HoverContents::Markup(markup) => markup.value,
    };
    HoverContents::Markup(MarkupContent {
        kind: MarkupKind::Markdown,
        value: restricted(text),
    })
}

/// Completion documentation restricted to web links and at most [`MAX_LEN`] bytes long.
/// The menu renders plain text as Markdown too, so both forms are restricted.
pub fn documentation(documentation: Documentation) -> Documentation {
    match documentation {
        Documentation::String(text) => Documentation::String(restricted(text)),
        Documentation::MarkupContent(markup) => Documentation::MarkupContent(MarkupContent {
            kind: markup.kind,
            value: restricted(markup.value),
        }),
    }
}

/// `markdown` with every link whose target does not start with `http://` or `https://` turned
/// into literal text; text that has no such link comes back unchanged.
///
/// Its first line alone has no such link either: in a narrow window the completion menu shows
/// only the first line of the documentation, which then parses on its own.
pub fn restrict_links(markdown: &str) -> Cow<'_, str> {
    // A link needs `[` or `<`, or else is a GFM literal: an email address has `@`, and a web
    // address `://` (a capitalized `HTTPS://` is linked as written).
    if !markdown.contains(['[', '<', '@']) && !markdown.contains("://") {
        return Cow::Borrowed(markdown);
    }
    let mut text = Cow::Borrowed(markdown);
    for _ in 0..MAX_ROUNDS {
        match escapes(&text) {
            Ok(offsets) if offsets.is_empty() => return text,
            Ok(offsets) => text = Cow::Owned(escape(&text, &offsets)),
            Err(Unrecognized) => break,
        }
    }
    Cow::Owned(code_block(markdown))
}

fn marked_text(item: MarkedString) -> String {
    match item {
        MarkedString::String(text) => text,
        MarkedString::LanguageString(code) => code.value,
    }
}

fn restricted(mut text: String) -> String {
    if text.len() > MAX_LEN {
        text.truncate(text.floor_char_boundary(MAX_LEN));
        text.push_str("\n\n…");
    }
    match restrict_links(&text) {
        Cow::Borrowed(_) => text,
        Cow::Owned(restricted) => restricted,
    }
}

/// A clickable construct whose opening character could not be found, or text the parser
/// rejected: the caller shows the text as a code block rather than guess.
struct Unrecognized;

/// The options gpui-kit's `TextView` parses Markdown with (`MarkdownExtensions::parse_options`
/// without MDX or frontmatter), so both agree on what is a link.
fn options() -> ParseOptions {
    let mut options = ParseOptions::gfm();
    options.constructs.math_text = true;
    options.constructs.math_flow = true;
    options
}

/// Byte offsets, ascending, of the characters a backslash must precede to make `text` and its
/// first line free of links to anything but the web.
fn escapes(text: &str) -> Result<Vec<usize>, Unrecognized> {
    let mut offsets = Vec::new();
    let first_line = text.split('\n').next().unwrap_or_default();
    // The first line is a prefix of `text`, so its offsets are offsets in `text` too.
    for view in [text, first_line] {
        let root = ::markdown::to_mdast(view, &options()).map_err(|_| Unrecognized)?;
        let nodes = descendants(&root);
        let definitions = definitions(&nodes);
        for node in nodes {
            if let Some(offset) = escape_offset(node, view, &definitions)? {
                offsets.push(offset);
            }
        }
        if first_line.len() == text.len() {
            break;
        }
    }
    offsets.sort_unstable();
    offsets.dedup();
    Ok(offsets)
}

/// Every node under `root`, without recursion: a deeply nested document must not overflow
/// the stack.
fn descendants(root: &Node) -> Vec<&Node> {
    let mut nodes = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        nodes.push(node);
        pending.extend(node.children().into_iter().flatten());
    }
    nodes
}

/// Each identifier's destination; the first definition in the document wins, as both the
/// parser and the renderer resolve references.
fn definitions<'a>(nodes: &[&'a Node]) -> HashMap<&'a str, &'a str> {
    let mut first: HashMap<&str, (usize, &str)> = HashMap::new();
    for node in nodes {
        let Node::Definition(definition) = node else {
            continue;
        };
        let start = node.position().map_or(usize::MAX, |p| p.start.offset);
        let entry = first
            .entry(definition.identifier.as_str())
            .or_insert((start, definition.url.as_str()));
        if start < entry.0 {
            *entry = (start, definition.url.as_str());
        }
    }
    first
        .into_iter()
        .map(|(identifier, (_, url))| (identifier, url))
        .collect()
}

/// Where a backslash turns `node` into text, if it is a link the editor must not show.
/// A definition is never escaped: the renderer draws nothing for it, and the references
/// that resolve to it are what can be clicked.
fn escape_offset(
    node: &Node,
    text: &str,
    definitions: &HashMap<&str, &str>,
) -> Result<Option<usize>, Unrecognized> {
    let opener: fn(&str) -> Option<usize> = match node {
        Node::Link(link) if !opens_safely(&link.url) => link_opener,
        Node::LinkReference(reference)
            if !definitions
                .get(reference.identifier.as_str())
                .is_some_and(|url| opens_safely(url)) =>
        {
            |span| span.starts_with('[').then_some(0)
        }
        Node::Html(html) if has_anchor(&html.value) => |span| after_indent(span, '<'),
        _ => return Ok(None),
    };
    let position = node.position().ok_or(Unrecognized)?;
    let span = text
        .get(position.start.offset..position.end.offset)
        .ok_or(Unrecognized)?;
    let offset = opener(span).ok_or(Unrecognized)?;
    Ok(Some(position.start.offset + offset))
}

/// The `literally starts with` rule: no case folding, decoding or trimming, since the target
/// is exactly what the renderer hands to the system.
fn opens_safely(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// An inline link opens with `[` and an autolink with `<`. A GFM literal is broken by an
/// escaped `@` when it is an email address (`a@b.co`, `mailto:a@b.co`, `xmpp:a@b.co`) and
/// by an escaped `:` when it is a web address in capitals (`HTTPS://…`).
fn link_opener(span: &str) -> Option<usize> {
    if span.starts_with(['[', '<']) {
        Some(0)
    } else {
        span.find('@').or_else(|| span.find(':'))
    }
}

/// A block keeps its indentation in its span.
fn after_indent(span: &str, opener: char) -> Option<usize> {
    let indent = span.len() - span.trim_start_matches([' ', '\t']).len();
    span[indent..].starts_with(opener).then_some(indent)
}

/// Whether raw HTML may contain an `<a>` start tag, which the renderer turns into a link.
/// Without an HTML parser the decoded `href` is unknown, so every anchor counts.
fn has_anchor(html: &str) -> bool {
    let bytes = html.as_bytes();
    bytes.windows(2).enumerate().any(|(ix, pair)| {
        pair[0] == b'<'
            && pair[1].eq_ignore_ascii_case(&b'a')
            && !bytes.get(ix + 2).is_some_and(u8::is_ascii_alphanumeric)
    })
}

/// `text` with a backslash before each byte in `offsets`, all of them ASCII punctuation.
fn escape(text: &str, offsets: &[usize]) -> String {
    let mut escaped = String::with_capacity(text.len() + offsets.len());
    let mut from = 0;
    for &offset in offsets {
        escaped.push_str(&text[from..offset]);
        escaped.push('\\');
        from = offset;
    }
    escaped.push_str(&text[from..]);
    escaped
}

/// `source` as one fenced code block, shown verbatim with no links: the fence is longer than
/// any run of backticks inside, so nothing in `source` can close it.
fn code_block(source: &str) -> String {
    let longest = source.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}\n{source}\n{fence}")
}
