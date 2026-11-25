//! Keep the order fixed while Ctrl is held, so repeated Tab reaches the third tab too.
use super::*;

#[derive(Default)]
pub(super) struct RecentTabs {
    order: Vec<PanelId>,
    cycle: Option<(Modifiers, Vec<PanelId>, usize)>,
}

impl RecentTabs {
    pub(super) fn activate(&mut self, id: PanelId) {
        if self.order.first() != Some(&id) {
            self.order.retain(|candidate| *candidate != id);
            self.order.insert(0, id);
        }
    }

    fn next(
        &mut self,
        open: &[PanelId],
        current: Option<PanelId>,
        modifiers: Modifiers,
    ) -> Option<PanelId> {
        self.order.retain(|candidate| open.contains(candidate));
        if self.cycle.as_ref().is_some_and(|(keys, tabs, _)| {
            *keys != modifiers
                || tabs.len() != open.len()
                || !tabs.iter().all(|id| open.contains(id))
        }) {
            self.cycle = None;
        }
        if self.cycle.is_none() {
            if let Some(id) = current.filter(|id| open.contains(id)) {
                self.activate(id);
            }
            let mut tabs = self.order.clone();
            for &id in open {
                if !tabs.contains(&id) {
                    tabs.push(id);
                }
            }
            if tabs.len() < 2 {
                return None;
            }
            self.cycle = Some((modifiers, tabs, 0));
        }
        let (_, tabs, index) = self.cycle.as_mut()?;
        *index = (*index + 1) % tabs.len();
        let next = tabs[*index];
        self.activate(next);
        if !modifiers.modified() {
            self.cycle = None;
        }
        Some(next)
    }
}

impl ProjectCenter {
    pub(in crate::ui::workspace) fn release_tab_cycle(&mut self, modifiers: Modifiers) {
        if self
            .recent_tabs
            .cycle
            .as_ref()
            .is_some_and(|(keys, _, _)| *keys != modifiers)
        {
            self.recent_tabs.cycle = None;
        }
    }

    pub(in crate::ui::workspace) fn switch_recent(&mut self, window: &mut Window, cx: &mut App) {
        let dock = self.dock.read(cx);
        let Some(layout) = dock.layout(DockPlacement::Center) else {
            return;
        };
        let ids: Vec<_> = layout.panels().collect();
        let focused = ids
            .iter()
            .copied()
            .find(|id| {
                dock.panel(*id)
                    .is_some_and(|p| p.focus_handle(cx).contains_focused(window, cx))
            })
            .or(self.focused);
        let Some(id) = self.recent_tabs.next(&ids, focused, window.modifiers()) else {
            return;
        };
        let focus = dock.panel(id).map(|p| p.focus_handle(cx));
        self.dock
            .update(cx, |dock, cx| dock.select_panel(id, window, cx));
        if let Some(focus) = focus {
            focus.focus(window, cx);
        }
        self.focused = Some(id);
    }
}

#[cfg(test)]
mod tests {
    use super::{Modifiers, PanelId, RecentTabs};
    #[test]
    fn holding_ctrl_cycles_then_releasing_returns_to_the_previous_tab() {
        let tabs = [1, 2, 3].map(PanelId::from_u64);
        let mut recent = RecentTabs::default();
        for id in tabs {
            recent.activate(id);
        }
        let ctrl = Modifiers {
            control: true,
            ..Default::default()
        };
        assert_eq!(recent.next(&tabs, Some(tabs[2]), ctrl), Some(tabs[1]));
        assert_eq!(recent.next(&tabs, Some(tabs[1]), ctrl), Some(tabs[0]));
        recent.cycle = None;
        assert_eq!(recent.next(&tabs, Some(tabs[0]), ctrl), Some(tabs[1]));
        assert_eq!(recent.next(&tabs[1..], Some(tabs[1]), ctrl), Some(tabs[2]));
    }
}
