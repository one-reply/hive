//! Applies the appearance settings (theme, font sizes) to the global `Theme`, which every
//! window reads, so a change in the settings window shows up everywhere at once.

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;
use hive_core::settings::{Settings, ThemeChoice};

use crate::app::AppState;

/// The dock's tab bar in dark mode; the stock `#171717` reads too light next to the terminal.
const DARK_TAB_BAR: u32 = 0x111111;

/// What `apply` last wrote to the global theme; other settings changes skip the reload.
#[derive(Clone, Copy, PartialEq)]
struct Appearance {
    theme: ThemeChoice,
    ui_font_size: f32,
    terminal_font_size: f32,
}

impl From<&Settings> for Appearance {
    fn from(settings: &Settings) -> Self {
        Self {
            theme: settings.theme,
            ui_font_size: settings.ui_font_size(),
            terminal_font_size: settings.terminal_font_size(),
        }
    }
}

/// Applies the stored appearance now and again whenever it changes.
pub fn init(cx: &mut App) {
    let store = AppState::get(cx).store.clone();
    let mut applied = Appearance::from(&store.read(cx).state.settings);
    apply(applied, None, cx);
    cx.observe(&store, move |store, cx| {
        let appearance = Appearance::from(&store.read(cx).state.settings);
        if appearance != applied {
            applied = appearance;
            apply(appearance, None, cx);
        }
    })
    .detach();
}

/// Follows the OS light/dark switch while the theme is `System`.
pub fn sync_system_appearance(window: &mut Window, cx: &mut App) {
    let appearance = Appearance::from(&AppState::get(cx).store.read(cx).state.settings);
    if appearance.theme == ThemeChoice::System {
        apply(appearance, Some(window), cx);
    }
}

fn apply(appearance: Appearance, window: Option<&mut Window>, cx: &mut App) {
    apply_theme(appearance.theme, window, cx);
    // `Theme::change` reloads the theme file, which may carry its own sizes, so the user's
    // sizes go on top in a separate update. `font_size` is the rem every window scales from.
    Theme::update(cx, |theme| {
        theme.font_size = px(appearance.ui_font_size);
        theme.mono_font_size = px(appearance.terminal_font_size);
        if theme.is_dark() {
            theme.tab_bar = rgb(DARK_TAB_BAR).into();
        }
    });
}

fn apply_theme(theme: ThemeChoice, window: Option<&mut Window>, cx: &mut App) {
    match theme {
        ThemeChoice::System => Theme::sync_system_appearance(window, cx),
        ThemeChoice::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemeChoice::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
}
