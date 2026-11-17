use super::*;
use lsp_types::{CompletionItem, Hover, LanguageString};

fn hover_fixture(json: &str) -> Hover {
    serde_json::from_str(json).expect("captured hover")
}

const VEC: &str = include_str!("../fixtures/rust-analyzer-hover-vec.json");
const PUSH: &str = include_str!("../fixtures/rust-analyzer-hover-push.json");
const DOC_LINKS: &str = include_str!("../fixtures/rust-analyzer-hover-doc-links.json");
const TYPESCRIPT: &str = include_str!("../fixtures/typescript-language-server-hover.json");
const COMPLETION: &str = include_str!("../fixtures/rust-analyzer-completion-item.json");

fn markup(contents: &HoverContents) -> &str {
    match contents {
        HoverContents::Markup(markup) => &markup.value,
        other => panic!("expected markup, got {other:?}"),
    }
}

/// Every target a click in the popover would pass to the system: links, resolved references
/// and `<a>` tags, in the whole text and in its first line alone.
fn clickable(markdown: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let first_line = markdown.split('\n').next().unwrap_or_default();
    for view in [markdown, first_line] {
        let root = ::markdown::to_mdast(view, &options()).expect("parses");
        let nodes = descendants(&root);
        let definitions = definitions(&nodes);
        for node in nodes {
            match node {
                Node::Link(link) => targets.push(link.url.clone()),
                Node::LinkReference(reference) => targets.push(
                    definitions
                        .get(reference.identifier.as_str())
                        .map_or("undefined reference".into(), |url| url.to_string()),
                ),
                Node::Html(html) if html.value.to_ascii_lowercase().contains("<a") => {
                    targets.push(html.value.clone())
                }
                _ => {}
            }
        }
    }
    targets.sort();
    targets.dedup();
    targets
}

fn code_blocks(markdown: &str) -> Vec<(Option<String>, String)> {
    let root = ::markdown::to_mdast(markdown, &options()).expect("parses");
    let mut blocks: Vec<_> = descendants(&root)
        .into_iter()
        .filter_map(|node| match node {
            Node::Code(code) => Some((code.lang.clone(), code.value.clone())),
            _ => None,
        })
        .collect();
    blocks.sort();
    blocks
}

fn is_web(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://")
}

#[test]
fn hovers_with_code_blocks_and_no_other_links_are_unchanged() {
    for json in [PUSH, TYPESCRIPT] {
        let captured = hover_fixture(json);
        let original = markup(&captured.contents).to_owned();
        assert!(!code_blocks(&original).is_empty());
        let restricted = hover(captured.contents);
        assert_eq!(markup(&restricted), original);
    }
}

#[test]
fn rust_analyzer_vec_hover_keeps_its_code_and_web_links() {
    let captured = hover_fixture(VEC);
    let original = markup(&captured.contents).to_owned();
    let restricted = hover(captured.contents);
    let restricted = markup(&restricted);

    assert_eq!(code_blocks(restricted), code_blocks(&original));
    let web: Vec<_> = clickable(&original)
        .into_iter()
        .filter(|target| is_web(target))
        .collect();
    assert!(!web.is_empty());
    assert_eq!(clickable(restricted), web);
    // Escaped, not removed: only backslashes were added.
    assert_eq!(restricted.replace('\\', ""), original.replace('\\', ""));
}

#[test]
fn doc_comment_links_to_files_and_shares_become_text() {
    let captured = hover_fixture(DOC_LINKS);
    let original = markup(&captured.contents).to_owned();
    assert!(
        clickable(&original)
            .iter()
            .any(|t| t.starts_with("file://"))
    );
    let restricted = hover(captured.contents);
    let restricted = markup(&restricted);

    assert_eq!(
        clickable(restricted),
        [
            "https://doc.rust-lang.org/stable/alloc/vec/struct.Vec.html",
            "https://docs.rs/demo/0.1.0/demo/struct.Config.html",
        ]
    );
    assert!(restricted.contains(r"\[notes](file:///tmp/notes/x.command)"));
    assert!(restricted.contains(r"\[the share](smb://host/share)"));
    assert!(restricted.contains(r"\<smb://host/share>"));
    assert_eq!(code_blocks(restricted), code_blocks(&original));
}

