//! The editable result of a conflict and its aligned source preview.
use super::*;
use gpui_kit::component::input::{Editor, Enter, Escape};

impl ConflictPanel {
    pub(super) fn render_conflict_editor(
        &self,
        region: &ConflictRegion,
        conflict_ix: usize,
        metrics: RowMetrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(editor) = self.editing.as_ref().filter(|e| e.conflict == conflict_ix) else {
            return div().into_any_element();
        };
        let line_count = editor.state.read(cx).value().lines().count().max(4);
        let editor_height = metrics.height * (line_count as f32).min(18.) + px(16.);
        let [apply_hover, cancel_hover] =
            ["inline-apply", "inline-cancel"].map(|id| Hover::new((id, conflict_ix), window, cx));
        let theme = cx.theme();

        let side_lines = |side, count: usize| {
            v_flex().flex_1().min_w_0().children(
                (0..count.min(rows::EDITOR_SIDE_LINES))
                    .map(|row| self.side_cell(region, side, row, metrics, cx)),
            )
        };
        let left_lines = side_lines(Side::Ours, region.current.len());
        let right_lines = side_lines(Side::Theirs, region.incoming.len());

        let center_editor = v_flex()
            .flex_1()
            .min_w_0()
            .p_1p5()
            .gap_1p5()
            .bg(theme.primary.opacity(0.06))
            .capture_action(cx.listener(|this, enter: &Enter, window, cx| {
                if enter.secondary {
                    cx.stop_propagation();
                    this.apply_editing(window, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                cx.stop_propagation();
                this.cancel_editing(window, cx);
            }))
            .child(
                div()
                    .w_full()
                    .h(editor_height)
                    .child(Editor::new(&editor.state).bordered(true).size_full()),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .justify_end()
                    .items_center()
                    .child(
                        Button::new(("inline-cancel", conflict_ix))
                            .cursor_pointer()
                            .xsmall()
                            .label("Cancel")
                            .tooltip(crate::ui::key_hint::tooltip(
                                "Cancel edit",
                                &Escape,
                                "Input",
                                window,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.cancel_editing(window, cx);
                            }))
                            .smooth_hover(ButtonLook::Ghost, &cancel_hover, cx),
                    )
                    .child(
                        Button::new(("inline-apply", conflict_ix))
                            .cursor_pointer()
                            .xsmall()
                            .label("Apply Edit")
                            .tooltip(crate::ui::key_hint::tooltip(
                                "Apply edit",
                                &Enter {
                                    secondary: true,
                                    shift: false,
                                },
                                "Input",
                                window,
                            ))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply_editing(window, cx);
                            }))
                            .smooth_hover(ButtonLook::Primary, &apply_hover, cx),
                    ),
            );

        h_flex()
            .w_full()
            .min_w_0()
            .items_stretch()
            .child(left_lines)
            .child(connector_gutter(Some(theme.primary), cx))
            .child(center_editor)
            .child(connector_gutter(Some(theme.primary), cx))
            .child(right_lines)
            .into_any_element()
    }
}
