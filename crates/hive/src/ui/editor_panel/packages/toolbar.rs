//! The toolbar's switch for the versions of every package.json, and its menu of package actions.
use super::*;
use crate::ui::key_hint;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};

/// Key context of a package.json tab, where the versions shortcut applies.
const PACKAGE_MANIFEST_CONTEXT: &str = "PackageManifest";

/// Show or hide the versions in every package.json; off, no registry client runs.
pub(in crate::ui::editor_panel) fn toggle(cx: &mut App) {
    let store = AppState::get(cx).store.clone();
    let mut settings = store.read(cx).state.settings.files.clone();
    settings.package_versions = !settings.package_versions;
    store.update(cx, |store, cx| store.set_files_settings(settings, cx));
}

/// The toolbar's switch for the versions, the same one Settings → Files has.
pub(in crate::ui::editor_panel) fn switch(
    document: &Entity<Document>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let shown = document.read(cx).shows_package_versions(cx);
    let hover = Hover::new("package-versions", window, cx);
    Button::new("package-versions")
        .small()
        .cursor_pointer()
        .icon(Lucide::Package)
        .selected(shown)
        .accessibility_label("Dependency versions")
        .tooltip(key_hint::tooltip(
            if shown {
                "Hide dependency versions"
            } else {
                "Show dependency versions"
            },
            &TogglePackageVersions,
            PACKAGE_MANIFEST_CONTEXT,
            window,
        ))
        .on_click(|_, _, cx| toggle(cx))
        .smooth_hover(ButtonLook::Ghost, &hover, cx)
}

/// The toolbar's menu of package actions: the ones that are not about a single dependency.
pub(in crate::ui::editor_panel) fn actions(
    document: &Entity<Document>,
    cx: &App,
) -> impl IntoElement + use<> {
    let shown = document.read(cx).shows_package_versions(cx);
    let installs = document.read(cx).install_on_save(cx);
    let document = document.clone();
    Button::new("package-actions")
        .small()
        .ghost()
        .cursor_pointer()
        .icon(Lucide::Ellipsis)
        .accessibility_label("Package actions")
        .tooltip("Package actions")
        .dropdown_menu(move |menu, _, _| {
            let (refresh, sort, install) = (document.clone(), document.clone(), document.clone());
            menu.item(
                PopupMenuItem::new("Refresh versions")
                    .icon(Lucide::RefreshCw)
                    .disabled(!shown)
                    .on_click(move |_, _, cx| refresh.update(cx, |d, cx| d.recheck_packages(cx))),
            )
            .item(
                PopupMenuItem::new("Sort dependencies")
                    .icon(Lucide::ListOrdered)
                    .on_click(move |_, window, cx| {
                        sort.update(cx, |d, cx| d.sort_packages(window, cx))
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("Install on save for this project")
                    .checked(installs)
                    .on_click(move |_, _, cx| {
                        install.update(cx, |d, cx| d.toggle_package_install(cx))
                    }),
            )
        })
}
