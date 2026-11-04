//! Hive's own shortcuts: their defaults, the user's reassignments from `Settings`, and
//! installing both into GPUI's keymap. Bindings of other layers (inputs, terminal) are kept.

use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use gpui_kit::*;
use hive_core::settings::Settings;

use crate::app::AppState;
use crate::ui::git_panel::CommitChanges;
use crate::ui::settings::OpenSettings;
use crate::ui::workspace::{NewTerminal, ToggleGitPanel, ToggleSidebar};

/// Key context every Hive shortcut is bound in: the main window's root.
pub(crate) const CONTEXT: &str = "Workspace";
// Match the input dispatch node too, where Textarea binds its own Cmd-Enter.
pub(crate) const COMMIT_CONTEXT: &str = "(SourceControl > Input) || SourceControl";

/// An action the user can reassign.
pub(crate) struct Shortcut {
    pub label: &'static str,
    pub default: &'static str,
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

pub(crate) static SHORTCUTS: [Shortcut; 5] = [
    Shortcut {
        label: "New terminal",
        default: "cmd-t",
        action: || Box::new(NewTerminal),
    },
    Shortcut {
        label: "Toggle sidebar",
        default: "cmd-shift-\\",
        action: || Box::new(ToggleSidebar),
    },
    Shortcut {
        label: "Open settings",
        default: "cmd-,",
        action: || Box::new(OpenSettings),
    },
    Shortcut {
        label: "Toggle source control",
        default: "cmd-shift-g",
        action: || Box::new(ToggleGitPanel),
    },
    Shortcut {
        label: "Commit staged changes",
        default: "cmd-enter",
        action: || Box::new(CommitChanges),
    },
];

/// Installs the shortcuts now and again whenever the user reassigns one.
pub fn init(cx: &mut App) {
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
    let context = KeyBindingContextPredicate::parse(CONTEXT)
        .map(Rc::new)
        .inspect_err(|err| tracing::error!(%err, "invalid shortcut context"))
        .ok();
    let settings = &AppState::get(cx).store.read(cx).state.settings;
    let bindings: Vec<KeyBinding> = SHORTCUTS
        .iter()
        .filter_map(|shortcut| {
            let context = if shortcut.name() == CommitChanges.name() {
                KeyBindingContextPredicate::parse(COMMIT_CONTEXT)
                    .ok()
                    .map(Rc::new)
            } else {
                context.clone()
            };
            load(shortcut, settings, context, cx)
        })
        .collect();
    cx.clear_key_bindings();
    cx.bind_keys(others.into_iter().chain(bindings));
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
        *other != ix && normalize(current) == keys
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
    use super::{Assignment, BTreeMap, Rejection, SHORTCUTS, assign};
    use gpui_kit::{Keystroke, OperatingSystem};

    const MAC: OperatingSystem = OperatingSystem::Mac;

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

    fn keystroke(source: &'static str) -> Keystroke {
        Keystroke::parse(source).expect("valid keystroke")
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

    #[test]
    fn keys_of_another_action_are_rejected() {
        let none = BTreeMap::new();
        assert_eq!(
            assign(0, &keystroke("cmd-shift-\\"), &none, MAC),
            Err(Rejection::TakenBy("Toggle sidebar"))
        );
    }

    #[test]
    fn reassigned_keys_free_the_default() {
        let mut keybindings = BTreeMap::new();
        keybindings.insert(SHORTCUTS[1].name().to_owned(), "cmd-b".to_owned());
        assert_eq!(
            assign(0, &keystroke("cmd-shift-\\"), &keybindings, MAC),
            Ok(Assignment::Custom("cmd-shift-\\".to_owned()))
        );
        assert_eq!(
            assign(0, &keystroke("cmd-b"), &keybindings, MAC),
            Err(Rejection::TakenBy("Toggle sidebar"))
        );
    }

    #[test]
    fn default_keys_reset_the_assignment() {
        assert_eq!(
            assign(0, &keystroke("cmd-t"), &BTreeMap::new(), MAC),
            Ok(Assignment::Default)
        );
    }
}
