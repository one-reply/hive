//! Embedded palettes: GPUI's theme format with Hive's terminal colors alongside it.

use std::sync::LazyLock;

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::theme::{ThemeConfig, ThemeMode};
use gpui_kit::{Hsla, rgb};
use hive_core::settings::{DarkTheme, LightTheme};
use serde::Deserialize;

/// Colors for the terminal and the Appearance swatch, resolved once at startup.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub(crate) struct TerminalPalette {
    pub(crate) background: Hsla,
    pub(crate) foreground: Hsla,
    pub(crate) cursor: Hsla,
    pub(crate) selection: Hsla,
    pub(crate) ansi: [Hsla; 16],
}

#[derive(Deserialize)]
pub(crate) struct Definition {
    #[serde(flatten)]
    pub(super) config: ThemeConfig,
    terminal: TerminalPalette,
}

impl Definition {
    pub(crate) fn terminal(&self) -> TerminalPalette {
        self.terminal
    }
}

const SOURCES: [(&str, ThemeMode, &str); 6] = [
    (
        "Hive Light",
        ThemeMode::Light,
        include_str!("../../../assets/themes/hive-light.json"),
    ),
    (
        "One Light",
        ThemeMode::Light,
        include_str!("../../../assets/themes/one-light.json"),
    ),
    (
        "Solarized Light",
        ThemeMode::Light,
        include_str!("../../../assets/themes/solarized-light.json"),
    ),
    (
        "Hive Dark",
        ThemeMode::Dark,
        include_str!("../../../assets/themes/hive-dark.json"),
    ),
    (
        "One Dark",
        ThemeMode::Dark,
        include_str!("../../../assets/themes/one-dark.json"),
    ),
    (
        "Dracula",
        ThemeMode::Dark,
        include_str!("../../../assets/themes/dracula.json"),
    ),
];

static THEMES: LazyLock<[Definition; 6]> = LazyLock::new(|| {
    SOURCES.map(|(name, mode, source)| {
        parse(source, mode).unwrap_or_else(|error| {
            tracing::error!(%error, theme = name, "invalid bundled theme; using stock colors");
            fallback(name, mode)
        })
    })
});

pub(crate) fn light(theme: LightTheme) -> &'static Definition {
    &THEMES[match theme {
        LightTheme::Hive => 0,
        LightTheme::One => 1,
        LightTheme::Solarized => 2,
    }]
}

pub(crate) fn dark(theme: DarkTheme) -> &'static Definition {
    &THEMES[match theme {
        DarkTheme::Hive => 3,
        DarkTheme::One => 4,
        DarkTheme::Dracula => 5,
    }]
}

pub(super) fn named(name: &str) -> Option<&'static Definition> {
    THEMES.iter().find(|theme| theme.config.name == name)
}

fn stock_highlight(mode: ThemeMode) -> std::sync::Arc<HighlightTheme> {
    if mode.is_dark() {
        HighlightTheme::default_dark()
    } else {
        HighlightTheme::default_light()
    }
}

fn parse(source: &str, mode: ThemeMode) -> serde_json::Result<Definition> {
    let mut value: serde_json::Value = serde_json::from_str(source)?;
    // GPUI replaces the entire highlight style; retain omitted Markdown/status roles.
    let mut highlight = serde_json::to_value(&stock_highlight(mode).style)?;
    if let Some(overrides) = value.get("highlight").and_then(|value| value.as_object()) {
        for (key, value) in overrides {
            if key == "syntax"
                && let (Some(base), Some(overrides)) =
                    (highlight[key].as_object_mut(), value.as_object())
            {
                base.extend(overrides.clone());
            } else {
                highlight[key] = value.clone();
            }
        }
    }
    if let Some(object) = value.as_object_mut() {
        object.insert("highlight".into(), highlight);
    }
    serde_json::from_value(value)
}

fn fallback(name: &str, mode: ThemeMode) -> Definition {
    let mut config = ThemeConfig::default();
    config.name = name.to_owned().into();
    config.mode = mode;
    config.highlight = Some(stock_highlight(mode).style.clone());
    let (background, foreground): (Hsla, Hsla) = if mode.is_dark() {
        (rgb(0x0a0a0a).into(), rgb(0xfafafa).into())
    } else {
        (rgb(0xffffff).into(), rgb(0x0a0a0a).into())
    };
    Definition {
        config,
        terminal: TerminalPalette {
            background,
            foreground,
            cursor: foreground,
            selection: foreground.opacity(0.3),
            ansi: [foreground; 16],
        },
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
