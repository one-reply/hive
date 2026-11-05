use super::*;
use crate::{
    state::documents::{OpenFile, TabPlacement},
    ui::editor_panel::EditorPanel,
};
use gpui_kit::component::dock::InsertTarget;

impl ProjectCenter {
    pub(in crate::ui::workspace) fn remove_files(
        &mut self,
        target: &hive_core::projects::Target,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut App,
    ) {
        let panels: Vec<_> = self
            .editors
            .iter()
            .filter(|panel| {
                let document = panel.read(cx).document.read(cx);
                document.target.host == target.host
                    && document.target.path == target.path
                    && document.path.starts_with(path)
            })
            .cloned()
            .collect();
        for panel in panels {
            self.dock
                .update(cx, |dock, cx| dock.remove_panel(panel, window, cx));
        }
        self.prune(cx);
    }
    pub(in crate::ui::workspace) fn show_file(
        &mut self,
        request: OpenFile,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.prune(cx);
        if let Some(panel) = self
            .editors
            .iter()
            .find(|panel| {
                let document = panel.read(cx).document.read(cx);
                document.target == request.target && document.path == request.path
            })
            .cloned()
        {
            panel.read(cx).document.clone().update(cx, |document, cx| {
                if !request.preview {
                    document.pinned = true;
                    cx.notify();
                }
                if let Some(position) = request.position {
                    document.goto(position, window, cx);
                }
            });
            self.select(&panel, window, cx);
            Focusable::focus_handle(&panel, cx).focus(window, cx);
            return;
        }
        let panel =
            cx.new(|cx| EditorPanel::new(request.clone(), self.dock.downgrade(), window, cx));
        self.add(panel_handle(panel.clone()), window, cx);
        if !request.preview {
            Focusable::focus_handle(&panel, cx).focus(window, cx);
        }
        self.editors.push(panel.clone());
        self.place_beside_source(&panel, &request.placement, window, cx);
        // Dock assigns the destination group asynchronously; only replace that group's preview.
        let candidates = self.editors.clone();
        let dock = self.dock.clone();
        window.defer(cx, move |window, cx| {
            let group = panel.read(cx).group.clone();
            for previous in candidates {
                if previous == panel {
                    continue;
                }
                let previous_panel = previous.read(cx);
                let document = previous_panel.document.read(cx);
                if previous_panel.group == group
                    && document.target == request.target
                    && !document.pinned
                    && !document.dirty
                {
                    dock.update(cx, |dock, cx| dock.remove_panel(previous, window, cx));
                }
            }
        });
        self.sync_alone(cx);
    }

    /// Moves a new tab right after the tab it was opened from; `Replacing` then closes that
    /// source tab unless it holds unsaved or in-flight edits.
    fn place_beside_source(
        &mut self,
        panel: &Entity<EditorPanel>,
        placement: &TabPlacement,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (TabPlacement::Beside(source) | TabPlacement::Replacing(source)) = placement else {
            return;
        };
        let Some(source_panel) = self
            .editors
            .iter()
            .find(|candidate| {
                *candidate != panel && candidate.read(cx).document.entity_id() == source.entity_id()
            })
            .cloned()
        else {
            return;
        };
        let Some(group) = source_panel
            .read(cx)
            .group
            .as_ref()
            .and_then(WeakEntity::upgrade)
        else {
            return;
        };
        let source_id = PanelId::from(source_panel.entity_id());
        let (node, source_ix) = {
            let group = group.read(cx);
            let ix = group
                .panels()
                .iter()
                .position(|p| p.panel_id(cx) == source_id);
            (group.node(), ix)
        };
        let Some(source_ix) = source_ix else {
            return;
        };
        let id = PanelId::from(panel.entity_id());
        let target = InsertTarget::Tabs {
            node,
            ix: Some(source_ix + 1),
            activate: true,
        };
        self.dock
            .update(cx, |dock, cx| dock.move_panel(id, target, window, cx));
        let replace = matches!(placement, TabPlacement::Replacing(_)) && {
            let document = source_panel.read(cx).document.read(cx);
            !document.dirty && !document.saving
        };
        if replace {
            self.dock
                .update(cx, |dock, cx| dock.remove_panel(source_panel, window, cx));
            self.prune(cx);
        }
    }
}
