//! Right-click menu of a tile: the same actions as ⌘C/⌘V and the Terminal app menu, plus the
//! installed agent CLIs, launched here or below an occupied pane.
use super::*;
use crate::app::AppState;
use crate::terminal::panel::{ClosePane, SplitDown, SplitRight};
use crate::ui::agent_logo;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};

impl TerminalView {
    /// Focus without stopping the event: the context menu opens on the same press, and the
    /// panel makes the focused tile the one its actions target. The rescan serves the next
    /// opening; this one reads the cached list, so a menu never waits on the disk.
    pub(super) fn context_mouse_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle.focus(window, cx);
        AppState::get(cx)
            .agents
            .clone()
            .update(cx, |agents, cx| agents.refresh_installed(cx));
    }

    /// Runs when the menu opens, so Copy reflects the selection at that moment. Actions are
    /// dispatched from this tile's focus handle: Copy/Paste land here, splits bubble up to
    /// the panel, which already treats the right-clicked tile as active.
    pub(super) fn context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let session = self.session.clone();
        let focus_handle = self.focus_handle.clone();
        let this = cx.weak_entity();
        // Agents start locally in the tile's worktree; an SSH tile would need a remote lookup.
        let offers_agents = self.cwd.is_some() && session.read(cx).host.is_local();
        move |menu, _, cx| {
            let has_selection = session.read(cx).has_selection();
            let agents = match offers_agents {
                true => AppState::get(cx).agents.read(cx).installed().to_vec(),
                false => Vec::new(),
            };
            let menu = menu
                .action_context(focus_handle.clone())
                .menu_with_icon_and_disabled("Copy", Lucide::Copy, Box::new(Copy), !has_selection)
                .menu_with_icon("Paste", Lucide::ClipboardPaste, Box::new(Paste))
                .separator();
            agents
                .into_iter()
                .fold(menu, |menu, agent| {
                    let this = this.clone();
                    menu.item(
                        PopupMenuItem::new(agent.name())
                            .icon(agent_logo::icon(agent, cx))
                            .on_click(move |_, _, cx| {
                                let _ = this.update(cx, |this, cx| {
                                    this.session
                                        .update(cx, |session, cx| session.request_agent(agent, cx));
                                });
                            }),
                    )
                })
                .separator()
                .menu_with_icon(
                    "Split Right",
                    Lucide::SquareSplitHorizontal,
                    Box::new(SplitRight),
                )
                .menu_with_icon(
                    "Split Down",
                    Lucide::SquareSplitVertical,
                    Box::new(SplitDown),
                )
                .separator()
                .menu_with_icon("Close Pane", Lucide::X, Box::new(ClosePane))
        }
    }
}
