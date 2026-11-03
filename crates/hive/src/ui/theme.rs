use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;
use hive_core::settings::ThemeChoice;

pub fn apply_theme(theme: ThemeChoice, window: Option<&mut Window>, cx: &mut App) {
    match theme {
        ThemeChoice::System => Theme::sync_system_appearance(window, cx),
        ThemeChoice::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemeChoice::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
}
