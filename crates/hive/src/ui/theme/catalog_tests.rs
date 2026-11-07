use super::*;
use gpui_kit::component::theme::{Theme, try_parse_color};
use std::rc::Rc;

#[test]
fn bundled_palettes_have_valid_colors_and_expected_modes() {
    for (name, mode, source) in SOURCES {
        let definition = parse(source, mode).unwrap();
        assert_eq!(definition.config.name, name);
        assert_eq!(definition.config.mode, mode);
        let raw: serde_json::Value = serde_json::from_str(source).unwrap();
        let known = serde_json::to_value(&definition.config.colors).unwrap();
        for (key, value) in raw["colors"].as_object().unwrap() {
            assert!(known.get(key).is_some(), "{name}: unknown color {key}");
            assert!(
                try_parse_color(value.as_str().unwrap()).is_ok(),
                "{name}: {key}"
            );
        }
        let known_highlight = serde_json::to_value(&definition.config.highlight).unwrap();
        for key in raw["highlight"].as_object().unwrap().keys() {
            assert!(
                known_highlight.get(key).is_some(),
                "{name}: unknown highlight role {key}"
            );
        }
        let mut theme = Theme::default();
        theme.apply_config(&Rc::new(definition.config));
        assert_eq!(definition.terminal.background, theme.background, "{name}");
        assert_eq!(definition.terminal.foreground, theme.foreground, "{name}");
        assert_ne!(
            definition.terminal.background,
            definition.terminal.foreground
        );
        assert_eq!(definition.terminal.ansi.len(), 16);
    }
    for choice in LightTheme::ALL {
        assert_eq!(light(choice).config.name, choice.label());
    }
    for choice in DarkTheme::ALL {
        assert_eq!(dark(choice).config.name, choice.label());
    }
}

#[test]
fn switching_palettes_leaves_no_colors_from_the_previous_theme() {
    for before in THEMES.iter() {
        for after in THEMES.iter() {
            let mut changed = Theme::default();
            changed.apply_config(&Rc::new(before.config.clone()));
            changed.apply_config(&Rc::new(after.config.clone()));
            let mut fresh = Theme::default();
            fresh.apply_config(&Rc::new(after.config.clone()));
            assert_eq!(changed.colors, fresh.colors);
            assert_eq!(changed.highlight_theme, fresh.highlight_theme);
            assert_eq!(changed.theme_name(), &after.config.name);
        }
    }
}

#[test]
fn hive_palettes_keep_stock_roles_and_distinct_jsx_colors() {
    for definition in [light(LightTheme::Hive), dark(DarkTheme::Hive)] {
        let style = definition.config.highlight.as_ref().unwrap();
        let stock = stock_highlight(definition.config.mode);
        assert_eq!(
            style.syntax.style("link_uri"),
            stock.style.syntax.style("link_uri")
        );
        assert_eq!(style.editor_background, stock.style.editor_background);
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
        let colors: Vec<_> = roles
            .iter()
            .map(|name| style.syntax.style(name).and_then(|style| style.color))
            .collect();
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

#[test]
fn malformed_and_truncated_palettes_fail_without_panicking() {
    for source in ["", "{", "null", "[]", "{}", r#"{"terminal":{"ansi":[]}}"#] {
        assert!(parse(source, ThemeMode::Light).is_err());
    }
    let source = SOURCES[0].2.replace("#ffffff", "invalid-color");
    assert!(parse(&source, ThemeMode::Light).is_err());
}
