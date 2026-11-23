//! The footer's menu of Git actions: a popup that rises and fades in above the split button,
//! and sinks and fades out again.
//!
//! The library's dropdown drops its menu in the frame it closes, so the menu is hosted here:
//! `Popup` places it, `Presence` keeps it rendered until it has faded out, and `PopupMenu`
//! still draws the rows and handles the keyboard, Escape and a click outside.
use super::*;
use crate::ui::key_hint;
use gpui_kit::base::Popup;
use gpui_kit::base::motion::{Presence, Transition};
use gpui_kit::component::menu::PopupMenu;
use std::time::Duration;

/// How long the menu takes to rise and fade in, and to sink and fade out.
const MENU_TRANSITION: Duration = Duration::from_millis(160);
/// How far below its place the menu starts rising from.
const MENU_RISE: Pixels = px(6.);
/// Space between the split button and the menu above it.
const MENU_GAP: Pixels = px(4.);
/// Room for the longest label beside the longest note.
const MENU_MIN_WIDTH: Pixels = px(240.);
/// Keeps a note clear of its label.
const NOTE_GAP: Pixels = px(24.);

/// Whether the footer's menu is open, and the popup that draws it.
#[derive(Default)]
pub(in crate::ui::git_panel) struct ActionsMenu {
    open: bool,
    /// Kept after closing, so the menu can fade out; the next opening replaces it.
    popup: Option<Entity<PopupMenu>>,
    /// What had the focus before the menu took it.
    restore_focus: Option<FocusHandle>,
    _dismissed: Option<Subscription>,
}

impl ActionsMenu {
    pub(in crate::ui::git_panel) fn is_open(&self) -> bool {
        self.open
    }
}

impl GitPanel {
    pub(in crate::ui::git_panel) fn toggle_actions_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.actions_menu.open {
            self.close_actions_menu(window, cx);
        } else {
            self.open_actions_menu(window, cx);
        }
    }

    /// Opens the menu on the actions as they stand now; a row checks again when it is chosen.
    fn open_actions_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Every row of a busy worktree would be unavailable.
        if self.git.read(cx).is_busy() {
            return;
        }
        let groups: Vec<Vec<Row>> = Entry::MENU
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|entry| Row {
                        entry: *entry,
                        label: self.entry_label(*entry, cx),
                        blocker: self.blocker(*entry, cx),
                    })
                    .collect()
            })
            .collect();
        let restore_focus = window.focused(cx);
        let panel = cx.weak_entity();
        let popup = PopupMenu::build(window, cx, |menu, _, _| {
            let menu = menu.min_w(MENU_MIN_WIDTH);
            // The library returns the focus here when the menu is dismissed.
            let menu = match &restore_focus {
                Some(handle) => menu.action_context(handle.clone()),
                None => menu,
            };
            groups
                .into_iter()
                .enumerate()
                .fold(menu, |menu, (index, group)| {
                    let menu = if index > 0 { menu.separator() } else { menu };
                    group
                        .into_iter()
                        .fold(menu, |menu, row| menu.item(row.item(panel.clone())))
                })
        });
        let dismissed = cx.subscribe_in(
            &popup,
            window,
            |this, popup, _: &DismissEvent, window, cx| {
                this.actions_menu.open = false;
                // Nothing had the focus before the menu, so nothing took it back.
                if popup.focus_handle(cx).contains_focused(window, cx) {
                    window.blur(cx);
                }
                cx.notify();
            },
        );
        self.actions_menu = ActionsMenu {
            open: true,
            popup: Some(popup.clone()),
            restore_focus,
            _dismissed: Some(dismissed),
        };
        // After the press that opened the menu, or the pressed button would take the focus.
        cx.defer_in(window, move |this, window, cx| {
            if this.actions_menu.open {
                popup.focus_handle(cx).focus(window, cx);
            }
        });
        cx.notify();
    }

    /// Closes the menu without a choice, when what it offered no longer applies.
    pub(in crate::ui::git_panel) fn close_actions_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.actions_menu.open {
            return;
        }
        self.actions_menu.open = false;
        let focused = self
            .actions_menu
            .popup
            .as_ref()
            .is_some_and(|popup| popup.focus_handle(cx).contains_focused(window, cx));
        if focused {
            match &self.actions_menu.restore_focus {
                Some(handle) => handle.focus(window, cx),
                None => window.blur(cx),
            }
        }
        cx.notify();
    }

    /// The name of `entry` in the menu; Rebase names the base it would replay onto.
    fn entry_label(&self, entry: Entry, cx: &App) -> SharedString {
        match (entry, &self.git.read(cx).base) {
            (Entry::Rebase, Some(base)) => {
                format!("Rebase from {}", branches::short_name(base)).into()
            }
            _ => entry.label().into(),
        }
    }

    fn run_menu_entry(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<Self>) {
        // A row of a menu that is already fading out is no longer a target.
        if self.actions_menu.open {
            self.perform(entry, window, cx);
        }
    }

    /// Anchors the menu above `trigger` and plays its entrance and exit.
    pub(super) fn actions_menu_host(
        &self,
        trigger: Div,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let presence = Presence::new("git-actions-menu", self.actions_menu.open)
            .transition(Transition::new(MENU_TRANSITION).ease(ease_in_out))
            .sample(window, cx);
        // The footer sits at the bottom of the window, so the menu opens upwards.
        let host = Popup::new("git-actions-menu", trigger)
            .w_full()
            .flex_none()
            .anchor(Anchor::BottomRight)
            .offset(MENU_GAP);
        let Some(popup) = self
            .actions_menu
            .popup
            .clone()
            .filter(|_| presence.should_render())
        else {
            return host;
        };
        // Opacity does not reach deferred content from outside, so the popup fades here.
        host.content(
            div()
                .relative()
                .top(MENU_RISE * (1. - presence.progress))
                .opacity(presence.progress)
                .child(popup),
        )
    }
}

/// One action of the menu, as it stood when the menu opened.
struct Row {
    entry: Entry,
    label: SharedString,
    blocker: Option<Blocker>,
}

impl Row {
    /// The name on the left; on the right why the action is unavailable, or its shortcut.
    fn item(self, panel: WeakEntity<GitPanel>) -> PopupMenuItem {
        let Self {
            entry,
            label,
            blocker,
        } = self;
        PopupMenuItem::element(move |window, cx| {
            let trailing = match blocker {
                Some(blocker) => Some(
                    div()
                        .flex_none()
                        .text_size(META_TEXT)
                        .text_color(cx.theme().muted_foreground)
                        .child(blocker.note())
                        .into_any_element(),
                ),
                None if entry == Entry::Commit => {
                    key_hint::for_action(&CommitChanges, "SourceControl", window, cx)
                        .map(IntoElement::into_any_element)
                }
                None => None,
            };
            h_flex()
                .flex_1()
                .min_w_0()
                .gap(NOTE_GAP)
                .justify_between()
                .child(div().truncate().child(label.clone()))
                .children(trailing)
        })
        .disabled(blocker.is_some())
        .on_click(move |_, window, cx| {
            let _ = panel.update(cx, |this, cx| this.run_menu_entry(entry, window, cx));
        })
    }
}
