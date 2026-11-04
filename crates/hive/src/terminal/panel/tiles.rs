//! Resizable terminal tiles and their close controls.
use super::*;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, v_resizable};

impl TerminalPanel {
    /// Splits become resizable groups. The group id encodes the node's structure (axis, first
    /// pane, child count), so its keyed size state resets to equal shares whenever the structure
    /// changes and survives drags otherwise.
    pub(super) fn render_pane(
        &self,
        pane: &PaneTree<Entity<TerminalView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match pane {
            PaneTree::Leaf(view) => self.render_leaf(view, window, cx),
            PaneTree::Split { axis, children } => {
                let id = Self::split_id(pane, *axis, children.len());
                let group = match axis {
                    Axis::Horizontal => h_resizable(id.clone()),
                    Axis::Vertical => v_resizable(id.clone()),
                };
                let sizes = self.split_sizes.get(&id);
                let children: Vec<_> = children
                    .iter()
                    .enumerate()
                    .map(|(index, child)| {
                        resizable_panel()
                            .when_some(
                                sizes
                                    .and_then(|s| s.get(index))
                                    .copied()
                                    .filter(|s| *s > 0.),
                                |panel, size| panel.size(px(size)),
                            )
                            .child(self.render_pane(child, window, cx))
                    })
                    .collect();
                let panel = cx.entity().downgrade();
                group
                    .children(children)
                    .on_resize(move |state, _, cx| {
                        let sizes = state
                            .read(cx)
                            .sizes()
                            .iter()
                            .map(|s| f32::from(*s))
                            .collect();
                        if let Some(panel) = panel.upgrade() {
                            panel.update(cx, |panel, cx| {
                                panel.split_sizes.insert(id.clone(), sizes);
                                panel.layout_changed(cx);
                            });
                        }
                    })
                    .into_any_element()
            }
        }
    }

    /// One pane. Among split panes, hovering one fades in a close button in its top-right
    /// corner; a lone pane is closed with its tab instead.
    fn render_leaf(
        &self,
        view: &Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane_id = SharedString::from(format!("terminal-pane-{}", view.entity_id()));
        let pane = div()
            .id(pane_id.clone())
            .relative()
            .size_full()
            .child(view.clone());
        if matches!(self.panes, PaneTree::Leaf(_)) {
            return pane.into_any_element();
        }
        let hover = Hover::new(pane_id.clone(), window, cx);
        let close_hover = Hover::new((ElementId::from(pane_id), "close"), window, cx);
        let theme = cx.theme();
        let (background, border) = (theme.background, theme.border);
        let close = Button::new(("close-pane", view.entity_id()))
            .cursor_pointer()
            .xsmall()
            .icon(IconName::Close)
            .tooltip_with_action("Close pane", &ClosePane, Some(keymap::TERMINAL_CONTEXT))
            .on_click(cx.listener({
                let view = view.clone();
                move |this, _, window, cx| {
                    // The pane's own handlers must not see a click that just closed it.
                    cx.stop_propagation();
                    this.close_pane(&view, window, cx)
                }
            }))
            .smooth_hover(ButtonLook::Ghost, &close_hover, cx);
        pane.on_hover(hover.listener())
            // An invisible button must not catch clicks meant for the terminal.
            .when(hover.progress() > 0., |pane| {
                pane.child(
                    div()
                        .absolute()
                        .top(PANE_CLOSE_INSET)
                        .right(PANE_CLOSE_INSET)
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .bg(background)
                        .opacity(hover.progress())
                        .child(close),
                )
            })
            .into_any_element()
    }
}
