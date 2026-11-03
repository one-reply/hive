//! Settings panel (right-side sheet): theme, project list.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::*;
use hive_core::settings::ThemeChoice;

use crate::app::AppState;

pub fn open(window: &mut Window, cx: &mut App) {
    let store = AppState::get(cx).store.clone();
    // The builder may run again whenever the sheet layer is rebuilt; it reads live state.
    window.open_sheet(cx, move |sheet, _, cx| {
        let state = &store.read(cx).state;
        let theme_index = ThemeChoice::ALL.iter().position(|t| *t == state.settings.theme);
        let projects: Vec<String> = state.projects.iter().map(|p| p.path.display().to_string()).collect();

        let theme_group = RadioGroup::vertical("theme")
            .selected_index(theme_index)
            .children(ThemeChoice::ALL.iter().map(|t| t.label()))
            .on_change({
                let store = store.clone();
                move |index, _, cx| store.update(cx, |s, cx| s.set_theme(ThemeChoice::ALL[*index], cx))
            });

        let project_rows = projects.into_iter().enumerate().map(|(i, path)| {
            let store = store.clone();
            h_flex()
                .justify_between()
                .gap_2()
                .child(div().text_sm().truncate().child(path))
                .child(
                    Button::new(("remove-project", i))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Delete)
                        .on_click(move |_, _, cx| store.update(cx, |s, cx| s.remove_project(i, cx))),
                )
        });

        let muted = cx.theme().muted_foreground;
        let title = |text: &'static str| div().text_xs().text_color(muted).child(text);

        sheet.title("Settings").size(px(380.)).child(
            v_flex()
                .gap_6()
                .p_4()
                .child(v_flex().gap_2().child(title("Theme")).child(theme_group))
                .child(v_flex().gap_2().child(title("Projects")).children(project_rows)),
        )
    });
}
