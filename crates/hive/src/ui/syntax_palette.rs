//! Hive's syntax colors. GPUI Kit's stock dark palette paints keywords, functions, tags,
//! attributes and properties in five shades of the same blue, so JSX and call-heavy code
//! reads as one color. Each role here gets its own hue: components and types amber, HTML tags
//! red, attributes orange italic, functions blue, keywords violet, properties teal.
//!
//! The palettes are Zed-format `syntax` maps laid over the stock theme, which keeps the
//! roles they leave out (Markdown titles, links, emphasis) and every editor color.

use std::sync::{Arc, LazyLock};

use gpui_kit::component::highlighter::{HighlightTheme, SyntaxColors};

const DARK: &str = include_str!("../../assets/syntax/dark.json");
const LIGHT: &str = include_str!("../../assets/syntax/light.json");

static DARK_THEME: LazyLock<Option<Arc<HighlightTheme>>> =
    LazyLock::new(|| overlay(&HighlightTheme::default_dark(), DARK));
static LIGHT_THEME: LazyLock<Option<Arc<HighlightTheme>>> =
    LazyLock::new(|| overlay(&HighlightTheme::default_light(), LIGHT));

/// The highlight theme for the current mode, or `None` to keep the stock one if a bundled
/// palette failed to load (already logged).
pub(crate) fn highlight_theme(dark: bool) -> Option<Arc<HighlightTheme>> {
    let theme = if dark { &DARK_THEME } else { &LIGHT_THEME };
    theme.as_ref().cloned()
}

fn overlay(base: &HighlightTheme, palette: &str) -> Option<Arc<HighlightTheme>> {
    match overlay_syntax(&base.style.syntax, palette) {
        Ok(syntax) => {
            let mut theme = base.clone();
            theme.style.syntax = syntax;
            Some(Arc::new(theme))
        }
        Err(error) => {
            tracing::error!(%error, theme = base.name, "syntax palette is invalid; keeping stock colors");
            None
        }
    }
}

fn overlay_syntax(base: &SyntaxColors, palette: &str) -> serde_json::Result<SyntaxColors> {
    let mut merged = serde_json::to_value(base)?;
    let palette: serde_json::Map<String, serde_json::Value> = serde_json::from_str(palette)?;
    if let Some(merged) = merged.as_object_mut() {
        merged.extend(palette);
    }
    serde_json::from_value(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(theme: &HighlightTheme, name: &str) -> Option<gpui_kit::Hsla> {
        theme.style.syntax.style(name)?.color
    }

    #[test]
    fn bundled_palettes_load_and_keep_stock_roles() {
        for dark in [true, false] {
            let theme = highlight_theme(dark).expect("bundled palette parses");
            let stock = if dark {
                HighlightTheme::default_dark()
            } else {
                HighlightTheme::default_light()
            };
            assert_eq!(color(&theme, "link_uri"), color(&stock, "link_uri"));
            assert_eq!(theme.style.editor_background, stock.style.editor_background);
        }
    }

    /// The point of the palette: the roles that sit side by side in JSX never share a color.
    #[test]
    fn jsx_roles_have_distinct_colors() {
        for dark in [true, false] {
            let theme = highlight_theme(dark).expect("bundled palette parses");
            let roles = [
                "type",
                "tag",
                "attribute",
                "function",
                "keyword",
                "property",
                "string",
                "constant",
                "operator",
                "punctuation.bracket",
                "variable",
            ];
            let colors: Vec<_> = roles.iter().map(|role| color(&theme, role)).collect();
            for (ix, color) in colors.iter().enumerate() {
                assert!(color.is_some(), "{} has no color", roles[ix]);
                assert!(
                    !colors[..ix].contains(color),
                    "{} repeats an earlier color",
                    roles[ix]
                );
            }
        }
    }
}
