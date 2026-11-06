//! Applies the appearance settings (theme, fonts, scrollbars) to the global `Theme`, which every
//! window reads, so a change in the settings window shows up everywhere at once.

use gpui_kit::component::scroll::ScrollbarMode;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;
use hive_core::settings::{Scrollbars, Settings, ThemeChoice};

use crate::app::AppState;

/// The dock's tab bar in dark mode; the stock `#171717` reads too light next to the terminal.
const DARK_TAB_BAR: u32 = 0x111111;

/// What `apply` last wrote to the global theme; other settings changes skip the reload.
#[derive(Clone, PartialEq)]
struct Appearance {
    theme: ThemeChoice,
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
    apply_theme(appearance.theme, window, cx);
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
    // `Theme::change` reloads the theme file, which may carry its own sizes and syntax colors,
    // so the user's sizes and Hive's palette go on top in a separate update. `font_size` is the rem every window scales from.
    Theme::update(cx, |theme| {
        theme.font_size = px(appearance.ui_font_size);
        theme.mono_font_size = px(appearance.terminal_font_size);
        theme.font_family = font_family;
        theme.mono_font_family = mono_font_family;
        if theme.is_dark() {
            theme.tab_bar = rgb(DARK_TAB_BAR).into();
        } else {
            // The stock light sidebar is an off-white gray; Hive's light sidebars are white.
            theme.sidebar = theme.background;
        }
        if let Some(highlight) = super::syntax_palette::highlight_theme(theme.is_dark()) {
            theme.highlight_theme = highlight;
        }
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

fn apply_theme(theme: ThemeChoice, window: Option<&mut Window>, cx: &mut App) {
    match theme {
        ThemeChoice::System => Theme::sync_system_appearance(window, cx),
        ThemeChoice::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemeChoice::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
}
