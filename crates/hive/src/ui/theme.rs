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
    hive_core::terminal::set_palette(reported_palette(&definition.terminal()));
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

/// What a terminal tells a program that asks for its colors: the colors it draws with.
fn reported_palette(palette: &catalog::TerminalPalette) -> hive_core::terminal::Palette {
    hive_core::terminal::Palette {
        foreground: to_rgb(palette.foreground),
        background: to_rgb(palette.background),
        cursor: to_rgb(palette.cursor),
        ansi: palette.ansi.map(to_rgb),
    }
}

fn to_rgb(color: Hsla) -> hive_core::terminal::Rgb {
    let color = Rgba::from(color);
    let byte = |channel: f32| (channel.clamp(0., 1.) * 255.).round() as u8;
    hive_core::terminal::Rgb {
        r: byte(color.r),
        g: byte(color.g),
        b: byte(color.b),
    }
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
    use super::{Appearance, catalog, reported_palette, resolved_mode};
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

    #[test]
    fn a_program_asking_for_terminal_colors_hears_the_theme_hex_values() {
        let reported = reported_palette(&catalog::dark(DarkTheme::Hive).terminal());
        let rgb = |r, g, b| hive_core::terminal::Rgb { r, g, b };
        assert_eq!(reported.background, rgb(0x0a, 0x0a, 0x0a));
        assert_eq!(reported.foreground, rgb(0xfa, 0xfa, 0xfa));
        assert_eq!(reported.ansi[1], rgb(0xe0, 0x55, 0x61));
        assert_eq!(reported.ansi[12], rgb(0x4d, 0xc4, 0xff));
    }
}
