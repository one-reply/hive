//! Shared presentation for Hive's modal forms.
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::*;
use std::cell::Cell;

/// Run on the first dialog frame, after the shell has taken focus.
pub(super) fn initial_focus(handle: FocusHandle) -> impl Fn(&mut Window, &mut App) {
    let pending = Cell::new(Some(handle));
    move |window, cx| {
        if let Some(handle) = pending.take() {
            window.defer(cx, move |window, cx| handle.focus(window, cx));
        }
    }
}

pub(super) fn field(label: &'static str, control: impl IntoElement, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(control)
}
