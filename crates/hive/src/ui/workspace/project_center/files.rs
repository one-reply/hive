use super::*;
use crate::{state::documents::OpenFile, ui::editor_panel::EditorPanel};

impl ProjectCenter {
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
                if let Some(line) = request.line {
                    document.goto(line, window, cx);
                }
            });
            self.select(&panel, window, cx);
            Focusable::focus_handle(&panel, cx).focus(window, cx);
            return;
        }
        let panel =
            cx.new(|cx| EditorPanel::new(request.clone(), self.dock.downgrade(), window, cx));
        self.add(panel_handle(panel.clone()), window, cx);
        self.editors.push(panel.clone());
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
}
