//! The library owns dock placement; Hive persists only typed tab contents and layout.
use super::*;
use crate::state::git::Target;
use gpui_kit::component::dock::{DockLayout, PaneNode, PaneRef};
use hive_core::state::workspace::{Layout, ProjectWorkspace, SplitAxis, Tab, Tabs};

impl ProjectCenter {
    pub(in crate::ui::workspace) fn snapshot(
        &mut self,
        saved: &mut ProjectWorkspace,
        window: &Window,
        cx: &App,
    ) {
        let dock = self.dock.read(cx);
        let Some(tree) = dock.layout(DockPlacement::Center) else {
            saved.layout = None;
            return;
        };
        let ids: Vec<_> = tree
            .panels()
            .filter(|id| self.snapshot_tab(*id, cx).is_some())
            .collect();
        if let Some(focused) = ids.iter().find(|id| {
            dock.panel(**id)
                .is_some_and(|p| p.focus_handle(cx).contains_focused(window, cx))
        }) {
            self.focused = Some(*focused);
        }
        saved.focused = self
            .focused
            .and_then(|id| ids.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        saved.layout = self.snapshot_node(tree.root(), cx);
    }

    fn snapshot_node(&self, node: &PaneNode, cx: &App) -> Option<Layout<Tabs>> {
        match node.kind() {
            PaneRef::Split {
                axis,
                children,
                sizes,
            } => {
                let (children, sizes): (Vec<_>, Vec<_>) = children
                    .iter()
                    .enumerate()
                    .filter_map(|(index, child)| {
                        self.snapshot_node(child, cx).map(|node| {
                            (
                                node,
                                sizes
                                    .get(index)
                                    .copied()
                                    .flatten()
                                    .map(f32::from)
                                    .unwrap_or(0.),
                            )
                        })
                    })
                    .unzip();
                (!children.is_empty()).then_some(Layout::Split {
                    axis: match axis {
                        Axis::Horizontal => SplitAxis::Horizontal,
                        Axis::Vertical => SplitAxis::Vertical,
                    },
                    sizes,
                    children,
                })
            }
            PaneRef::Tabs { panels, active_ix } => {
                let mut active = 0;
                let mut items = Vec::new();
                for (index, id) in panels.iter().enumerate() {
                    if let Some(tab) = self.snapshot_tab(*id, cx) {
                        if index <= active_ix {
                            active = items.len();
                        }
                        items.push(tab);
                    }
                }
                (!items.is_empty()).then_some(Layout::Leaf {
                    value: Tabs { items, active },
                })
            }
        }
    }

    fn snapshot_tab(&self, id: PanelId, cx: &App) -> Option<Tab> {
        if let Some(panel) = self
            .editors
            .iter()
            .find(|p| PanelId::from(p.entity_id()) == id)
        {
            let document = panel.read(cx).document.read(cx);
            if !document.pinned {
                return None;
            }
            let editor = document.editor.read(cx);
            let cursor = editor.cursor_position();
            let scroll = editor.scroll_offset();
            return Some(Tab::File {
                worktree: document.target.path.clone(),
                path: document.path.clone(),
                cursor: [cursor.line, cursor.character],
                scroll: [scroll.x.into(), scroll.y.into()],
            });
        }
        if let Some(panel) = self
            .terminals
            .iter()
            .find(|p| PanelId::from(p.entity_id()) == id)
        {
            return Some(panel.read(cx).snapshot(cx));
        }
        let panel = self
            .diffs
            .iter()
            .find(|p| PanelId::from(p.entity_id()) == id)?;
        let request = &panel.read(cx).review.read(cx).request;
        Some(Tab::Diff {
            worktree: request.target.path.clone(),
            path: request.path.clone(),
            old_path: request.old_path.clone(),
            source: request.source.clone(),
            whole_file_only: request.whole_file_only,
        })
    }

    pub(in crate::ui::workspace) fn restore(
        &mut self,
        saved: &ProjectWorkspace,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(layout) = &saved.layout else {
            return;
        };
        let mut panels = Vec::new();
        let layout = self.restore_node(layout, saved, &mut panels, window, cx);
        self.focused = panels
            .get(saved.focused)
            .or_else(|| panels.first())
            .copied();
        self.dock
            .update(cx, |dock, cx| dock.set_center(layout, window, cx));
        self.sync_alone(cx);
        if let Some(focused) = self.focused {
            let dock = self.dock.clone();
            // set_center delivers panel activations on the next tick; restore focus after them.
            window.defer(cx, move |window, cx| {
                let focus = dock
                    .read(cx)
                    .panel(focused)
                    .map(|panel| panel.focus_handle(cx));
                if let Some(focus) = focus {
                    focus.focus(window, cx);
                }
            });
        }
    }

    fn restore_node(
        &mut self,
        layout: &Layout<Tabs>,
        saved: &ProjectWorkspace,
        panels: &mut Vec<PanelId>,
        window: &mut Window,
        cx: &mut App,
    ) -> DockLayout {
        match layout {
            Layout::Split {
                axis,
                children,
                sizes,
            } => {
                let mut layout = match axis {
                    SplitAxis::Horizontal => DockLayout::h_split(),
                    SplitAxis::Vertical => DockLayout::v_split(),
                };
                for (index, child) in children.iter().enumerate() {
                    let size = sizes
                        .get(index)
                        .copied()
                        .filter(|s| s.is_finite() && *s > 0.)
                        .map(px);
                    layout =
                        layout.child(self.restore_node(child, saved, panels, window, cx), size);
                }
                layout
            }
            Layout::Leaf { value } => {
                let mut layout = DockLayout::tabs();
                let mut active = 0;
                let mut count = 0;
                for (index, tab) in value.items.iter().enumerate() {
                    if let Some(panel) = self.restore_tab(tab, saved, window, cx) {
                        if index <= value.active {
                            active = count;
                        }
                        count += 1;
                        panels.push(panel.panel_id(cx));
                        layout = layout.panel_view(panel, cx);
                    }
                }
                layout.active_index(active)
            }
            Layout::Unknown => DockLayout::tabs(),
        }
    }

    fn restore_tab(
        &mut self,
        tab: &Tab,
        saved: &ProjectWorkspace,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Arc<dyn BasePanelView>> {
        match tab {
            Tab::File {
                worktree,
                path,
                cursor,
                scroll,
            } => {
                let request = crate::state::documents::OpenFile {
                    target: Target {
                        project: saved.project.clone(),
                        path: worktree.clone(),
                        host: saved.host.clone(),
                    },
                    path: path.clone(),
                    line: None,
                    preview: false,
                };
                let panel = cx.new(|cx| {
                    crate::ui::editor_panel::EditorPanel::new(
                        request,
                        self.dock.downgrade(),
                        window,
                        cx,
                    )
                });
                panel.update(cx, |panel, _| panel.restoring = true);
                panel
                    .read(cx)
                    .document
                    .clone()
                    .update(cx, |d, cx| d.restore_position(*cursor, *scroll, window, cx));
                self.editors.push(panel.clone());
                Some(panel_handle(panel))
            }
            Tab::Terminal { panes, active } => {
                let panel =
                    TerminalPanel::restore(panes, *active, self.dock.downgrade(), window, cx)?;
                self.terminals.push(panel.clone());
                Some(panel_handle(panel))
            }
            Tab::Diff {
                worktree,
                path,
                old_path,
                source,
                whole_file_only,
            } => {
                let request = DiffRequest {
                    target: Target {
                        project: saved.project.clone(),
                        path: worktree.clone(),
                        host: saved.host.clone(),
                    },
                    path: path.clone(),
                    old_path: old_path.clone(),
                    source: source.clone(),
                    whole_file_only: *whole_file_only,
                };
                let panel = cx.new(|cx| DiffPanel::new(request, self.dock.downgrade(), window, cx));
                self.diffs.push(panel.clone());
                Some(panel_handle(panel))
            }
            Tab::Unknown => None,
        }
    }
}
