//! Appearance page: theme and UI font size.

use gpui_kit::component::button::{Button, ButtonGroup};
use gpui_kit::component::{Selectable as _, Sizable as _, v_flex};
use gpui_kit::*;
use hive_core::settings::{ThemeChoice, UI_FONT_SIZE_DEFAULT, UI_FONT_SIZE_MAX, UI_FONT_SIZE_MIN};

use super::{font_size_stepper, setting_row, setting_title};
use crate::app::AppState;

pub(super) fn render(window: &mut Window, cx: &mut App) -> impl IntoElement {
    let store = AppState::get(cx).store.clone();
    let settings = &store.read(cx).state.settings;
    let (current, font_size) = (settings.theme, settings.ui_font_size());

    let themes = ButtonGroup::new("theme")
        .outline()
        .small()
        .children(ThemeChoice::ALL.iter().map(|theme| {
            Button::new(theme.label())
                .cursor_pointer()
                .label(theme.label())
                .selected(*theme == current)
        }))
        .on_click({
            let store = store.clone();
            move |clicked: &Vec<usize>, _, cx| {
                if let Some(theme) = clicked.first().and_then(|ix| ThemeChoice::ALL.get(*ix)) {
                    store.update(cx, |store, cx| store.set_theme(*theme, cx));
                }
            }
        });
    let font_size = font_size_stepper(
        "ui-font-size",
        font_size,
        UI_FONT_SIZE_MIN..=UI_FONT_SIZE_MAX,
        UI_FONT_SIZE_DEFAULT,
        move |size, cx| store.update(cx, |store, cx| store.set_ui_font_size(size, cx)),
        window,
        cx,
    );

    v_flex()
        .child(setting_row(
            setting_title(
                "Theme",
                "Follow the system, or always use light or dark.",
                cx,
            ),
            themes,
            cx,
        ))
        .child(setting_row(
            setting_title(
                "UI font size",
                "Scales text and controls in every window.",
                cx,
            ),
            font_size,
            cx,
        ))
}
