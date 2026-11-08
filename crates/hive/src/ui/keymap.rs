//! Hive's own shortcuts: their defaults, the user's reassignments from `Settings`, and
//! installing both into GPUI's keymap. Bindings of other layers (inputs, terminal) are kept.

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use gpui_kit::*;
use hive_core::settings::Settings;

use crate::app::AppState;
use crate::terminal::panel::{ClosePane, SplitDown, SplitRight};
use crate::terminal::view::{Copy, Paste};
use crate::ui::app_menu::{self, AddProject, CloseWindow, ToggleFullScreen};
use crate::ui::editor_panel::{CloseFile, OpenFileSearch, SaveFile};
use crate::ui::files_panel::{
    CancelFileEdit, DeleteSelectedFile, OpenSelectedFile, RenameSelectedFile,
};
use crate::ui::git_panel::CommitChanges;
use crate::ui::settings::OpenSettings;
use crate::ui::workspace::{
    NewTerminal, QuickSwitch, ShowFilesPanel, ShowGitPanel, ToggleGitPanel, ToggleSidebar,
};

/// Key context of window-wide shortcuts: the main window's root.
pub(crate) const CONTEXT: &str = "Workspace";
// Match the input dispatch node too, where Textarea binds its own Cmd-Enter.
pub(crate) const COMMIT_CONTEXT: &str = "(SourceControl > Input) || SourceControl";
/// Key context of a file's tab, an editor or a diff, which "Close file" closes.
pub(crate) const FILE_TAB_CONTEXT: &str = "EditorPanel || DiffPanel";
/// Key context of a focused code editor, gpui-component's `Input mode=editor`.
pub(crate) const CODE_EDITOR_CONTEXT: &str = "Input && mode == editor";
/// Key context of pane shortcuts: a focused terminal tab.
pub(crate) const TERMINAL_CONTEXT: &str = "TerminalPanel";
/// Key context of a focused terminal pane's own shortcuts.
pub(crate) const TERMINAL_VIEW_CONTEXT: &str = "Terminal";

/// The group a shortcut is listed under on the Shortcuts page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Category {
    Workspace,
    Terminal,
    SourceControl,
    Files,
}

impl Category {
    /// Every category, in the order the Shortcuts page shows them.
    pub const ALL: [Category; 4] = [
        Category::Workspace,
        Category::Terminal,
        Category::SourceControl,
        Category::Files,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Workspace => "Workspace",
            Category::Terminal => "Terminal",
            Category::SourceControl => "Source control",
            Category::Files => "Files",
        }
    }
}

/// An action the user can reassign.
pub(crate) struct Shortcut {
    pub label: &'static str,
    pub default: &'static str,
    pub category: Category,
    /// Where the binding applies; narrower than `CONTEXT` when a wider one would steal keys.
    pub context: &'static str,
    action: fn() -> Box<dyn Action>,
}

impl Shortcut {
    pub fn action(&self) -> Box<dyn Action> {
        (self.action)()
    }

    /// Stable name (`"workspace::NewTerminal"`), the key under which `Settings` stores it.
    pub fn name(&self) -> &'static str {
        self.action().name()
    }

    /// The keystrokes in effect: the user's, or the default.
    pub fn keystrokes<'a>(&self, settings: &'a Settings) -> &'a str {
        settings.keybinding(self.name()).unwrap_or(self.default)
    }
}

