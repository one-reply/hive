//! Repairs syntax grammars that GPUI Kit registers incompletely, so every code view that
//! highlights through its `LanguageRegistry` (the diff tabs) colors them fully.

use std::sync::Once;

use gpui_kit::component::highlighter::{GrammarConfig, LanguageRegistry};

/// For the same node the earliest pattern wins, and GPUI Kit's JavaScript and TypeScript
/// queries open with `(identifier) @variable`, which silently defeats their own later constant
/// and type rules.
/// These copies of them go first so `API_URL` reads as a constant and `AppHeader` as a type.
const SPECIAL_IDENTIFIERS: &str = r#"
((identifier) @constant (#match? @constant "^_*[A-Z_][A-Z\\d_]*$"))
((identifier) @type (#match? @type "^[A-Z]"))
"#;

/// JSX captures from `tree-sitter-javascript`'s `highlights-jsx.scm`, ahead of the JavaScript
/// and TypeScript queries for the same reason: their `@variable` would otherwise hide
/// lowercase tags. GPUI Kit's JavaScript query has no JSX captures of its own.
const JSX_HIGHLIGHTS: &str = r#"
(jsx_opening_element (identifier) @tag (#match? @tag "^[a-z][^.]*$"))
(jsx_closing_element (identifier) @tag (#match? @tag "^[a-z][^.]*$"))
(jsx_self_closing_element (identifier) @tag (#match? @tag "^[a-z][^.]*$"))

(jsx_attribute (property_identifier) @attribute)
(jsx_opening_element (["<" ">"]) @punctuation.bracket)
(jsx_closing_element (["</" ">"]) @punctuation.bracket)
(jsx_self_closing_element (["<" "/>"]) @punctuation.bracket)
"#;

/// Registers the repaired grammars before any code view highlights. Repeat calls are no-ops,
/// so the prefixes are never stacked twice.
pub fn init() {
    static REPAIRED: Once = Once::new();
    REPAIRED.call_once(|| {
        let registry = LanguageRegistry::singleton();
        match registry.language("javascript") {
            Some(javascript) => registry.register("javascript", &javascript_grammar(javascript)),
            None => tracing::warn!("javascript grammar missing; js and jsx stay unhighlighted"),
        }
        let Some(typescript) = registry.language("typescript") else {
            tracing::warn!("typescript grammar missing; ts and tsx keep stock highlighting");
            return;
        };
        let typescript = typescript_grammar(typescript);
        registry.register("typescript", &typescript);
        match registry.language("tsx") {
            Some(tsx) => registry.register("tsx", &tsx_grammar(tsx, typescript)),
            None => tracing::warn!("tsx grammar missing; tsx stays unhighlighted"),
        }
    });
}

/// The JavaScript grammar parses JSX, so `.js` and `.jsx` files take the JSX captures too.
///
/// GPUI Kit runs the locals query in the same pass as the highlights but never resolves
/// scopes, and an identifier's first capture names its color: `@local.definition` and
/// `@local.reference` have none, so components, constants and attribute values came out
/// uncolored. The TypeScript grammars have no locals query either.
fn javascript_grammar(mut javascript: GrammarConfig) -> GrammarConfig {
    javascript.highlights = format!(
        "{SPECIAL_IDENTIFIERS}\n{JSX_HIGHLIGHTS}\n{}",
        javascript.highlights
    )
    .into();
    javascript.locals = Default::default();
    javascript
}

fn typescript_grammar(mut typescript: GrammarConfig) -> GrammarConfig {
    typescript.highlights = format!("{SPECIAL_IDENTIFIERS}\n{}", typescript.highlights).into();
    typescript
}

/// GPUI Kit's `tsx` uses `tree-sitter-typescript`'s `HIGHLIGHTS_QUERY`, which holds only the
/// TypeScript additions to the JavaScript query: no keywords, strings or comments. Its
/// `typescript` query is complete and its nodes all exist in the TSX grammar (a superset), so
/// `tsx` keeps its grammar and takes the `typescript` queries plus the JSX captures.
fn tsx_grammar(mut tsx: GrammarConfig, typescript: GrammarConfig) -> GrammarConfig {
    tsx.highlights = format!("{JSX_HIGHLIGHTS}\n{}", typescript.highlights).into();
    tsx.injections = typescript.injections;
    tsx.injection_languages = typescript.injection_languages;
    tsx.locals = typescript.locals;
    tsx
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::Rope;
    use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};

    /// The lines from the React Native diff where every token rendered in one color.
    const TSX: &str = "import { AppHeader } from '@components/header';\n\
        const loadMore = () => {\n  if (hasNextPage && !isFetching) {\n    return <View style={styles.row}><div /></View>;\n  }\n};\n";
    /// A Next.js provider: a client directive, a component and nested JSX with attributes.
    const JSX: &str = "'use client';\nimport { Provider } from 'react-redux';\n\
        const API_URL = '/api';\n\
        const ContextProvider = ({ children }) => {\n  return (\n    <Provider store={store}>\n      <main className='page'>{children}</main>\n    </Provider>\n  );\n};\n";
    const TS: &str = "const API_URL = new AppClient('https://example.com');\n";

    /// The color painted over `token` in `source`, as the diff paints it.
    fn color_of(language: &str, source: &str, token: &str) -> Option<gpui_kit::Hsla> {
        color_in(language, source, token, token)
    }

    /// The color painted over `token` where it first appears inside `context`.
    fn color_in(
        language: &str,
        source: &str,
        context: &str,
        token: &str,
    ) -> Option<gpui_kit::Hsla> {
        let mut highlighter = SyntaxHighlighter::new(language);
        highlighter.update(None, &Rope::from_str(source), None);
        let start = source.find(context).expect("context is in the sample")
            + context.find(token).expect("token is in its context");
        highlighter
            .styles(&(0..source.len()), &*HighlightTheme::default_dark())
            .into_iter()
            .filter(|(range, _)| range.start <= start && start + token.len() <= range.end)
            .filter_map(|(_, style)| style.color)
            .next_back()
    }

    fn theme_color(name: &str) -> Option<gpui_kit::Hsla> {
        HighlightTheme::default_dark()
            .style
            .syntax
            .style(name)?
            .color
    }

    #[test]
    fn tsx_colors_keywords_strings_and_jsx() {
        init();
        for (token, name) in [
            ("import", "keyword"),
            ("const", "keyword"),
            ("'@components/header'", "string"),
            ("AppHeader", "type"),
            ("View", "type"),
            ("div", "tag"),
            ("style", "attribute"),
        ] {
            assert_eq!(color_of("tsx", TSX, token), theme_color(name), "{token}");
        }
    }

    #[test]
    fn javascript_colors_jsx_components_tags_and_attributes() {
        init();
        for (context, token, name) in [
            ("import", "import", "keyword"),
            ("'react-redux'", "'react-redux'", "string"),
            ("API_URL", "API_URL", "constant"),
            ("ContextProvider", "ContextProvider", "function"),
            ("<Provider", "Provider", "type"),
            ("</Provider", "Provider", "type"),
            ("<main", "main", "tag"),
            ("className", "className", "attribute"),
            ("store={", "store", "attribute"),
            ("={store}", "store", "variable"),
            ("{ children }", "children", "property"),
        ] {
            assert_eq!(
                color_in("javascript", JSX, context, token),
                theme_color(name),
                "{token} in {context}"
            );
        }
    }

    #[test]
    fn typescript_colors_constants_and_types_over_variables() {
        init();
        assert_eq!(
            color_of("typescript", TS, "API_URL"),
            theme_color("constant")
        );
        assert_eq!(color_of("typescript", TS, "AppClient"), theme_color("type"));
    }
}
