//! Applies the appearance settings (theme, fonts, scrollbars) to the global `Theme`, which every
//! window reads, so a change in the settings window shows up everywhere at once.

use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;
use hive_core::settings::{DarkTheme, LightTheme, Scrollbars, Settings, ThemeChoice};
use std::rc::Rc;

use crate::app::AppState;

pub(crate) mod catalog;

/// What `apply` last wrote to the global theme; other settings changes skip the reload.
#[derive(Clone, PartialEq)]
struct Appearance {
    theme: ThemeChoice,
    light_theme: LightTheme,
    dark_theme: DarkTheme,
    ui_font_size: f32,
    terminal_font_size: f32,
    ui_font_family: Option<SharedString>,
    code_font_family: Option<SharedString>,
    scrollbars: Scrollbars,
}

impl From<&Settings> for Appearance {
    fn from(settings: &Settings) -> Self {
        Self {
            theme: settings.theme,
            light_theme: settings.light_theme(),
            dark_theme: settings.dark_theme(),
            ui_font_size: settings.ui_font_size(),
            terminal_font_size: settings.terminal_font_size(),
            ui_font_family: settings.ui_font_family().map(SharedString::new),
            code_font_family: settings.code_font_family().map(SharedString::new),
            scrollbars: settings.scrollbars,
        }
    }
}

/// Applies the stored appearance now and again whenever it changes.
pub fn init(cx: &mut App) {
    let store = AppState::get(cx).store.clone();
    let mut applied = Appearance::from(&store.read(cx).state.settings);
    apply(&applied, None, cx);
    cx.observe(&store, move |store, cx| {
        let appearance = Appearance::from(&store.read(cx).state.settings);
        if appearance != applied {
            apply(&appearance, None, cx);
            applied = appearance;
        }
    })
    .detach();
}

/// Follows the OS light/dark switch while the theme is `System`.
pub fn sync_system_appearance(window: &mut Window, cx: &mut App) {
    let appearance = Appearance::from(&AppState::get(cx).store.read(cx).state.settings);
    if appearance.theme == ThemeChoice::System {
        apply(&appearance, Some(window), cx);
    }
}

fn apply(appearance: &Appearance, window: Option<&mut Window>, cx: &mut App) {
    let system = window
        .as_ref()
        .map(|window| window.appearance())
        .unwrap_or_else(|| cx.window_appearance());
    let mode = resolved_mode(appearance.theme, system.into());
    let definition = if mode.is_dark() {
        catalog::dark(appearance.dark_theme)
    } else {
        catalog::light(appearance.light_theme)
    };
    let config = Rc::new(definition.config.clone());
    Theme::update(cx, |theme| theme.apply_config(&config));
    // A theme reload keeps the previous families, so the defaults are written back explicitly;
    // the theme then swaps an uninstalled default monospace family for an installed one.
    let defaults = Theme::default();
    let font_family = appearance
        .ui_font_family
        .clone()
        .unwrap_or(defaults.font_family);
    let mono_font_family = appearance
        .code_font_family
        .clone()
        .unwrap_or(defaults.mono_font_family);
    // User typography wins over palette defaults; `font_size` is every window's rem.
    Theme::update(cx, |theme| {
        theme.font_size = px(appearance.ui_font_size);
        theme.mono_font_size = px(appearance.terminal_font_size);
        theme.font_family = font_family;
        theme.mono_font_family = mono_font_family;
    });
    apply_scrollbars(appearance.scrollbars, cx);
}

fn apply_scrollbars(scrollbars: Scrollbars, cx: &mut App) {
    match scrollbar_mode(scrollbars) {
        Some(mode) => Theme::set_scrollbar_mode(mode, cx),
        None => Theme::sync_scrollbar_appearance(cx),
    }
}

/// The fixed mode for `scrollbars`, or `None` to follow macOS.
fn scrollbar_mode(scrollbars: Scrollbars) -> Option<ScrollbarMode> {
    match scrollbars {
        Scrollbars::System => None,
        Scrollbars::Scrolling => Some(ScrollbarMode::Scrolling),
        Scrollbars::Hover => Some(ScrollbarMode::Hover),
        Scrollbars::Always => Some(ScrollbarMode::Always),
    }
}

fn resolved_mode(theme: ThemeChoice, system: ThemeMode) -> ThemeMode {
    match theme {
        ThemeChoice::System => system,
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
    }
}

pub(crate) fn terminal_palette(cx: &App) -> catalog::TerminalPalette {
    let theme = Theme::global(cx);
    catalog::named(theme.theme_name())
        .unwrap_or_else(|| {
            if theme.is_dark() {
                catalog::dark(DarkTheme::Hive)
            } else {
                catalog::light(LightTheme::Hive)
            }
        })
        .terminal()
}

#[cfg(test)]
mod tests {
    use super::{Appearance, resolved_mode};
    use gpui_kit::component::theme::ThemeMode;
    use hive_core::settings::{DarkTheme, LightTheme, Settings, ThemeChoice};

    #[test]
    fn only_system_follows_the_os_appearance() {
        for system in [ThemeMode::Light, ThemeMode::Dark] {
            assert_eq!(resolved_mode(ThemeChoice::System, system), system);
            assert_eq!(resolved_mode(ThemeChoice::Light, system), ThemeMode::Light);
            assert_eq!(resolved_mode(ThemeChoice::Dark, system), ThemeMode::Dark);
        }
    }

    #[test]
    fn appearance_observer_tracks_both_palette_choices() {
        let mut settings = Settings::default();
        let original = Appearance::from(&settings);
        settings.set_light_theme(LightTheme::One);
        assert!(Appearance::from(&settings) != original);
        settings.set_light_theme(LightTheme::Hive);
        settings.set_dark_theme(DarkTheme::Dracula);
        assert!(Appearance::from(&settings) != original);
    }
}