pub(crate) static SHORTCUTS: [Shortcut; 24] = [
    Shortcut {
        label: "Open file",
        default: "cmd-p",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(OpenFileSearch),
    },
    Shortcut {
        label: "Save file",
        default: "cmd-s",
        category: Category::Workspace,
        context: "EditorPanel",
        action: || Box::new(SaveFile),
    },
    Shortcut {
        label: "Show suggestions",
        // VS Code's and Zed's key; macOS may keep it for switching input sources.
        default: "ctrl-space",
        category: Category::Workspace,
        context: CODE_EDITOR_CONTEXT,
        action: || Box::new(gpui_kit::component::input::ShowCompletions),
    },
    Shortcut {
        label: "Close file",
        default: "cmd-w",
        category: Category::Workspace,
        context: FILE_TAB_CONTEXT,
        action: || Box::new(CloseFile),
    },
    Shortcut {
        label: "New terminal",
        default: "cmd-t",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(NewTerminal),
    },
    Shortcut {
        label: "Toggle left sidebar",
        default: "cmd-shift-\\",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(ToggleSidebar),
    },
    Shortcut {
        label: "Toggle right sidebar",
        default: "cmd-\\",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(ToggleGitPanel),
    },
    Shortcut {
        label: "Show Git",
        default: "cmd-shift-g",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(ShowGitPanel),
    },
    Shortcut {
        label: "Show Files",
        default: "cmd-shift-e",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(ShowFilesPanel),
    },
    Shortcut {
        label: "Open settings",
        default: "cmd-,",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(OpenSettings),
    },
    Shortcut {
        label: "Add project",
        default: "cmd-o",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(AddProject),
    },
    Shortcut {
        label: "Close window",
        // Not cmd-w: a focused terminal closes its pane with it.
        default: "cmd-shift-w",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(CloseWindow),
    },
    Shortcut {
        label: "Toggle full screen",
        default: "ctrl-cmd-f",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(ToggleFullScreen),
    },
    Shortcut {
        label: "Go to workspace or agent",
        default: "cmd-k",
        category: Category::Workspace,
        context: CONTEXT,
        action: || Box::new(QuickSwitch),
    },
    Shortcut {
        label: "Split terminal right",
        default: "cmd-d",
        category: Category::Terminal,
        context: TERMINAL_CONTEXT,
        action: || Box::new(SplitRight),
    },
    Shortcut {
        label: "Split terminal down",
        default: "cmd-shift-d",
        category: Category::Terminal,
        context: TERMINAL_CONTEXT,
        action: || Box::new(SplitDown),
    },
    Shortcut {
        label: "Close terminal pane",
        default: "cmd-w",
        category: Category::Terminal,
        context: TERMINAL_CONTEXT,
        action: || Box::new(ClosePane),
    },
    Shortcut {
        label: "Copy from terminal",
        default: "cmd-c",
        category: Category::Terminal,
        context: TERMINAL_VIEW_CONTEXT,
        action: || Box::new(Copy),
    },
    Shortcut {
        label: "Paste into terminal",
        default: "cmd-v",
        category: Category::Terminal,
        context: TERMINAL_VIEW_CONTEXT,
        action: || Box::new(Paste),
    },
    Shortcut {
        label: "Commit staged changes",
        default: "cmd-enter",
        category: Category::SourceControl,
        context: COMMIT_CONTEXT,
        action: || Box::new(CommitChanges),
    },
    Shortcut {
        label: "Open selected file",
        // VS Code's macOS keys: Enter renames the selected entry, Cmd-Down opens it.
        default: "cmd-down",
        category: Category::Files,
        context: "(FilesPanel > Tree) && !Input",
        action: || Box::new(OpenSelectedFile),
    },
    Shortcut {
        label: "Rename file",
        default: "enter",
        category: Category::Files,
        context: "(FilesPanel > Tree) && !Input",
        action: || Box::new(RenameSelectedFile),
    },
    Shortcut {
        label: "Delete file permanently",
        default: "cmd-backspace",
        category: Category::Files,
        context: "(FilesPanel > Tree) && !Input",
        action: || Box::new(DeleteSelectedFile),
    },
    Shortcut {
        label: "Cancel file name edit",
        default: "escape",
        category: Category::Files,
        context: "FileNameEdit > Input",
        action: || Box::new(CancelFileEdit),
    },
];

/// The Shortcuts page's label of `action`, so other screens name it the same way.
pub(crate) fn label(action: &dyn Action) -> Option<&'static str> {
    SHORTCUTS
        .iter()
        .find(|shortcut| shortcut.name() == action.name())
        .map(|shortcut| shortcut.label)
}

