//! Terminal page: default CLI agent, close confirmation and terminal font size.

use gpui_kit::component::v_flex;
use gpui_kit::component::{
    Sizable as _,
    button::Button,
    menu::{DropdownMenu as _, PopupMenuItem},
    switch::Switch,
};
use gpui_kit::*;
use hive_core::agent::launch::AGENTS;
use hive_core::settings::{
    TERMINAL_FONT_SIZE_DEFAULT, TERMINAL_FONT_SIZE_MAX, TERMINAL_FONT_SIZE_MIN,
};

use super::{font_size_stepper, setting_row, setting_title};
use crate::app::AppState;

pub(super) fn render(window: &mut Window, cx: &mut App) -> impl IntoElement {
    let store = AppState::get(cx).store.clone();
    let font_size = store.read(cx).state.settings.terminal_font_size();
    let confirm_close = store.read(cx).state.settings.confirm_close_running_agent;
    let confirm_store = store.clone();
    let agent = store.read(cx).state.settings.default_cli_agent();
    let agent_store = store.clone();
    let picker = Button::new("default-cli-agent")
        .cursor_pointer()
        .small()
        .label(agent.name())
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _, _| {
            AGENTS.into_iter().fold(menu, |menu, choice| {
                let store = agent_store.clone();
                menu.item(
                    PopupMenuItem::new(choice.name())
                        .checked(choice == agent)
                        .on_click(move |_, _, cx| {
                            store.update(cx, |store, cx| store.set_default_cli_agent(choice, cx))
                        }),
                )
            })
        });
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
        setting_title("Default CLI agent", "Receives Git error drafts in this worktree. Messages are never submitted automatically.", cx),
        picker,
        cx,
    )).child(setting_row(
        setting_title("Confirm closing running agents", "Ask before closing a terminal pane or tab in which a CLI agent is running.", cx),
        Switch::new("confirm-close-running-agent")
            .accessibility_label("Confirm closing running agents")
            .checked(confirm_close)
            .on_click(move |value, _, cx| {
                confirm_store.update(cx, |store, cx| store.set_confirm_close_running_agent(*value, cx))
            }),
        cx,
    )).child(setting_row(
        setting_title("Font size", "Text size in every terminal tab.", cx),
        stepper,
        cx,
    ))
}
