//! JetBrains-style 3-pane virtualized merge surface: Current | Result | Incoming.
use super::rows::DisplayRow;
use super::toolbar::{GUTTER_WIDTH, SideNames};
use super::*;
mod choices;
mod scroll;
use gpui_kit::component::input::{Editor, Redo, Undo};
use gpui_kit::component::scroll::Scrollbar;
use hive_core::git::conflict_merge::{
    ConflictRegion, ConflictResolution, MergeDocument, MergeSegment,
};

const LINE_NUMBER_WIDTH: Pixels = px(42.);
const ROW_PADDING_X: Pixels = px(6.);
const SIDE_BORDER_OPACITY: f32 = 0.45;
const SIDE_BG_OPACITY: f32 = 0.08;
const INLINE_DIFF_OPACITY: f32 = 0.24;
const FILLER_BG_OPACITY: f32 = 0.22;
const CONFLICT_HEADER_OPACITY: f32 = 0.16;
const UNDECIDED_OPACITY: f32 = 0.12;

#[derive(Clone, Copy)]
struct RowMetrics {
    font_size: Pixels,
    height: Pixels,
}

impl ConflictPanel {
    pub(super) fn three_way_body(
        &self,
        document: Arc<MergeDocument>,
        names: &SideNames,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(result) = &self.result else {
            return div().into_any_element();
        };
        let header = self
            .current
            .and_then(|ix| document.conflict(ix))
            .map(|region| self.render_conflict_header(region, names, window, cx));
        let saving = self.conflict.read(cx).is_saving();
        let editor = Editor::new(&result.state)
            .aria_label("Merge result")
            .bordered(false)
            .readonly(saving)
            .size_full();
        let center = div().flex_1().min_w_0().h_full().child(editor);
        v_flex()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .capture_action(cx.listener(|this, _: &Undo, w, cx| this.replay(false, w, cx)))
            .capture_action(cx.listener(|this, _: &Redo, w, cx| this.replay(true, w, cx)))
            .children(header)
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.source_pane(document.clone(), Side::Ours, cx))
                    .child(plain_gutter(cx))
                    .child(center)
                    .child(plain_gutter(cx))
                    .child(self.source_pane(document, Side::Theirs, cx)),
            )
            .into_any_element()
    }

    fn replay(&mut self, redo: bool, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if !self.conflict.read(cx).is_saving()
            && let Some(result) = &mut self.result
        {
            result.replay(redo, window, cx);
            cx.notify();
        }
    }

    fn source_pane(
        &self,
        document: Arc<MergeDocument>,
        side: Side,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let panel = cx.weak_entity();
        let generation = self.synced_generation;
        let state = self.lists[usize::from(side == Side::Theirs)].clone();
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .child(
                list(state.clone(), move |ix, window, cx| {
                    panel
                        .update(cx, |this, cx| {
                            if this.synced_generation != generation {
                                return div().into_any_element();
                            }
                            this.rows.get(ix).map_or_else(
                                || div().into_any_element(),
                                |row| this.source_row(row, &document, side, window, cx),
                            )
                        })
                        .unwrap_or_else(|_| div().into_any_element())
                })
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::vertical(&state).viewport_from_layout()),
            )
    }

    fn source_row(
        &self,
        row: &DisplayRow,
        doc: &MergeDocument,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let font_size = window.rem_size() * (f32::from(cx.theme().mono_font_size) / 16.);
        let metrics = RowMetrics {
            font_size,
            height: font_size * 1.5,
        };
        let (segment, line) = row.location();
        let part = &doc.segments[segment];
        match (row, part) {
            (DisplayRow::Common { .. }, MergeSegment::Common(lines)) => {
                let src = &lines[line];
                Self::code_cell(
                    src.number,
                    src.text().to_owned().into(),
                    self.highlights.common_styles(segment, line),
                    None,
                    src.number,
                    metrics,
                    cx,
                )
            }
            (DisplayRow::Fold { range, .. }, _) => {
                let label = format!("↕ {} unchanged lines", range.len());
                let id = (
                    if side == Side::Ours {
                        "left-fold"
                    } else {
                        "right-fold"
                    },
                    segment,
                );
                let hover = Hover::new(id, window, cx);
                small_action(id, label)
                    .on_click(cx.listener(move |this, _, _, cx| this.expand_fold(segment, cx)))
                    .smooth_hover(ButtonLook::Ghost, &hover, cx)
                    .into_any_element()
            }
            (DisplayRow::ConflictBase { line, .. }, MergeSegment::Conflict(region)) => {
                let Some(src) = region.base.as_ref().and_then(|lines| lines.get(*line)) else {
                    return div().into_any_element();
                };
                let theme = cx.theme();
                Self::code_cell(
                    src.number,
                    src.text().to_owned().into(),
                    self.highlights.base_styles(region.index, *line),
                    Some((theme.muted, theme.muted_foreground)),
                    src.number,
                    metrics,
                    cx,
                )
            }
            (DisplayRow::ConflictLine { .. }, MergeSegment::Conflict(region)) => {
                let ix = region.index;
                div()
                    .w_full()
                    .child(self.side_cell(region, side, line, metrics, cx))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            if event.click_count == 1 {
                                this.current = Some(ix);
                                this.reveal_current(true, window, cx);
                                cx.notify();
                            }
                        }),
                    )
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn code_cell(
        line_number: usize,
        text: SharedString,
        styles: Vec<(std::ops::Range<usize>, HighlightStyle)>,
        tint: Option<(Hsla, Hsla)>,
        working_line: usize,
        metrics: RowMetrics,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .flex_1()
            .min_w_0()
            .min_h(metrics.height)
            .items_start()
            .overflow_hidden()
            .when_some(tint, |el, (background, border)| {
                el.bg(background).border_l_2().border_color(border)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        this.open_file(Some(working_line), cx);
                    }
                }),
            )
            .child(
                div()
                    .w(LINE_NUMBER_WIDTH)
                    .h(metrics.height)
                    .flex_none()
                    .pr_2()
                    .flex()
                    .items_center()
                    .justify_end()
                    .text_size(metrics.font_size)
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.muted_foreground)
                    .child(line_number.to_string()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px(ROW_PADDING_X)
                    .py(px(1.))
                    .text_size(metrics.font_size)
                    .line_height(metrics.height)
                    .font_family(theme.mono_font_family.clone())
                    .text_color(theme.foreground)
                    .child(if text.is_empty() {
                        div().h(metrics.height).into_any_element()
                    } else {
                        StyledText::new(text)
                            .with_highlights(styles)
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }

    fn side_cell(
        &self,
        region: &ConflictRegion,
        side: Side,
        row: usize,
        metrics: RowMetrics,
        cx: &Context<Self>,
    ) -> AnyElement {
        let (lines, accent) = match side {
            Side::Ours => (&region.current, cx.theme().success),
            Side::Theirs => (&region.incoming, cx.theme().info),
        };
        let Some(line) = lines.get(row) else {
            return filler_cell(metrics.height, cx);
        };
        let background = accent.opacity(INLINE_DIFF_OPACITY);
        let styles = match side {
            Side::Ours => self
                .highlights
                .current_styles(region.index, row, background),
            Side::Theirs => self
                .highlights
                .incoming_styles(region.index, row, background),
        };
        Self::code_cell(
            line.number,
            line.text().to_owned().into(),
            styles,
            Some((
                accent.opacity(SIDE_BG_OPACITY),
                accent.opacity(SIDE_BORDER_OPACITY),
            )),
            line.number,
            metrics,
            cx,
        )
    }
}

fn plain_gutter(cx: &App) -> Div {
    div()
        .w(GUTTER_WIDTH)
        .flex_none()
        .border_x_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted.opacity(0.12))
}

fn filler_cell(height: Pixels, cx: &App) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .min_h(height)
        .bg(cx.theme().muted.opacity(FILLER_BG_OPACITY))
        .into_any_element()
}

fn small_action(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id).cursor_pointer().xsmall().label(label)
}
