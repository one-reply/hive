//! Projects page: the folders listed in the sidebar.

use std::path::PathBuf;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::setting_row;
use crate::app::AppState;
use crate::state::store::Store;

pub(super) fn render(cx: &App) -> impl IntoElement {
    let store = AppState::get(cx).store.clone();
    let paths: Vec<PathBuf> = store
        .read(cx)
        .state
        .projects
        .iter()
        .map(|project| project.path.clone())
        .collect();
    let muted = cx.theme().muted_foreground;

    let rows = paths.into_iter().enumerate().map(|(ix, path)| {
        let store = store.clone();
        let label = div()
            .min_w_0()
            .text_sm()
            .truncate()
            .child(path.display().to_string());
        let remove = Button::new(("remove-project", ix))
            .cursor_pointer()
            .ghost()
            .small()
            .icon(IconName::Delete)
            .tooltip("Remove from Hive")
            .on_click(move |_, _, cx| {
                store.update(cx, |store, cx| store.remove_project(&path, cx))
            });
        setting_row(label, remove, cx)
    });

    let rows: Vec<Div> = rows.collect();
    v_flex()
        .when(rows.is_empty(), |el| {
            el.child(
                div()
                    .py_3()
                    .text_sm()
                    .text_color(muted)
                    .child("No projects yet."),
            )
        })
        .children(rows)
        .child(
            h_flex().pt_4().gap_3().items_center().child(
                Button::new("settings-add-project")
                    .cursor_pointer()
                    .outline()
                    .small()
                    .icon(Lucide::FolderPlus)
                    .label("Add project")
                    .on_click(move |_, _, cx| {
                        Store::prompt_add_projects(AppState::get(cx).store.clone(), cx)
                    }),
            ),
        )
}
