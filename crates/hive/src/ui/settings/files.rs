use super::{font_size_stepper, setting_row, setting_title};
use crate::{
    app::AppState,
    ui::hover::{ButtonLook, Hover, SmoothButton as _},
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::{
    component::{
        Selectable as _, Sizable as _,
        button::{Button, ButtonGroup},
        switch::Switch,
        v_flex,
    },
    *,
};
use hive_core::settings::{FILES_FONT_SIZE_DEFAULT, FileDensity, FileIcons, FilesSettings};

pub(super) fn render(window: &mut Window, cx: &mut App) -> impl IntoElement {
    let store = AppState::get(cx).store.clone();
    let settings = store.read(cx).state.settings.files.clone();
    let icon_store = store.clone();
    let icons = [
        ("Colored", FileIcons::Colored),
        ("Monochrome", FileIcons::Monochrome),
        ("Off", FileIcons::Off),
    ];
    let icon_picker = ButtonGroup::new("files-icons")
        .outline()
        .small()
        .children(icons.iter().map(|(label, value)| {
            Button::new(*label)
                .cursor_pointer()
                .label(*label)
                .selected(*value == settings.icons)
        }))
        .on_click(move |selected: &Vec<usize>, _, cx| {
            if let Some((_, value)) = selected.first().and_then(|ix| icons.get(*ix)) {
                let mut settings = icon_store.read(cx).state.settings.files.clone();
                settings.icons = *value;
                icon_store.update(cx, |store, cx| store.set_files_settings(settings, cx));
            }
        });
    let density_store = store.clone();
    let densities = [
        ("Compact", FileDensity::Compact),
        ("Comfortable", FileDensity::Comfortable),
    ];
    let density_picker = ButtonGroup::new("files-density")
        .outline()
        .small()
        .children(densities.iter().map(|(label, value)| {
            Button::new(*label)
                .cursor_pointer()
                .label(*label)
                .selected(*value == settings.density)
        }))
        .on_click(move |selected: &Vec<usize>, _, cx| {
            if let Some((_, value)) = selected.first().and_then(|ix| densities.get(*ix)) {
                let mut settings = density_store.read(cx).state.settings.files.clone();
                settings.density = *value;
                density_store.update(cx, |store, cx| store.set_files_settings(settings, cx));
            }
        });
    let font_store = store.clone();
    let font = font_size_stepper(
        "files-font-size",
        settings.font_size(),
        11.0..=20.0,
        FILES_FONT_SIZE_DEFAULT,
        move |size, cx| {
            let mut settings = font_store.read(cx).state.settings.files.clone();
            settings.set_font_size(size);
            font_store.update(cx, |store, cx| store.set_files_settings(settings, cx));
        },
        window,
        cx,
    );
    let mut page = v_flex()
        .child(setting_row(
            setting_title(
                "File icons",
                "Recognize languages and file types at a glance.",
                cx,
            ),
            icon_picker,
            cx,
        ))
        .child(setting_row(
            setting_title(
                "Font size",
                "Scales proportionally with Appearance → UI font size.",
                cx,
            ),
            font,
            cx,
        ))
        .child(setting_row(
            setting_title(
                "Row density",
                "Choose tighter rows or more space between entries.",
                cx,
            ),
            density_picker,
            cx,
        ));
    type Toggle = (
        &'static str,
        &'static str,
        bool,
        fn(&mut FilesSettings, bool),
    );
    let toggles: [Toggle; 6] = [
        (
            "Git decorations",
            "Show staged, unstaged and changed-folder indicators.",
            settings.git_decorations,
            |s, v| s.git_decorations = v,
        ),
        (
            "Auto-reveal active file",
            "Keep the active editor visible without moving keyboard focus.",
            settings.auto_reveal,
            |s, v| s.auto_reveal = v,
        ),
        (
            "Show hidden files",
            "Include dotfiles and folders. Git internals stay hidden.",
            settings.show_hidden,
            |s, v| s.show_hidden = v,
        ),
        (
            "Show Git-ignored files",
            "Display ignored paths with subdued labels.",
            settings.show_ignored,
            |s, v| s.show_ignored = v,
        ),
        (
            "Save on focus change",
            "Save edits when the editor loses focus or its tab closes.",
            settings.save_on_focus_change,
            |s, v| s.save_on_focus_change = v,
        ),
        (
            "Code completion",
            "Suggest names from the open file and import paths while typing.",
            settings.code_completion,
            |s, v| s.code_completion = v,
        ),
    ];
    for (label, description, checked, set) in toggles {
        let store = store.clone();
        page = page.child(setting_row(
            setting_title(label, description, cx),
            Switch::new(label)
                .accessibility_label(label)
                .checked(checked)
                .on_click(move |value, _, cx| {
                    let mut settings = store.read(cx).state.settings.files.clone();
                    set(&mut settings, *value);
                    store.update(cx, |store, cx| store.set_files_settings(settings, cx));
                }),
            cx,
        ));
    }
    let hover = Hover::new("reset-files-settings", window, cx);
    page.child(
        // Separates the page-wide reset from the last row's divider.
        div().pt_4().child(
            Button::new("reset-files-settings")
                .small()
                .cursor_pointer()
                .icon(Lucide::RotateCcw)
                .label("Reset Files settings")
                .on_click(move |_, _, cx| {
                    store.update(cx, |store, cx| {
                        store.set_files_settings(FilesSettings::default(), cx)
                    })
                })
                .smooth_hover(ButtonLook::Outline, &hover, cx),
        ),
    )
}
