//! Terminal page: font size of terminal tabs.

use gpui_kit::component::v_flex;
use gpui_kit::*;
use hive_core::settings::{
    TERMINAL_FONT_SIZE_DEFAULT, TERMINAL_FONT_SIZE_MAX, TERMINAL_FONT_SIZE_MIN,
};

use super::{font_size_stepper, setting_row, setting_title};
use crate::app::AppState;

pub(super) fn render(window: &mut Window, cx: &mut App) -> impl IntoElement {
    let store = AppState::get(cx).store.clone();
    let font_size = store.read(cx).state.settings.terminal_font_size();
    let stepper = font_size_stepper(
        "terminal-font-size",
        font_size,
        TERMINAL_FONT_SIZE_MIN..=TERMINAL_FONT_SIZE_MAX,
        TERMINAL_FONT_SIZE_DEFAULT,
        move |size, cx| store.update(cx, |store, cx| store.set_terminal_font_size(size, cx)),
        window,
        cx,
    );
    v_flex().child(setting_row(
        setting_title("Font size", "Text size in every terminal tab.", cx),
        stepper,
        cx,
    ))
}
