//! Shortcut keys as Hive shows them: modifiers in the order the keymap writes them (⌃⌥⌘⇧, so
//! `cmd-shift-\` reads ⌘⇧\), drawn like gpui-component's `Kbd`, which only knows Apple's ⌃⌥⇧⌘.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::*;

/// The keys of `keystroke`: modifier symbols, then the key as `Kbd` names it.
pub(crate) fn label(keystroke: &Keystroke) -> String {
    let modifiers = keystroke.modifiers;
    let mut label: String = [
        (modifiers.control, "⌃"),
        (modifiers.alt, "⌥"),
        (modifiers.platform, "⌘"),
        (modifiers.shift, "⇧"),
    ]
    .into_iter()
    .filter_map(|(pressed, symbol)| pressed.then_some(symbol))
    .collect();
    let mut key = keystroke.clone();
    key.modifiers = Modifiers::none();
    label.push_str(&Kbd::format(&key));
    label
}

/// `keystroke` as a key chip.
pub(crate) fn render(keystroke: &Keystroke, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .flex_shrink_0()
        .py_0p5()
        .px_1()
        .min_w_5()
        .rounded(theme.radius / 2.)
        .bg(theme.tokens.muted)
        .text_color(theme.muted_foreground)
        .text_xs()
        .text_center()
        .line_height(relative(1.))
        .child(label(keystroke))
}

/// The chip of the binding `action` has in `context`, if it has one.
pub(crate) fn for_action(
    action: &dyn Action,
    context: &str,
    window: &Window,
    cx: &App,
) -> Option<Div> {
    Some(render(&binding(action, context, window)?, cx))
}

/// `text` with the binding `action` has in `context` now, for a tooltip: a shortcut the user
/// moved in Settings shows where it went.
pub(crate) fn tooltip(
    text: &str,
    action: &dyn Action,
    context: &str,
    window: &Window,
) -> SharedString {
    with_shortcut(text, binding(action, context, window).as_ref()).into()
}

fn with_shortcut(text: &str, keystroke: Option<&Keystroke>) -> String {
    match keystroke {
        Some(keystroke) => format!("{text} ({})", label(keystroke)),
        None => text.to_owned(),
    }
}

fn binding(action: &dyn Action, context: &str, window: &Window) -> Option<Keystroke> {
    let context = KeyContext::parse(context)
        .inspect_err(|err| tracing::error!(%err, context, "invalid key context"))
        .ok()?;
    let binding = window.highest_precedence_binding_for_action_in_context(action, context)?;
    Some(binding.keystrokes().first()?.as_keystroke().clone())
}

#[cfg(test)]
mod tests {
    use super::{label, with_shortcut};
    use gpui_kit::Keystroke;

    #[test]
    fn modifiers_follow_the_keymap_order() {
        let label = |keys| label(&Keystroke::parse(keys).expect("valid keystroke"));
        assert_eq!(label("cmd-shift-\\"), "⌘⇧\\");
        assert_eq!(label("ctrl-cmd-f"), "⌃⌘F");
        assert_eq!(label("ctrl-alt-shift-cmd-k"), "⌃⌥⌘⇧K");
        assert_eq!(label("cmd-enter"), "⌘⏎");
        assert_eq!(label("f2"), "F2");
    }

    #[test]
    fn a_tooltip_names_the_current_shortcut_or_none() {
        let rebound = Keystroke::parse("ctrl-alt-p").expect("valid keystroke");
        assert_eq!(
            with_shortcut("Problems in this worktree", Some(&rebound)),
            "Problems in this worktree (⌃⌥P)"
        );
        assert_eq!(with_shortcut("Go to symbol", None), "Go to symbol");
    }
}
