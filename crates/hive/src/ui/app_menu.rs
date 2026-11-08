//! The macOS menu bar. App-wide actions are handled here; the rest dispatch to the focused
//! window's views (workspace, terminal, inputs), which GPUI enables or disables accordingly.

use gpui_kit::component::input;
use gpui_kit::*;

use crate::app::AppState;
use crate::terminal::panel::{ClosePane, SplitDown, SplitRight};
use crate::ui::settings::OpenSettings;
use crate::ui::workspace::{
    NewTerminal, ShowFilesPanel, ShowGitPanel, ToggleGitPanel, ToggleSidebar,
};

actions!(
    hive,
    [
        Quit,
        About,
        CheckForUpdates,
        Hide,
        HideOthers,
        ShowAll,
        AddProject,
        CloseWindow,
        Minimize,
        Zoom,
        ToggleFullScreen,
        ShowLogs,
        ShowDataFolder,
    ]
);

/// Registers the handlers and the fixed system shortcuts. Runs before `keymap::init`, which
/// installs the reassignable ones and then the menus themselves.
pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &AddProject, cx| {
        if let Some(handle) = cx.active_window() {
            let _ = handle.update(cx, |_, window, cx| crate::ui::add_project::open(window, cx));
        }
    });
    cx.on_action(|_: &ShowLogs, cx| cx.reveal_path(&hive_core::state::hive_dir().join("logs")));
    cx.on_action(|_: &ShowDataFolder, cx| cx.reveal_path(&hive_core::state::hive_dir()));
    cx.on_action(|_: &CloseWindow, cx| {
        if let Some(handle) = cx.active_window() {
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    crate::ui::editor_panel::close::all(window, cx, |window, _| {
                        window.remove_window()
                    })
                });
            });
        }
    });
    cx.on_action(|_: &Minimize, cx| with_active_window(cx, |window| window.minimize_window()));
    cx.on_action(|_: &Zoom, cx| with_active_window(cx, |window| window.zoom_window()));
    cx.on_action(|_: &ToggleFullScreen, cx| {
        with_active_window(cx, |window| window.toggle_fullscreen())
    });
    cx.on_action(|_: &About, cx| show_about(cx));
    cx.on_action(|_: &CheckForUpdates, cx| {
        crate::ui::settings::open_updates(cx);
        AppState::get(cx).updates.read(cx).check();
    });
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
}

/// Sets the menu bar. Called whenever the keymap changes: macOS reads each item's shortcut
/// from the keymap only at this moment.
pub(crate) fn install(cx: &mut App) {
    cx.set_menus([
        Menu::new("Hive").items([
            MenuItem::action("About Hive", About),
            MenuItem::action("Check for Updates…", CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Hive", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Hive", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open File…", crate::ui::editor_panel::OpenFileSearch),
            MenuItem::action("Save File", crate::ui::editor_panel::SaveFile),
            MenuItem::action("Close File", crate::ui::editor_panel::CloseFile),
            MenuItem::action("Add Project…", AddProject),
            MenuItem::action("New Terminal", NewTerminal),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Left Sidebar", ToggleSidebar),
            MenuItem::action("Toggle Right Sidebar", ToggleGitPanel),
            MenuItem::action("Show Git", ShowGitPanel),
            MenuItem::action("Show Files", ShowFilesPanel),
            MenuItem::separator(),
            MenuItem::action("Toggle Full Screen", ToggleFullScreen),
        ]),
        Menu::new("Terminal").items([
            MenuItem::action("Split Right", SplitRight),
            MenuItem::action("Split Down", SplitDown),
            MenuItem::separator(),
            MenuItem::action("Close Pane", ClosePane),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
        Menu::new("Help").items([
            MenuItem::action("Show Logs in Finder", ShowLogs),
            MenuItem::action("Show Data Folder in Finder", ShowDataFolder),
        ]),
    ]);
}

fn with_active_window(cx: &mut App, act: impl FnOnce(&mut Window)) {
    let Some(handle) = cx.active_window() else {
        return;
    };
    if let Err(err) = handle.update(cx, |_, window, _| act(window)) {
        tracing::warn!(%err, "failed to act on the active window");
    }
}

/// GPUI has no native About panel, so the version goes into an alert on the active window.
fn show_about(cx: &mut App) {
    let Some(handle) = cx.active_window() else {
        return;
    };
    let detail = format!("Version {}", env!("CARGO_PKG_VERSION"));
    let answer = handle.update(cx, |_, window, cx| {
        window.prompt(PromptLevel::Info, "Hive", Some(&detail), &["OK"], cx)
    });
    match answer {
        // Awaited so the alert isn't torn down with its dropped receiver.
        Ok(answer) => cx.foreground_executor().spawn(answer).detach(),
        Err(err) => tracing::warn!(%err, "failed to show the About alert"),
    }
}