/// Installs the shortcuts now and again whenever the user reassigns one.
pub fn init(cx: &mut App) {
    cx.bind_keys(terminal_input_bindings());
    cx.bind_keys([KeyBinding::new(
        "down",
        crate::ui::files_panel::FocusFileResults,
        Some("FileSearch > Input"),
    )]);
    let store = AppState::get(cx).store.clone();
    let mut installed = store.read(cx).state.settings.keybindings().clone();
    install(cx);
    cx.observe(&store, move |store, cx| {
        let keybindings = store.read(cx).state.settings.keybindings();
        if *keybindings != installed {
            installed = keybindings.clone();
            install(cx);
        }
    })
    .detach();
}

fn terminal_input_bindings() -> [KeyBinding; 2] {
    // Root's focus traversal runs before on_key_down; let a focused PTY receive these keys.
    ["tab", "shift-tab"].map(|key| KeyBinding::new(key, NoAction, Some(TERMINAL_VIEW_CONTEXT)))
}

/// Replaces Hive's bindings in the keymap, leaving every other binding in its order.
fn install(cx: &mut App) {
    let ours: HashSet<&str> = SHORTCUTS.iter().map(Shortcut::name).collect();
    let others: Vec<KeyBinding> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|binding| !ours.contains(binding.action().name()))
        .cloned()
        .collect();
    let settings = &AppState::get(cx).store.read(cx).state.settings;
    let bindings: Vec<KeyBinding> = SHORTCUTS
        .iter()
        .filter_map(|shortcut| {
            let context = KeyBindingContextPredicate::parse(shortcut.context)
                .map(Rc::new)
                .inspect_err(|err| tracing::error!(%err, context = shortcut.context, "invalid shortcut context"))
                .ok();
            load(shortcut, settings, context, cx)
        })
        .collect();
    cx.clear_key_bindings();
    cx.bind_keys(others.into_iter().chain(bindings));
    app_menu::install(cx);
}

/// The user's keystrokes for `shortcut`; a string `state.json` can't parse falls back to the default.
fn load(
    shortcut: &Shortcut,
    settings: &Settings,
    context: Option<Rc<KeyBindingContextPredicate>>,
    cx: &App,
) -> Option<KeyBinding> {
    let bind = |keys: &str| {
        KeyBinding::load(keys, shortcut.action(), context.clone(), false, None, cx)
            .inspect_err(|err| tracing::warn!(keys, action = shortcut.name(), %err, "bad shortcut"))
            .ok()
    };
    bind(shortcut.keystrokes(settings)).or_else(|| bind(shortcut.default))
}

