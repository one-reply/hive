//! Who gets the keyboard, and with it the selected project, when a terminal tab or pane
//! comes to the front.
//!
//! The dock tells a tab it is displayed (`BasePanel::set_active`) whatever the reason: the
//! user picked it, or the tab in front of it closed on its own. A tab that took the keyboard
//! every time sent what the user was typing elsewhere — a commit message and its Enter — into
//! a shell they were not looking at.
use super::*;
use gpui_kit::component::dock::PanelId;

/// Why a tab or pane came to the front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reveal {
    /// The user opened it, or closed what was in front of it.
    Requested,
    /// Nobody at the keyboard asked: what was in front of it ended on its own, or an agent
    /// opened it.
    Unprompted,
}

/// Whether what came to the front takes the keyboard. `keyboard_was_here` says the keyboard
/// was already in the terminals it joins or replaces, where leaving it would mean typing into
/// something no longer on screen.
pub(super) fn takes_focus(reveal: Reveal, keyboard_was_here: bool) -> bool {
    match reveal {
        Reveal::Requested => true,
        Reveal::Unprompted => keyboard_was_here,
    }
}

impl TerminalPanel {
    /// Whether the keyboard is in one of this tab's panes.
    pub(super) fn holds_focus(&self, window: &Window, cx: &App) -> bool {
        self.panes
            .find(&|view: &Entity<TerminalView>| view.read(cx).focus_handle().is_focused(window))
            .is_some()
    }

    /// Whether the keyboard is in a tab of this tab's group, this one included. A tab that
    /// has just come to the front covers the others, so this is asked of the frame before.
    pub(super) fn group_holds_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        if self.holds_focus(window, cx) {
            return true;
        }
        let Some(group) = self.group.as_ref().and_then(|group| group.upgrade()) else {
            return false;
        };
        // This runs inside our own update, and a tab's focus handle is read from its entity:
        // ours is leased, so it is left out by id.
        let me = PanelId::from(cx.entity_id());
        group
            .read(cx)
            .panels()
            .iter()
            .filter(|panel| panel.panel_id(cx) != me)
            .any(|panel| panel.focus_handle(cx).contains_focused(window, cx))
    }

    /// Gives the keyboard to the tab the group shows now that this one is gone. The dock
    /// does that itself only when the keyboard was inside the closed tab; a click on a close
    /// button has already taken it out.
    pub(super) fn focus_successor(
        group: Option<WeakEntity<TabGroup>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let successor = group
            .and_then(|group| group.upgrade())
            .and_then(|group| group.read(cx).active_panel(cx));
        if let Some(successor) = successor {
            successor.focus_handle(cx).focus(window, cx);
        }
    }
}

#[cfg(test)]
mod tests;
