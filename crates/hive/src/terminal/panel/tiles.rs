//! Resizable terminal tiles and their close controls.
use super::*;
use gpui_kit::base::motion::{Transition, transition};
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

    /// One pane. Among split panes, the focused one glows inward from its edges and hovering
    /// one fades in a close button in its top-right corner; a lone pane has nothing to be told
    /// apart from and is closed with its tab instead.
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
        let close_hover = Hover::new((ElementId::from(pane_id.clone()), "close"), window, cx);
        let focused = view.read(cx).focus_handle().is_focused(window);
        let glow = transition(
            (ElementId::from(pane_id), "focus-glow"),
            if focused { 1. } else { 0. },
            Transition::new(FOCUS_GLOW_DURATION).ease(ease_in_out),
            window,
            cx,
        );
        let theme = cx.theme();
        let (background, border) = (theme.background, theme.border);
        let glow_color = focus_glow_color(theme.foreground, glow);
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
                    this.request_close_pane(view.clone(), window, cx)
                }
            }))
            .smooth_hover(ButtonLook::Ghost, &close_hover, cx);
        pane.on_hover(hover.listener())
            .when(glow > 0., |pane| {
                // Drawn over the terminal, which paints its own opaque background; it has no
                // listeners, so the terminal under it still gets the pointer.
                pane.child(div().absolute().inset_0().shadow(vec![
                    BoxShadow::new(px(0.), px(0.), glow_color)
                        .blur_radius(FOCUS_GLOW_DEPTH)
                        .inset(),
                ]))
            })
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

/// The glow of the focused pane, `strength` of the way in. The theme's text color is light
/// on a dark theme and dark on a light one, which is the contrast a glow over the terminal
/// background needs.
fn focus_glow_color(foreground: Hsla, strength: f32) -> Hsla {
    foreground.opacity(FOCUS_GLOW_OPACITY * strength.clamp(0., 1.))
}

#[cfg(test)]
mod tests {
    use super::{FOCUS_GLOW_OPACITY, focus_glow_color};
    use gpui_kit::{black, white};

    #[test]
    fn glow_takes_the_text_color_so_it_contrasts_with_either_theme() {
        let dark_theme = focus_glow_color(white(), 1.);
        let light_theme = focus_glow_color(black(), 1.);
        assert_eq!(dark_theme.l, 1.);
        assert_eq!(light_theme.l, 0.);
        assert_eq!(dark_theme.a, FOCUS_GLOW_OPACITY);
        assert_eq!(light_theme.a, FOCUS_GLOW_OPACITY);
    }

    #[test]
    fn glow_fades_with_its_strength_and_never_exceeds_its_ceiling() {
        assert_eq!(focus_glow_color(white(), 0.).a, 0.);
        assert_eq!(focus_glow_color(white(), 0.5).a, FOCUS_GLOW_OPACITY / 2.);
        assert_eq!(focus_glow_color(white(), 3.).a, FOCUS_GLOW_OPACITY);
        assert_eq!(focus_glow_color(white(), -1.).a, 0.);
    }
}