/// Why a recorded keystroke can't become a shortcut.
#[derive(Debug, PartialEq)]
pub(crate) enum Rejection {
    /// A plain key would be stolen from typing in the terminal.
    NeedsModifier,
    /// Another Hive action already uses it.
    TakenBy(&'static str),
}

impl Rejection {
    pub fn message(&self) -> SharedString {
        match self {
            Rejection::NeedsModifier => "Use ⌘, ⌃ or ⌥, or a function key.".into(),
            Rejection::TakenBy(label) => format!("Already used by “{label}”.").into(),
        }
    }
}

/// What to store for a recorded keystroke.
#[derive(Debug, PartialEq)]
pub(crate) enum Assignment {
    /// It is the action's default: drop the reassignment.
    Default,
    /// Store these keystrokes.
    Custom(String),
}

/// Checks `keystroke` as the new shortcut for `SHORTCUTS[ix]` against the shortcuts in effect.
pub(crate) fn assign(
    ix: usize,
    keystroke: &Keystroke,
    keybindings: &BTreeMap<String, String>,
    os: OperatingSystem,
) -> Result<Assignment, Rejection> {
    let modifiers = &keystroke.modifiers;
    let is_function_key = keystroke
        .key
        .strip_prefix('f')
        .is_some_and(|n| n.parse::<u8>().is_ok());
    if !(modifiers.platform || modifiers.control || modifiers.alt || is_function_key) {
        return Err(Rejection::NeedsModifier);
    }
    let keys = keystroke.unparse(os);
    let normalize = |keys: &str| {
        Keystroke::parse_with_os(keys, os)
            .map(|k| k.unparse(os))
            .unwrap_or_default()
    };
    let taken = SHORTCUTS.iter().enumerate().find(|(other, shortcut)| {
        let current = keybindings
            .get(shortcut.name())
            .map_or(shortcut.default, String::as_str);
        let is_file = |context| matches!(context, "EditorPanel" | FILE_TAB_CONTEXT);
        let is_terminal = |context| matches!(context, TERMINAL_CONTEXT | TERMINAL_VIEW_CONTEXT);
        let (theirs, ours) = (shortcut.context, SHORTCUTS[ix].context);
        let editor_terminal =
            (is_file(theirs) && is_terminal(ours)) || (is_file(ours) && is_terminal(theirs));
        *other != ix && !editor_terminal && normalize(current) == keys
    });
    if let Some((_, shortcut)) = taken {
        return Err(Rejection::TakenBy(shortcut.label));
    }
    if normalize(SHORTCUTS[ix].default) == keys {
        return Ok(Assignment::Default);
    }
    Ok(Assignment::Custom(keys))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the `gpui_kit::*` glob brings in GPUI's own `test` macro.
    use super::{Assignment, BTreeMap, Category, Rejection, SHORTCUTS, assign};
    use gpui_kit::{Keystroke, OperatingSystem};

    const MAC: OperatingSystem = OperatingSystem::Mac;

    #[test]
    fn file_mutations_never_match_editor_terminal_or_inline_input() {
        use gpui_kit::{KeyBindingContextPredicate, KeyContext};
        let contexts = |names: &[&str]| {
            names
                .iter()
                .map(|name| KeyContext::parse(name).unwrap())
                .collect::<Vec<_>>()
        };
        for shortcut in SHORTCUTS
            .iter()
            .filter(|s| s.label == "Rename file" || s.label == "Delete file permanently")
        {
            let predicate = KeyBindingContextPredicate::parse(shortcut.context).unwrap();
            assert!(predicate.eval(&contexts(&["Workspace", "FilesPanel", "Tree"])));
            for scope in [
                &["Workspace", "EditorPanel", "Input"][..],
                &["Workspace", "TerminalPanel", "Terminal"][..],
                &["Workspace", "FilesPanel", "FileNameEdit", "Input"][..],
                &["Workspace", "FilesPanel", "Tree", "FileNameEdit", "Input"][..],
            ] {
                assert!(
                    predicate.depth_of(&contexts(scope)).is_none(),
                    "{} in {scope:?}",
                    shortcut.label
                );
            }
        }
    }

    #[test]
    fn close_file_matches_editor_and_diff_tabs_only() {
        use gpui_kit::{KeyBindingContextPredicate, KeyContext};
        let predicate =
            KeyBindingContextPredicate::parse(super::FILE_TAB_CONTEXT).expect("file tab context");
        let contexts = |names: &[&str]| {
            names
                .iter()
                .map(|n| KeyContext::parse(n).expect("context"))
                .collect::<Vec<_>>()
        };
        assert!(predicate.eval(&contexts(&["Workspace", "EditorPanel"])));
        assert!(predicate.eval(&contexts(&["Workspace", "DiffPanel"])));
        assert!(!predicate.eval(&contexts(&["Workspace", "TerminalPanel", "Terminal"])));
        assert!(!predicate.eval(&contexts(&["Workspace", "FilesPanel", "Tree"])));
    }

    #[test]
    fn commit_binding_matches_only_source_control_and_its_inputs() {
        use gpui_kit::{KeyBindingContextPredicate, KeyContext};
        let predicate =
            KeyBindingContextPredicate::parse(super::COMMIT_CONTEXT).expect("commit context");
        let contexts = |names: &[&str]| {
            names
                .iter()
                .map(|n| KeyContext::parse(n).expect("context"))
                .collect::<Vec<_>>()
        };
        assert!(predicate.eval(&contexts(&["Workspace", "SourceControl", "Input"])));
        assert!(predicate.eval(&contexts(&["Workspace", "SourceControl"])));
        assert!(!predicate.eval(&contexts(&["Workspace", "TerminalPanel"])));
        assert!(!predicate.eval(&contexts(&["Workspace", "Settings", "Input"])));
    }

    #[test]
    fn suggestions_open_only_in_code_editors() {
        use gpui_kit::{KeyBindingContextPredicate, KeyContext};
        let predicate = KeyBindingContextPredicate::parse(super::CODE_EDITOR_CONTEXT)
            .expect("code editor context");
        let contexts = |names: &[&str]| {
            names
                .iter()
                .map(|n| KeyContext::parse(n).expect("context"))
                .collect::<Vec<_>>()
        };
        assert!(predicate.eval(&contexts(&[
            "Workspace",
            "EditorPanel",
            "Input mode=editor"
        ])));
        assert!(!predicate.eval(&contexts(&["Workspace", "SourceControl", "Input"])));
        assert!(!predicate.eval(&contexts(&["Workspace", "FileSearch", "Input"])));
        assert!(!predicate.eval(&contexts(&["Workspace", "TerminalPanel", "Terminal"])));
    }

    fn keystroke(source: &'static str) -> Keystroke {
        Keystroke::parse(source).expect("valid keystroke")
    }

    #[test]
    fn every_shortcut_has_a_parsable_context_and_default() {
        use gpui_kit::KeyBindingContextPredicate;
        for shortcut in &SHORTCUTS {
            assert!(
                KeyBindingContextPredicate::parse(shortcut.context).is_ok(),
                "context of {}",
                shortcut.label
            );
            assert!(
                Keystroke::parse(shortcut.default).is_ok(),
                "default of {}",
                shortcut.label
            );
        }
    }

    #[test]
    fn terminal_tabs_reach_the_pty_without_disabling_ui_focus_navigation() {
        use gpui_kit::{KeyBinding, KeyContext, Keymap};

        let tabs = ["tab", "shift-tab"];
        let mut keymap = Keymap::default();
        // The action is immaterial: Root's focus traversal must not consume these keys.
        keymap.add_bindings(tabs.map(|key| KeyBinding::new(key, super::NewTerminal, Some("Root"))));
        keymap.add_bindings(super::terminal_input_bindings());
        keymap.add_bindings([KeyBinding::new(
            "cmd-d",
            super::SplitRight,
            Some(super::TERMINAL_CONTEXT),
        )]);
        let contexts = |names: &[&str]| {
            names
                .iter()
                .map(|name| KeyContext::parse(name).expect("context"))
                .collect::<Vec<_>>()
        };
        let terminal = contexts(&["Root", "Workspace", "TerminalPanel", "Terminal"]);
        for key in tabs {
            let (bindings, pending) = keymap.bindings_for_input(&[keystroke(key)], &terminal);
            assert!(
                bindings.is_empty() && !pending,
                "{key} must reach on_key_down"
            );
            for path in [
                &["Root", "Workspace", "Input"][..],
                &["Root", "Settings", "Input"][..],
            ] {
                assert_eq!(
                    keymap
                        .bindings_for_input(&[keystroke(key)], &contexts(path))
                        .0
                        .len(),
                    1
                );
            }
        }
        assert_eq!(
            keymap
                .bindings_for_input(&[keystroke("cmd-d")], &terminal)
                .0
                .len(),
            1
        );
    }

    #[test]
    fn split_shortcuts_apply_only_inside_a_terminal_tab() {
        use gpui_kit::{KeyBindingContextPredicate, KeyContext};
        let predicate =
            KeyBindingContextPredicate::parse(super::TERMINAL_CONTEXT).expect("terminal context");
        let contexts = |names: &[&str]| {
            names
                .iter()
                .map(|n| KeyContext::parse(n).expect("context"))
                .collect::<Vec<_>>()
        };
        // GPUI matches an identifier against the innermost node of the prefix it is given.
        assert!(predicate.eval(&contexts(&["Workspace", "TerminalPanel"])));
        assert!(!predicate.eval(&contexts(&["Workspace"])));
        assert!(!predicate.eval(&contexts(&["Workspace", "SourceControl"])));
    }

    #[test]
    fn every_category_lists_its_shortcuts_contiguously() {
        // The Shortcuts page renders the registry in order under category headers.
        let order: Vec<Category> = SHORTCUTS.iter().map(|s| s.category).collect();
        let mut seen: Vec<Category> = Vec::new();
        for category in order {
            if seen.last() != Some(&category) {
                assert!(!seen.contains(&category), "{category:?} appears twice");
                seen.push(category);
            }
        }
        assert_eq!(seen, Category::ALL);
    }

    #[test]
    fn no_default_is_taken_by_another_shortcut() {
        for (ix, shortcut) in SHORTCUTS.iter().enumerate() {
            let result = assign(ix, &keystroke(shortcut.default), &BTreeMap::new(), MAC);
            assert!(
                !matches!(result, Err(Rejection::TakenBy(_))),
                "{}: {result:?}",
                shortcut.label
            );
        }
    }

    #[test]
    fn plain_keys_are_rejected() {
        let none = BTreeMap::new();
        assert_eq!(
            assign(0, &keystroke("t"), &none, MAC),
            Err(Rejection::NeedsModifier)
        );
        assert_eq!(
            assign(0, &keystroke("shift-t"), &none, MAC),
            Err(Rejection::NeedsModifier)
        );
        assert!(assign(0, &keystroke("f5"), &none, MAC).is_ok());
    }

    fn shortcut(label: &str) -> usize {
        SHORTCUTS
            .iter()
            .position(|s| s.label == label)
            .expect("registered shortcut")
    }

    #[test]
    fn editor_and_terminal_can_share_close_keys() {
        assert_eq!(
            assign(
                shortcut("Close file"),
                &keystroke("cmd-w"),
                &BTreeMap::new(),
                MAC
            ),
            Ok(Assignment::Default)
        );
        assert!(
            assign(
                shortcut("Close file"),
                &keystroke("cmd-p"),
                &BTreeMap::new(),
                MAC
            )
            .is_err()
        );
    }

    #[test]
    fn keys_of_another_action_are_rejected() {
        let none = BTreeMap::new();
        assert_eq!(
            assign(
                shortcut("New terminal"),
                &keystroke("cmd-shift-\\"),
                &none,
                MAC
            ),
            Err(Rejection::TakenBy("Toggle left sidebar"))
        );
    }

    #[test]
    fn reassigned_keys_free_the_default() {
        let mut keybindings = BTreeMap::new();
        keybindings.insert(
            SHORTCUTS[shortcut("Toggle left sidebar")].name().to_owned(),
            "cmd-b".to_owned(),
        );
        assert_eq!(
            assign(
                shortcut("New terminal"),
                &keystroke("cmd-shift-\\"),
                &keybindings,
                MAC
            ),
            Ok(Assignment::Custom("cmd-shift-\\".to_owned()))
        );
        assert_eq!(
            assign(
                shortcut("New terminal"),
                &keystroke("cmd-b"),
                &keybindings,
                MAC
            ),
            Err(Rejection::TakenBy("Toggle left sidebar"))
        );
    }

    #[test]
    fn default_keys_reset_the_assignment() {
        assert_eq!(
            assign(
                shortcut("New terminal"),
                &keystroke("cmd-t"),
                &BTreeMap::new(),
                MAC
            ),
            Ok(Assignment::Default)
        );
    }
}