#[test]
fn every_link_form_to_something_other_than_the_web_becomes_text() {
    for source in [
        "[notes](file:///Users/me/repo/x.command)",
        "[share](smb://host/share)",
        "[run](./run.command) and [up](../x)",
        "[entity](&#102;ile:///x.command) [hex](&#x66;ile:///y)",
        "<file:///x.command> and <smb://host/share>",
        "[text][run], [run][] and [run]\n\n[run]: ./run.command",
        "[spaced](<file:///Users/me/a b.command>)",
        "[script](javascript:alert(1)) [upper](HTTPS://example.com)",
        "a bare HTTPS://example.com/a in capitals",
        "write to a@b.co, <mailto:c@d.co> or xmpp:e@f.co",
        "inline <a href=\"file:///x.command\">notes</a> here",
        "<div>\n<a href=\"file:///x.command\">notes</a>\n</div>",
        "<A\nHREF=\"file:///x\">notes</A>",
        "[outer [inner](file:///x)](file:///y)",
        "> - [quoted](file:///q)\n>\n>   [deep]: file:///d\n>\n>   [deep]",
        "| name |\n|---|\n| [cell](file:///c) |",
        "<!--\n\n--> <a href=\"file:///x\">after a comment</a>",
        "![image](https://x/i.png) [![wrapped](https://x/i.png)](file:///x)",
    ] {
        assert!(
            clickable(source).iter().any(|target| !is_web(target)),
            "sample has no link: {source:?}"
        );
        let restricted = restrict_links(source);
        assert!(
            clickable(&restricted).iter().all(|target| is_web(target)),
            "{source:?} became {restricted:?}"
        );
        assert_eq!(restricted.replace('\\', ""), source.replace('\\', ""));
    }
}

#[test]
fn web_links_stay_clickable_and_untouched() {
    for source in [
        "[docs](https://docs.rs/x) and [plain](http://example.com)",
        "<https://example.com/a>",
        "[docs][d]\n\n[d]: https://docs.rs/x",
        "see https://example.com and www.example.com",
        "[first][d]\n\n[d]: https://docs.rs/x\n[d]: file:///x",
    ] {
        let restricted = restrict_links(source);
        assert!(matches!(restricted, Cow::Borrowed(_)), "{source:?}");
        let targets = clickable(source);
        assert!(
            !targets.is_empty() && targets.iter().all(|target| is_web(target)),
            "{source:?}"
        );
    }
}

#[test]
fn a_shadowed_reference_resolves_like_the_renderer() {
    let source = "[x]\n\n[x]: file:///x\n[x]: https://example.com";
    assert_eq!(clickable(source), ["file:///x"]);
    assert!(clickable(&restrict_links(source)).is_empty());
}

#[test]
fn the_first_line_alone_has_no_links_either() {
    // Whole, the link is inside a code span; the narrow completion menu shows line one only.
    let source = "`[a](file:///x)\n`";
    assert_eq!(clickable(source.split('\n').next().unwrap()), ["file:///x"]);
    assert!(clickable(&restrict_links(source)).is_empty());
}

#[test]
fn text_that_does_not_settle_is_shown_as_a_code_block() {
    let source = format!("{}](file:///x) ````", "[".repeat(MAX_ROUNDS + 2));
    let restricted = restrict_links(&source);
    assert_eq!(restricted, format!("`````\n{source}\n`````"));
    assert!(clickable(&restricted).is_empty());
    assert_eq!(code_blocks(&restricted), [(None, source)]);
}

#[test]
fn text_without_link_syntax_is_borrowed() {
    assert!(matches!(
        restrict_links("Returns `Vec<u8>` for Vec<u8> input."),
        Cow::Borrowed(_)
    ));
    assert!(matches!(restrict_links("no links"), Cow::Borrowed(_)));
    assert_eq!(restrict_links(""), "");
}

#[test]
fn hover_items_are_joined_as_the_popover_joins_them() {
    let contents = HoverContents::Array(vec![
        MarkedString::String("<!--".into()),
        MarkedString::String("--> <a href=\"file:///x\">x</a>".into()),
    ]);
    let restricted = hover(contents);
    assert!(clickable(markup(&restricted)).is_empty());

    let code = HoverContents::Scalar(MarkedString::LanguageString(LanguageString {
        language: "rust".into(),
        value: "[x](file:///x)".into(),
    }));
    assert_eq!(markup(&hover(code)), r"\[x](file:///x)");
}

#[test]
fn long_hover_is_cut_on_a_character_boundary() {
    let contents = HoverContents::Markup(MarkupContent {
        kind: MarkupKind::PlainText,
        value: "é".repeat(MAX_LEN),
    });
    let restricted = hover(contents);
    let value = markup(&restricted);
    assert_eq!(value.len(), MAX_LEN + "\n\n…".len());
    assert!(value.ends_with("é\n\n…"));
}

#[test]
fn completion_documentation_is_restricted_in_both_forms() {
    let item: CompletionItem = serde_json::from_str(COMPLETION).expect("captured item");
    let Some(Documentation::MarkupContent(original)) = item.documentation.clone() else {
        panic!("captured markup documentation");
    };
    assert!(
        clickable(&original.value)
            .iter()
            .any(|target| !is_web(target))
    );
    let Some(Documentation::MarkupContent(restricted)) = item.documentation.map(documentation)
    else {
        panic!("markup stays markup");
    };
    assert_eq!(restricted.kind, MarkupKind::Markdown);
    assert!(
        clickable(&restricted.value)
            .iter()
            .all(|target| is_web(target))
    );
    assert_eq!(code_blocks(&restricted.value), code_blocks(&original.value));

    let Documentation::String(plain) =
        documentation(Documentation::String("see <smb://host/share>".into()))
    else {
        panic!("a string stays a string");
    };
    assert_eq!(plain, r"see \<smb://host/share>");
}
