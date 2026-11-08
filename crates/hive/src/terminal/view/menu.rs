//! Right-click menu of a tile: the same actions as ⌘C/⌘V and the Terminal app menu.
use super::*;
use crate::terminal::panel::{ClosePane, SplitDown, SplitRight};
use gpui_kit::component::menu::PopupMenu;

impl TerminalView {
    /// Runs when the menu opens, so Copy reflects the selection at that moment. Actions are
    /// dispatched from this tile's focus handle: Copy/Paste land here, splits bubble up to
    /// the panel, which already treats the right-clicked tile as active.
    pub(super) fn context_menu(
        &self,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let session = self.session.clone();
        let focus_handle = self.focus_handle.clone();
        move |menu, _, cx| {
            let has_selection = session.read(cx).has_selection();
            menu.action_context(focus_handle.clone())
                .menu_with_disabled("Copy", Box::new(Copy), !has_selection)
                .menu("Paste", Box::new(Paste))
                .separator()
                .menu("Split Right", Box::new(SplitRight))
                .menu("Split Down", Box::new(SplitDown))
                .separator()
                .menu("Close Pane", Box::new(ClosePane))
        }
    }
}
